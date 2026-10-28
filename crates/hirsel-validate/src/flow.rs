use super::shared::Shared;
use super::*;
use crate::bit_rows::BitRows;
use hirsel_syntax::PassingMode;
use hirsel_type_analysis::parametric::{ParametricInput, parametric_inputs, unique_rows};
use hirsel_type_analysis::representation::{
    RepresentationError, RepresentationFailure, expand_representation,
};
use hirsel_types::rows::Rows;
use hirsel_types::{
    BuiltinType, Origin, OriginArgument, OriginRoot, ParameterRef, Primitive, Relation,
    Sendability, SignatureKind, TypeKind,
};

#[path = "authority_flow.rs"]
mod authority_flow;
#[path = "borrow_cursors.rs"]
mod borrow_cursors;
#[path = "cleanup_order.rs"]
mod cleanup_order;
#[path = "creation.rs"]
mod creation;
#[path = "crossing.rs"]
pub(super) mod crossing;
#[path = "lifetimes.rs"]
pub(super) mod lifetimes;
#[path = "loan_content.rs"]
mod loan_content;
#[path = "seal_flow.rs"]
mod seal_flow;
#[path = "state_proof.rs"]
mod state_proof;
#[path = "tags.rs"]
pub(super) mod tags;
#[path = "uniqueness.rs"]
mod uniqueness;

#[cfg(test)]
#[path = "flow_evidence_laws.rs"]
mod evidence_laws;

// A powerset lattice, with absent entry states denoting unreachable blocks.
// Transfer functions are monotone; every bit only increases at a join. No path
// enumeration, recursion, iteration cutoff, or reducibility assumption.
const DEAD: u8 = 1;
pub(super) fn unique_row_path(row: hirsel_types::UniqueResultRow) -> Vec<crossing::Part> {
    match row {
        hirsel_types::UniqueResultRow::Root => vec![],
        hirsel_types::UniqueResultRow::Tuple(field) => vec![crossing::Part::Field(field)],
        hirsel_types::UniqueResultRow::Payload { variant, field } => {
            vec![crossing::Part::Variant(variant, field)]
        }
    }
}
const LIVE: u8 = 2;
const MOVED: u8 = 4;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
struct MustFacts(u8);
impl MustFacts {
    const OWNED: u8 = 1;
    const GLOBAL: u8 = 2;
    const SENDABLE: u8 = 4;
    const AUTHORITY: u8 = 8;
    const FRESH_RECEIPT: u8 = 16;

    fn has(self, fact: u8) -> bool {
        self.0 & fact != 0
    }
    fn set(&mut self, fact: u8, value: bool) {
        if value {
            self.0 |= fact;
        } else {
            self.0 &= !fact;
        }
    }
    fn join_live(self, other: Self, live: bool, incoming_live: bool) -> Self {
        match (live, incoming_live) {
            (true, true) => Self(self.0 & other.0),
            (false, true) => other,
            _ => self,
        }
    }
}

impl crate::bit_rows::Value for MustFacts {
    const BITS: usize = 8;
    fn bits(self) -> u8 {
        self.0
    }
    fn value(bits: u8) -> &'static Self {
        const VALUES: [MustFacts; 32] = {
            let mut out = [MustFacts(0); 32];
            let mut i = 0;
            while i < 32 {
                out[i] = MustFacts(i as u8);
                i += 1;
            }
            out
        };
        &VALUES[bits as usize]
    }
}
impl BitRows<MustFacts> {
    fn set_fact(&mut self, index: usize, fact: u8, value: bool) {
        let mut row = self[index];
        row.set(fact, value);
        self.set(index, row);
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(super) struct State {
    cursor_views: Rows<Option<borrow_cursors::View>>,
    cursor_contracts: super::origin_dependencies::DependencyRows<borrow_cursors::Contract>,
    envelopes: Rows<Option<BTreeSet<usize>>>,
    /// Hypothetical replay of an edge disproved by the current tag facts.
    audit: bool,
    cleanup_roots: usize,
    live_temporaries: usize,
    cleanup_conflicts: usize,
    cleanup_loans: usize,
    cleanup_maybe_loans: usize,
    contract_readonly: Shared<BTreeSet<usize>>,
    origins: super::flow_origins::OriginState,
    places: BitRows<u8>,
    /// A finalizer suppressed by an explicit committed whole destructure (#97).
    defused: BitRows<u8>,
    /// Ownership, deep Global content, current Iter sendability and permitted
    /// authority must hold on every path where this canonical place is live.
    must: BitRows<MustFacts>,
    ownership_conflicts: BitRows<bool>,
    shared: BitRows<bool>,
    /// Real payload aliases, excluding root weakening by a mutable hand-back.
    nested_shared: BitRows<bool>,
    /// MODE-2: a shared root whose only aliases are a related call result's
    /// counted shares. They are local to its borrow, so none outlives the
    /// body except through a counted result: the hand-back of a body
    /// returning no counted content still finds it unique.
    borrow_only: BitRows<bool>,
    /// The roots whose related call result shares a local's content may
    /// hold, through every value transfer from that result (MODE-2).
    borrowed_shares: Rows<loan_content::Content>,
    /// Current content evidence, independent of the receiving storage contract.
    current_unique: BitRows<bool>,
    crossing_parts: Rows<crossing::Parts>,
    unique_parts: Rows<crossing::Parts>,
    tags: Shared<tags::Facts>,
    loans: BitRows<u8>,
    /// Known installed-content partitions, with correlated expiration when
    /// each retained permission ends. Access loans remain separate.
    content_loans: Rows<loan_content::Content>,
    /// Borrowed storage roots retained by each value, independently of loan IDs.
    sources: Rows<BTreeSet<usize>>,
    /// May precede relation between temporary evaluations. Writes reset a row
    /// and column and put the new evaluation after all live temporaries.
    before: Shared<BTreeSet<(usize, usize)>>,
}
impl State {
    fn join(&mut self, other: &Self, model: &Model) -> bool {
        let previous = self.clone();
        self.join_since(&previous, other, model);
        *self != previous
    }
    /// Join `other`, which is `base` changed in place, into a state that
    /// already joined `base`. Every row join is idempotent and associative, so
    /// a row equal in `base` and `other` already holds its join: only the
    /// change since `base` is visited. `join` is the case `base == self`.
    fn join_since(&mut self, base: &Self, other: &Self, model: &Model) {
        self.cursor_views
            .join_since(&base.cursor_views, &other.cursor_views, |a, b| {
                match (a.as_mut(), b) {
                    (Some(a), Some(b)) => a.join(b),
                    (None, Some(b)) => *a = Some(b.clone()),
                    _ => (),
                }
            });
        self.cursor_contracts.union_since(
            &base.cursor_contracts,
            &other.cursor_contracts,
            |a, b| {
                a.cover.retain(|anchor| b.cover.contains(anchor));
                a.expired.extend(b.expired.iter().cloned());
                a.snapshot |= b.snapshot;
            },
        );
        self.envelopes
            .join_since(&base.envelopes, &other.envelopes, |a, b| {
                *a = creation::join([a.clone(), b.clone()])
            });
        let same_loans = self.cleanup_loans == other.cleanup_loans
            && self.cleanup_maybe_loans == other.cleanup_maybe_loans;
        let same_cleanup = self.cleanup_roots == other.cleanup_roots
            && self.cleanup_conflicts == other.cleanup_conflicts
            && self.live_temporaries == other.live_temporaries;
        if !base.contract_readonly.shares(&other.contract_readonly)
            && !self.contract_readonly.shares(&other.contract_readonly)
        {
            self.contract_readonly.extend(&other.contract_readonly);
        }
        self.origins.join_since(&base.origins, &other.origins);
        if !base.tags.shares(&other.tags) && !self.tags.shares(&other.tags) {
            tags::join(&mut self.tags, &other.tags);
        }
        let changed: BTreeSet<_> = base
            .must
            .differences(&other.must)
            .into_iter()
            .chain(base.places.differences(&other.places))
            .chain(base.defused.differences(&other.defused))
            .chain(
                base.ownership_conflicts
                    .differences(&other.ownership_conflicts),
            )
            .collect();
        let roots: Vec<_> = changed
            .iter()
            .copied()
            .filter(|i| *i < self.ownership_conflicts.len())
            .collect();
        let loans = base.loans.differences(&other.loans);
        work::count_by(
            Counter::ValidateJoinRows,
            (changed.len() + loans.len()) as u64,
        );
        for i in changed {
            let live = self.places[i] & LIVE != 0;
            let incoming_live = other.places[i] & LIVE != 0;
            let defused = match (live, incoming_live) {
                (true, true) => self.defused[i] | other.defused[i],
                (false, true) => other.defused[i],
                _ => self.defused[i],
            };
            self.defused.set(i, defused);
            // Canonical roots occupy the first locals.len() rows. Ownership
            // conflicts matter only when both alternatives have a live root.
            if i < self.ownership_conflicts.len() {
                self.ownership_conflicts.set(
                    i,
                    self.ownership_conflicts[i]
                        || other.ownership_conflicts[i]
                        || live
                            && incoming_live
                            && self.must[i].has(MustFacts::OWNED)
                                != other.must[i].has(MustFacts::OWNED),
                );
            }
            self.must.set(
                i,
                self.must[i].join_live(other.must[i], live, incoming_live),
            );
        }
        self.places.or_since(&base.places, &other.places);
        self.loans.or_since(&base.loans, &other.loans);
        // A borrow-only root is shared (every write keeps that), so a root
        // whose two rows agree in `base` and `other` already holds its join.
        let sharing: BTreeSet<_> = base
            .shared
            .differences(&other.shared)
            .into_iter()
            .chain(base.borrow_only.differences(&other.borrow_only))
            .collect();
        work::count_by(Counter::ValidateJoinRows, sharing.len() as u64);
        for i in sharing {
            let only = (self.shared[i] || other.shared[i])
                && (self.borrow_only[i] || !self.shared[i])
                && (other.borrow_only[i] || !other.shared[i]);
            if self.borrow_only[i] != only {
                self.borrow_only.set(i, only);
            }
        }
        self.shared.or_since(&base.shared, &other.shared);
        self.nested_shared
            .or_since(&base.nested_shared, &other.nested_shared);
        self.borrowed_shares
            .join_since(&base.borrowed_shares, &other.borrowed_shares, |a, b| {
                a.join(b)
            });
        self.current_unique
            .and_since(&base.current_unique, &other.current_unique);
        self.unique_parts
            .join_since(&base.unique_parts, &other.unique_parts, |a, b| a.join(b));
        self.crossing_parts
            .join_since(&base.crossing_parts, &other.crossing_parts, |a, b| {
                a.join(b)
            });
        if !base.before.shares(&other.before)
            && !self.before.shares(&other.before)
            && !other.before.is_subset(&self.before)
        {
            self.before.extend(other.before.iter().copied());
        }
        self.content_loans
            .union_since(&base.content_loans, &other.content_loans, |a, b| a.join(b));
        self.sources
            .union_since(&base.sources, &other.sources, |a, b| a.extend(b));
        if !same_cleanup {
            for root in roots {
                model.refresh_cleanup(self, root);
            }
        }
        if !same_loans {
            for index in loans {
                model.refresh_cleanup_loan(self, index);
            }
        }
    }
}

/// The violation of a `@preserves_unique` body whose re-proof fails (MODE-2).
pub(super) const UNIQUE_PROOF_FAILED: &str = "preserved unique hand-back";

struct Model {
    creation: bool,
    crossing_depth: usize,
    header_names: BTreeSet<OriginRoot>,
    scope_locals: super::scope_locals::ScopeLocals,
    /// Canonical places, including a synthetic whole-place for every local.
    places: Vec<Place>,
    mapping: Vec<usize>,
    body_roots: Vec<Option<PlaceId>>,
    captured_origins: BTreeMap<OriginRoot, (usize, bool)>,
    inert: Vec<bool>,
    root_accesses: Vec<Vec<usize>>,
    /// Unnamed preparation/cursor holders of independently checked for protocols.
    protocol_holders: BTreeSet<usize>,
    canonical: crate::hashed::HashMap<(u32, hirsel_types::SharedPath<Projection>), usize>,
    root_parts: Vec<Vec<usize>>,
    roots: Vec<usize>,
    loans: Vec<(LoanId, PlaceId, bool)>,
    borrowed: Vec<bool>,
    mutable_parameters: Vec<bool>,
    state_promises: Vec<state_proof::Selection>,
    release: Vec<bool>,
    temporary: Vec<bool>,
    /// Scope nesting depth of each local; releases nest by it (DROP-2).
    depth: Vec<usize>,
    trivial_cleanup: bool,
    cleanup_sets: std::cell::RefCell<super::cleanup_proof::Sets>,
    cleanup_chains: Vec<Option<super::cleanup_proof::Chain>>,
    simple_cleanup: bool,
    noalloc_drop_checks: bool,
    result_reads: BTreeSet<(u32, Option<u32>)>,
    result_shareable: BTreeSet<hirsel_types::TypeParameterRef>,
    selected_sources: BTreeSet<usize>,
    environment_paths: std::rc::Rc<BTreeSet<Vec<super::flow_origins::EnvironmentField>>>,
    origin_paths: BTreeSet<Vec<hirsel_types::FieldId>>,
    /// The origin paths each place type admits (`admits_path`). A partition
    /// keeps no path its value has no field for, since no typed place reads it.
    typed_origin_paths: HashMap<TypeId, Vec<Vec<hirsel_types::FieldId>>>,
    /// The parameter root a `@preserves_unique` re-proof hands back (MODE-2).
    unique_proof: Option<usize>,
    /// The failure edge being checked is a release's that runs no user code
    /// (DROP-1), which ends no uniqueness hand-back proof.
    releasing: std::cell::Cell<bool>,
    /// While a return's cleanup runs, the roots whose shares it returns.
    returning_shares: std::cell::RefCell<BTreeSet<usize>>,
}
/// Whether a value of `ty` may have a part at `path`. Only a known layout
/// can exclude one: a nominal type is read through its representation, and
/// a generic, builtin, function or opaque type admits every path.
fn admits_path(
    signatures: &Reader<'_>,
    types: &TypeInterner,
    ty: TypeId,
    path: &[hirsel_types::FieldId],
) -> bool {
    let Some((field, rest)) = path.split_first() else {
        return true;
    };
    let field = field.0 as usize;
    match types.get(ty).map(|row| &row.kind) {
        Some(TypeKind::Primitive(_)) => false,
        Some(TypeKind::Tuple(parts)) => parts
            .get(field)
            .is_some_and(|part| admits_path(signatures, types, *part, rest)),
        Some(TypeKind::Record(fields)) => fields
            .get(field)
            .is_some_and(|part| admits_path(signatures, types, part.ty, rest)),
        Some(TypeKind::Variant(variants)) => variants.iter().any(|variant| {
            variant
                .fields
                .get(field)
                .is_some_and(|(_, part)| admits_path(signatures, types, *part, rest))
        }),
        // Arguments enter the representation as its parameters, which admit
        // every path, so no substitution is needed.
        Some(TypeKind::Named { definition, .. }) => match signatures.get(*definition) {
            Some(Signature {
                kind: SignatureKind::Type(t),
                ..
            }) => t
                .representation
                .is_none_or(|rep| admits_path(signatures, types, rep, path)),
            _ => true,
        },
        _ => true,
    }
}
impl Model {
    fn origin_paths_of(&self, ty: TypeId) -> &[Vec<hirsel_types::FieldId>] {
        &self.typed_origin_paths[&ty]
    }
    fn new(checker: &mut Checker<'_>) -> Self {
        let body = checker.body;
        let mut places: Vec<_> = body
            .locals
            .iter()
            .enumerate()
            .map(|(i, l)| Place {
                loan: None,
                binding: None,
                root: LocalId(i as u32),
                projection: vec![].into(),
                ty: l.ty,
            })
            .collect();
        let roots = (0..places.len()).collect();
        let mut canonical: crate::hashed::HashMap<_, _> = places
            .iter()
            .enumerate()
            .map(|(i, p)| ((p.root.0, p.projection.clone()), i))
            .collect();
        let mapping = body
            .places
            .iter()
            .map(|p| {
                work::count(Counter::ValidatePlaceRows);
                *canonical
                    .entry((p.root.0, p.projection.clone()))
                    .or_insert_with(|| {
                        let i = places.len();
                        places.push(p.clone());
                        i
                    })
            })
            .collect();
        let mut body_roots = vec![None; body.locals.len()];
        let mut root_accesses = vec![vec![]; body.locals.len()];
        for (index, place) in body.places.iter().enumerate() {
            root_accesses[place.root.0 as usize].push(index);
            if place.projection.is_empty() {
                body_roots[place.root.0 as usize].get_or_insert(PlaceId(index as u32));
            }
        }
        let mut protocol_holders = BTreeSet::new();
        for receipt in &checker.definition.protocol_associations {
            if receipt.body != checker.site.body {
                continue;
            }
            // protocol::receipts has already checked the exact declaration,
            // fingerprint and open/next receivers, including private operations.
            let Terminator::Call {
                arguments,
                returns: Some(CallReturn { destination, .. }),
                ..
            } = &body.blocks[receipt.open.0 as usize].terminator
            else {
                continue;
            };
            for place in [arguments[0].value.place, *destination] {
                let root = body.places[place.0 as usize].root.0 as usize;
                if matches!(
                    body.locals[root].storage,
                    Storage::Local | Storage::Temporary
                ) && !root_accesses[root].iter().any(|&access| {
                    body.places[access].binding.as_ref().is_some_and(|origin| {
                        matches!(
                            origin.root,
                            OriginRoot::Local(..) | OriginRoot::Parameter(_)
                        )
                    })
                }) {
                    protocol_holders.insert(root);
                }
            }
        }
        let mut captured_origins = BTreeMap::<OriginRoot, (usize, bool)>::new();
        for (index, place) in body.places.iter().enumerate() {
            work::count(Counter::ValidatePlaceRows);
            if body.locals[place.root.0 as usize].storage == Storage::Capture {
                if let Some(origin) = &place.binding {
                    // An access to the same storage through a loan is that
                    // place, and a part of it names a field of the same
                    // value: neither is a second carrier of the name.
                    captured_origins
                        .entry(origin.root.clone())
                        .and_modify(|(first, multiple)| {
                            let whole = &body.places[*first];
                            let part = place.projection.len()
                                - origin.fields.len().min(place.projection.len());
                            *multiple |= whole.root != place.root
                                || whole.projection.len() != part
                                || !place.projection.starts_with(whole.projection.iter());
                        })
                        .or_insert((index, false));
                }
            }
        }
        let mut root_parts = vec![vec![]; body.locals.len()];
        for (index, place) in places.iter().enumerate() {
            root_parts[place.root.0 as usize].push(index);
        }
        let loans: Vec<_> = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| {
                if let InstructionKind::BeginLoan {
                    id, place, mutable, ..
                } = i.kind
                {
                    Some((id, place, mutable))
                } else {
                    None
                }
            })
            .collect();
        let mut borrowed = vec![false; body.locals.len()];
        let mut mutable_parameters = vec![false; body.locals.len()];
        let function = if checker.site.body == checker.definition.entry {
            checker
                .signatures
                .get(checker.definition.path)
                .and_then(|s| {
                    if let SignatureKind::Function(f) = &s.kind {
                        Some(f)
                    } else {
                        None
                    }
                })
        } else {
            let inputs = checker
                .scratch
                .definition_inputs(checker.t, checker.definition);
            inputs.producers[checker.site.body.0 as usize]
                .first_function
                .and_then(|ty| match &checker.t.get(ty)?.kind {
                    TypeKind::Function(function) => Some(function),
                    _ => None,
                })
        };
        let creation = checker.body_callable().is_some_and(|(types, f)| {
            matches!(
                types
                    .get(f.result)
                    .and_then(|row| row.erased_captures.as_ref()),
                Some(hirsel_types::ErasedCaptures::FromArguments(_))
            )
        });
        let state_promises = state_proof::selections(function, body);
        if let Some(f) = function {
            for (p, l) in f.parameters.iter().zip(&body.parameters) {
                borrowed[l.0 as usize] = p.passing != PassingMode::Take;
                mutable_parameters[l.0 as usize] = p.passing == PassingMode::Mut;
            }
        }
        // The independent entry/use proof runs before any flow model is built.
        // Only that contextual callback's designated formal owns the grant.
        if let Some(grant) = &body.arena_scope_grant {
            if let Some(local) = body.parameters.get(grant.arena_parameter.0 as usize) {
                borrowed[local.0 as usize] = false;
            }
        }
        if checker.site.body == checker.definition.entry
            && checker
                .signatures
                .get(checker.definition.path)
                .is_some_and(|s| matches!(&s.kind, SignatureKind::Type(t) if t.resource))
        {
            // DROP-3: finalization borrows self; its fields are released by the
            // caller after the finalizer returns, not by this body.
            for l in &body.parameters {
                borrowed[l.0 as usize] = true;
            }
        }
        let release: Vec<_> = body
            .locals
            .iter()
            .map(|l| needs_release(checker, l.ty, checker.t))
            .collect();
        let inputs = checker
            .scratch
            .definition_inputs(checker.t, checker.definition);
        // Copy/Take describes construction of the environment, not ownership
        // at every call. A many-callable lends even its owned fields; only a
        // once-callable transfers them to the invocation. In particular its
        // body must not drop a handle still owned by the environment.
        let lends_captures = inputs.producers[checker.site.body.0 as usize].lends_captures;
        for &(owner, value) in &inputs.producers[checker.site.body.0 as usize].captures {
            work::count(Counter::ValidateDefinitionInputRows);
            let Rvalue::Closure { captures, .. } =
                &checker.definition.bodies[owner.0 as usize].rvalues[value.0 as usize]
            else {
                unreachable!()
            };
            let slots = body
                .locals
                .iter()
                .enumerate()
                .filter(|(_, local)| local.storage == Storage::Capture);
            for ((index, _), operand) in slots.zip(captures) {
                borrowed[index] =
                    lends_captures || matches!(operand.usage, UseKind::Borrow | UseKind::Mut);
                mutable_parameters[index] = operand.usage == UseKind::Mut;
            }
        }
        let mut environment_paths = inputs.environment_paths.clone();
        // A constructor may store a loan in an unobserved sibling of an
        // observed place. Keep that direct sibling distinct instead of
        // summarizing it at the parent and poisoning the observed child.
        // Only typed places contribute children: no recursive path closure.
        let mut children = HashMap::default();
        let store = checker.t;
        for place in &body.places {
            if place.projection.is_empty() {
                continue; // Constructor field keys already distinguish roots.
            }
            let keys = children.entry(place.ty).or_insert_with(|| {
                use super::flow_origins::EnvironmentField as F;
                let layout = expand_representation(
                    store,
                    checker.memo,
                    place.ty,
                    &mut |_, path, arguments: &[GenericArgument]| {
                        Ok(checker.signatures.get(path).cloned().map(|signature| {
                            (
                                std::sync::Arc::new(signature),
                                vec![hirsel_types::AuthoritySummary::Unknown; arguments.len()],
                            )
                        }))
                    },
                    None,
                );
                let kind = match layout {
                    Ok(root) => store.get(root).map(|row| row.kind.clone()),
                    Err(RepresentationError {
                        failure: RepresentationFailure::Query(error),
                        ..
                    }) => checker.proof_failure(error, None),
                    Err(_) => None, // Opaque rows expose no sibling schema.
                };
                match kind {
                    Some(TypeKind::Tuple(fields)) => (0..fields.len())
                        .map(|i| F::Field(i as u32))
                        .collect::<Vec<_>>(),
                    Some(TypeKind::Record(fields)) => {
                        (0..fields.len()).map(|i| F::Field(i as u32)).collect()
                    }
                    Some(TypeKind::Box(_)) => vec![F::Field(0)],
                    Some(TypeKind::Variant(variants)) => variants
                        .iter()
                        .enumerate()
                        .flat_map(|(v, variant)| {
                            (0..variant.fields.len()).map(move |f| F::Variant(v as u32, f as u32))
                        })
                        .collect(),
                    Some(TypeKind::Builtin { kind, .. })
                        if matches!(
                            kind,
                            hirsel_types::BuiltinType::Array
                                | hirsel_types::BuiltinType::List
                                | hirsel_types::BuiltinType::Inline
                                | hirsel_types::BuiltinType::Simd
                                | hirsel_types::BuiltinType::Map
                                | hirsel_types::BuiltinType::SortedMap
                                | hirsel_types::BuiltinType::Set
                        ) =>
                    {
                        vec![F::Index]
                    }
                    _ => vec![],
                }
            });
            if !keys.is_empty() {
                let prefix = super::flow_origins::EnvironmentField::path(&place.projection);
                for key in keys.iter() {
                    let mut path = prefix.clone();
                    path.push(key.clone());
                    // Intermediate construction temporaries carry the suffix
                    // before an outer constructor adds the observed prefix.
                    for start in 0..path.len() {
                        std::rc::Rc::make_mut(&mut environment_paths)
                            .insert(path[start..].to_vec());
                    }
                }
            }
        }
        let result_reads = result_read_sites(body);
        let result_shareable = checker.result_permissions();
        let selected_sources = match checker
            .lifetime(&checker.t.get(body.result).unwrap().modes.relation)
            .named_parameters()
        {
            Some(parameters) => parameters
                .iter()
                .filter_map(|p| {
                    let ordinal = match p {
                        ParameterRef::Bound {
                            depth: 0,
                            parameter,
                        } => parameter.0,
                        ParameterRef::Body { scope, parameter }
                            if *scope == body.parameter_scope =>
                        {
                            parameter.0
                        }
                        _ => return None,
                    };
                    body.parameters
                        .get(ordinal as usize)
                        .map(|local| local.0 as usize)
                        .filter(|root| borrowed[*root])
                })
                .collect(),
            _ => BTreeSet::new(),
        };
        let temporary = body
            .locals
            .iter()
            .map(|local| matches!(local.storage, Storage::Temporary | Storage::ReturnSlot))
            .collect();
        // Parent walks are bounded by the scope count, so malformed cycles end.
        let depth: Vec<usize> = body
            .locals
            .iter()
            .map(|local| {
                let mut depth = 0;
                let mut scope = body.scopes.get(local.scope.0 as usize);
                while let Some(parent) = scope.and_then(|s| s.parent) {
                    if depth == body.scopes.len() {
                        break;
                    }
                    depth += 1;
                    scope = body.scopes.get(parent.0 as usize);
                }
                depth
            })
            .collect();
        let trivial_cleanup = loans.is_empty() && !release.iter().any(|release| *release);
        let loan_sources: BTreeMap<_, _> = loans
            .iter()
            .map(|(id, source, _)| (id.0, *source))
            .collect();
        let simple_cleanup = !mutable_parameters.iter().any(|mutable| *mutable)
            && loans.iter().all(|(id, source, _)| {
                let access = &body.places[source.0 as usize];
                access.loan.is_none_or(|parent| {
                    parent.0 < id.0
                        && loan_sources.get(&parent.0).is_some_and(|source| {
                            let ancestor = &body.places[source.0 as usize];
                            access.root == ancestor.root
                                && access.projection.starts_with(&ancestor.projection)
                        })
                })
            });
        // Its keys are roots (locals) and loan ids, from begun loans and from
        // the cleanup chain's own loan ends.
        let largest_key = body
            .cleanup_nodes
            .iter()
            .filter_map(|node| match node.action {
                CleanupAction::EndLoan { loan, .. } => Some(loan.0),
                _ => None,
            })
            .chain(loans.iter().map(|(id, _, _)| id.0))
            .chain(body.locals.len().checked_sub(1).map(|i| i as u32))
            .max()
            .unwrap_or(0);
        let mut cleanup_sets = super::cleanup_proof::Sets::new(largest_key);
        let mut cleanup_chains: Vec<Option<super::cleanup_proof::Chain>> = Vec::new();
        for node in &body.cleanup_nodes {
            if !simple_cleanup {
                cleanup_chains.push(None);
                continue;
            }
            let chain = (|| {
                let mut chain = match node.parent {
                    None => super::cleanup_proof::Chain::default(),
                    Some(parent) => cleanup_chains[parent.0 as usize]?,
                };
                let place = match node.action {
                    CleanupAction::Drop {
                        place,
                        if_live: true,
                    } if chain.loans == 0 => place,
                    CleanupAction::EndLoan { loan, .. } => {
                        if chain.first_loan.is_some_and(|next| loan.0 <= next) {
                            return None;
                        }
                        chain.first_loan = Some(loan.0);
                        chain.loans = cleanup_sets.set(chain.loans, loan.0, true);
                        return Some(chain);
                    }
                    _ => return None,
                };
                let place = &body.places[place.0 as usize];
                let root = place.root.0;
                if !place.projection.is_empty()
                    || place.loan.is_some()
                    || place.binding.is_some()
                    || !release[root as usize]
                {
                    return None;
                }
                let temporary = matches!(
                    body.locals[root as usize].storage,
                    Storage::Temporary | Storage::ReturnSlot
                );
                // With at most one counted temporary, its dynamic evaluation
                // order needs no pair relation. Releases still nest by scope.
                let release = (depth[root as usize], temporary, root as usize);
                if (temporary && chain.temporary)
                    || chain
                        .next
                        .is_some_and(|next| !cleanup_order::releases_after(Some(release), next))
                {
                    return None;
                }
                chain.temporary |= temporary;
                chain.next = Some(release);
                chain.roots = cleanup_sets.set(chain.roots, root, true);
                Some(chain)
            })();
            cleanup_chains.push(chain);
        }
        let selectors = &inputs.origin_selectors;
        let mut origin_paths = selectors.clone();
        for place in &places {
            let path: Vec<_> = place
                .projection
                .iter()
                .map_while(|p| match p {
                    Projection::Field(f) | Projection::VariantField { field: f, .. } => {
                        Some(hirsel_types::FieldId(*f))
                    }
                    Projection::Index(_) => None,
                })
                .collect();
            if !path.is_empty() {
                origin_paths.insert(path);
            }
        }
        let mut prefixes = BTreeSet::new();
        for place in &body.places {
            let prefix: Vec<_> = place
                .projection
                .iter()
                .map_while(|p| match p {
                    Projection::Field(f) | Projection::VariantField { field: f, .. } => {
                        Some(hirsel_types::FieldId(*f))
                    }
                    Projection::Index(_) => None,
                })
                .collect();
            if !prefix.is_empty() {
                prefixes.insert(prefix);
            }
        }
        for prefix in prefixes {
            for selector in selectors {
                let mut path = prefix.clone();
                path.extend(selector);
                origin_paths.insert(path);
            }
        }
        let mut typed_origin_paths = HashMap::default();
        for place in body.places.iter().chain(&places) {
            typed_origin_paths.entry(place.ty).or_insert_with(|| {
                origin_paths
                    .iter()
                    .filter(|path| admits_path(checker.signatures, checker.t, place.ty, path))
                    .cloned()
                    .collect()
            });
        }
        let mut authorities = HashMap::default();
        let inert = places
            .iter()
            .map(|place| {
                *authorities.entry(place.ty).or_insert_with(|| {
                    checker.authority_with_generics(
                        checker.t,
                        place.ty,
                        &checker.generic_declarations(),
                    ) == hirsel_types::AuthoritySummary::inert()
                })
            })
            .collect();
        let noalloc_drop_checks = if checker.noalloc_body() {
            let declarations = checker.generic_declarations();
            let mut effects = HashMap::default();
            body.locals.iter().any(|local| {
                *effects.entry(local.ty).or_insert_with(|| {
                    !checker.bound(
                        checker.t,
                        local.ty,
                        hirsel_types::Bound::NoallocDrop,
                        &declarations,
                    )
                })
            })
        } else {
            false
        };
        // Successive destructuring can consume a complete typed payload path
        // through several temporary roots. Retain that path, rather than only
        // the longest projection on one root. Callable headers are not payload.
        let mut function_depths = HashMap::default();
        for local in &body.locals {
            let mut pending = vec![(local.ty, false)];
            while let Some((id, leaving)) = pending.pop() {
                let row = checker.t.get(id).unwrap();
                if leaving {
                    let depth = row
                        .kind
                        .value_parts()
                        .iter()
                        .filter_map(|child| function_depths.get(child).copied().flatten())
                        .max()
                        .map(|depth: usize| depth + 1);
                    function_depths.insert(id, depth);
                } else if !function_depths.contains_key(&id) {
                    if matches!(row.kind, TypeKind::Function(_)) {
                        function_depths.insert(id, Some(0));
                    } else {
                        function_depths.insert(id, None);
                        pending.push((id, true));
                        pending.extend(
                            row.kind
                                .value_parts()
                                .into_iter()
                                .map(|child| (child, false)),
                        );
                    }
                }
            }
        }
        let function_depth = function_depths
            .values()
            .filter_map(|depth| *depth)
            .max()
            .unwrap_or(0);
        let declarations: BTreeSet<_> = checker
            .definition
            .bodies
            .iter()
            .filter(|frame| frame.parameter_scope != body.parameter_scope)
            .flat_map(|frame| {
                (0..frame.parameters.len()).map(move |ordinal| {
                    OriginRoot::Parameter(ParameterRef::body(
                        frame.parameter_scope,
                        hirsel_types::ParamId(ordinal as u32),
                    ))
                })
            })
            .collect();
        let mut header_names = BTreeSet::new();
        for local in body.locals.iter().filter(|_| !declarations.is_empty()) {
            let _ = checker.scratch.origin_types[checker.site.body.0 as usize].visit(
                checker.t,
                local.ty,
                &|_| true,
                |origin, depth| {
                    if (depth > 0
                        || local.storage == Storage::Capture && checker.arena_place(origin))
                        && declarations.contains(&origin.root)
                    {
                        header_names.insert(origin.root.clone());
                    }
                    Ok(())
                },
            );
        }
        Self {
            header_names,
            protocol_holders,
            crossing_depth: body
                .places
                .iter()
                .map(|place| place.projection.len())
                .max()
                .unwrap_or(0)
                .max(function_depth)
                .max(1),
            scope_locals: super::scope_locals::ScopeLocals::new(
                body.scopes.iter().map(|scope| scope.parent),
                body.locals.iter().map(|local| local.scope),
            ),
            noalloc_drop_checks,
            inert,
            trivial_cleanup,
            simple_cleanup,
            cleanup_sets: std::cell::RefCell::new(cleanup_sets),
            cleanup_chains,
            result_reads,
            result_shareable,
            selected_sources,
            environment_paths,
            typed_origin_paths,
            origin_paths,
            places,
            mapping,
            canonical,
            body_roots,
            captured_origins,
            root_accesses,
            root_parts,
            roots,
            loans,
            borrowed,
            mutable_parameters,
            state_promises,
            creation,
            release,
            temporary,
            depth,
            unique_proof: None,
            releasing: std::cell::Cell::new(false),
            returning_shares: std::cell::RefCell::new(BTreeSet::new()),
        }
    }
    fn refresh_cleanup(&self, state: &mut State, root: usize) {
        if !self.release[root] {
            return;
        }
        let live = state.places[self.roots[root]] & LIVE != 0;
        let mut sets = self.cleanup_sets.borrow_mut();
        state.live_temporaries = sets.set(
            state.live_temporaries,
            root as u32,
            live && self.temporary[root],
        );
        state.cleanup_roots = sets.set(
            state.cleanup_roots,
            root as u32,
            live && state.must[root].has(MustFacts::OWNED),
        );
        state.cleanup_conflicts = sets.set(
            state.cleanup_conflicts,
            root as u32,
            live && state.ownership_conflicts[root],
        );
    }
    fn refresh_cleanup_loan(&self, state: &mut State, index: usize) {
        if !self.simple_cleanup {
            return;
        }
        let mask = state.loans[index];
        let key = self.loans[index].0.0;
        let mut sets = self.cleanup_sets.borrow_mut();
        state.cleanup_loans = sets.set(state.cleanup_loans, key, mask & LIVE != 0);
        state.cleanup_maybe_loans = sets.set(
            state.cleanup_maybe_loans,
            key,
            mask & LIVE != 0 && mask != LIVE,
        );
    }
    fn proves_cleanup(&self, body: &Body, state: &State, id: CleanupId) -> bool {
        if !self.simple_cleanup || state.cleanup_conflicts != 0 || state.cleanup_maybe_loans != 0 {
            return false;
        }
        let proof = match body.cleanups[id.0 as usize].head {
            None => Some(super::cleanup_proof::Chain::default()),
            Some(head) => self.cleanup_chains[head.0 as usize],
        };
        proof.is_some_and(|proof| {
            proof.roots == state.cleanup_roots && proof.loans == state.cleanup_loans
        })
    }
    fn access_chain(&self, body: &Body, access: &Place) -> Option<BTreeSet<usize>> {
        let mut result = BTreeSet::new();
        let mut access = access;
        while let Some(id) = access.loan {
            let index = self.loan(id);
            let source = &body.places[self.loans[index].1.0 as usize];
            if !result.insert(index)
                || source.root != access.root
                || !access.projection.starts_with(&source.projection)
            {
                return None;
            }
            access = source;
        }
        Some(result)
    }
    fn binding_valid(&self, state: &State, access: &Place, body: &Body) -> bool {
        let Some(binding) = &access.binding else {
            return true;
        };
        state
            .origins
            .locations
            .get(&binding.root)
            .is_some_and(|places| {
                !places.is_empty()
                    && places.iter().all(|(index, required)| {
                        let base = &self.places[*index];
                        let permission = required.is_none()
                            || self.access_chain(body, access).is_some_and(|chain| {
                                chain.iter().any(|i| Some(self.loans[*i].0.0) == *required)
                            });
                        permission
                            && base.root == access.root
                            && access.projection.starts_with(&base.projection)
                            && access
                                .projection
                                .iter()
                                .skip(base.projection.len())
                                .map(|p| match p {
                                    Projection::Field(f)
                                    | Projection::VariantField { field: f, .. } => {
                                        Some(hirsel_types::FieldId(*f))
                                    }
                                    Projection::Index(_) => None,
                                })
                                .eq(binding.fields.iter().copied().map(Some))
                    })
            })
    }
    fn loan(&self, id: LoanId) -> usize {
        self.loans.iter().position(|l| l.0 == id).unwrap()
    }
    fn overlaps(&self, a: usize, b: usize) -> bool {
        let a = &self.places[a];
        let b = &self.places[b];
        if a.root != b.root {
            return false;
        }
        for (a, b) in a.projection.iter().zip(&b.projection) {
            match (a, b) {
                // All elements of a sequence are one place; the fields below
                // that place are still distinct (OWN-3, #77).
                (Projection::Index(_), Projection::Index(_)) => (),
                (
                    Projection::VariantField { variant: a, .. },
                    Projection::VariantField { variant: b, .. },
                ) if a != b => return true,
                _ if a != b => return false,
                _ => (),
            }
        }
        true
    }
    fn contains(&self, a: usize, b: usize) -> bool {
        work::count(Counter::ValidatePlaceRows);
        let a = &self.places[a];
        let b = &self.places[b];
        a.root == b.root
            && a.projection.len() <= b.projection.len()
            && a.projection.iter().zip(&b.projection).all(|(a, b)| {
                a == b || matches!((a, b), (Projection::Index(_), Projection::Index(_)))
            })
    }
    /// A whole-value access observes every tracked component, without
    /// invalidating a disjoint sibling when only one field is accessed.
    fn parts(&self, place: PlaceId) -> impl Iterator<Item = usize> + '_ {
        let index = self.mapping[place.0 as usize];
        self.contents(index)
    }
    fn contents(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        self.root_parts[self.places[index].root.0 as usize]
            .iter()
            .copied()
            .filter(move |part| self.contains(index, *part))
    }
    fn initial(
        &self,
        body: &Body,
        types: &TypeInterner,
        origin_types: &super::origin_types::Types,
        lifetime_global: &dyn Fn(TypeId) -> bool,
        arena: &dyn Fn(&Origin) -> bool,
        captures: &[super::flow_origins::CapturedOrigin],
    ) -> State {
        let cursor_len = if body.borrow_cursors.is_empty() {
            0
        } else {
            body.locals.len()
        };
        let mut state = State {
            cursor_views: Rows::new(None, cursor_len)
                .with_copy_counter(Counter::ValidateCopiedRows),
            cursor_contracts: super::origin_dependencies::DependencyRows::new(cursor_len),
            envelopes: Rows::new(None, if self.creation { body.locals.len() } else { 0 })
                .with_copy_counter(Counter::ValidateCopiedRows),
            audit: false,
            cleanup_roots: 0,
            live_temporaries: 0,
            cleanup_conflicts: 0,
            cleanup_loans: 0,
            cleanup_maybe_loans: 0,
            contract_readonly: body
                .parameters
                .iter()
                .map(|p| p.0 as usize)
                .filter(|root| self.borrowed[*root] && !self.mutable_parameters[*root])
                .collect(),
            origins: super::flow_origins::OriginState::new(self.places.len()),
            places: BitRows::new(DEAD, self.places.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            defused: BitRows::new(1, self.places.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            must: BitRows::new(MustFacts::default(), self.places.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            ownership_conflicts: BitRows::new(false, body.locals.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            shared: BitRows::new(false, body.locals.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            nested_shared: BitRows::new(false, body.locals.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            borrow_only: BitRows::new(false, body.locals.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            borrowed_shares: Rows::new(loan_content::Content::default(), body.locals.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            current_unique: BitRows::new(false, body.locals.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            unique_parts: Rows::new(crossing::Parts::default(), body.locals.len()),
            crossing_parts: Rows::new(crossing::Parts::default(), body.locals.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            tags: BTreeMap::new().into(),
            loans: BitRows::new(DEAD, self.loans.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            content_loans: Rows::new(loan_content::Content::default(), body.locals.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            sources: Rows::new(BTreeSet::new(), self.places.len())
                .with_copy_counter(Counter::ValidateCopiedRows),
            before: BTreeSet::new().into(),
        };
        for (i, l) in body.locals.iter().enumerate() {
            if body.parameters.contains(&LocalId(i as u32)) || l.storage == Storage::Capture {
                for &p in &self.root_parts[i] {
                    {
                        state.places.set(p, LIVE);
                        if self.borrowed[i] && !self.mutable_parameters[i]
                            || !self.borrowed[i] && !lifetime_global(l.ty)
                        {
                            state.sources[p].insert(i);
                        }
                    }
                }
                state.must.set_fact(i, MustFacts::OWNED, !self.borrowed[i]);
                state
                    .must
                    .set_fact(i, MustFacts::GLOBAL, lifetime_global(l.ty));
                state.must.set_fact(
                    i,
                    MustFacts::SENDABLE,
                    types.get(l.ty).unwrap().modes.sendability
                        == hirsel_types::Sendability::Sendable,
                );
                state.current_unique.set(
                    i,
                    types.get(l.ty).unwrap().modes.uniqueness == hirsel_types::Uniqueness::Unique,
                );
            }
        }
        for root in 0..body.locals.len() {
            self.refresh_cleanup(&mut state, root);
        }
        for (ordinal, local) in body.parameters.iter().enumerate() {
            let origin = hirsel_types::Origin {
                root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::body(
                    body.parameter_scope,
                    hirsel_types::ParamId(ordinal as u32),
                )),
                fields: vec![].into(),
            };
            state.origins.identities[self.roots[local.0 as usize]].insert(origin.clone());
            state.origins.locations.insert(
                origin.root.clone(),
                BTreeSet::from([(self.roots[local.0 as usize], None)]),
            );

            if !self.borrowed[local.0 as usize] {
                state.origins.owners[self.roots[local.0 as usize]].insert(origin.clone());
            }
            state
                .origins
                .bindings
                .insert(origin.root.clone(), BTreeSet::from([origin]));
        }
        for (index, place) in self.places.iter().enumerate() {
            if state.places[index] != LIVE {
                continue;
            }
            let proof = super::flow_origins::remember(
                &types,
                origin_types,
                place.ty,
                &state.origins.bindings,
                arena,
            );
            state.origins.retained.set(
                index,
                super::flow_origins::retained(&types, origin_types, place.ty, &proof, arena),
            );
            state.origins.proofs[index] = proof;
        }
        for ((local, _), capture) in body
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.storage == Storage::Capture)
            .zip(captures)
        {
            let root = self.roots[local];
            state.crossing_parts[local] = capture.crossing.clone();
            if capture.nested_shared {
                state.shared.set(local, true);
                state.nested_shared.set(local, true);
            }
            if capture.crossing.get(&[]) == Some(true) {
                state.must.set_fact(local, MustFacts::SENDABLE, true);
            }
            tags::capture(&mut state, local as u32, &capture.tags);
            state.origins.identities[root] = capture.identities.clone().into();
            state.origins.parts[root] = capture.parts.clone().into();
            state.origins.owners[root] = capture.owners.clone().into();
            for (place, p) in body.places.iter().enumerate() {
                let introduced = body.blocks.iter().flat_map(|b| &b.instructions).any(|i| matches!(i.kind, InstructionKind::BindOrigin { place: target, .. } if target.0 as usize == place));
                if !introduced && p.root.0 as usize == local && p.projection.is_empty() {
                    if let Some(binding) = &p.binding {
                        state
                            .origins
                            .locations
                            .insert(binding.root.clone(), BTreeSet::from([(root, None)]));
                        state
                            .origins
                            .bindings
                            .insert(binding.root.clone(), capture.identities.clone());
                    }
                }
            }
            if capture.contract_readonly {
                state.contract_readonly.insert(local);
            }
            for identity in capture
                .identities
                .iter()
                .chain(capture.proof.values().flatten())
                .chain(capture.parts.values().flatten())
            {
                state
                    .origins
                    .bindings
                    .entry(identity.root.clone())
                    .or_insert_with(|| {
                        BTreeSet::from([Origin {
                            root: identity.root.clone(),
                            fields: vec![].into(),
                        }])
                    });
            }
            for &index in &self.root_parts[local] {
                let place = &self.places[index];
                {
                    state.origins.proofs[index] = capture.proof.clone();
                    state.origins.retained.set(
                        index,
                        super::flow_origins::retained(
                            &types,
                            origin_types,
                            place.ty,
                            &capture.proof,
                            arena,
                        ),
                    );
                    if place.projection.is_empty() {
                        state
                            .origins
                            .environments
                            .set(index, capture.environment.clone());
                    }
                    state.origins.expired[index] = capture.expired.clone();
                }
            }
        }
        // A parameter type may name an owner of an enclosing body that this
        // body does not capture. Nothing here can observe or change that
        // owner, so its name is a constant of the declaration: every caller
        // proved the argument against it. Captured owners keep the proof
        // their environment carries.
        // Callable headers can state the same fixed declaration contract
        // before invocation. These names are metadata, not captured values;
        // retain any independently transported capture proof already present.
        // Captured arena bounds retain their declaration names too: arena
        // placement is deliberately absent from a captured data-origin proof.
        for name in &self.header_names {
            state
                .origins
                .bindings
                .entry(name.clone())
                .or_insert_with(|| {
                    BTreeSet::from([Origin {
                        root: name.clone(),
                        fields: vec![].into(),
                    }])
                });
        }
        let outer = |root: &OriginRoot| match root {
            OriginRoot::Parameter(hirsel_types::ParameterRef::Body { scope, .. }) => {
                *scope != body.parameter_scope
            }
            OriginRoot::Local(..) | OriginRoot::Expression(..) => true,
            _ => false,
        };
        for local in &body.parameters {
            let root = self.roots[local.0 as usize];
            let mut names = BTreeSet::new();
            let _ = origin_types.visit(&types, self.places[root].ty, &|_| true, |origin, _| {
                if outer(&origin.root) && !state.origins.bindings.contains_key(&origin.root) {
                    names.insert(origin.root.clone());
                }
                Ok(())
            });
            // Captures and header names were installed after the initial proofs.
            // Rebuild even when they already supplied every enclosing name.
            for name in names {
                let identity = Origin {
                    root: name.clone(),
                    fields: vec![].into(),
                };
                state
                    .origins
                    .bindings
                    .insert(name, BTreeSet::from([identity]));
            }
            for &index in &self.root_parts[local.0 as usize] {
                let place = &self.places[index];
                let proof = super::flow_origins::remember(
                    &types,
                    origin_types,
                    place.ty,
                    &state.origins.bindings,
                    arena,
                );
                state.origins.retained.set(
                    index,
                    super::flow_origins::retained(&types, origin_types, place.ty, &proof, arena),
                );
                state.origins.proofs[index] = proof;
            }
            // A function or abstract T may contain borrowed captures. Its
            // written root lifetime names actual retained dependencies, not
            // callable-header metadata. Keep them in the independent hidden
            // environment so copies/transfers cannot erase them with a wider
            // receiving row; concrete data deep copies still detach as before.
            let row = types.get(body.locals[local.0 as usize].ty).unwrap();
            if matches!(row.kind, TypeKind::Function(_) | TypeKind::Parameter(_)) {
                let mut retained = BTreeSet::new();
                for reach in row.modes.relation.reaches() {
                    let hirsel_types::Reach::Place(origin) = reach else {
                        continue;
                    };
                    if arena(origin) {
                        continue;
                    }
                    retained.insert(origin.clone());
                    if let Some(identities) = state.origins.bindings.get(&origin.root) {
                        for identity in identities {
                            let mut identity = identity.clone();
                            identity.fields.extend(&origin.fields);
                            retained.insert(identity);
                        }
                    }
                }
                let environment =
                    super::flow_origins::Environment::closure(retained, BTreeSet::new());
                state
                    .origins
                    .environments
                    .update(root, |current| current.join(&environment));
            }
        }
        state
    }
}

/// Backward demand follows only value transfers into the result, killing
/// overwritten slots. Calls and captures never grant the result-read permission.
/// The root may now share counted storage with another value (MODE-2).
fn share_root(state: &mut State, root: usize) {
    state.shared.set(root, true);
    state.nested_shared.set(root, true);
    state.borrow_only.set(root, false);
}
fn result_read_sites(body: &Body) -> BTreeSet<(u32, Option<u32>)> {
    let mut incoming = vec![BTreeSet::new(); body.blocks.len()];
    let graph: Vec<Vec<usize>> = body
        .blocks
        .iter()
        .map(|block| {
            body.successors(&block.terminator)
                .into_iter()
                .map(|b| b.0 as usize)
                .collect()
        })
        .collect();
    let mut predecessors = vec![vec![]; body.blocks.len()];
    for (id, next) in graph.iter().enumerate() {
        for &next in next {
            predecessors[next].push(id);
        }
    }
    let mut pending: VecDeque<_> = hirsel_graph::postorder(
        &graph,
        std::iter::once(body.entry.0 as usize).chain(0..graph.len()),
    )
    .into();
    let mut queued = vec![true; body.blocks.len()];
    // Demand starts empty and grows monotonically. A read permission once
    // discovered therefore remains present at the least fixed point.
    let mut sites = BTreeSet::new();
    while let Some(block_id) = pending.pop_front() {
        queued[block_id] = false;
        work::count(Counter::ValidateResultDemandSteps);
        let block = &body.blocks[block_id];
        let mut demand: BTreeSet<u32> = graph[block_id]
            .iter()
            .flat_map(|target| incoming[*target].iter().copied())
            .collect();
        match &block.terminator {
            Terminator::Return { value, .. } => {
                demand.insert(body.places[value.place.0 as usize].root.0);
                sites.insert((block_id as u32, None));
            }
            Terminator::Call {
                returns: Some(CallReturn { destination, .. }),
                ..
            } => {
                demand.remove(&body.places[destination.0 as usize].root.0);
            }
            _ => (),
        }
        for (index, instruction) in block.instructions.iter().enumerate().rev() {
            if let InstructionKind::Assign { destination, value } = instruction.kind {
                let destination = &body.places[destination.0 as usize];
                if !demand.contains(&destination.root.0) {
                    continue;
                }
                if destination.projection.is_empty() {
                    demand.remove(&destination.root.0);
                }
                let operands: Vec<_> = match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(value) => vec![value],
                    Rvalue::Construct { fields, .. } => fields.iter().map(|f| &f.value).collect(),
                    _ => vec![],
                };
                if !operands.is_empty() {
                    sites.insert((block_id as u32, Some(index as u32)));
                }
                for operand in operands {
                    demand.insert(body.places[operand.place.0 as usize].root.0);
                }
            }
        }
        if incoming[block_id] != demand {
            incoming[block_id] = demand;
            for &previous in &predecessors[block_id] {
                if !queued[previous] {
                    queued[previous] = true;
                    pending.push_back(previous);
                }
            }
        }
    }
    sites
}

fn structural_property(
    c: &Checker<'_>,
    root: TypeId,
    source: &TypeInterner,
    bound: hirsel_types::Bound,
) -> bool {
    let query = match bound {
        hirsel_types::Bound::Plain => super::proofs::Proof::Plain,
        hirsel_types::Bound::Copyable => super::proofs::Proof::Copyable,
        _ => unreachable!(),
    };
    c.proven(source, root, query, c.generic_declarations())
}

pub(super) fn needs_release(c: &Checker<'_>, t: TypeId, store: &TypeInterner) -> bool {
    !structural_property(c, t, store, hirsel_types::Bound::Plain)
}

fn copyable(c: &Checker<'_>, t: TypeId, store: &TypeInterner) -> bool {
    structural_property(c, t, store, hirsel_types::Bound::Copyable)
}

impl Checker<'_> {
    /// User-visible destruction (DROP-1/2): a value that may contain a
    /// resource, which is noncopyable, or a once callable.
    fn destroyed(&self, t: TypeId) -> bool {
        let store = self.t;
        !copyable(self, t, store)
            || store.get(t).unwrap().modes.multiplicity == hirsel_types::Multiplicity::Once
    }
}

impl Checker<'_> {
    pub(super) fn flow(&mut self, captures: &[super::flow_origins::CapturedOrigin], report: bool) {
        self.flow_with(captures, report, None);
    }
    /// With `proof`, re-prove a `@preserves_unique` body (MODE-2): the selected
    /// parameter and every counted taken parameter enter unique, other `mut`
    /// parameters as declared, and the selected parameter must leave unique.
    pub(super) fn flow_with(
        &mut self,
        captures: &[super::flow_origins::CapturedOrigin],
        report: bool,
        proof: Option<hirsel_types::ParamId>,
    ) {
        let started = super::profile_start();
        let mut model = Model::new(self);
        model.unique_proof = proof
            .and_then(|p| self.body.parameters.get(p.0 as usize))
            .map(|l| l.0 as usize);
        let loop_expirations = super::flow_origins::loop_expirations(self.body);
        // Dead-edge audits never contaminate executable predecessor joins.
        let mut incoming = vec![[None, None]; self.body.blocks.len()];
        let lifetime_global = |ty: TypeId| {
            self.lifetime(&self.t.get(ty).unwrap().modes.relation)
                .is_global()
        };
        let mut initial = model.initial(
            self.body,
            self.t,
            &self.scratch.origin_types[self.site.body.0 as usize],
            &lifetime_global,
            &|o| self.arena_place(o),
            captures,
        );
        for (root, local) in self.body.locals.iter().enumerate() {
            if !self.body.parameters.contains(&LocalId(root as u32))
                && local.storage != Storage::Capture
            {
                continue;
            }
            let proof = self
                .proof(
                    self.t,
                    local.ty,
                    super::proofs::Proof::Content,
                    &self.generic_declarations(),
                )
                .unwrap_or_else(|error| {
                    self.proof_failure(error, hirsel_types::Sendability::NotSendable)
                });
            if proof == hirsel_types::Sendability::Sendable {
                initial.must.set_fact(root, MustFacts::SENDABLE, true);
                // This is referent evidence. The parameter's borrowed access
                // remains Local, while owned extraction can acquire its Global
                // content independently of the access relation.
                initial.must.set_fact(root, MustFacts::GLOBAL, true);
            }
        }
        // MODE-2: the selection and every taken parameter enter unique; another
        // `mut` parameter enters as declared.
        if let Some(selected) = model.unique_proof {
            for local in &self.body.parameters {
                let root = local.0 as usize;
                if root == selected || !model.borrowed[root] && !model.mutable_parameters[root] {
                    initial.current_unique.set(root, true);
                }
            }
        }
        self.initial_authority(&model, &mut initial);
        self.initial_creation(&model, &mut initial);
        incoming[self.body.entry.0 as usize][0] = Some(initial);
        let graph: Vec<Vec<usize>> = self
            .body
            .blocks
            .iter()
            .map(|block| {
                self.flow_successors(&block.terminator)
                    .into_iter()
                    .map(|id| id.0 as usize)
                    .collect()
            })
            .collect();
        let mut rank = vec![0; graph.len()];
        for (index, block) in hirsel_graph::postorder(
            &graph,
            std::iter::once(self.body.entry.0 as usize).chain(0..graph.len()),
        )
        .into_iter()
        .rev()
        .enumerate()
        {
            rank[block] = index;
        }
        // All acyclic predecessors run before their join. Cycles still revisit
        // nodes until the same monotone fixed point, independently per audit class.
        let mut pending =
            BTreeSet::from([(rank[self.body.entry.0 as usize], self.body.entry.0, 0)]);
        while let Some((_, block, class)) = pending.pop_first() {
            let block = BlockId(block);
            work::count(Counter::ValidateFlowSteps);
            let mut state = incoming[block.0 as usize][class].clone().unwrap();
            self.transfer(&model, &mut state, block, false);
            for &target in &graph[block.0 as usize] {
                let target = BlockId(target as u32);
                let mut edge = state.clone();
                if !self.tag_edge(&mut edge, block, target) {
                    continue;
                }
                if let Some(roots) = loop_expirations.get(&(block.0, target.0)) {
                    edge.widen_cursors(&model, self.body);
                    for origin in roots {
                        edge.expire_origin(origin);
                    }
                }
                let class = usize::from(edge.audit);
                let changed = if let Some(previous) = &mut incoming[target.0 as usize][class] {
                    previous.join(&edge, &model)
                } else {
                    incoming[target.0 as usize][class] = Some(edge);
                    true
                };
                if changed {
                    pending.insert((rank[target.0 as usize], target.0, class));
                }
            }
        }
        self.returned.clear();
        for (b, classes) in incoming.into_iter().enumerate() {
            for mut state in classes.into_iter().flatten() {
                self.transfer(&model, &mut state, BlockId(b as u32), report);
            }
        }
        if let Some(started) = started {
            eprintln!(
                "hirsel-validate-flow-profile\t{}\t{}\t{:?}\t{}",
                self.definition.path,
                self.site.body.0,
                proof,
                started.elapsed().as_nanos()
            );
        }
    }

    fn flow_successors(&mut self, terminator: &Terminator) -> Vec<BlockId> {
        if let Terminator::Switch {
            value,
            arms,
            otherwise,
        } = terminator
        {
            let store = self.t;
            let count = match self.layout(store, self.place_type(value.place)) {
                Some(TypeKind::Variant(vs)) => Some(vs.len()),
                Some(TypeKind::Primitive(Primitive::Bool)) => Some(2),
                _ => None,
            };
            let default = &self.body.blocks[otherwise.0 as usize];
            if default.instructions.is_empty()
                && matches!(default.terminator, Terminator::Unreachable)
                && count
                    .is_some_and(|count| (0..count).all(|v| arms.iter().any(|a| a.0 as usize == v)))
            {
                return arms.iter().map(|a| a.1).collect();
            }
        }
        self.body.successors(terminator)
    }

    fn environment(
        &self,
        model: &Model,
        state: &State,
        place: PlaceId,
    ) -> super::flow_origins::Environment {
        let access = &self.body.places[place.0 as usize];
        state.origins.environments[model.roots[access.root.0 as usize]].project(
            &super::flow_origins::EnvironmentField::path(&access.projection),
        )
    }
    fn set_environment(
        &self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        environment: &super::flow_origins::Environment,
    ) {
        let access = &self.body.places[place.0 as usize];
        state
            .origins
            .environments
            .update(model.roots[access.root.0 as usize], |current| {
                current.replace(
                    &super::flow_origins::EnvironmentField::path(&access.projection),
                    environment,
                    &model.environment_paths,
                )
            });
    }
    fn borrowed_capture_crossing_uses(
        &self,
        body: BodyId,
    ) -> Vec<(usize, Vec<hirsel_types::OwnedTypePart>)> {
        let nested = &self.definition.bodies[body.0 as usize];
        let captures: Vec<_> = nested
            .locals
            .iter()
            .enumerate()
            .filter(|(_, local)| local.storage == Storage::Capture)
            .map(|(root, _)| root)
            .collect();
        let mut required = Vec::new();
        for block in &nested.blocks {
            let Terminator::Call {
                callee, arguments, ..
            } = &block.terminator
            else {
                continue;
            };
            let (function, actuals, roles) = match callee {
                Callee::Definition { path, arguments } => {
                    let Some(signature) = self.signatures.get(path) else {
                        continue;
                    };
                    let SignatureKind::Function(function) = &signature.kind else {
                        continue;
                    };
                    (function, arguments, signature.metadata.roles.clone())
                }
                Callee::Indirect {
                    forward_caller: _,
                    value,
                    arguments,
                } => {
                    let TypeKind::Function(function) = &self
                        .t
                        .get(nested.places[value.place.0 as usize].ty)
                        .unwrap()
                        .kind
                    else {
                        continue;
                    };
                    (function, arguments, std::sync::Arc::default())
                }
            };
            let store = self.t;
            for argument in arguments {
                let place = &nested.places[argument.value.place.0 as usize];
                let Some(ordinal) = captures
                    .iter()
                    .position(|root| *root == place.root.0 as usize)
                else {
                    continue;
                };
                let Ok(Some(expected)) = hirsel_types::generic_crossing_parameter(
                    store,
                    self.memo,
                    function,
                    argument.parameter.0 as usize,
                    actuals,
                    &roles,
                ) else {
                    continue;
                };
                let receipts = hirsel_types::owned_crossing_receipts(
                    &store,
                    expected,
                    &self.generic_declarations(),
                    &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
                )
                .unwrap_or_else(|error| self.proof_failure(error, Vec::new()));
                for (suffix, independent) in receipts {
                    if independent {
                        continue;
                    }
                    let mut path: Vec<_> = place
                        .projection
                        .iter()
                        .map(|projection| match projection {
                            Projection::Field(field) => hirsel_types::OwnedTypePart::Field(*field),
                            Projection::VariantField { variant, field } => {
                                hirsel_types::OwnedTypePart::VariantField {
                                    variant: *variant,
                                    field: *field,
                                }
                            }
                            Projection::Index(_) => hirsel_types::OwnedTypePart::Elements,
                        })
                        .collect();
                    path.extend(suffix);
                    if !required.contains(&(ordinal, path.clone())) {
                        required.push((ordinal, path));
                    }
                }
            }
        }
        required
    }

    fn captured_environment(
        &self,
        model: &Model,
        state: &State,
        body: BodyId,
        captures: &[Operand],
        operand: &Operand,
    ) -> super::flow_origins::Environment {
        let mut environment = self.environment(model, state, operand.place);
        let nested = &self.definition.bodies[body.0 as usize];
        let slots: Vec<_> = nested
            .locals
            .iter()
            .enumerate()
            .filter(|(_, local)| local.storage == Storage::Capture)
            .map(|(root, _)| root)
            .collect();
        let mut remap = |place: u32, suffix: &[hirsel_types::OwnedTypePart]| {
            let dependency = &self.body.places[place as usize];
            let mut dependency_path = crossing::path(&dependency.projection);
            dependency_path.extend(suffix.iter().map(|part| match part {
                hirsel_types::OwnedTypePart::Field(f) => crossing::Part::Field(*f),
                hirsel_types::OwnedTypePart::VariantField { variant, field } => {
                    crossing::Part::Variant(*variant, *field)
                }
                hirsel_types::OwnedTypePart::Elements => crossing::Part::Elements,
            }));
            for (ordinal, capture) in captures.iter().enumerate() {
                let source = &self.body.places[capture.place.0 as usize];
                let prefix = crossing::path(&source.projection);
                if source.root != dependency.root || !dependency_path.starts_with(&prefix) {
                    continue;
                }
                let root = *slots.get(ordinal)?;
                let place = nested
                    .places
                    .iter()
                    .position(|p| p.root.0 as usize == root && p.projection.is_empty())?;
                let path = dependency_path[prefix.len()..]
                    .iter()
                    .map(|part| match part {
                        crossing::Part::Field(f) => hirsel_types::OwnedTypePart::Field(*f),
                        crossing::Part::Variant(v, f) => {
                            hirsel_types::OwnedTypePart::VariantField {
                                variant: *v,
                                field: *f,
                            }
                        }
                        crossing::Part::Elements => hirsel_types::OwnedTypePart::Elements,
                    })
                    .collect();
                return Some((place as u32, path));
            }
            // The owner is outside this frame. Its enclosing invocation checks
            // the retained environment; this body cannot change that owner.
            None
        };
        environment.remap_crossing(&mut remap);
        environment.remap_sharing(|place| remap(place, &[]).map(|(place, _)| place));
        environment
    }

    fn rvalue_environment(
        &mut self,
        model: &Model,
        state: &State,
        value: &Rvalue,
    ) -> super::flow_origins::Environment {
        use super::flow_origins::{Environment, EnvironmentField};
        match value {
            Rvalue::Use(value)
            | Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } => self.environment(model, state, value.place),
            Rvalue::Closure { body, captures, .. } => {
                let mut requirements = Vec::new();
                let semantic = self.borrowed_capture_crossing_uses(*body);
                let mut retained = BTreeSet::new();
                let mut expired = BTreeSet::new();
                for (ordinal, capture) in captures.iter().enumerate() {
                    let index = model.mapping[capture.place.0 as usize];
                    let inactive = self.inactive_variant_payload(capture.place);
                    // Global environments own copied/taken identities. Their
                    // internal brand relations were proved at creation and do
                    // not depend on the caller binding remaining in scope.
                    // Borrowed captures retain the outer identity and loan.
                    if matches!(capture.usage, UseKind::Borrow | UseKind::Mut) {
                        retained.extend(self.origin_identity(model, state, capture.place));
                        if !inactive {
                            retained.extend(state.origins.retained[index].iter().cloned());
                            expired.extend(state.origins.expired[index].iter().cloned());
                        }
                    }
                    if matches!(capture.usage, UseKind::Borrow | UseKind::Mut) {
                        requirements.extend(
                            semantic
                                .iter()
                                .filter(|(capture, _)| *capture == ordinal)
                                .map(|(_, path)| (capture.place, path.clone())),
                        );
                    }
                    let environment = self.environment(model, state, capture.place);
                    requirements.extend(
                        environment
                            .crossing()
                            .into_iter()
                            .map(|(place, path)| (PlaceId(place), path)),
                    );
                    retained.extend(environment.retained());
                    expired.extend(environment.expired());
                }
                let mut environment = Environment::closure(retained, expired);
                for capture in captures {
                    if matches!(capture.usage, UseKind::Borrow | UseKind::Mut) {
                        environment.share_capture(capture.place);
                    }
                }
                for (place, path) in requirements {
                    environment.require_crossing(place, path);
                }
                environment
            }
            Rvalue::Construct {
                constructor,
                fields,
                ..
            } => {
                let mut environment = Environment::default();
                for field in fields {
                    let key = match constructor {
                        Constructor::Variant { variant, .. } => {
                            EnvironmentField::Variant(*variant, field.field)
                        }
                        Constructor::Sequence | Constructor::Map | Constructor::Set => {
                            EnvironmentField::Index
                        }
                        _ => EnvironmentField::Field(field.field),
                    };
                    environment.insert(
                        &[key],
                        &self.environment(model, state, field.value.place),
                        &model.environment_paths,
                    );
                }
                environment
            }
            _ => Environment::default(),
        }
    }
    fn origin_identity(&self, model: &Model, state: &State, place: PlaceId) -> BTreeSet<Origin> {
        Self::access_identity(model, state, &self.body.places[place.0 as usize], self.body)
    }
    fn access_identity(
        model: &Model,
        state: &State,
        access: &Place,
        body: &Body,
    ) -> BTreeSet<Origin> {
        if !model.binding_valid(state, access, body) {
            return BTreeSet::new();
        }
        Self::storage_identity(model, state, access)
    }
    /// Transfers determine value identity. A binding authorizes a checked
    /// storage access; its spelling cannot replace the value's identity.
    fn storage_identity(model: &Model, state: &State, access: &Place) -> BTreeSet<Origin> {
        if !state.cursor_views.is_empty()
            && state.cursor_views[access.root.0 as usize]
                .as_ref()
                .is_some_and(|view| view.imprecise)
        {
            return BTreeSet::new();
        }
        let fields: Option<Vec<_>> = access
            .projection
            .iter()
            .map(|p| match p {
                Projection::Field(f) | Projection::VariantField { field: f, .. } => {
                    Some(hirsel_types::FieldId(*f))
                }
                Projection::Index(_) => None,
            })
            .collect();
        if let Some(fields) = &fields {
            // The longest recorded prefix of the path partitions it.
            let parts = &state.origins.parts[model.roots[access.root.0 as usize]];
            if let Some((len, identities)) = (0..=fields.len())
                .rev()
                .find_map(|len| parts.get(&fields[..len]).map(|ids| (len, ids)))
            {
                return identities
                    .iter()
                    .map(|origin| {
                        let mut origin = origin.clone();
                        origin.fields.extend(&fields[len..]);
                        origin
                    })
                    .collect();
            }
        }
        if let Some(&index) = model
            .canonical
            .get(&(access.root.0, access.projection.clone()))
        {
            if !state.origins.identities[index].is_empty() {
                return (*state.origins.identities[index]).clone();
            }
        }
        let mut identities =
            (*state.origins.identities[model.roots[access.root.0 as usize]]).clone();
        for projection in &access.projection {
            let field = match projection {
                Projection::Field(f) | Projection::VariantField { field: f, .. } => {
                    hirsel_types::FieldId(*f)
                }
                Projection::Index(_) => return BTreeSet::new(),
            };
            identities = identities
                .into_iter()
                .map(|mut o| {
                    o.fields.push(field);
                    o
                })
                .collect();
        }
        identities
    }
    /// A transfer of a projection receives the partition relative to that
    /// projection, rather than the enclosing storage's field ordinals.
    fn projected_identity(
        model: &Model,
        state: &State,
        access: &Place,
        fields: &[hirsel_types::FieldId],
    ) -> BTreeSet<Origin> {
        let mut projected = access.clone();
        projected
            .projection
            .extend(fields.iter().map(|f| Projection::Field(f.0)));
        let identities = Self::storage_identity(model, state, &projected);
        if !identities.is_empty() {
            return identities;
        }
        // An indexed value has its own independently introduced identity.
        // Its fields derive from that value, not the sequence storage root.
        Self::storage_identity(model, state, access)
            .into_iter()
            .map(|mut origin| {
                origin.fields.extend(fields);
                origin
            })
            .collect()
    }
    fn storage_partition(
        model: &Model,
        state: &State,
        access: &Place,
    ) -> BTreeMap<Vec<hirsel_types::FieldId>, BTreeSet<Origin>> {
        model
            .origin_paths_of(access.ty)
            .iter()
            .map(|path| {
                (
                    path.clone(),
                    Self::projected_identity(model, state, access, path),
                )
            })
            .collect()
    }
    fn forget_written_partition(model: &Model, state: &mut State, access: &Place) {
        let path: Vec<_> = access
            .projection
            .iter()
            .map_while(|p| match p {
                Projection::Field(f) | Projection::VariantField { field: f, .. } => {
                    Some(hirsel_types::FieldId(*f))
                }
                Projection::Index(_) => None,
            })
            .collect();
        state.origins.parts[model.roots[access.root.0 as usize]]
            .retain(|fields, _| !fields.starts_with(&path) && !path.starts_with(fields));
    }
    /// A copied storage prefix detaches from its canonical owner. Unchanged
    /// fields retain the identities derived before mutation; a transferred
    /// owner instead renews its existing identity through the ordinary rule.
    fn refresh_copied_prefix(
        &self,
        model: &Model,
        state: &mut State,
        access: &Place,
        origin: &Origin,
    ) -> bool {
        let root = model.roots[access.root.0 as usize];
        if !state.must[access.root.0 as usize].has(MustFacts::OWNED)
            || state
                .origins
                .locations
                .get(&origin.root)
                .is_none_or(|locations| locations.iter().any(|(_, loan)| loan.is_some()))
        {
            return false;
        }
        // A whole fresh introduction already established this storage's
        // namespace. A following field introduction is a partition judgment,
        // not a second mutation of all its descendants.
        if state.origins.owners[root].contains(&Origin {
            root: origin.root.clone(),
            fields: vec![].into(),
        }) {
            return false;
        }
        let Some(written) = access
            .projection
            .iter()
            .map(|p| match p {
                Projection::Field(f) | Projection::VariantField { field: f, .. } => {
                    Some(hirsel_types::FieldId(*f))
                }
                Projection::Index(_) => None,
            })
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        let old: BTreeSet<_> = state.origins.parts[root]
            .get(&written)
            .cloned()
            .unwrap_or_else(|| {
                state.origins.identities[root]
                    .iter()
                    .map(|o| {
                        let mut o = o.clone();
                        o.fields.extend(&written);
                        o
                    })
                    .collect()
            });
        if old.iter().all(|identity| {
            state.origins.owners[root]
                .iter()
                .any(|owner| super::flow_origins::contains(owner, identity))
        }) {
            return false;
        }
        let fresh = Origin {
            root: origin.root.clone(),
            fields: vec![].into(),
        };
        let parts = model
            .origin_paths_of(model.places[root].ty)
            .iter()
            .map(|path| {
                let identities = if path.starts_with(&written) || written.starts_with(path) {
                    let mut o = fresh.clone();
                    o.fields.extend(path);
                    BTreeSet::from([o])
                } else {
                    state.origins.parts[root]
                        .get(path)
                        .cloned()
                        .unwrap_or_else(|| {
                            state.origins.identities[root]
                                .iter()
                                .map(|o| {
                                    let mut o = o.clone();
                                    o.fields.extend(path);
                                    o
                                })
                                .collect()
                        })
                };
                (path.clone(), identities)
            })
            .collect();
        state.expire_origin(origin);
        state.origins.introduce(origin);
        state.origins.identities[root] = BTreeSet::from([fresh.clone()]).into();
        state.origins.parts[root] = parts;
        state
            .origins
            .bindings
            .insert(fresh.root.clone(), BTreeSet::from([fresh]));
        if let Some(&index) = model
            .canonical
            .get(&(access.root.0, access.projection.clone()))
        {
            state.origins.identities[index] = BTreeSet::from([origin.clone()]).into();
            state.origins.owners[index].insert(origin.clone());
        }
        true
    }
    fn projection_origin(
        &mut self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        report: bool,
    ) {
        self.active_tag(state, place, report);
        let access = &self.body.places[place.0 as usize];
        if access.projection.is_empty() {
            return;
        }
        let index = model.mapping[place.0 as usize];
        let store = self.t;
        let body = self.body;
        let root = self.body.locals[access.root.0 as usize].ty;
        // A declaration-owned Local physical row stays valid when its current
        // contents prove Global. A stronger Global occurrence needs that proof.
        let root = if self
            .lifetime(&self.t.get(access.ty).unwrap().modes.relation)
            .is_global()
            && self.content_proof(Some(state), place).deep
            && store.get(root).unwrap().modes.relation.has_environment()
        {
            let mut row = store.get(root).unwrap().clone();
            row.modes.relation = self.placement(&row.modes.relation);
            let root = store.intern(row);
            self.derive_sendability(store, root)
        } else {
            root
        };
        let expected = self.projection_type_at(store, access, root, |depth, field| {
            let mut sibling = access.clone();
            sibling.projection.truncate(depth);
            sibling.projection.push(Projection::Field(field.0));
            sibling.binding = access.binding.clone().and_then(|mut binding| {
                let base = access.projection.len().checked_sub(binding.fields.len())?;
                binding.fields.truncate(depth.checked_sub(base)?);
                binding.fields.push(field);
                Some(binding)
            });
            let identities = Self::access_identity(model, state, &sibling, body);
            (identities.len() == 1).then(|| identities.first().unwrap().clone())
        });
        let mut proof = state.origins.bindings.clone();
        proof.extend(state.origins.proofs[index].clone());
        let expected = expected.and_then(|ty| {
            super::flow_origins::canonical(store, ty, &proof, &|o| self.arena_place(o)).ok()
        });
        let actual =
            super::flow_origins::canonical(store, access.ty, &proof, &|o| self.arena_place(o)).ok();
        let expected = expected.map(|root| self.canonical_plain(store, root));
        let actual = actual.map(|root| {
            let physical = store.get(root).unwrap().modes.clone();
            let root = self.canonical_plain(store, root);
            let mut row = store.get(root).unwrap().clone();
            row.modes.relation = physical.relation;
            row.modes.uniqueness = physical.uniqueness;
            store.intern(row)
        });
        let actual = actual.zip(expected).map(|(actual, expected)| {
            let actual = self.comparison_evidence(store, expected, actual);
            let mut row = store.get(actual).unwrap().clone();
            if !matches!(
                row.kind,
                TypeKind::Function(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
            ) {
                row.modes.sendability = store.get(expected).unwrap().modes.sendability.clone();
            }
            store.intern(row)
        });
        if !matches!((expected, actual), (Some(a), Some(b)) if self.t.semantic_eq(a, b)) {
            self.flow_error(report, ViolationKind::InvalidProjection(place));
            return;
        }
        let proof = super::flow_origins::remember(
            self.t,
            &self.scratch.origin_types[self.site.body.0 as usize],
            access.ty,
            &proof,
            &|o| self.arena_place(o),
        );
        let retained = super::flow_origins::retained(
            self.t,
            &self.scratch.origin_types[self.site.body.0 as usize],
            access.ty,
            &proof,
            &|o| self.arena_place(o),
        );
        let ended = state.origins.expired_by(&retained);
        state.origins.expired[index].extend(ended);
        state.origins.proofs[index] = proof;
        state
            .origins
            .retained
            .update(index, |row| row.extend(retained));
    }
    fn bind_fields(
        &mut self,
        model: &Model,
        state: &mut State,
        source: PlaceId,
        fields: &[PlaceId],
        report: bool,
    ) {
        self.available(model, state, source, false, report);
        self.access_loans(model, state, source, false, report);
        let aggregate = &self.body.places[source.0 as usize];
        let root = aggregate.root.0 as usize;
        let mut names = BTreeSet::new();
        let mut slots = BTreeSet::new();
        let source_identity = self.origin_identity(model, state, source);
        let valid = self.body.locals[root].storage == Storage::Temporary
            && fields.iter().all(|field| {
                let access = &self.body.places[field.0 as usize];
                access.root == aggregate.root
                    && access.loan == aggregate.loan
                    && access.projection.len() == aggregate.projection.len() + 1
                    && access.projection.starts_with(&aggregate.projection)
                    && matches!(
                        access.projection.last(),
                        Some(Projection::Field(_) | Projection::VariantField { .. })
                    )
                    && slots.insert(model.mapping[field.0 as usize])
                    && access.binding.as_ref().is_some_and(|binding| {
                        binding.fields.is_empty()
                            && matches!(binding.root, OriginRoot::Local(..))
                            && names.insert(binding.root.clone())
                            && !self.body.places.iter().any(|p| {
                                self.body.locals[p.root.0 as usize].storage == Storage::Capture
                                    && p.binding.as_ref().is_some_and(|b| b.root == binding.root)
                            })
                    })
            });
        if !valid {
            self.flow_error(report, ViolationKind::InvalidOrigin(source));
            return;
        }
        // Introductions are simultaneous: a declaration may name a later field.
        // All data still resides in the independently checked source aggregate.
        for field in fields {
            let access = &self.body.places[field.0 as usize];
            let origin = access.binding.as_ref().unwrap();
            let index = model.mapping[field.0 as usize];
            state.expire_origin(origin);
            state.origins.introduce(origin);
            state.origins.ended.remove(&origin.root);
            let identity = if state.must[root].has(MustFacts::OWNED) {
                BTreeSet::from([origin.clone()])
            } else {
                let field = match access.projection.last().unwrap() {
                    Projection::Field(field) | Projection::VariantField { field, .. } => *field,
                    Projection::Index(_) => unreachable!(),
                };
                source_identity
                    .iter()
                    .cloned()
                    .map(|mut identity| {
                        identity.fields.push(hirsel_types::FieldId(field));
                        identity
                    })
                    .collect()
            };
            state.origins.identities[index] = identity.clone().into();
            let path: Option<Vec<_>> = access
                .projection
                .iter()
                .map(|p| match p {
                    Projection::Field(f) | Projection::VariantField { field: f, .. } => {
                        Some(hirsel_types::FieldId(*f))
                    }
                    Projection::Index(_) => None,
                })
                .collect();
            if let Some(path) = path {
                let parts = &mut state.origins.parts[model.roots[root]];
                parts.retain(|fields, _| !fields.starts_with(&path));
                parts.insert(path, identity.clone());
            }
            if state.must[root].has(MustFacts::OWNED) {
                state.origins.owners[index] = identity.clone().into();
            }
            state.origins.locations.insert(
                origin.root.clone(),
                BTreeSet::from([(index, access.loan.map(|id| id.0))]),
            );
            state.origins.bindings.insert(origin.root.clone(), identity);
        }
        for field in fields {
            self.projection_origin(model, state, *field, report);
        }
    }
    fn end_origin(&self, model: &Model, state: &mut State, place: PlaceId) {
        let access = &self.body.places[place.0 as usize];
        if let Some(index) = access
            .projection
            .iter()
            .position(|p| matches!(p, Projection::Index(_)))
        {
            let mut prefix = access.clone();
            prefix.projection.truncate(index);
            let root = model.roots[prefix.root.0 as usize];
            let mut ends: BTreeSet<_> = Self::storage_identity(model, state, &prefix)
                .into_iter()
                .filter(|identity| {
                    model.mutable_parameters[prefix.root.0 as usize]
                        || state.origins.owners[root]
                            .iter()
                            .any(|owner| super::flow_origins::contains(owner, identity))
                })
                .collect();
            // Named aliases may designate this storage through different loans.
            // Only names at or above the index-free prefix own its container.
            let mut copied = Vec::new();
            for (name, locations) in &state.origins.locations {
                for (location, loan) in locations {
                    let base = &model.places[*location];
                    if base.root == prefix.root && prefix.projection.starts_with(&base.projection) {
                        let fields = prefix
                            .projection
                            .iter()
                            .skip(base.projection.len())
                            .map(|projection| match projection {
                                Projection::Field(field)
                                | Projection::VariantField { field, .. } => {
                                    hirsel_types::FieldId(*field)
                                }
                                Projection::Index(_) => unreachable!("index-free prefix"),
                            })
                            .collect();
                        let origin = Origin {
                            root: name.clone(),
                            fields,
                        };
                        if loan.is_none() {
                            copied.push(origin.clone());
                        }
                        ends.insert(origin);
                    }
                }
            }
            // Detach an owned copy at the enclosing container, preserving
            // unrelated field identities and borrowed aliases.
            for origin in &copied {
                self.refresh_copied_prefix(model, state, &prefix, origin);
            }
            for origin in &ends {
                state.expire_origin(origin);
            }
            // Old retained values stay expired; the live container can establish
            // new brands after its mutation, without changing its storage name.
            for origin in &ends {
                state.origins.introduce(origin);
            }
            return;
        }

        let index = model.mapping[place.0 as usize];
        let owners: Vec<_> = state.origins.owners[index].iter().cloned().collect();
        let binding = self.body.places[place.0 as usize].binding.clone();
        // An owning name also ends the underlying identity; a copied name
        // only detaches itself, preserving brands of the original owner.
        let ends: Vec<_> = if binding.is_some() {
            let owned = &state.origins.owners
                [model.roots[self.body.places[place.0 as usize].root.0 as usize]];
            self.origin_identity(model, state, place)
                .into_iter()
                .filter(|identity| {
                    owned
                        .iter()
                        .any(|owner| super::flow_origins::contains(owner, identity))
                })
                .collect()
        } else {
            vec![]
        };
        let ending: Vec<_> = binding
            .iter()
            .chain(&ends)
            .chain(&owners)
            .cloned()
            .collect();
        self.keep_inner_relations(model, state, index, &ending);
        if let Some(origin) = &binding {
            state.expire_origin(origin);
            for identity in ends {
                state.expire_origin(&identity);
            }
            if origin.fields.is_empty() {
                state.origins.bindings.remove(&origin.root);
                state.origins.ended.insert(origin.root.clone(), true);
            }
        }
        for owner in owners {
            state.expire_origin(&owner);
        }
    }
    /// A written take moves the value on with its contents (PROV-5: moving
    /// the record keeps the relation). A part's brand on another part of the
    /// same value, retained when a field was read through this name, is that
    /// relation, so it does not expire with the identities ending here; a
    /// value outside this storage that retains them still does (PROV-1).
    fn keep_inner_relations(
        &self,
        model: &Model,
        state: &mut State,
        index: usize,
        ending: &[Origin],
    ) {
        for i in model.contents(index) {
            state.origins.retained.update(i, |row| {
                row.retain(|value| {
                    !ending
                        .iter()
                        .any(|owner| super::flow_origins::contains(owner, value))
                })
            });
        }
    }
    fn set_origin_proof(
        &self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        proof: BTreeMap<OriginRoot, BTreeSet<Origin>>,
        mut expired: BTreeSet<Origin>,
    ) {
        let index = model.mapping[place.0 as usize];
        let retained = super::flow_origins::retained(
            self.t,
            &self.scratch.origin_types[self.site.body.0 as usize],
            self.place_type(place),
            &proof,
            &|o| self.arena_place(o),
        );
        expired.extend(state.origins.expired_by(&retained));
        for i in model.contents(index) {
            {
                state.origins.proofs[i] = proof.clone();
                state.origins.retained.set(i, retained.clone());
                state.origins.expired[i] = expired.clone();
            }
        }
    }
    fn call_origin_proof(
        &mut self,
        model: &Model,
        state: &mut State,
        arguments: &[CallArgument],
        report: bool,
    ) -> super::flow_origins::CallOriginProof {
        for argument in arguments {
            self.projection_origin(model, state, argument.value.place, report);
        }
        let mut origins = super::flow_origins::CallOriginProof {
            acquired_global: arguments
                .iter()
                .filter(|a| {
                    let access = &self.body.places[a.value.place.0 as usize];
                    let root = access.root.0 as usize;
                    a.value.usage != UseKind::Mut
                        && self
                            .lifetime(
                                &self
                                    .t
                                    .get(self.body.locals[root].ty)
                                    .unwrap()
                                    .modes
                                    .relation,
                            )
                            .is_global()
                        && self.content_proof(Some(state), a.value.place).deep
                })
                .map(|a| a.value.place.0)
                .collect(),
            current_sendable: arguments
                .iter()
                .filter(|a| {
                    let store = self.t;
                    let ty = self.transferred_type(state, &a.value, store);
                    store.get(ty).unwrap().modes.sendability == hirsel_types::Sendability::Sendable
                })
                .map(|a| a.value.place.0)
                .collect(),
            current_unique: arguments
                .iter()
                .filter(|a| self.operand_unique(state, &a.value))
                .map(|a| a.value.place.0)
                .collect(),
            arguments: BTreeMap::new(),
            fields: BTreeMap::new(),
            names: super::flow_origins::LiveNames::new(&state.origins),
            proofs: BTreeMap::new(),
            weakened: arguments
                .iter()
                .filter(|a| {
                    let root = self.body.places[a.value.place.0 as usize].root.0 as usize;
                    (state.shared[root] || a.value.usage == UseKind::Copy)
                        && needs_release(self, self.place_type(a.value.place), self.t)
                        && !(a.value.usage == UseKind::Copy && self.flat_value(a.value.place))
                })
                .map(|a| a.value.place.0)
                .collect(),
        };
        for argument in arguments {
            let ty = self.body.places[argument.value.place.0 as usize].ty;
            for fields in model.origin_paths_of(ty) {
                origins.fields.insert(
                    (ParameterRef::bound(argument.parameter), fields.clone()),
                    Self::projected_identity(
                        model,
                        state,
                        &self.body.places[argument.value.place.0 as usize],
                        fields,
                    ),
                );
            }
            let parameter = ParameterRef::bound(argument.parameter);
            let identities = self.origin_identity(model, state, argument.value.place);
            let identity = if identities.len() == 1 {
                identities.first().unwrap().clone()
            } else {
                Origin {
                    root: OriginRoot::Parameter(parameter),
                    fields: vec![].into(),
                }
            };
            let types = self.t;
            let brand = super::flow_origins::canonical(
                types,
                self.place_type(argument.value.place),
                &state.origins.proofs[model.mapping[argument.value.place.0 as usize]],
                &|o| self.arena_place(o),
            )
            .ok()
            .map(|t| types.get(t).unwrap().brand.clone())
            .unwrap_or_default();
            origins.arguments.insert(
                parameter,
                OriginArgument {
                    brand,
                    identity: identity.clone(),
                    relation: {
                        let relation = &self
                            .t
                            .get(self.place_type(argument.value.place))
                            .unwrap()
                            .modes
                            .relation;
                        if self.arena_type(self.place_type(argument.value.place)) {
                            // An Arena argument bounds a result that names it
                            // by its own place: fresh blocks go there (PROV-4).
                            Relation::place(identity.clone())
                        } else if self.current_content_global(state, &argument.value) {
                            // An arena-only source contributes its arena
                            // bound; its read loan ends at return (MODE-1).
                            // A region ends with its scope and a taken
                            // parameter's storage with this frame, so either
                            // bounds a dependent result to this frame (PROV-4,
                            // MODE-1).
                            let placement = self.placement(relation);
                            if placement.is_global() && self.lifetime(relation).is_global()
                                || !(placement
                                    .reaches()
                                    .iter()
                                    .any(|r| matches!(r, hirsel_types::Reach::Region(_)))
                                    || self.taken_storage(model, argument.value.place))
                            {
                                placement
                            } else {
                                Relation::environment().union(&placement)
                            }
                        } else {
                            relation.clone()
                        }
                    },
                },
            );
            origins.proofs.insert(
                argument.value.place.0,
                state.origins.proofs[model.mapping[argument.value.place.0 as usize]].clone(),
            );
        }
        origins
    }
    /// Typing received these operands of the current site only as deep
    /// Global content; each needs the must-owned Global proof here.
    fn acquisitions(&mut self, state: &State, report: bool) {
        let Some(block) = self.site.block else {
            return;
        };
        let owed = self.scratch.acquisitions[self.site.body.0 as usize]
            .borrow()
            .get(&(block.0, self.site.instruction))
            .cloned()
            .unwrap_or_default();
        for (place, entity) in owed {
            if !self.content_proof(Some(state), place).deep {
                self.flow_error(report, ViolationKind::TypeMismatch { entity });
            }
        }
    }
    fn flow_error(&mut self, report: bool, kind: ViolationKind) {
        if report {
            self.error(kind);
        }
    }
    /// The explicit access chain grants only the loan and its ancestors.
    /// Containment and activity are checked at every reborrow, never inferred
    /// from an arbitrary overlapping live loan.
    fn access_loans(
        &mut self,
        model: &Model,
        state: &State,
        place: PlaceId,
        mutable: bool,
        report: bool,
    ) -> BTreeSet<usize> {
        let Some(result) = model.access_chain(self.body, &self.body.places[place.0 as usize])
        else {
            self.flow_error(report, ViolationKind::InvalidUse(place));
            return BTreeSet::new();
        };
        for &index in &result {
            let (id, _, writable) = model.loans[index];
            if mutable && !writable {
                self.flow_error(report, ViolationKind::InvalidUse(place));
                return BTreeSet::new();
            }
            if state.loans[index] != LIVE {
                self.flow_error(report, ViolationKind::ExpiredLoan { loan: id, place });
                return BTreeSet::new();
            }
        }
        result
    }
    fn available(
        &mut self,
        model: &Model,
        state: &State,
        place: PlaceId,
        take: bool,
        report: bool,
    ) -> bool {
        self.active_tag(state, place, report);
        if !model.binding_valid(state, &self.body.places[place.0 as usize], self.body) {
            self.flow_error(report, ViolationKind::InvalidOrigin(place));
        }
        let index = model.mapping[place.0 as usize];
        let mask = model
            .contents(index)
            .fold(0, |bits, i| bits | state.places[i]);
        if mask == LIVE {
            return true;
        }
        let error = if mask & MOVED != 0 {
            if take {
                ViolationKind::DoubleMove(place)
            } else {
                ViolationKind::UseAfterMove(place)
            }
        } else {
            ViolationKind::ReadBeforeInit(place)
        };
        self.flow_error(report, error);
        false
    }
    fn indices(&mut self, model: &Model, state: &State, place: PlaceId, report: bool) {
        for projection in &self.body.places[place.0 as usize].projection {
            if let Projection::Index(l) = projection {
                let mask = state.places[model.roots[l.0 as usize]];
                if mask != LIVE {
                    self.flow_error(
                        report,
                        if mask & MOVED != 0 {
                            ViolationKind::UseAfterMove(place)
                        } else {
                            ViolationKind::ReadBeforeInit(place)
                        },
                    );
                }
            }
        }
    }
    fn conflict(
        &mut self,
        model: &Model,
        state: &State,
        place: PlaceId,
        dropping: bool,
        allow_alias: bool,
        report: bool,
    ) {
        let permissions = if allow_alias {
            self.access_loans(model, state, place, true, report)
        } else {
            BTreeSet::new()
        };
        let index = model.mapping[place.0 as usize];
        for (i, (loan, source, _)) in model.loans.iter().enumerate() {
            if state.loans[i] & LIVE != 0
                && !permissions.contains(&i)
                && model.overlaps(index, model.mapping[source.0 as usize])
            {
                self.flow_error(
                    report,
                    if dropping {
                        ViolationKind::LoanOutlivesSource { loan: *loan, place }
                    } else {
                        ViolationKind::LoanConflict { loan: *loan, place }
                    },
                );
            }
        }
    }
    fn clear(&self, model: &Model, state: &mut State, place: PlaceId, mask: u8) {
        let access = &self.body.places[place.0 as usize];
        let path = super::flow_origins::EnvironmentField::path(&access.projection);
        state.content_loans[access.root.0 as usize].clear(&path);
        state.borrowed_shares[access.root.0 as usize].clear(&path);
        let index = model.mapping[place.0 as usize];
        for i in model.contents(index) {
            {
                state.places.set(i, mask);
                state.defused.set(i, 1);
            }
        }
        if model.places[index].projection.is_empty() {
            state.must.set_fact(
                model.places[index].root.0 as usize,
                MustFacts::GLOBAL,
                false,
            );
            state.must.set_fact(
                model.places[index].root.0 as usize,
                MustFacts::SENDABLE,
                false,
            );
            state
                .must
                .set_fact(model.places[index].root.0 as usize, MustFacts::OWNED, false);
            state
                .ownership_conflicts
                .set(model.places[index].root.0 as usize, false);
            model.refresh_cleanup(state, model.places[index].root.0 as usize);
        }
    }
    /// An allocation destination is an implicit read of its exact Arena
    /// binding. A lexical type origin alone cannot supply a missing capture.
    fn allocation_destination(
        &mut self,
        model: &Model,
        state: &State,
        value: &Rvalue,
        report: bool,
    ) {
        let (Rvalue::Construct {
            destination: Destination::Arena(origin),
            ..
        }
        | Rvalue::Closure {
            destination: Destination::Arena(origin),
            ..
        }) = value
        else {
            return;
        };
        let parameter = match &origin.root {
            hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                scope,
                parameter,
            }) if *scope == self.body.parameter_scope && origin.fields.is_empty() => {
                self.body.parameters.get(parameter.0 as usize).copied()
            }
            _ => None,
        };
        let place = self
            .body
            .places
            .iter()
            .enumerate()
            .find(|(_, place)| {
                place.projection.is_empty()
                    && (parameter == Some(place.root) || place.binding.as_ref() == Some(origin))
                    && self.arena_type(place.ty)
            })
            .map(|(index, _)| PlaceId(index as u32));
        if let Some(place) = place {
            self.available(model, state, place, false, report);
        } else {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "allocation arena token",
                },
            );
        }
    }

    fn read(&mut self, model: &Model, state: &mut State, operand: &Operand, report: bool) {
        if self.site.instruction.is_none() && self.site.block.is_some_and(|b| matches!(&self.body.blocks[b.0 as usize].terminator, Terminator::Return { value, .. } if value == operand)) {
            self.mode_transfer(state, self.body.result, operand, report);
        }
        self.read_operand(model, state, operand, false, false, false, report);
    }
    /// `transport` moves a whole value into a body-owned slot. That is not a
    /// use of the value: one whose brand owner has ended may sit there and be
    /// dropped, and the slot inherits the ended brand for every later use.
    fn inactive_variant_payload(&mut self, place: PlaceId) -> bool {
        let ty = self.place_type(place);
        let row = self.t.get(ty).unwrap();
        row.brand.alternatives.is_empty()
            && row.modes.relation.is_global()
            && matches!(row.kind, TypeKind::Named { .. })
            && {
                let store = self.t;
                self.brandless_variant_operand(store, place, ty)
            }
    }

    fn inactive_callable_result(&mut self, place: PlaceId) -> bool {
        let row = self.t.get(self.place_type(place)).unwrap();
        let TypeKind::Function(function) = &row.kind else {
            return false;
        };
        if !row.brand.alternatives.is_empty() || !function.parameters.is_empty() {
            return false;
        }
        let result = function.result;
        let row = self.t.get(result).unwrap();
        if !row.brand.alternatives.is_empty()
            || !row.modes.relation.is_global()
            || !matches!(row.kind, TypeKind::Named { .. })
        {
            return false;
        }
        let Some(callees) = self.local_callable_bodies(self.site.body, place) else {
            return false;
        };
        let store = self.t;
        for callee in callees {
            let body = &self.definition.bodies[callee.0 as usize];
            let mut returns = false;
            for block in &body.blocks {
                if let Terminator::Return { value, .. } = &block.terminator {
                    if !self.brandless_variant_from(store, callee, value.place, result) {
                        return false;
                    }
                    returns = true;
                }
            }
            if !returns {
                return false;
            }
        }
        true
    }

    fn read_operand(
        &mut self,
        model: &Model,
        state: &mut State,
        operand: &Operand,
        relocate_view: bool,
        deep_copy: bool,
        transport: bool,
        report: bool,
    ) {
        let place = operand.place;
        if matches!(operand.usage, UseKind::Take | UseKind::Copy)
            && !(relocate_view
                && self.body.locals[self.body.places[place.0 as usize].root.0 as usize].storage
                    == Storage::Temporary)
        {
            if operand.usage == UseKind::Copy {
                self.state_copy(model, state, place, report);
            } else {
                self.state_write(model, state, place, report);
            }
        }
        if self.contract
            && (operand.usage == UseKind::Mut
                || operand.usage == UseKind::Take
                    && needs_release(self, self.place_type(place), self.t))
        {
            let mut read = operand.clone();
            read.usage = UseKind::Borrow;
            let root = self.body.places[place.0 as usize].root.0 as usize;
            let readonly = state.contract_readonly.contains(&root)
                || !state.must[root].has(MustFacts::OWNED)
                    && !relocate_view
                    && self
                        .sources(model, state, &read)
                        .iter()
                        .any(|source| state.contract_readonly.contains(source));
            if readonly {
                self.flow_error(report, ViolationKind::InvalidUse(place));
            }
        }
        if matches!(operand.usage, UseKind::Take | UseKind::Mut) && self.readonly_map_entry(place)
            || operand.usage == UseKind::Mut && self.readonly_byte_index(place)
        {
            self.flow_error(report, ViolationKind::InvalidUse(place));
        }
        self.projection_origin(model, state, place, report);
        self.indices(model, state, place, report);
        self.available(model, state, place, operand.usage == UseKind::Take, report);
        let index = model.mapping[place.0 as usize];
        if state.defused[index] & 2 != 0 {
            self.flow_error(report, ViolationKind::InvalidUse(place));
        }
        let root = model.places[index].root.0 as usize;
        let expired_identity = model
            .parts(place)
            .any(|part| !state.origins.expired[part].is_empty());
        let inactive_payload = expired_identity
            && (self.inactive_variant_payload(place) || self.inactive_callable_result(place));
        if !transport
            && (!inactive_payload && expired_identity
                || !self.environment(model, state, place).expired().is_empty())
        {
            self.flow_error(report, ViolationKind::InvalidOrigin(place));
        }
        let expired = self.installed_loans(state, place).expired();
        for dependency in &expired {
            self.flow_error(
                report,
                ViolationKind::ExpiredLoan {
                    loan: model.loans[*dependency].0,
                    place,
                },
            );
        }
        match operand.usage {
            UseKind::Take => {
                self.access_loans(model, state, place, false, report);
                if needs_release(self, self.place_type(place), self.t)
                    && (model.borrowed[root]
                        || state.ownership_conflicts[root]
                        || (!state.must[root].has(MustFacts::OWNED)
                            && !(relocate_view
                                && self.body.locals[root].storage == Storage::Temporary
                                && self.body.places[place.0 as usize].projection.is_empty())))
                {
                    self.flow_error(report, ViolationKind::BorrowedMove(place));
                }
                self.conflict(model, state, place, false, false, report);
                // The mutable calling convention copies inline bits when a plain
                // value is returned; the caller's storage remains initialized.
                if !model.mutable_parameters[root]
                    || needs_release(self, self.place_type(place), self.t)
                {
                    self.clear(model, state, place, MOVED);
                }
            }
            UseKind::Borrow | UseKind::Mut => {
                // Plain reads copy bits; borrowed parameter conventions establish
                // their external loan without a synthetic LoanId.
                // Match observes storage atomically and produces no borrowed
                // value. Availability, alias permission and exclusivity still
                // apply; guard binding views establish their own ordinary loans.
                let match_observation = self.site.instruction.is_none()
                    && operand.usage == UseKind::Borrow
                    && self.site.block.is_some_and(|b| matches!(self.body.blocks[b.0 as usize].terminator,
                        Terminator::Match { table, .. } if self.body.matches[table.0 as usize].scrutinee == place));
                let implicit = match_observation
                    || model.borrowed[root]
                        && (operand.usage != UseKind::Mut || model.mutable_parameters[root]);
                let dependencies = self.dependencies(model, state, operand);
                let derived = !dependencies.is_empty()
                    && expired.is_empty()
                    && (operand.usage != UseKind::Mut
                        || dependencies.iter().all(|i| model.loans[*i].2));
                let permissions =
                    self.access_loans(model, state, place, operand.usage == UseKind::Mut, report);
                let active = derived || !permissions.is_empty();
                if (needs_release(self, self.place_type(place), self.t)
                    || operand.usage == UseKind::Mut)
                    && !implicit
                    && !active
                {
                    self.flow_error(report, ViolationKind::MissingLoan(place));
                }
                for (i, (loan, source, mutable)) in model.loans.iter().enumerate() {
                    if state.loans[i] & LIVE != 0
                        && (operand.usage == UseKind::Mut || *mutable)
                        && !permissions.contains(&i)
                        && model.overlaps(model.mapping[source.0 as usize], index)
                    {
                        self.flow_error(report, ViolationKind::LoanConflict { loan: *loan, place });
                    }
                }
            }
            UseKind::Copy => {
                if !deep_copy
                    && needs_release(self, self.place_type(place), self.t)
                    && !self.flat_value(place)
                {
                    self.mark_shared(model, state, operand);
                }
                let permissions = self.access_loans(model, state, place, false, report);
                if !copyable(self, self.place_type(place), self.t)
                    && !self.result_read_permission(model, state, operand)
                {
                    self.flow_error(report, ViolationKind::InvalidUse(place));
                }
                for (i, (loan, source, mutable)) in model.loans.iter().enumerate() {
                    if *mutable
                        && !permissions.contains(&i)
                        && state.loans[i] & LIVE != 0
                        && model.overlaps(model.mapping[source.0 as usize], index)
                    {
                        self.flow_error(report, ViolationKind::LoanConflict { loan: *loan, place });
                    }
                }
            }
        }
    }
    fn mark_shared(&self, model: &Model, state: &mut State, operand: &Operand) {
        let access = &self.body.places[operand.place.0 as usize];
        if model.creation {
            self.share_creation(state, operand);
        }
        let stable_functions: Vec<_> = model.root_accesses[access.root.0 as usize]
            .iter()
            .copied()
            .map(|index| (index, &self.body.places[index]))
            .filter(|(_, place)| {
                place.root == access.root
                    && place.projection.starts_with(&access.projection)
                    && matches!(self.t.get(place.ty).unwrap().kind, TypeKind::Function(_))
            })
            .filter_map(|(index, place)| {
                let types = self.t;
                let actual = self.transferred_type(
                    state,
                    &Operand {
                        place: PlaceId(index as u32),
                        usage: UseKind::Borrow,
                    },
                    types,
                );
                (types.get(actual).unwrap().modes.sendability == Sendability::Sendable)
                    .then(|| crossing::path(&place.projection))
            })
            .collect();
        let store = self.t;
        let mut referent = operand.clone();
        referent.usage = UseKind::Borrow;
        let current = self.transferred_type(state, &referent, store);
        let independent =
            self.proof(
                &store,
                current,
                super::proofs::Proof::AfterSharing,
                &self.generic_declarations(),
            )
            .unwrap_or_else(|error| {
                self.proof_failure(error, hirsel_types::Sendability::NotSendable)
            }) == hirsel_types::Sendability::Sendable;
        share_root(state, access.root.0 as usize);
        state.unique_parts[access.root.0 as usize] = crossing::Parts::default();
        if !independent {
            let path = crossing::path(&access.projection);
            let mut parts = state.crossing_parts[access.root.0 as usize].project(&path);
            if store.get(current).unwrap().modes.sendability == Sendability::Sendable {
                parts.set(&[], true);
            }
            self.share_crossing_parts(&mut parts, operand.place);
            state.crossing_parts[access.root.0 as usize].install(&path, &parts);
            state
                .must
                .set_fact(access.root.0 as usize, MustFacts::SENDABLE, false);
            for path in stable_functions {
                // Copying an owner shares function handles without changing
                // their immutable environments. Preserve the proved fact.
                let mut proved = crossing::Parts::default();
                proved.set(&[], true);
                state.crossing_parts[access.root.0 as usize].install(&path, &proved);
            }
        }
        // A function share aliases its environment, not the referents that a
        // scoped environment borrows. Invocation checks those retained inputs
        // separately; copying the function cannot invalidate their ownership.
        if matches!(store.get(current).unwrap().kind, TypeKind::Function(_)) {
            return;
        }
        let mut source = operand.clone();
        source.usage = UseKind::Borrow;
        for root in self.sources(model, state, &source) {
            share_root(state, root);
            if !independent && root != access.root.0 as usize {
                state.crossing_parts[root].set(&[], false);
                state.must.set_fact(root, MustFacts::SENDABLE, false);
            }
        }
        for loan in self.dependencies(model, state, &source) {
            share_root(
                state,
                self.body.places[model.loans[loan].1.0 as usize].root.0 as usize,
            );
        }
    }
    fn result_read_permission(&self, model: &Model, state: &State, operand: &Operand) -> bool {
        let Some(block) = self.site.block else {
            return false;
        };
        if !model
            .result_reads
            .contains(&(block.0, self.site.instruction))
        {
            return false;
        }
        let mut source = operand.clone();
        source.usage = UseKind::Borrow;
        let sources = self.sources(model, state, &source);
        !sources.is_empty()
            && sources.is_subset(&model.selected_sources)
            && self.bound(
                self.t,
                self.place_type(operand.place),
                hirsel_types::Bound::Copyable,
                &self
                    .generic_declarations()
                    .with_copyable(&model.result_shareable),
            )
    }
    fn result_permissions(&self) -> BTreeSet<hirsel_types::TypeParameterRef> {
        if self.site.body == self.definition.entry {
            if let Some(signature) = self.signatures.get(self.definition.path) {
                if let SignatureKind::Function(f) = &signature.kind {
                    return hirsel_types::local_result_permissions(
                        self.t,
                        f,
                        self.body.parameter_scope,
                    );
                }
            }
        }
        for body in &self.definition.bodies {
            for instruction in body.blocks.iter().flat_map(|b| &b.instructions) {
                if let InstructionKind::Assign { destination, value } = instruction.kind {
                    if matches!(body.rvalues[value.0 as usize], Rvalue::Closure { body: child, .. } if child == self.site.body)
                    {
                        if let TypeKind::Function(f) = &self
                            .t
                            .get(body.places[destination.0 as usize].ty)
                            .unwrap()
                            .kind
                        {
                            return hirsel_types::local_result_permissions(
                                self.t,
                                f,
                                self.body.parameter_scope,
                            );
                        }
                    }
                }
            }
        }
        BTreeSet::new()
    }
    fn installed_loans(&self, state: &State, place: PlaceId) -> loan_content::Content {
        let access = &self.body.places[place.0 as usize];
        state.content_loans[access.root.0 as usize].project(
            &super::flow_origins::EnvironmentField::path(&access.projection),
        )
    }
    fn operand_loans(
        &self,
        model: &Model,
        state: &State,
        operand: &Operand,
    ) -> loan_content::Content {
        let mut content = self.installed_loans(state, operand.place);
        if matches!(operand.usage, UseKind::Borrow | UseKind::Mut) {
            if let Some(chain) =
                model.access_chain(self.body, &self.body.places[operand.place.0 as usize])
            {
                content.add(chain.into_iter().collect());
            }
        }
        // A copy is an owned share, except of a scoped function value, whose
        // environment still borrows its captures (CLO-1). A global function
        // value owns its captures (CLO-4) and is shared like any counted value.
        let ty = self.t.get(self.place_type(operand.place)).unwrap();
        if operand.usage == UseKind::Copy
            && !(matches!(ty.kind, TypeKind::Function(_))
                && !self.lifetime(&ty.modes.relation).is_global())
        {
            content = loan_content::Content::default();
        }
        // Taking plain bits copies them, so they hold no loan either (MODE-1),
        // as `sources` judges: an error payload `?` takes from a borrowed
        // Result is independent of the Result's bound.
        if operand.usage == UseKind::Take
            && !needs_release(self, self.place_type(operand.place), self.t)
        {
            content = loan_content::Content::default();
        }
        content
    }
    /// The content loans of a borrowed `Local` root whose value is related to
    /// other places: it holds at least one loan, and every source it is
    /// bounded by is the place of a loan it holds. An owned root, including a
    /// copy, carries a source bound no loan covers (MODE-1).
    fn related_local(
        &self,
        model: &Model,
        state: &State,
        operand: &Operand,
    ) -> Option<BTreeSet<usize>> {
        let root = self.body.places[operand.place.0 as usize].root.0 as usize;
        if operand.usage != UseKind::Borrow || self.body.locals[root].storage != Storage::Local {
            return None;
        }
        let loans = self.installed_loans(state, operand.place).held();
        let lent: BTreeSet<usize> = loans
            .iter()
            .map(|loan| {
                let source = model.loans[*loan].1;
                self.body.places[source.0 as usize].root.0 as usize
            })
            .collect();
        let sources = model
            .parts(operand.place)
            .flat_map(|part| state.sources[part].iter().copied());
        (!loans.is_empty() && sources.into_iter().all(|source| lent.contains(&source)))
            .then_some(loans)
    }
    fn dependencies(&self, model: &Model, state: &State, operand: &Operand) -> BTreeSet<usize> {
        self.operand_loans(model, state, operand).held()
    }
    fn rvalue_loans(&self, model: &Model, state: &State, value: &Rvalue) -> loan_content::Content {
        use super::flow_origins::EnvironmentField;
        let mut content = loan_content::Content::default();
        if let Rvalue::Construct {
            constructor,
            fields,
            ..
        } = value
        {
            for field in fields {
                let key = match constructor {
                    Constructor::Variant { variant, .. } => {
                        EnvironmentField::Variant(*variant, field.field)
                    }
                    Constructor::Sequence | Constructor::Map | Constructor::Set => {
                        EnvironmentField::Index
                    }
                    _ => EnvironmentField::Field(field.field),
                };
                content.insert(
                    &[key],
                    &self.operand_loans(model, state, &field.value),
                    &model.environment_paths,
                );
            }
        } else {
            for operand in typing::operands(value) {
                content.join(&self.operand_loans(model, state, operand));
            }
        }
        content
    }
    fn set_loan_content(
        &self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        content: &loan_content::Content,
    ) {
        let access = &self.body.places[place.0 as usize];
        state.content_loans[access.root.0 as usize].replace(
            &super::flow_origins::EnvironmentField::path(&access.projection),
            content,
            &model.environment_paths,
        );
    }
    fn sources(&self, model: &Model, state: &State, operand: &Operand) -> BTreeSet<usize> {
        let ty_id = self.place_type(operand.place);
        let ty = self.t.get(ty_id).unwrap();
        // MEM-12 copies a flat value's pointer-free bytes into independent
        // storage. Prove flatness in this value's declaring generic namespace;
        // a Local value still retains its placement dependencies.
        if operand.usage == UseKind::Copy
            && self.lifetime(&ty.modes.relation).is_global()
            && self.bound(
                self.t,
                ty_id,
                hirsel_types::Bound::Flat,
                &self.generic_declarations(),
            )
        {
            return BTreeSet::new();
        }
        // MEM-11, CONC-2: a copied Shared root acquires its own atomic count on
        // an immutable root that is never region-placed, and a copied Mutex or
        // Atomic its own count on a run-owned cell, also inside an aggregate.
        if operand.usage == UseKind::Copy && self.copy_detaches(self.t, ty_id) {
            return BTreeSet::new();
        }
        // Plain bits have no region dependency (MODE-1). A claimed Global row
        // otherwise cannot erase a borrowed view retained in storage or fields.
        if !needs_release(self, self.place_type(operand.place), self.t) {
            return BTreeSet::new();
        }
        let index = model.mapping[operand.place.0 as usize];
        let root = model.places[index].root.0 as usize;
        let mut sources = model
            .parts(operand.place)
            .flat_map(|part| state.sources[part].iter().copied())
            .collect::<BTreeSet<_>>();
        if self.lifetime(&ty.modes.relation).is_global() && operand.usage == UseKind::Copy {
            // A share of global counted storage acquires its own owner, also
            // through borrowed staging slots. Local/arena sources remain bounded.
            // A global function value owns its captures and shares the same way.
            let mut pending = sources.clone();
            let mut seen = BTreeSet::new();
            let mut global = true;
            while let Some(source) = pending.pop_first() {
                if !seen.insert(source) {
                    continue;
                }
                let source_ty = self.t.get(self.body.locals[source].ty).unwrap();
                if !source_ty.modes.relation.is_global() {
                    global = false;
                    break;
                }
                pending.extend(&state.sources[model.roots[source]]);
            }
            if global {
                sources.clear();
            }
        }
        if sources.is_empty()
            && (!(state.must[root].has(MustFacts::OWNED)
                && !state.ownership_conflicts[root]
                && matches!(operand.usage, UseKind::Copy | UseKind::Take))
                && !self.global_operand(state, operand)
                || matches!(operand.usage, UseKind::Borrow | UseKind::Mut))
        {
            sources.insert(root);
        }
        sources
    }
    fn set_sources(
        &self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        sources: BTreeSet<usize>,
    ) {
        let index = model.mapping[place.0 as usize];
        for i in model.contents(index) {
            {
                state.sources[i] = sources.clone();
            }
        }
    }
    /// Signature-selected result dependencies; argument evaluation order is not
    /// parameter order. A nested callable retains its own parameter binder.
    /// The lifetime relation of a callee's result.
    fn result_relation(&self, callee: &Callee) -> Option<hirsel_types::Relation> {
        let (store, f) = match callee {
            Callee::Definition { path, .. } => {
                let signature = self.signatures.get(path)?;
                let SignatureKind::Function(f) = &signature.kind else {
                    return None;
                };
                (self.t, f)
            }
            Callee::Indirect { value: o, .. } => {
                let TypeKind::Function(f) = &self.t.get(self.place_type(o.place)).unwrap().kind
                else {
                    return None;
                };
                (self.t, f)
            }
        };
        Some(self.signature_lifetime(store, f, &store.get(f.result).unwrap().modes.relation))
    }
    /// MODE-1 field-path containment (#77): a result related only to field
    /// path F of a parameter depends on its argument through a read loan
    /// active on the argument place extended by F, not on the whole
    /// argument. None when no such loan is active.
    fn field_dependencies(
        &self,
        model: &Model,
        state: &State,
        callee: &Callee,
        arguments: &[CallArgument],
        operand: &Operand,
    ) -> Option<BTreeSet<usize>> {
        let argument = arguments.iter().find(|a| a.value.place == operand.place)?;
        let path = self
            .result_relation(callee)?
            .named_field_path(&hirsel_types::ParameterRef::bound(argument.parameter))?;
        let access = &self.body.places[operand.place.0 as usize];
        let index = (0..model.loans.len()).find(|&i| {
            let (_, place, mutable) = model.loans[i];
            let source = &self.body.places[place.0 as usize];
            !mutable
                && state.loans[i] == LIVE
                && source.root == access.root
                && source.projection.len() == access.projection.len() + path.len()
                && source.projection.starts_with(&access.projection)
                && source
                    .projection
                    .iter()
                    .skip(access.projection.len())
                    .zip(&path)
                    .all(|(step, field)| *step == Projection::Field(field.0))
        })?;
        let place = model.loans[index].1;
        let mut held = model.access_chain(self.body, &self.body.places[place.0 as usize])?;
        held.insert(index);
        held.extend(self.installed_loans(state, place).held());
        Some(held)
    }
    fn result_arguments<'b>(
        &self,
        callee: &'b Callee,
        arguments: &'b [CallArgument],
    ) -> (Vec<&'b Operand>, Option<&'b Operand>) {
        let Some(relation) = self.result_relation(callee) else {
            return (vec![], None);
        };
        if relation.is_global() {
            return (vec![], None);
        }
        match relation.named_parameters() {
            Some(parameters) => {
                let operands: Vec<_> = arguments
                    .iter()
                    .filter(|a| {
                        parameters.contains(&hirsel_types::ParameterRef::bound(a.parameter))
                    })
                    .map(|a| &a.value)
                    .collect();
                let environment = if parameters
                    .iter()
                    .any(|p| !matches!(p, hirsel_types::ParameterRef::Bound { depth: 0, .. }))
                {
                    match callee {
                        Callee::Indirect { value, .. } => Some(value),
                        _ => None,
                    }
                } else {
                    None
                };
                (operands, environment)
            }
            None => {
                let operands: Vec<_> = arguments.iter().map(|a| &a.value).collect();
                let environment = match callee {
                    Callee::Indirect { value, .. } => Some(value),
                    _ => None,
                };
                (operands, environment)
            }
        }
    }
    fn set_dependencies(
        &self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        dependencies: BTreeSet<usize>,
    ) {
        self.set_loan_content(
            model,
            state,
            place,
            &loan_content::Content::whole(dependencies),
        );
    }
    fn write(
        &mut self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        owned: bool,
        independent_data: bool,
        report: bool,
    ) {
        self.state_install(model, state, place, independent_data, report);
        if self.readonly_map_entry(place) || self.readonly_byte_index(place) {
            self.flow_error(report, ViolationKind::InvalidUse(place));
        }
        self.projection_origin(model, state, place, report);
        self.set_environment(
            model,
            state,
            place,
            &super::flow_origins::Environment::default(),
        );
        self.indices(model, state, place, report);
        let index = model.mapping[place.0 as usize];
        let root = model.places[index].root.0 as usize;
        if state.places[index] & LIVE != 0 {
            self.flow_error(report, ViolationKind::WriteToLive(place));
        }
        self.conflict(model, state, place, false, true, report);
        if model.borrowed[root] && !model.mutable_parameters[root] {
            self.flow_error(report, ViolationKind::BorrowedMove(place));
        }
        if model.mutable_parameters[root] && !owned {
            self.flow_error(report, ViolationKind::BorrowedMove(place));
        }
        // A field assignment requires its aggregate container to exist.
        if !model.places[index].projection.is_empty() && state.places[model.roots[root]] != LIVE {
            self.flow_error(report, ViolationKind::ReadBeforeInit(place));
        }
        Self::forget_written_partition(model, state, &self.body.places[place.0 as usize]);
        for i in model.contents(index) {
            {
                state.places.set(i, LIVE);
                state.defused.set(i, 1);
                state.origins.clear_value(i);
            }
        }
        if model.places[index].projection.is_empty() {
            state
                .must
                .set_fact(root, MustFacts::OWNED, owned && !model.borrowed[root]);
            state.ownership_conflicts.set(root, false);
            if state.before.iter().any(|(a, b)| *a == root || *b == root) {
                state.before.retain(|(a, b)| *a != root && *b != root);
            }
            if model.temporary[root] && model.release[root] {
                let after: Vec<_> = model
                    .cleanup_sets
                    .borrow()
                    .members(state.live_temporaries)
                    .into_iter()
                    .map(|i| {
                        work::count(Counter::ValidateCleanupRootRows);
                        i as usize
                    })
                    .filter(|i| *i != root)
                    .collect();
                if !after.is_empty() {
                    state.before.extend(after.into_iter().map(|i| (i, root)));
                }
            }
        }
        model.refresh_cleanup(state, root);
    }
    fn remaining_drop_allocates(&mut self, model: &Model, state: &State, place: PlaceId) -> bool {
        let access = &self.body.places[place.0 as usize];
        let mut pending = vec![(
            self.place_type(place),
            access.projection.clone(),
            model.mapping[place.0 as usize],
            false,
        )];
        let store = self.t;
        let generics = self.generic_declarations();
        let mut nominals = hirsel_types::NominalExpansions::default();
        while let Some((ty, path, inherited, exit)) = pending.pop() {
            if exit {
                nominals.leave(ty, inherited);
                continue;
            }
            let index = model
                .canonical
                .get(&(access.root.0, path.clone()))
                .copied()
                .unwrap_or(inherited);
            if state.places[index] & LIVE == 0
                || self.bound(&store, ty, hirsel_types::Bound::NoallocDrop, &generics)
            {
                continue;
            }
            // Resource finalizers run before their still-owned fields. A moved
            // child never erases the enclosing resource's destructor promise.
            if let TypeKind::Named { definition, .. } = &store.get(ty).unwrap().kind {
                if self.signatures.get(*definition).is_some_and(|signature| matches!(&signature.kind,
                    SignatureKind::Type(declaration) if declaration.resource && state.defused[index] != 2 && declaration.destructor_allocation != hirsel_types::Allocation::NoAlloc)) {
                    return true;
                }
            }
            if matches!(store.get(ty).unwrap().kind, TypeKind::Named { .. }) {
                if !nominals
                    .enter(ty, index, store)
                    .unwrap_or_else(|error| self.proof_failure(error, false))
                {
                    continue;
                }
                pending.push((ty, path.clone(), index, true));
            }
            let fields: Vec<_> = match self.layout(store, ty) {
                Some(TypeKind::Record(fields)) => fields.iter().map(|field| field.ty).collect(),
                Some(TypeKind::Tuple(fields)) => fields.to_vec(),
                Some(TypeKind::Box(child)) => vec![child],
                _ => return true,
            };
            for (field, ty) in fields.into_iter().enumerate() {
                let mut child = path.clone();
                child.push(Projection::Field(field as u32));
                pending.push((ty, child, index, false));
            }
        }
        false
    }

    fn drop_place(
        &mut self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        conditional: bool,
        report: bool,
        allocation_effects: bool,
    ) {
        self.projection_origin(model, state, place, report);
        self.indices(model, state, place, report);
        let index = model.mapping[place.0 as usize];
        if !conditional && state.places[index] != LIVE {
            self.flow_error(
                report,
                if state.places[index] & MOVED != 0 {
                    ViolationKind::UseAfterMove(place)
                } else {
                    ViolationKind::ReadBeforeInit(place)
                },
            );
        }
        if report
            && allocation_effects
            && model.noalloc_drop_checks
            && state.places[index] & LIVE != 0
            && self.remaining_drop_allocates(model, state, place)
        {
            self.flow_error(
                true,
                ViolationKind::TypeMismatch {
                    entity: "body destructor allocation",
                },
            );
        }
        if state.places[index] & LIVE != 0 {
            self.conflict(model, state, place, true, true, report);
            // destroying content with user-visible destruction uses it,
            // so every loan it holds must still be active. DROP-1 values are
            // destroyed without observation and are not checked.
            if self.destroyed(self.place_type(place)) {
                for dependency in self.installed_loans(state, place).expired() {
                    self.flow_error(
                        report,
                        ViolationKind::ExpiredLoan {
                            loan: model.loans[dependency].0,
                            place,
                        },
                    );
                }
            }
            let root = model.places[index].root.0 as usize;
            if (!state.must[root].has(MustFacts::OWNED) || state.ownership_conflicts[root])
                && !model.mutable_parameters[root]
                && needs_release(self, self.place_type(place), self.t)
            {
                self.flow_error(report, ViolationKind::BorrowedMove(place));
            }
        }
        if state.places[index] & LIVE != 0 {
            for owner in (*state.origins.owners[index]).clone() {
                state.expire_origin(&owner);
            }
        }
        self.clear(model, state, place, DEAD);
    }
    fn end_loan(
        &mut self,
        model: &Model,
        state: &mut State,
        id: LoanId,
        if_active: bool,
        report: bool,
    ) {
        let index = model.loan(id);
        if !if_active && state.loans[index] != LIVE {
            self.flow_error(report, ViolationKind::InactiveLoan(id));
        }
        if state.loans[index] & LIVE != 0 {
            for (child, (loan, source, _)) in model.loans.iter().enumerate() {
                if child != index && state.loans[child] & LIVE != 0 {
                    let parents =
                        model.access_chain(self.body, &self.body.places[source.0 as usize]);
                    if parents.is_some_and(|parents| parents.contains(&index)) {
                        self.flow_error(
                            report,
                            ViolationKind::LoanConflict {
                                loan: *loan,
                                place: *source,
                            },
                        );
                    }
                }
            }
            for root in 0..self.body.locals.len() {
                if state.places[model.roots[root]] & LIVE != 0 {
                    state.content_loans[root].expire(index);
                }
            }
        }
        state.loans.set(index, DEAD);
        model.refresh_cleanup_loan(state, index);
    }
    fn cleanup(
        &mut self,
        model: &Model,
        state: &mut State,
        id: CleanupId,
        exact: bool,
        report: bool,
        allocation_effects: bool,
    ) {
        if model.trivial_cleanup && self.body.cleanups[id.0 as usize].head.is_none() {
            if exact {
                self.mutable_storage_exit(model, state, report);
            }
            return;
        }
        let before = state.clone();
        let expected: Vec<_> = if exact {
            model
                .cleanup_sets
                .borrow()
                .members(before.cleanup_roots)
                .into_iter()
                .map(|root| {
                    work::count(Counter::ValidateCleanupRootRows);
                    LocalId(root)
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut actual = Vec::new();
        let mut ordered = cleanup_order::Order::default();
        let mut parts = Vec::new();
        for action in self.body.cleanup_actions(id) {
            work::count(Counter::ValidateCleanupReplaySteps);
            match *action {
                CleanupAction::Drop { place, if_live } => {
                    let p = &self.body.places[place.0 as usize];
                    let index = model.mapping[place.0 as usize];
                    if !p.projection.is_empty() && needs_release(self, p.ty, self.t) {
                        if p.projection
                            .iter()
                            .any(|p| matches!(p, Projection::Index(_)))
                        {
                            self.flow_error(report, ViolationKind::UnsupportedCleanup(id));
                        }
                        if state.places[index] & LIVE != 0 {
                            parts.push(place);
                        }
                    }
                    if p.projection.is_empty() {
                        if model.release[p.root.0 as usize]
                            && (!if_live || state.places[index] & LIVE != 0)
                        {
                            actual.push(p.root);
                        }
                        if state.places[index] & LIVE != 0 {
                            work::count(Counter::ValidateCleanupRootRows);
                            if model.release[p.root.0 as usize] {
                                ordered.observe(
                                    p.root.0 as usize,
                                    model.temporary[p.root.0 as usize],
                                    model.depth[p.root.0 as usize],
                                );
                            }
                        }
                    }
                    self.drop_place(model, state, place, if_live, report, allocation_effects);
                }
                CleanupAction::EndLoan { loan, if_active } => {
                    self.end_loan(model, state, loan, if_active, report)
                }
                // Frozen RegionId has no declaration/ownership arena. The M2
                // owner must supply a future adapter, rather than accept unknown
                // cleanup effects as proof that an owned slot was released.
                _ => self.flow_error(report, ViolationKind::UnsupportedCleanup(id)),
            }
        }
        let mut sorted = actual.clone();
        sorted.sort_by_key(|l| l.0);
        if exact && sorted != expected {
            self.flow_error(
                report,
                ViolationKind::CleanupMismatch {
                    cleanup: id,
                    expected,
                    actual,
                },
            );
        }
        if ordered.invalid(&before.before) {
            self.flow_error(report, ViolationKind::CleanupOrder(id));
        }
        for (i, a) in parts.iter().enumerate() {
            for b in &parts[i + 1..] {
                let a = &self.body.places[a.0 as usize];
                let b = &self.body.places[b.0 as usize];
                if a.root != b.root {
                    continue;
                }
                for (a, b) in a.projection.iter().zip(&b.projection) {
                    let ordinal = |p: &Projection| match p {
                        Projection::Field(f) => Some((0, *f)),
                        Projection::VariantField { variant, field } => Some((*variant, *field)),
                        _ => None,
                    };
                    if let (Some(a), Some(b)) = (ordinal(a), ordinal(b)) {
                        if a != b {
                            if a.0 == b.0 && a.1 < b.1 {
                                self.flow_error(report, ViolationKind::CleanupOrder(id));
                            }
                            break;
                        }
                    }
                }
            }
        }
        if exact {
            self.mutable_storage_exit(model, state, report);
            for (i, (loan, _, _)) in model.loans.iter().enumerate() {
                if state.loans[i] & LIVE != 0 {
                    self.flow_error(report, ViolationKind::LiveLoanAtExit(*loan));
                }
            }
        }
    }
    fn replace(
        &mut self,
        model: &Model,
        state: &mut State,
        destination: PlaceId,
        value: &Operand,
        displaced: PlaceId,
        if_live: bool,
        failure: CleanupId,
        report: bool,
    ) {
        let independent_data = self.independent_global_data(state, value);
        self.state_install(model, state, destination, independent_data, report);
        self.replace_creation(model, state, destination, value, displaced);
        if report && self.noalloc_body() {
            let root = self.body.places[destination.0 as usize].root.0 as usize;
            if self.replacement_allocates(
                destination,
                state.current_unique[root] && !state.shared[root],
            ) {
                self.flow_error(
                    true,
                    ViolationKind::TypeMismatch {
                        entity: "body replacement allocation",
                    },
                );
            }
        }
        self.projection_origin(model, state, destination, report);
        let availability = state.places[model.mapping[destination.0 as usize]];
        if if_live {
            if availability & LIVE == 0 || availability == LIVE {
                self.flow_error(report, ViolationKind::InvalidUse(destination));
            }
        } else {
            self.available(model, state, destination, false, report);
        }
        self.conflict(model, state, destination, false, true, report);
        let target = model.mapping[destination.0 as usize];
        let old = model.mapping[displaced.0 as usize];
        let root = model.places[target].root.0 as usize;
        let old_root = model.places[old].root.0 as usize;
        let source = model.mapping[value.place.0 as usize];
        if (self.readonly_map_entry(destination) || self.readonly_byte_index(destination))
            || (model.borrowed[root] && !model.mutable_parameters[root])
            || self.body.locals[old_root].storage != Storage::Temporary
            || !model.places[old].projection.is_empty()
            || model.overlaps(target, old)
            || model.overlaps(source, old)
            || model.overlaps(target, source)
        {
            self.flow_error(report, ViolationKind::InvalidUse(destination));
        }
        // Derive the old content from root storage and declaration projections;
        // the producer's localized access row is not the storage contract.
        let store = self.t;
        let access = self.body.places[destination.0 as usize].clone();
        let body = self.body;
        let content = self.projection_type(store, &access, |depth, field| {
            let mut sibling = access.clone();
            sibling.projection.truncate(depth);
            sibling.projection.push(Projection::Field(field.0));
            sibling.binding = access.binding.clone().and_then(|mut binding| {
                let base = access.projection.len().checked_sub(binding.fields.len())?;
                binding.fields.truncate(depth.checked_sub(base)?);
                binding.fields.push(field);
                Some(binding)
            });
            let identities = Self::access_identity(model, state, &sibling, body);
            (identities.len() == 1).then(|| identities.first().unwrap().clone())
        });
        // A projected mut access lends the underlying storage contract. Its
        // owning staging share can be Local without making that caller-owned
        // content Local, as when a closure lends a list for mutable iteration.
        let local_receiver = state.must[root].has(MustFacts::OWNED)
            && (access.projection.is_empty() || access.loan.is_none())
            && !state.ownership_conflicts[root]
            && !model.borrowed[root]
            && self
                .lifetime(
                    &self
                        .t
                        .get(self.body.locals[root].ty)
                        .unwrap()
                        .modes
                        .relation,
                )
                .has_environment();
        // A `mut` parameter that declares a relation receives content within
        // it (S2), so its displaced content may be local to those places.
        let related_receiver = model.mutable_parameters[root]
            && self
                .t
                .get(self.body.locals[root].ty)
                .unwrap()
                .modes
                .relation
                .reaches()
                .iter()
                .any(|r| matches!(r, hirsel_types::Reach::Place(o) if !self.arena_place(o)));
        let content = content.map(|ty| {
            if local_receiver || related_receiver {
                return ty;
            }
            hirsel_types::storage_content_type(
                store,
                ty,
                &self.generic_declarations(),
                &mut |path| {
                    Ok::<_, hirsel_types::QueryError>(
                        self.signatures.get(path).cloned().map(std::sync::Arc::new),
                    )
                },
            )
            .unwrap_or_else(|error| self.proof_failure(error, ty))
        });
        let mut proof = state.origins.bindings.clone();
        proof.extend(state.origins.proofs[target].clone());
        let content = content.and_then(|ty| {
            super::flow_origins::canonical(store, ty, &proof, &|o| self.arena_place(o)).ok()
        });
        let displaced_type =
            super::flow_origins::canonical(store, self.place_type(displaced), &proof, &|o| {
                self.arena_place(o)
            })
            .ok();
        // Copies can weaken the old value's uniqueness and sendability after
        // its storage declaration. The displaced row may retain weaker evidence,
        // but must preserve its receiving lifetime and retained brands.
        // Both sides as this frame sees them (S2).
        let frame = |store: &TypeInterner, ty: TypeId| {
            let mut row = store.get(ty).unwrap().clone();
            row.modes.relation = self.frame_relation(&row.modes.relation);
            store.intern(row)
        };
        let content = content.map(|ty| frame(store, ty));
        let displaced_type = displaced_type.map(|ty| frame(store, ty));
        let displaced_comparison = displaced_type.map(|actual| {
            let Some(expected) = content else {
                return actual;
            };
            let mut row = store.get(actual).unwrap().clone();
            if row.modes.sendability == hirsel_types::Sendability::Sendable
                && store.get(expected).unwrap().modes.sendability
                    != hirsel_types::Sendability::Sendable
            {
                let old = self.transferred_type(
                    state,
                    &Operand {
                        place: destination,
                        usage: UseKind::Borrow,
                    },
                    store,
                );
                if store.get(old).unwrap().modes.sendability == hirsel_types::Sendability::Sendable
                {
                    row.modes.sendability = store.get(expected).unwrap().modes.sendability.clone();
                    return store.intern(row);
                }
            }
            actual
        });
        if !matches!((content, displaced_comparison), (Some(expected), Some(actual))
        if (local_receiver || related_receiver || self.lifetime(&store.get(actual).unwrap().modes.relation).is_global())
            && store.get(actual).unwrap().brand == store.get(expected).unwrap().brand
            && {
                let (expected, actual) = super::typing::matching_crossing_contracts(store, expected, actual, &mut HashMap::default());
                self.compatible(store, actual, expected, hirsel_types::TypeRelation::Value)
            })
        {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "displaced storage content",
                },
            );
        }
        if let Some(bound) = self.carrier_bound(destination)
            && !self
                .authority(self.place_type(value.place))
                .is_subset_of(&bound)
        {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "erased carrier write",
                },
            );
        }
        let incoming_type = self.transferred_type(state, value, store);
        let value_type = super::flow_origins::canonical(
            store,
            incoming_type,
            &state.origins.proofs[source],
            &|o| self.arena_place(o),
        )
        .ok();
        if !matches!((content, value_type), (Some(expected), Some(actual)) if self.received_type(store, expected, actual, value.usage))
        {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "replacement",
                },
            );
        }
        // A moved Plain value is its own bits, wherever it was read from: as
        // for any other Take operand, only storage with release obligations
        // must be owned to move into a replacement.
        let owned = value.usage == UseKind::Copy
            || value.usage == UseKind::Take
                && (state.must[model.places[source].root.0 as usize].has(MustFacts::OWNED)
                    || !needs_release(self, self.place_type(value.place), self.t));
        let new_loans = self.operand_loans(model, state, value);
        let old_loans = self.installed_loans(state, destination);
        let new_shares = self.operand_shares(state, value);
        let old_shares = self.operand_shares(
            state,
            &Operand {
                place: destination,
                usage: UseKind::Take,
            },
        );
        let dependencies = new_loans.held();
        let sources = self.sources(model, state, value);
        if !owned
            || !local_receiver
                && (!dependencies.is_empty()
                    || !sources.is_empty()
                    || !self
                        .lifetime(
                            &self
                                .t
                                .get(self.place_type(value.place))
                                .unwrap()
                                .modes
                                .relation,
                        )
                        .is_global())
        {
            self.flow_error(report, ViolationKind::BorrowedMove(value.place));
        }
        // A failed address check happens before either storage value changes.
        if access
            .projection
            .iter()
            .any(|p| matches!(p, Projection::Index(_)))
        {
            self.failure_cleanup(model, state, failure, report, true);
        }
        // Replacement itself expires indexed-container evidence, independently
        // of any producer-written EndOrigin instruction.
        if access
            .projection
            .iter()
            .any(|p| matches!(p, Projection::Index(_)))
        {
            self.end_origin(model, state, destination);
        }
        let incoming = state.clone();
        let new_tags = self.tag_parts(state, value);
        let old_tags = self.tag_parts(
            state,
            &Operand {
                place: destination,
                usage: UseKind::Take,
            },
        );
        let new_authority = self.operand_authority(model, state, value.place);
        let old_authority = self.operand_authority(model, state, destination);
        let new_environment = self.environment(model, state, value.place);
        let old_environment = self.environment(model, state, destination);
        let new_global = self.global_operand(state, value)
            && sources.is_empty()
            && dependencies.is_empty()
            && new_environment.retained().is_empty();
        let old_global = state.must[root].has(MustFacts::GLOBAL);
        let old_sendable = state.must[root].has(MustFacts::SENDABLE);
        let content = self.t;
        let transferred = self.transferred_type(state, value, content);
        let new_sendable = content.get(transferred).unwrap().modes.sendability
            == hirsel_types::Sendability::Sendable;
        let new_crossing = self.operand_crossing_parts(state, value);
        let old_crossing = self.operand_crossing_parts(
            state,
            &Operand {
                place: destination,
                usage: UseKind::Take,
            },
        );
        let new_unique = self.operand_unique(state, value);
        let new_receipt = self.operand_fresh_receipt(state, value);
        let old_receipt = self.operand_fresh_receipt(
            state,
            &Operand {
                place: destination,
                usage: UseKind::Take,
            },
        );
        let old_unique = self.operand_unique(
            state,
            &Operand {
                place: destination,
                usage: UseKind::Take,
            },
        );
        self.mode_transfer(state, self.place_type(destination), value, report);
        self.read(model, state, value, report);
        self.reset_unique(state, displaced);
        self.set_current_unique(state, displaced, old_unique);
        self.set_fresh_receipt(state, displaced, old_receipt);
        self.set_current_global(state, displaced, old_global);
        self.set_current_sendable(state, displaced, old_sendable);
        self.install_crossing_parts(state, displaced, &old_crossing, model.crossing_depth);
        self.write(model, state, displaced, true, false, report);
        if if_live {
            for part in model.parts(displaced) {
                state.places.set(part, LIVE | DEAD);
            }
            model.refresh_cleanup(state, old_root);
        }
        self.set_authority(model, state, displaced, old_authority);
        self.install_tags(model, state, displaced, old_tags);
        self.set_loan_content(model, state, displaced, &old_loans);
        self.set_borrowed_shares(model, state, displaced, old_shares, true);
        self.set_sources(
            model,
            state,
            displaced,
            model
                .parts(destination)
                .flat_map(|part| incoming.sources[part].iter().copied())
                .collect(),
        );
        state.origins.parts[old] = incoming.origins.parts[target].clone();
        state.origins.identities[old] = incoming.origins.identities[target].clone();
        state.origins.owners[old] = incoming.origins.owners[target].clone();
        self.set_origin_proof(
            model,
            state,
            displaced,
            incoming.origins.proofs[target].clone(),
            model
                .parts(destination)
                .flat_map(|part| incoming.origins.expired[part].iter().cloned())
                .collect(),
        );
        self.set_environment(model, state, displaced, &old_environment);
        // This state transition is atomic: no failure edge observes dead storage.
        self.clear(model, state, destination, MOVED);
        self.reset_unique(state, destination);
        self.set_current_unique(state, destination, new_unique);
        self.set_fresh_receipt(state, destination, new_receipt);
        self.set_current_global(state, destination, new_global);
        self.set_current_sendable(state, destination, new_sendable);
        self.install_crossing_parts(state, destination, &new_crossing, model.crossing_depth);
        self.write(model, state, destination, owned, independent_data, report);
        self.set_authority(model, state, destination, new_authority);
        self.install_tags(model, state, destination, new_tags);
        self.set_loan_content(model, state, destination, &new_loans);
        self.set_borrowed_shares(model, state, destination, new_shares, true);
        self.set_sources(model, state, destination, sources);
        state.origins.identities[target] = incoming.origins.identities[source].clone();
        state.origins.owners[target] = incoming.origins.owners[source].clone();
        self.set_origin_proof(
            model,
            state,
            destination,
            incoming.origins.proofs[source].clone(),
            incoming.origins.expired[source].clone(),
        );
        self.set_environment(model, state, destination, &new_environment);
        if model.mutable_parameters[root] {
            self.storage_dependencies(model, state, destination, report);
        }
    }
    fn storage_dependencies(&mut self, model: &Model, state: &State, place: PlaceId, report: bool) {
        let index = model.mapping[place.0 as usize];
        let loans = self.installed_loans(state, place);
        if !loans.held().is_empty() || !loans.expired().is_empty() {
            self.flow_error(report, ViolationKind::InvalidResultSource(place));
        }
        for i in model.contents(index) {
            if !state.sources[i].is_empty() {
                self.flow_error(report, ViolationKind::InvalidResultSource(place));
            }
            if !state.origins.expired[i].is_empty() {
                self.flow_error(report, ViolationKind::InvalidOrigin(place));
            }
        }
    }
    /// A mut callee hands back its exact receiving contract on both outcomes.
    /// Its mutation expires outside holders, but the returned storage carries
    /// the declared brands against the owners current after the call, including
    /// sibling owners inside that same aggregate. Hidden capture dependencies
    /// are independent of the declared type and retain their expiration.
    /// Expiration caused by an owner outside this argument also survives.
    fn mutable_origin_handback(&self, model: &Model, state: &mut State, place: PlaceId) {
        let names = super::flow_origins::LiveNames::new(&state.origins);
        let mut owners = self.origin_identity(model, state, place);
        owners.extend(self.body.places[place.0 as usize].binding.iter().cloned());
        for index in model.parts(place) {
            let ty = model.places[index].ty;
            let proof = super::flow_origins::remember(
                self.t,
                &self.scratch.origin_types[self.site.body.0 as usize],
                ty,
                &names,
                &|origin| self.arena_place(origin),
            );
            let retained = super::flow_origins::retained(
                self.t,
                &self.scratch.origin_types[self.site.body.0 as usize],
                ty,
                &proof,
                &|origin| self.arena_place(origin),
            );
            let mut expired = state.origins.expired[index].clone();
            expired.retain(|ended| {
                !owners
                    .iter()
                    .any(|owner| super::flow_origins::contains(owner, ended))
            });
            expired.extend(state.origins.expired_by(&retained));
            state.origins.expired[index] = expired;
            state.origins.proofs[index] = proof;
            state.origins.retained.set(index, retained);
        }
    }
    /// A whole taken data graph may forget brands only when its receiving
    /// declaration needs none. Generic and hand-back contracts keep evidence.
    fn taken_brandless_data(&mut self, callee: &Callee, argument: &CallArgument) -> bool {
        if argument.value.usage != UseKind::Take
            || !self.body.places[argument.value.place.0 as usize]
                .projection
                .is_empty()
        {
            return false;
        }
        let function = match callee {
            Callee::Definition { path, .. } => {
                self.signatures.get(path).and_then(|s| match &s.kind {
                    SignatureKind::Function(f) => Some(f),
                    _ => None,
                })
            }
            Callee::Indirect { value, .. } => {
                self.t
                    .get(self.place_type(value.place))
                    .and_then(|row| match &row.kind {
                        TypeKind::Function(f) => Some(f),
                        _ => None,
                    })
            }
        };
        let Some(parameter) =
            function.and_then(|f| f.parameters.get(argument.parameter.0 as usize))
        else {
            return false;
        };
        if parameter.passing != hirsel_syntax::PassingMode::Take
            || !self.bound(
                self.t,
                parameter.ty,
                hirsel_types::Bound::Data,
                &self.generic_declarations(),
            )
        {
            return false;
        }
        let mut pending = vec![parameter.ty];
        let mut seen = HashSet::default();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = self.t.get(ty).unwrap();
            if !row.brand.alternatives.is_empty() || matches!(row.kind, TypeKind::Parameter(_)) {
                return false;
            }
            pending.extend(row.kind.value_parts());
            if matches!(row.kind, TypeKind::Named { .. }) {
                let Some(layout) = self.layout(self.t, ty) else {
                    return false;
                };
                pending.extend(layout.value_parts());
            }
        }
        true
    }
    fn callee_preserves_state(
        &self,
        callee: &Callee,
        ordinal: hirsel_types::ParamId,
        advance: bool,
    ) -> bool {
        let function = match callee {
            Callee::Definition { path, .. } => {
                self.signatures.get(path).and_then(|s| match &s.kind {
                    SignatureKind::Function(f) => Some(f),
                    _ => None,
                })
            }
            Callee::Indirect { value, .. } => {
                self.t
                    .get(self.place_type(value.place))
                    .and_then(|row| match &row.kind {
                        TypeKind::Function(f) => Some(f),
                        _ => None,
                    })
            }
        };
        function.is_some_and(|f| {
            f.state_promises.iter().any(|p| {
                p.selector.parameter == ParameterRef::bound(ordinal)
                    && p.selector.fields.is_empty()
                    && p.preserve_captures
                    && (!advance || p.advance_only)
            })
        })
    }
    fn callee_preserves_unique(&self, callee: &Callee, ordinal: hirsel_types::ParamId) -> bool {
        let Callee::Definition { path, .. } = callee else {
            // MODE-2: a function value's promise may come from an unproved body.
            return false;
        };
        self.signatures.get(path).is_some_and(|signature| {
            let SignatureKind::Function(f) = &signature.kind else {
                return false;
            };
            if hirsel_types::unique_summary::same_module(path, self.definition.path.path())
                && signature
                    .metadata
                    .inferred_unique
                    .preserves_unique
                    .contains(&ordinal)
            {
                return true;
            }
            // A foreign declaration has no body to prove a promise (LANG-7).
            f.foreign.is_none()
                && (f.state_promises.iter().any(|p| {
                    p.selector.parameter == ParameterRef::bound(ordinal) && p.preserve_unique
                }) || ordinal == hirsel_types::ParamId(1)
                    && super::format_association(self.t, signature, self.signatures))
        })
    }
    /// MODE-2: a direct generic call whose result holds only values of type
    /// parameters it cannot copy returns a unique result when every argument
    /// that may supply them is unique (`parametric_inputs`).
    fn parametric_unique(
        &self,
        state: &State,
        callee: &Callee,
        arguments: &[CallArgument],
    ) -> bool {
        let Callee::Definition { path, .. } = callee else {
            return false;
        };
        let Some(signature) = self.signatures.get(path) else {
            return false;
        };
        let SignatureKind::Function(function) = &signature.kind else {
            return false;
        };
        let mut lookup =
            |path: DefId| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new));
        // A bodyless core declaration has no checked body.
        let checked = !path.package_is("core", "1");
        let Some(inputs) = parametric_inputs(self.t, function, checked, &mut lookup) else {
            return false;
        };
        let types = self.t;
        arguments.iter().all(|a| {
            let ty = self.place_type(a.value.place);
            match inputs.get(a.parameter.0 as usize) {
                Some(ParametricInput::Free) => true,
                Some(ParametricInput::Unique) => {
                    !needs_release(self, ty, types)
                        || super::handle_root(types, ty)
                        || self.operand_unique(state, &a.value)
                }
                Some(ParametricInput::UniqueResult) => {
                    matches!(&types.get(ty).unwrap().kind, TypeKind::Function(f)
                    if unique_rows(
                        types,
                        f.result,
                        &self.generic_declarations(),
                        &mut |path: DefId| {
                            Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new))
                        },
                    ))
                }
                None => false,
            }
        })
    }
    // Await consumes a Task whose result contract is fixed by its producer.
    // MODE-2 keeps the physical bare-R result slot aliased, but moving a
    // declared unique payload out of that linear handle preserves its proof.
    fn awaited_unique(&self, callee: &Callee, arguments: &[CallArgument]) -> bool {
        let Callee::Definition { path, .. } = callee else {
            return false;
        };
        if !path.is_core("task.await") {
            return false;
        }
        let Some(handle) = arguments.iter().find(|argument| {
            argument.parameter == hirsel_types::ParamId(1) && argument.value.usage == UseKind::Take
        }) else {
            return false;
        };
        let TypeKind::Builtin {
            kind: BuiltinType::Task,
            arguments,
        } = &self.t.row(self.place_type(handle.value.place)).kind
        else {
            return false;
        };
        matches!(arguments.first(), Some(hirsel_types::GenericArgument::Type(result))
            if self.t.row(*result).modes.uniqueness == hirsel_types::Uniqueness::Unique)
    }
    /// A borrowed parameter a bodyless core promise reads and stores nothing of.
    fn callee_unretained(&self, callee: &Callee, ordinal: hirsel_types::ParamId) -> bool {
        let Callee::Definition { path, .. } = callee else {
            return false;
        };
        self.signatures.get(path).is_some_and(|s| match &s.kind {
            SignatureKind::Function(f) => f
                .state_promises
                .iter()
                .any(|p| p.preserve_unique && p.unretained.contains(&ordinal)),
            _ => false,
        })
    }
    /// Borrow-only shares follow the particular fields that retain them.
    fn operand_shares(&self, state: &State, operand: &Operand) -> loan_content::Content {
        if !needs_release(self, self.place_type(operand.place), self.t) {
            return loan_content::Content::default();
        }
        let access = &self.body.places[operand.place.0 as usize];
        state.borrowed_shares[access.root.0 as usize].project(
            &super::flow_origins::EnvironmentField::path(&access.projection),
        )
    }
    fn rvalue_shares(&self, model: &Model, state: &State, value: &Rvalue) -> loan_content::Content {
        use super::flow_origins::EnvironmentField;
        let mut shares = loan_content::Content::default();
        if matches!(
            value,
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                ..
            }
        ) {
            return shares;
        }
        if let Rvalue::Construct {
            constructor,
            fields,
            ..
        } = value
        {
            for field in fields {
                let key = match constructor {
                    Constructor::Variant { variant, .. } => {
                        EnvironmentField::Variant(*variant, field.field)
                    }
                    Constructor::Sequence | Constructor::Map | Constructor::Set => {
                        EnvironmentField::Index
                    }
                    _ => EnvironmentField::Field(field.field),
                };
                shares.insert(
                    &[key],
                    &self.operand_shares(state, &field.value),
                    &model.environment_paths,
                );
            }
        } else {
            for operand in typing::operands(value) {
                shares.join(&self.operand_shares(state, operand));
            }
        }
        shares
    }
    fn set_borrowed_shares(
        &self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        shares: loan_content::Content,
        whole: bool,
    ) {
        let access = &self.body.places[place.0 as usize];
        let path = super::flow_origins::EnvironmentField::path(&access.projection);
        let installed = &mut state.borrowed_shares[access.root.0 as usize];
        if whole {
            installed.replace(&path, &shares, &model.environment_paths);
        } else {
            installed.insert(&path, &shares, &model.environment_paths);
        }
    }
    fn mark_borrowed_share(
        &self,
        model: &Model,
        state: &mut State,
        operand: &Operand,
    ) -> BTreeSet<usize> {
        let access = &self.body.places[operand.place.0 as usize];
        let mut roots = BTreeSet::from([access.root.0 as usize]);
        if !matches!(self.t.row(access.ty).kind, TypeKind::Function(_)) {
            let mut source = operand.clone();
            source.usage = UseKind::Borrow;
            roots.extend(self.sources(model, state, &source));
            roots.extend(
                self.dependencies(model, state, &source)
                    .into_iter()
                    .map(|loan| self.body.places[model.loans[loan].1.0 as usize].root.0 as usize),
            );
        }
        // A result carries only shares its operands introduce, not unrelated
        // borrow-only shares that were already live before this call.
        let restorable: Vec<_> = roots
            .into_iter()
            .filter(|&root| {
                state.borrow_only[root] || !state.shared[root] && state.current_unique[root]
            })
            .collect();
        self.mark_shared(model, state, operand);
        let mut borrowed = BTreeSet::new();
        for root in restorable {
            if state.shared[root] {
                state.borrow_only.set(root, true);
                borrowed.insert(root);
            }
        }
        borrowed
    }
    /// MODE-2: a root aliased only by related call results' shares is unique
    /// again once no live local holds one.
    fn restore_borrowed(&self, model: &Model, state: &mut State) {
        let borrowed: Vec<usize> = state
            .borrow_only
            .entries()
            .into_iter()
            .filter(|(_, only)| **only)
            .map(|(root, _)| root)
            .collect();
        for root in borrowed {
            let held = state
                .borrowed_shares
                .entries()
                .into_iter()
                .any(|(local, shares)| {
                    shares.contains(root)
                        && model
                            .contents(model.roots[local])
                            .any(|i| state.places[i] & LIVE != 0)
                });
            if !held {
                state.shared.set(root, false);
                state.nested_shared.set(root, false);
                state.borrow_only.set(root, false);
            }
        }
    }
    fn mutable_storage_exit(&mut self, model: &Model, state: &State, report: bool) {
        for root in 0..model.mutable_parameters.len() {
            if !model.mutable_parameters[root] {
                continue;
            }
            let place = model.body_roots[root].unwrap();
            // MODE-2: the caller still holds a `mut x: unique T` argument, or a
            // `@preserves_unique` re-proof's selection, as unique.
            let ty = self.place_type(place);
            let promised = match model.unique_proof {
                Some(selected) => selected == root,
                None => {
                    self.t.get(ty).unwrap().modes.uniqueness == hirsel_types::Uniqueness::Unique
                }
            };
            if promised
                && !model.releasing.get()
                && needs_release(self, ty, self.t)
                && !super::handle_root(self.t, ty)
                && (state.shared[root]
                    && !(state.borrow_only[root]
                        && !model.returning_shares.borrow().contains(&root))
                    || !state.current_unique[root])
            {
                self.flow_error(
                    report,
                    ViolationKind::TypeMismatch {
                        entity: if model.unique_proof.is_some() {
                            UNIQUE_PROOF_FAILED
                        } else {
                            "mutable unique hand-back"
                        },
                    },
                );
            }
            if state.nested_shared[root]
                && !(state.borrow_only[root] && !model.returning_shares.borrow().contains(&root))
                && self.has_unique_parts(self.t, ty)
            {
                self.flow_error(
                    report,
                    ViolationKind::TypeMismatch {
                        entity: "mutable nested unique hand-back",
                    },
                );
            }
            for i in model.contents(model.roots[root]) {
                if state.places[i] != LIVE {
                    self.flow_error(report, ViolationKind::ReadBeforeInit(place));
                }
            }
            self.storage_dependencies(model, state, place, report);
            self.nested_crossing_receipt(
                state,
                self.t,
                self.place_type(place),
                &Operand {
                    place,
                    usage: UseKind::Mut,
                },
                report,
            );
            let row = self.t.get(self.place_type(place)).unwrap();
            if hirsel_types::crossing_requirement(row) == hirsel_types::Sendability::Sendable {
                let store = self.t;
                let actual = self.transferred_type(
                    state,
                    &Operand {
                        place,
                        usage: UseKind::Mut,
                    },
                    store,
                );
                if store.get(actual).unwrap().modes.sendability
                    != hirsel_types::Sendability::Sendable
                {
                    self.flow_error(
                        report,
                        ViolationKind::TypeMismatch {
                            entity: "mutable crossing hand-back",
                        },
                    );
                }
            }
        }
    }
    fn failure_cleanup(
        &mut self,
        model: &Model,
        state: &State,
        id: CleanupId,
        report: bool,
        may_fail: bool,
    ) {
        if report && model.trivial_cleanup && self.body.cleanups[id.0 as usize].head.is_none() {
            self.mutable_storage_exit(model, state, true);
            return;
        }
        if report
            && (model.noalloc_drop_checks && may_fail
                || !model.proves_cleanup(self.body, state, id))
        {
            self.cleanup(model, &mut state.clone(), id, true, true, may_fail);
        }
    }
    fn transfer(&mut self, model: &Model, state: &mut State, block_id: BlockId, report: bool) {
        self.site.block = Some(block_id);
        let block = &self.body.blocks[block_id.0 as usize];
        for (i, instruction) in block.instructions.iter().enumerate() {
            self.site.instruction = Some(i as u32);
            self.acquisitions(state, report);
            self.restore_borrowed(model, state);
            match &instruction.kind {
                InstructionKind::BindOrigin {
                    source,
                    place,
                    kind,
                } => {
                    self.projection_origin(model, state, *source, report);
                    self.access_loans(model, state, *source, false, report);
                    let identity = self.origin_identity(model, state, *source);
                    self.available(model, state, *source, false, report);
                    let index = model.mapping[place.0 as usize];
                    let Some(origin) = &self.body.places[place.0 as usize].binding else {
                        self.flow_error(report, ViolationKind::InvalidOrigin(*place));
                        continue;
                    };
                    let from = &self.body.places[source.0 as usize];
                    let to = &self.body.places[place.0 as usize];
                    // A name carried by capture storage belongs to the
                    // environment. The exception is the name of the expression
                    // being bound, on its own place: an element or block value
                    // computed from a capture is this body's value.
                    work::count(Counter::ValidatePlaceRows);
                    let captured = model.captured_origins.get(&origin.root).is_some_and(
                        |(first, multiple)| {
                            let first = &self.body.places[*first];
                            *multiple
                                || first.root != to.root
                                || first.projection != to.projection
                                || !matches!(origin.root, OriginRoot::Expression(..))
                        },
                    );
                    if from.root != to.root
                        || from.loan != to.loan
                        || from.projection != to.projection
                        || (*kind == OriginBindingKind::Fresh && captured)
                        || (!origin.fields.is_empty() && *kind != OriginBindingKind::Fresh)
                        || !(matches!(
                            origin.root,
                            OriginRoot::Local(..) | OriginRoot::Expression(..)
                        ) || (*kind == OriginBindingKind::Fresh
                            && matches!(origin.root, OriginRoot::Parameter(ParameterRef::Body { scope, .. }) if scope == self.body.parameter_scope)
                            && model.binding_valid(state, to, self.body)))
                    {
                        self.flow_error(report, ViolationKind::InvalidOrigin(*place));
                        continue;
                    }
                    if *kind == OriginBindingKind::Fresh {
                        // Reintroducing an existing live name is an identity
                        // join. Snapshot its field aliases before replacing the
                        // whole identity, so feedback preserves agreeing parts.
                        if origin.fields.is_empty()
                            && from
                                .binding
                                .as_ref()
                                .is_some_and(|o| o.root == origin.root && o.fields.is_empty())
                            && state.origins.locations.contains_key(&origin.root)
                            && !state.origins.ended.contains_key(&origin.root)
                        {
                            for path in model.origin_paths_of(model.places[index].ty) {
                                let identities = state.origins.parts[index]
                                    .get(path)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        state.origins.identities[index]
                                            .iter()
                                            .map(|o| {
                                                let mut o = o.clone();
                                                o.fields.extend(path);
                                                o
                                            })
                                            .collect()
                                    });
                                state.origins.parts[index].insert(path.clone(), identities);
                            }
                        } else if origin.fields.is_empty() {
                            // A newly named transferred owner or replacement
                            // introduces a new namespace for all its fields.
                            state.origins.parts[index].clear();
                        }
                        if origin.fields.is_empty()
                            || !self.refresh_copied_prefix(model, state, to, origin)
                        {
                            state.expire_origin(origin);
                            state.origins.introduce(origin);
                            if !origin.fields.is_empty() {
                                // Replacing a field invalidates old parent brands.
                                // New brands refer to the enclosing owned storage,
                                // whose transferred identity survives the mutation.
                                let root = model.roots[to.root.0 as usize];
                                let owned: BTreeSet<_> = state.origins.identities[root]
                                    .iter()
                                    .filter(|identity| {
                                        state.origins.owners[root].iter().any(|owner| {
                                            super::flow_origins::contains(owner, identity)
                                        })
                                    })
                                    .cloned()
                                    .collect();
                                let identities = if owned.is_empty() {
                                    BTreeSet::from([Origin {
                                        root: origin.root.clone(),
                                        fields: vec![].into(),
                                    }])
                                } else {
                                    owned
                                };
                                for identity in &identities {
                                    state.origins.introduce(identity);
                                }
                                state.origins.identities[root] = identities.clone().into();
                                state
                                    .origins
                                    .bindings
                                    .insert(origin.root.clone(), identities);
                            }
                            let identities = if origin.fields.is_empty() {
                                BTreeSet::from([origin.clone()])
                            } else {
                                self.origin_identity(model, state, *place)
                            };
                            for identity in &identities {
                                state.origins.introduce(identity);
                            }
                            state.origins.identities[index] = identities.clone().into();
                            state.origins.owners[index].extend(identities);
                        }
                        // The written field takes the binding's own name
                        // whenever the binding is the sole owner of its
                        // storage: named freshly, or holding a computed value
                        // it received whole.
                        let root = model.roots[to.root.0 as usize];
                        let sole = state.origins.identities[root].len() == 1
                            && state.origins.identities[root].iter().all(|identity| {
                                identity.fields.is_empty()
                                    && (identity.root == origin.root
                                        || (matches!(identity.root, OriginRoot::Expression(..))
                                            && state.origins.owners[root].contains(identity)))
                            });
                        if !origin.fields.is_empty()
                            && sole
                            && !to
                                .projection
                                .iter()
                                .any(|p| matches!(p, Projection::Index(_)))
                        {
                            let path: Vec<_> = to
                                .projection
                                .iter()
                                .map_while(|p| match p {
                                    Projection::Field(f)
                                    | Projection::VariantField { field: f, .. } => {
                                        Some(hirsel_types::FieldId(*f))
                                    }
                                    Projection::Index(_) => None,
                                })
                                .collect();
                            state.origins.parts[model.roots[to.root.0 as usize]]
                                .insert(path, BTreeSet::from([origin.clone()]));
                            state.origins.identities[index] =
                                BTreeSet::from([origin.clone()]).into();
                            state.origins.owners[index] = BTreeSet::from([origin.clone()]).into();
                        }
                    } else {
                        state.origins.identities[index] = identity.into();
                    }
                    if origin.fields.is_empty() {
                        state.origins.locations.insert(
                            origin.root.clone(),
                            BTreeSet::from([(index, to.loan.map(|id| id.0))]),
                        );
                        state.origins.ended.remove(&origin.root);
                        state.origins.bindings.insert(
                            origin.root.clone(),
                            (*state.origins.identities[index]).clone(),
                        );
                        // The structural projection check proved the layout.
                        // Introducing the owner supplies its field-path proofs.
                        let names = BTreeMap::from([(
                            origin.root.clone(),
                            (*state.origins.identities[index]).clone(),
                        )]);
                        for &ordinal in &model.root_accesses[model.places[index].root.0 as usize] {
                            let access = &self.body.places[ordinal];
                            let projected = model.mapping[ordinal];
                            if state.places[projected] == LIVE
                                && model.contains(index, projected)
                                && !access.projection.is_empty()
                            {
                                let proof = super::flow_origins::remember(
                                    self.t,
                                    &self.scratch.origin_types[self.site.body.0 as usize],
                                    access.ty,
                                    &names,
                                    &|o| self.arena_place(o),
                                );
                                for (name, values) in proof {
                                    state.origins.proofs[projected]
                                        .entry(name)
                                        .or_insert(values);
                                }
                                let retained = super::flow_origins::retained(
                                    self.t,
                                    &self.scratch.origin_types[self.site.body.0 as usize],
                                    access.ty,
                                    &state.origins.proofs[projected],
                                    &|o| self.arena_place(o),
                                );
                                state
                                    .origins
                                    .retained
                                    .update(projected, |row| row.extend(retained));
                            }
                        }
                    }
                }
                InstructionKind::BindFields { source, fields } => {
                    self.bind_fields(model, state, *source, fields, report);
                }
                InstructionKind::Defuse { place } => {
                    self.available(model, state, *place, true, report);
                    self.conflict(model, state, *place, false, false, report);
                    let index = model.mapping[place.0 as usize];
                    let root = model.places[index].root.0 as usize;
                    let owner = match &self.t.get(self.place_type(*place)).unwrap().kind {
                        TypeKind::Named { definition, .. } => Some(definition),
                        _ => None,
                    };
                    let valid = owner.is_some_and(|owner| {
                        owner.path().0.package == self.definition.path.path().0.package
                            && owner.path().0.modules == self.definition.path.path().0.modules
                            && self.signatures.get(*owner).is_some_and(|signature| matches!(
                                &signature.kind, SignatureKind::Type(td) if td.resource && td.drop.is_some()
                            ))
                    });
                    if !valid || state.defused[index] != 1 {
                        self.flow_error(report, ViolationKind::InvalidUse(*place));
                    }
                    if model.borrowed[root]
                        || !state.must[root].has(MustFacts::OWNED)
                        || state.ownership_conflicts[root]
                    {
                        self.flow_error(report, ViolationKind::BorrowedMove(*place));
                    }
                    state.defused.set(index, 2);
                }
                InstructionKind::EndOrigin(place) => {
                    self.available(model, state, *place, false, report);
                    self.end_origin(model, state, *place);
                }
                InstructionKind::StorageDead { place, if_live } => {
                    if needs_release(self, self.place_type(*place), self.t) {
                        self.flow_error(report, ViolationKind::InvalidUse(*place));
                    } else {
                        self.drop_place(model, state, *place, *if_live, report, true);
                    }
                }
                InstructionKind::EndScope(scope) => {
                    // OWN-1/MEM-10: a copied Local value can end its access
                    // loan while retaining a lasting source bound. Check that
                    // same source evidence at scope exit, before retiring views.
                    let locals = model.scope_locals.within(*scope);
                    let ending: BTreeSet<_> = locals.iter().copied().collect();
                    let mut reported = BTreeSet::new();
                    for (part, sources) in state.sources.entries() {
                        let local = model.places[part].root.0 as usize;
                        if ending.contains(&local) || state.places[part] & LIVE == 0 {
                            continue;
                        }
                        if sources.iter().any(|source| ending.contains(source))
                            && reported.insert(local)
                        {
                            if let Some(place) = model.body_roots[local] {
                                self.flow_error(report, ViolationKind::InvalidResultSource(place));
                            }
                        }
                    }
                    for &local in &locals {
                        let root = model.roots[local];
                        if state.places[root] & LIVE == 0 {
                            continue;
                        }
                        if model.release[local]
                            && (state.must[local].has(MustFacts::OWNED)
                                || state.ownership_conflicts[local])
                        {
                            self.flow_error(
                                report,
                                ViolationKind::LeakedSlot(LocalId(local as u32)),
                            );
                            continue;
                        }
                        if let Some(place) = model.body_roots[local] {
                            if !model.release[local] {
                                self.drop_place(model, state, place, true, report, true);
                            } else {
                                // A nonowning view loses storage without releasing its caller's value.
                                self.conflict(model, state, place, true, true, report);
                                self.clear(model, state, place, DEAD);
                            }
                        }
                    }
                    // An ended local's storage holds no value: forget the
                    // origins its last value carried, so paths that ended
                    // different locals agree on every one they did not use.
                    for local in locals {
                        if !state.cursor_views.is_empty() {
                            state.cursor_views.set(local, None);
                            state
                                .cursor_contracts
                                .set(local, borrow_cursors::Contract::default());
                        }
                        for &part in &model.root_parts[local] {
                            if state.places[part] & LIVE != 0 {
                                continue;
                            }
                            if state.places[part] != DEAD {
                                state.places.set(part, DEAD);
                            }
                            if !state.origins.identities[part].is_empty() {
                                state.origins.identities[part].clear();
                            }
                            if !state.origins.owners[part].is_empty() {
                                state.origins.owners[part].clear();
                            }
                            if !state.origins.parts[part].is_empty() {
                                state.origins.parts[part].clear();
                            }
                        }
                    }
                }
                InstructionKind::StorageLive(local) => {
                    let root = model.roots[local.0 as usize];
                    if state.places[root] & LIVE != 0 {
                        if let Some(p) = model.body_roots[local.0 as usize] {
                            self.flow_error(report, ViolationKind::WriteToLive(p));
                        } else {
                            self.flow_error(
                                report,
                                ViolationKind::WrongKind {
                                    entity: "StorageLive on initialized local",
                                    index: local.0,
                                },
                            );
                        }
                    }
                    for (i, (loan, source, _)) in model.loans.iter().enumerate() {
                        if state.loans[i] & LIVE != 0
                            && self.body.places[source.0 as usize].root == *local
                        {
                            self.flow_error(
                                report,
                                ViolationKind::LoanOutlivesSource {
                                    loan: *loan,
                                    place: *source,
                                },
                            );
                        }
                    }
                    for &p in &model.root_parts[local.0 as usize] {
                        state.places.set(p, DEAD);
                    }
                    state
                        .must
                        .set_fact(local.0 as usize, MustFacts::OWNED, false);
                    model.refresh_cleanup(state, local.0 as usize);
                }
                InstructionKind::Replace {
                    destination,
                    value,
                    displaced,
                    if_live,
                } => {
                    if *if_live {
                        let following = self
                            .site
                            .instruction
                            .and_then(|index| block.instructions.get(index as usize + 1));
                        if !following.is_some_and(|next| matches!(next.kind,
                            InstructionKind::Drop { place, if_live: true } | InstructionKind::StorageDead { place, if_live: true } if place == *displaced)) {
                            self.flow_error(report, ViolationKind::InvalidUse(*displaced));
                        }
                    }
                    self.replace(
                        model,
                        state,
                        *destination,
                        value,
                        *displaced,
                        *if_live,
                        instruction.on_failure,
                        report,
                    );
                }
                InstructionKind::Assign { destination, value } => {
                    let rvalue = &self.body.rvalues[value.0 as usize];
                    self.allocation_destination(model, state, rvalue, report);
                    let cursor_view =
                        self.cursor_assignment(model, state, *destination, *value, report);
                    self.assign_creation(model, state, *destination, rvalue);
                    if report {
                        if let Rvalue::Construct {
                            constructor,
                            fields,
                            ..
                        } = rvalue
                        {
                            let witness;
                            let fields = if let Constructor::SortedMap { by } = constructor {
                                witness = vec![ConstructField {
                                    field: 0,
                                    value: by.clone(),
                                }];
                                &witness
                            } else {
                                fields
                            };
                            let arguments: Vec<_> = fields
                                .iter()
                                .map(|field| CallArgument {
                                    parameter: hirsel_types::ParamId(field.field),
                                    value: field.value.clone(),
                                })
                                .collect();
                            let origins = self.call_origin_proof(model, state, &arguments, report);
                            self.constructor_origin_types(
                                *destination,
                                constructor,
                                fields,
                                &origins,
                            );
                        }
                    }
                    if let Rvalue::Closure { body, captures, .. } = rvalue {
                        for operand in captures {
                            // Borrowed capture bodies cannot mutate caller state;
                            // mutable capture effects need the nested state proof.
                            if operand.usage == UseKind::Mut {
                                self.state_capture(model, state, operand.place, report);
                            }
                        }
                        let captures: Vec<_> = captures
                            .iter()
                            .map(|operand| {
                                let index = model.mapping[operand.place.0 as usize];
                                let inactive = self.inactive_variant_payload(operand.place);
                                super::flow_origins::CapturedOrigin {
                                    nested_shared: state.nested_shared[self.body.places
                                        [operand.place.0 as usize]
                                        .root
                                        .0
                                        as usize]
                                        || operand.usage == UseKind::Copy
                                            && !self.flat_value(operand.place)
                                            && !super::handle_root(
                                                self.t,
                                                self.place_type(operand.place),
                                            ),
                                    crossing: self.operand_crossing_parts(state, operand),
                                    contract_readonly: matches!(
                                        operand.usage,
                                        UseKind::Borrow | UseKind::Mut
                                    ) && (state.contract_readonly.contains(
                                        &(self.body.places[operand.place.0 as usize].root.0
                                            as usize),
                                    ) || self
                                        .sources(model, state, operand)
                                        .iter()
                                        .any(|source| state.contract_readonly.contains(source))),
                                    // Mutable captures can change between invocations.
                                    // Every invocation must establish its own tag proof.
                                    tags: if operand.usage == UseKind::Mut {
                                        tags::Parts::new()
                                    } else {
                                        self.tag_parts(state, operand)
                                    },
                                    environment: self.captured_environment(
                                        model, state, *body, captures, operand,
                                    ),
                                    identities: self.origin_identity(model, state, operand.place),
                                    parts: Self::storage_partition(
                                        model,
                                        state,
                                        &self.body.places[operand.place.0 as usize],
                                    ),
                                    proof: state.origins.proofs[index].clone(),
                                    expired: if inactive {
                                        BTreeSet::new()
                                    } else {
                                        model
                                            .parts(operand.place)
                                            .flat_map(|part| {
                                                state.origins.expired[part].iter().cloned()
                                            })
                                            .collect()
                                    },
                                    owners: if operand.usage == UseKind::Take {
                                        (*state.origins.owners[index]).clone()
                                    } else {
                                        BTreeSet::new()
                                    },
                                }
                            })
                            .collect();
                        if let Some(previous) = self.captured.get_mut(&body.0) {
                            super::flow_origins::join_captures(previous, &captures);
                        } else {
                            self.captured.insert(body.0, captures);
                        }
                    }
                    let origin_parts = match rvalue {
                        Rvalue::Use(o) => Self::storage_partition(
                            model,
                            state,
                            &self.body.places[o.place.0 as usize],
                        ),
                        _ => BTreeMap::new(),
                    };
                    let origin_identity = match rvalue {
                        Rvalue::Use(o) => self.origin_identity(model, state, o.place),
                        Rvalue::ReadConstant { definition, .. }
                        | Rvalue::FunctionReference { definition, .. } => {
                            BTreeSet::from([Origin {
                                root: OriginRoot::Definition(
                                    hirsel_types::constant_identity(
                                        definition.clone(),
                                        &mut |id| {
                                            self.signatures
                                                .get(id.path())
                                                .cloned()
                                                .map(std::sync::Arc::new)
                                                .ok_or(hirsel_types::QueryError::NotImplemented)
                                        },
                                    )
                                    .unwrap_or_else(|_| definition.clone()),
                                ),
                                fields: vec![].into(),
                            }])
                        }
                        _ => BTreeSet::new(),
                    };
                    let mut origin_proof = super::flow_origins::remember(
                        self.t,
                        &self.scratch.origin_types[self.site.body.0 as usize],
                        self.place_type(*destination),
                        &state.origins.bindings,
                        &|o| self.arena_place(o),
                    );
                    let origin_owners = typing::operands(rvalue)
                        .into_iter()
                        .filter(|o| o.usage == UseKind::Take)
                        .flat_map(|o| {
                            state.origins.owners[model.mapping[o.place.0 as usize]]
                                .iter()
                                .cloned()
                        })
                        .collect();
                    let tag_parts = self.constructed_tags(state, rvalue);
                    self.rvalue_modes(state, *destination, rvalue, report);
                    let environment = self.rvalue_environment(model, state, rvalue);
                    let mut origin_expired = BTreeSet::new();
                    for operand in typing::operands(rvalue) {
                        let input = model.mapping[operand.place.0 as usize];
                        origin_expired.extend(
                            model
                                .parts(operand.place)
                                .flat_map(|part| state.origins.expired[part].iter().cloned()),
                        );
                        for (root, identities) in &state.origins.proofs[input] {
                            origin_proof.insert(root.clone(), identities.clone());
                        }
                    }
                    let mut sources: BTreeSet<_> = if matches!(
                        rvalue,
                        Rvalue::Construct {
                            constructor: Constructor::SortedMap { .. },
                            ..
                        }
                    ) {
                        BTreeSet::new()
                    } else {
                        typing::operands(rvalue)
                            .into_iter()
                            .flat_map(|o| self.sources(model, state, o))
                            .collect()
                    };
                    let authority = self.rvalue_authority(model, state, rvalue);
                    let nested_shared =
                        self.rvalue_shares_unique_parts(state, *destination, rvalue);
                    let unique = self.rvalue_unique(state, rvalue);
                    let fresh_receipt = self.rvalue_fresh_receipt(state, rvalue);
                    let sendable = self.rvalue_sendable(state, *destination, rvalue);
                    let crossing_parts = self.rvalue_crossing_parts(state, *destination, rvalue);
                    let unique_parts = self.rvalue_unique_parts(state, *destination, rvalue);
                    let mut global = self.rvalue_global(state, *destination, rvalue);
                    // PROV-4: scoped content of an unplaced local holder is
                    // bounded by a placement its type no longer names. This
                    // frame is its source, which no result can declare.
                    let bounded = match rvalue {
                        Rvalue::Use(o) => self.scoped_parts_into_unplaced(*destination, o),
                        Rvalue::Construct { fields, .. } => fields
                            .iter()
                            .any(|field| self.scoped_into_unplaced(*destination, &field.value)),
                        _ => false,
                    };
                    if bounded {
                        global = false;
                    }
                    let owned = match rvalue {
                        Rvalue::Use(o) => match o.usage {
                            UseKind::Borrow | UseKind::Mut => false,
                            UseKind::Copy => true,
                            UseKind::Take => state.must
                                [self.body.places[o.place.0 as usize].root.0 as usize]
                                .has(MustFacts::OWNED),
                        },
                        _ => true,
                    };
                    // MEM-5: assembling an inline tuple from whole-value
                    // takes only transfers ownership. It cannot destroy its
                    // inputs on an invented constructor failure. Keep the
                    // conservative failure join for copies and projections,
                    // whose extraction may still need checks or preparation.
                    let tuple_transfer = matches!(rvalue,
                        Rvalue::Construct { constructor: Constructor::Tuple, fields, .. }
                            if fields.iter().all(|field|
                                field.value.usage == UseKind::Take
                                    && self.body.places[field.value.place.0 as usize]
                                        .projection.is_empty()));
                    let may_fail = !tuple_transfer
                        && matches!(
                            rvalue,
                            Rvalue::Unary { .. }
                                | Rvalue::Binary { .. }
                                | Rvalue::Construct { .. }
                                | Rvalue::Closure { .. }
                        )
                        || typing::operands(rvalue).iter().any(|o| {
                            self.body.places[o.place.0 as usize]
                                .projection
                                .iter()
                                .any(|p| matches!(p, Projection::Index(_)))
                        });
                    // The failure edge joins the state after every operand
                    // prefix. `joined` is the last prefix joined, so each
                    // join visits only what that operand changed.
                    let mut failure_state = may_fail.then(|| {
                        work::count_by(
                            Counter::ValidateFailureStateRows,
                            state.places.len() as u64,
                        );
                        (state.clone(), state.clone())
                    });
                    let shares = self.rvalue_shares(model, state, rvalue);
                    let mut content_loans = self.rvalue_loans(model, state, rvalue);
                    let mut dependencies = content_loans.held();
                    let target = &self.body.places[destination.0 as usize];
                    let transport = matches!(rvalue, Rvalue::Use(o)
                        if matches!(o.usage, UseKind::Take | UseKind::Copy)
                            && self.body.places[o.place.0 as usize].projection.is_empty())
                        && target.projection.is_empty()
                        && matches!(
                            self.body.locals[target.root.0 as usize].storage,
                            Storage::Local | Storage::Temporary
                        );
                    for operand in typing::operands(rvalue) {
                        self.read_operand(
                            model,
                            state,
                            operand,
                            matches!(rvalue, Rvalue::Use(_)),
                            matches!(
                                rvalue,
                                Rvalue::Unary {
                                    op: hirsel_syntax::UnaryOp::Unique,
                                    ..
                                }
                            ),
                            transport,
                            report,
                        );
                        if let Some((failure_state, joined)) = &mut failure_state {
                            failure_state.join_since(joined, state, model);
                            *joined = state.clone();
                        }
                    }
                    // Operand extraction/constructor preparation may fail on
                    // either side of a Take. Conditional drops cover the union;
                    // the destination is not initialized on any failure edge.
                    if let Some((failure_state, _)) = &failure_state {
                        self.failure_cleanup(
                            model,
                            failure_state,
                            instruction.on_failure,
                            report,
                            true,
                        );
                    }
                    drop(failure_state);
                    // MODE-2 deep copy creates independent counted storage.
                    // Hidden closure captures remain bounded: deepcopy does not
                    // turn a borrowed capture into an owned capture.
                    if matches!(
                        rvalue,
                        Rvalue::Unary {
                            op: hirsel_syntax::UnaryOp::Unique,
                            ..
                        }
                    ) && environment.retained().is_empty()
                        && !matches!(
                            self.t.get(self.place_type(*destination)).unwrap().kind,
                            TypeKind::Function(_) | TypeKind::Parameter(_)
                        )
                    {
                        sources.clear();
                        dependencies.clear();
                        content_loans = loan_content::Content::default();
                    }
                    // MODE-1/OWN-8: owned plain results copy bits, so their
                    // lifetime is independent of the operand's access loan.
                    // Explicit Borrow/Mut views still retain that dependency.
                    if owned && !needs_release(self, self.place_type(*destination), self.t) {
                        dependencies.clear();
                        content_loans = loan_content::Content::default();
                        sources.clear();
                    }
                    let access = &self.body.places[destination.0 as usize];
                    let global_storage = self
                        .lifetime(
                            &self
                                .t
                                .get(self.place_type(*destination))
                                .unwrap()
                                .modes
                                .relation,
                        )
                        .is_global()
                        || !access.projection.is_empty()
                            && self
                                .lifetime(
                                    &self
                                        .t
                                        .get(self.body.locals[access.root.0 as usize].ty)
                                        .unwrap()
                                        .modes
                                        .relation,
                                )
                                .is_global();
                    if global_storage
                        && (owned
                            || (!self.body.places[destination.0 as usize]
                                .projection
                                .is_empty()
                                && state.must
                                    [self.body.places[destination.0 as usize].root.0 as usize]
                                    .has(MustFacts::OWNED)))
                        && (!sources.is_empty()
                            || !dependencies.is_empty()
                            || !environment.retained().is_empty())
                    {
                        self.flow_error(report, ViolationKind::InvalidResultSource(*destination));
                    }
                    // Content that borrows named storage is bounded by it.
                    if bounded && sources.is_empty() && dependencies.is_empty() {
                        sources.insert(self.body.places[destination.0 as usize].root.0 as usize);
                    }
                    // A row's Global promise cannot override composed content
                    // bounds during a whole move or a projected installation.
                    global &= sources.is_empty()
                        && dependencies.is_empty()
                        && environment.retained().is_empty();
                    self.reset_unique(state, *destination);
                    self.set_current_unique(state, *destination, unique);
                    self.set_fresh_receipt(state, *destination, fresh_receipt);
                    if nested_shared
                        && self.body.places[destination.0 as usize]
                            .projection
                            .is_empty()
                    {
                        state.shared.set(
                            self.body.places[destination.0 as usize].root.0 as usize,
                            true,
                        );
                        state.nested_shared.set(
                            self.body.places[destination.0 as usize].root.0 as usize,
                            true,
                        );
                    }
                    self.set_current_global(state, *destination, global);
                    self.set_current_sendable(state, *destination, sendable);
                    self.install_unique_parts(
                        state,
                        *destination,
                        &unique_parts,
                        model.crossing_depth,
                    );
                    self.install_crossing_parts(
                        state,
                        *destination,
                        &crossing_parts,
                        model.crossing_depth,
                    );
                    self.write(
                        model,
                        state,
                        *destination,
                        owned,
                        global && (unique || sendable),
                        report,
                    );
                    self.set_authority(model, state, *destination, authority);
                    self.install_tags(model, state, *destination, tag_parts);
                    self.set_loan_content(model, state, *destination, &content_loans);
                    self.set_sources(model, state, *destination, sources);
                    self.set_borrowed_shares(model, state, *destination, shares, true);
                    let root = self.body.places[destination.0 as usize].root.0 as usize;
                    if model.mutable_parameters[root] {
                        if let Rvalue::Use(operand) = rvalue {
                            let generics = self.generic_declarations();
                            let mut lookup = |path: DefId| {
                                Ok::<_, hirsel_types::QueryError>(
                                    self.signatures.get(path).cloned().map(std::sync::Arc::new),
                                )
                            };
                            if !hirsel_types::ContextMemo::new(self.t, &generics, &mut lookup)
                                .storage(
                                    self.place_type(*destination),
                                    self.place_type(operand.place),
                                )
                                .unwrap_or_else(|error| self.proof_failure(error, false))
                            {
                                self.flow_error(
                                    report,
                                    ViolationKind::TypeMismatch {
                                        entity: "storage replacement",
                                    },
                                );
                            }
                        }
                        self.storage_dependencies(model, state, *destination, report);
                    }

                    state.origins.identities[model.mapping[destination.0 as usize]] =
                        origin_identity.into();
                    state.origins.parts[model.mapping[destination.0 as usize]] =
                        origin_parts.into();
                    self.set_origin_proof(model, state, *destination, origin_proof, origin_expired);
                    state.origins.owners[model.mapping[destination.0 as usize]] = origin_owners;
                    self.set_environment(model, state, *destination, &environment);
                    if !state.cursor_views.is_empty() {
                        state.cursor_views.set(root, cursor_view);
                    }
                }
                InstructionKind::Snapshot { destination, value } => {
                    let authority = self.operand_authority(model, state, value.place);
                    let tag_parts = self.tag_parts(state, value);
                    let source = model.mapping[value.place.0 as usize];
                    let identity = self.origin_identity(model, state, value.place);
                    let parts = Self::storage_partition(
                        model,
                        state,
                        &self.body.places[value.place.0 as usize],
                    );
                    let environment = self.environment(model, state, value.place);
                    let proof = state.origins.proofs[source].clone();
                    let expired = state.origins.expired[source].clone();
                    let transfer_store = self.t;
                    let transferred = self.transferred_type(state, value, transfer_store);
                    let sendable = transfer_store.get(transferred).unwrap().modes.sendability
                        == Sendability::Sendable;
                    let crossing = self.operand_crossing_parts(state, value);
                    let shares = self.operand_shares(state, value);
                    self.mode_transfer(state, self.place_type(*destination), value, report);
                    self.read(model, state, value, report);
                    self.reset_unique(state, *destination);
                    self.set_borrowed_shares(model, state, *destination, shares, true);
                    self.set_current_sendable(state, *destination, sendable);
                    self.install_crossing_parts(
                        state,
                        *destination,
                        &crossing,
                        model.crossing_depth,
                    );
                    self.write(model, state, *destination, true, false, report);
                    self.set_authority(model, state, *destination, authority);
                    self.install_tags(model, state, *destination, tag_parts);
                    self.set_dependencies(model, state, *destination, BTreeSet::new());
                    self.set_origin_proof(model, state, *destination, proof, expired);
                    state.origins.identities[model.mapping[destination.0 as usize]] =
                        identity.into();
                    state.origins.parts[model.mapping[destination.0 as usize]] = parts.into();
                    state.origins.owners[model.mapping[destination.0 as usize]].clear();
                    self.set_environment(model, state, *destination, &environment);
                }
                InstructionKind::BeginLoan {
                    id,
                    place,
                    mutable,
                    if_inactive,
                } => {
                    if *mutable
                        && (self.readonly_map_entry(*place) || self.readonly_byte_index(*place))
                    {
                        self.flow_error(report, ViolationKind::InvalidUse(*place));
                    }
                    let permissions = self.access_loans(model, state, *place, *mutable, report);
                    let root = self.body.places[place.0 as usize].root.0 as usize;
                    if *mutable && model.borrowed[root] && !model.mutable_parameters[root] {
                        self.flow_error(report, ViolationKind::InvalidUse(*place));
                    }
                    self.available(model, state, *place, false, report);
                    self.indices(model, state, *place, report);
                    let index = model.loan(*id);
                    if *if_inactive && (*mutable || state.loans[index] != (LIVE | DEAD)) {
                        self.flow_error(
                            report,
                            ViolationKind::WrongKind {
                                entity: "conditional loan begin",
                                index: id.0,
                            },
                        );
                    }
                    if !*if_inactive && state.loans[index] & LIVE != 0 {
                        self.flow_error(
                            report,
                            ViolationKind::LoanConflict {
                                loan: *id,
                                place: *place,
                            },
                        );
                    }
                    for (i, (loan, source, other_mutable)) in model.loans.iter().enumerate() {
                        if state.loans[i] & LIVE != 0
                            && (*mutable || *other_mutable)
                            && !permissions.contains(&i)
                            && model.overlaps(
                                model.mapping[source.0 as usize],
                                model.mapping[place.0 as usize],
                            )
                        {
                            self.flow_error(
                                report,
                                ViolationKind::LoanConflict {
                                    loan: *loan,
                                    place: *place,
                                },
                            );
                        }
                    }
                    state.loans.set(index, LIVE);
                    model.refresh_cleanup_loan(state, index);
                }
                InstructionKind::EndLoan {
                    loan: id,
                    if_active,
                } => self.end_loan(model, state, *id, *if_active, report),
                InstructionKind::Drop { place, if_live } => {
                    if self.readonly_map_entry(*place) || self.readonly_byte_index(*place) {
                        self.flow_error(report, ViolationKind::InvalidUse(*place));
                    }
                    let live = state.places[model.mapping[place.0 as usize]] & LIVE != 0;
                    let may_fail = live && needs_release(self, self.place_type(*place), self.t);
                    // MODE-2: a release that runs user code may fail while a
                    // hand-back is weakened; a copyable one runs none (DROP-1).
                    let silent = !(live && self.destroyed(self.place_type(*place)));
                    self.drop_place(model, state, *place, *if_live, report, true);
                    model.releasing.set(silent);
                    self.failure_cleanup(model, state, instruction.on_failure, report, may_fail);
                    model.releasing.set(false);
                }
                InstructionKind::RunCleanup(id) => {
                    // An empty/dead cleanup can never enter its emergency edge.
                    // Still validate the ownership snapshot independently.
                    let may_fail = self.body.cleanup_actions(*id).any(|action| {
                        matches!(action,
                        CleanupAction::Drop { place, .. }
                            if state.places[model.mapping[place.0 as usize]] & LIVE != 0
                                && needs_release(self, self.place_type(*place), self.t))
                    });
                    let silent = !self.body.cleanup_actions(*id).any(|action| {
                        matches!(action,
                        CleanupAction::Drop { place, .. }
                            if state.places[model.mapping[place.0 as usize]] & LIVE != 0
                                && self.destroyed(self.place_type(*place)))
                    });
                    self.cleanup(model, state, *id, false, report, true);
                    model.releasing.set(silent);
                    self.failure_cleanup(model, state, instruction.on_failure, report, may_fail);
                    model.releasing.set(false);
                }
                InstructionKind::Check { condition, .. } => {
                    self.read(model, state, condition, report);
                    self.failure_cleanup(model, state, instruction.on_failure, report, true);
                }
                // An exhausted charge stops the evaluation without cleanup (CT-4).
                InstructionKind::Charge(_) => {}
            }
        }
        self.site.instruction = None;
        self.acquisitions(state, report);
        self.restore_borrowed(model, state);
        match &block.terminator {
            Terminator::Branch { condition, .. } => self.read(model, state, condition, report),
            Terminator::Switch { value, .. } => self.read(model, state, value, report),
            Terminator::Match { table, .. } => {
                let place = self.body.matches[table.0 as usize].scrutinee;
                self.read(
                    model,
                    state,
                    &Operand {
                        place,
                        usage: UseKind::Borrow,
                    },
                    report,
                );
            }
            Terminator::Call {
                callee,
                arguments,
                returns,
                on_failure,
            } => {
                let protocol = self.consuming_protocol(callee);
                let seal_root = self.seal_flow(model, state, callee, arguments, report);
                let protocol_unique = protocol
                    && arguments.first().is_some_and(|a| {
                        matches!(a.value.usage, UseKind::Take | UseKind::Mut)
                            && self.operand_unique(state, &a.value)
                    });
                let protocol_fresh = protocol
                    && arguments.first().is_some_and(|a| {
                        let access = &self.body.places[a.value.place.0 as usize];
                        let root = access.root.0 as usize;
                        matches!(a.value.usage, UseKind::Take | UseKind::Mut)
                            && access.projection.is_empty()
                            && !state.shared[root]
                            && state.must[root].has(MustFacts::FRESH_RECEIPT)
                    });
                // A diverging call has no result: only its argument effects
                // and its failure edge are modeled.
                let destination = returns.map(|r| r.destination);
                if let Some(destination) = destination {
                    self.call_creation(model, state, callee, arguments, destination);
                }
                for argument in arguments {
                    if argument.value.usage == UseKind::Mut {
                        self.state_argument(model, state, callee, argument, arguments, report);
                    }
                }
                let mut invocation_values: Vec<_> = arguments
                    .iter()
                    .filter(|argument| {
                        let (types, function) = match callee {
                            Callee::Definition { path, .. } => {
                                let Some(signature) = self.signatures.get(path) else {
                                    return false;
                                };
                                let hirsel_types::SignatureKind::Function(function) =
                                    &signature.kind
                                else {
                                    return false;
                                };
                                (self.t, function)
                            }
                            Callee::Indirect { value, .. } => {
                                let types = self.t;
                                let hirsel_types::TypeKind::Function(function) =
                                    &types.get(self.place_type(value.place)).unwrap().kind
                                else {
                                    return false;
                                };
                                (types, function)
                            }
                        };
                        let Some(parameter) =
                            function.parameters.get(argument.parameter.0 as usize)
                        else {
                            return false;
                        };
                        if parameter.passing != hirsel_syntax::PassingMode::Borrow {
                            return true;
                        }
                        let mut pending = vec![parameter.ty];
                        let mut seen = HashSet::default();
                        while let Some(ty) = pending.pop() {
                            if !seen.insert(ty) {
                                continue;
                            }
                            let row = types.get(ty).unwrap();
                            if matches!(
                                row.kind,
                                TypeKind::Function(_)
                                    | TypeKind::Named { .. }
                                    | TypeKind::Builtin {
                                        kind: BuiltinType::Iter,
                                        ..
                                    }
                            ) {
                                return true;
                            }
                            pending.extend(row.kind.value_parts());
                        }
                        false
                    })
                    .map(|argument| &argument.value)
                    .collect();
                if let Callee::Indirect { value, .. } = callee {
                    invocation_values.push(value);
                }
                for value in invocation_values {
                    for (place, required) in self.environment(model, state, value.place).crossing()
                    {
                        let access = &self.body.places[place as usize];
                        let root = access.root.0 as usize;
                        let mut path = crossing::path(&access.projection);
                        path.extend(required.iter().map(|part| match part {
                            hirsel_types::OwnedTypePart::Field(field) => {
                                crossing::Part::Field(*field)
                            }
                            hirsel_types::OwnedTypePart::VariantField { variant, field } => {
                                crossing::Part::Variant(*variant, *field)
                            }
                            hirsel_types::OwnedTypePart::Elements => crossing::Part::Elements,
                        }));
                        let valid = if path.is_empty() {
                            state.must[root].has(MustFacts::SENDABLE)
                        } else {
                            state.crossing_parts[root].get(&path) != Some(false)
                        };
                        if !valid {
                            self.flow_error(
                                report,
                                ViolationKind::TypeMismatch {
                                    entity: "borrowed capture crossing receipt",
                                },
                            );
                        }
                    }
                }
                self.call_crossing_receipts(state, callee, arguments, report);
                let authority = self.call_authority(model, state, callee, arguments, report);
                let origin_proof = self.call_origin_proof(model, state, arguments, report);
                if report {
                    self.call_type(callee, arguments, destination, Some(&origin_proof));
                }
                for (i, a) in arguments.iter().enumerate() {
                    for b in &arguments[i + 1..] {
                        if (a.value.usage == UseKind::Mut || b.value.usage == UseKind::Mut)
                            && model.overlaps(
                                model.mapping[a.value.place.0 as usize],
                                model.mapping[b.value.place.0 as usize],
                            )
                        {
                            self.flow_error(
                                report,
                                ViolationKind::AliasedArguments {
                                    first: a.parameter.0,
                                    second: b.parameter.0,
                                },
                            );
                        }
                    }
                }
                for argument in arguments {
                    if argument.value.usage == UseKind::Mut {
                        state.unique_parts
                            [self.body.places[argument.value.place.0 as usize].root.0 as usize] =
                            crossing::Parts::default();
                        self.invalidate_tags(state, argument.value.place);
                    }
                }
                let callbacks: Vec<_> = arguments
                    .iter()
                    .map(|a| &a.value)
                    .chain(if let Callee::Indirect { value, .. } = callee {
                        Some(value)
                    } else {
                        None
                    })
                    .collect();
                for value in callbacks {
                    if matches!(
                        self.t.get(self.place_type(value.place)).unwrap().kind,
                        TypeKind::Function(_)
                    ) || !self
                        .environment(model, state, value.place)
                        .retained()
                        .is_empty()
                    {
                        for loan in self.dependencies(model, state, value) {
                            if model.loans[loan].2 {
                                self.invalidate_tags(state, model.loans[loan].1);
                            }
                        }
                    }
                }
                // MODE-2: the result and each `mut` argument may receive the
                // borrow-only shares of every argument. A global result of a
                // named function holds nothing a borrowed or `mut` argument
                // reaches, which stays the caller's (MODE-1): only taken and
                // copied arguments can pass it a share.
                let argument_shares: BTreeSet<usize> = arguments
                    .iter()
                    .flat_map(|a| self.operand_shares(state, &a.value).held())
                    .collect();
                let global_result = matches!(callee, Callee::Definition { .. })
                    && self
                        .result_relation(callee)
                        .is_some_and(|relation| relation.is_global());
                let mut borrowed: BTreeSet<usize> = if global_result {
                    arguments
                        .iter()
                        .filter(|a| matches!(a.value.usage, UseKind::Take | UseKind::Copy))
                        .flat_map(|a| self.operand_shares(state, &a.value).held())
                        .collect()
                } else {
                    argument_shares.clone()
                };
                let result = if let Some(destination) = destination {
                    let (result_arguments, result_environment) =
                        self.result_arguments(callee, arguments);
                    let mut returned_environment = super::flow_origins::Environment::default();
                    if !self.bound(
                        self.t,
                        self.place_type(destination),
                        hirsel_types::Bound::Plain,
                        &self.generic_declarations(),
                    ) {
                        for value in result_arguments.iter().copied().chain(result_environment) {
                            let environment = self.environment(model, state, value.place);
                            for (place, path) in environment.crossing() {
                                returned_environment.require_crossing(PlaceId(place), path);
                            }
                            if matches!(
                                self.t.get(self.place_type(destination)).unwrap().kind,
                                TypeKind::Function(_)
                            ) {
                                for place in environment.sharing() {
                                    returned_environment.share_capture(PlaceId(place));
                                }
                            }
                        }
                    }
                    // MODE-1: the result owns shares of Global selected arguments.
                    // In a mixed result, only Local or placed sources bound it;
                    // the preparation loan on a Global temporary ends at the call.
                    // A placed temporary keeps only its arena bound: its read loan
                    // ends at return (P2).
                    let local_arguments: Vec<_> = result_arguments
                        .iter()
                        .copied()
                        .map(|o| (o, false))
                        .chain(result_environment.map(|o| (o, true)))
                        .filter(|(o, _)| {
                            !(self.content_proof(Some(state), o.place).deep
                                && self.current_content_global(state, o))
                                || (self.placed(
                                    &self.t.get(self.place_type(o.place)).unwrap().modes.relation,
                                ) && !model.temporary
                                    [self.body.places[o.place.0 as usize].root.0 as usize])
                        })
                        .collect();
                    let sources = local_arguments
                        .iter()
                        .flat_map(|(o, _)| self.sources(model, state, o))
                        .collect();
                    let result_row = self.t.get(self.place_type(destination)).unwrap();
                    let shared_result = matches!(
                        result_row.kind,
                        TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::Shared,
                            ..
                        }
                    );
                    if !shared_result
                        && !protocol
                        && needs_release(self, self.place_type(destination), self.t)
                        && !self.flat_value(destination)
                    {
                        // MODE-2 (#80): a counted result of calling a closure may
                        // share what its environment borrows, whatever its result
                        // relation, as the share in its body would.
                        let environment = result_environment.or(match callee {
                            Callee::Indirect { value, .. } => Some(value),
                            Callee::Definition { .. } => None,
                        });
                        for operand in result_arguments.iter().copied().chain(environment) {
                            if matches!(callee, Callee::Definition { .. }) {
                                borrowed.extend(self.mark_borrowed_share(model, state, operand));
                            } else {
                                self.mark_shared(model, state, operand);
                            }
                            if matches!(callee, Callee::Indirect { value, .. } if value.place == operand.place)
                            {
                                for place in self.environment(model, state, operand.place).sharing()
                                {
                                    self.mark_shared(
                                        model,
                                        state,
                                        &Operand {
                                            place: PlaceId(place),
                                            usage: UseKind::Borrow,
                                        },
                                    );
                                }
                            }
                        }
                    }
                    let composed_result = self.bound(
                        self.t,
                        self.place_type(destination),
                        hirsel_types::Bound::Data,
                        &self.generic_declarations(),
                    );
                    let result_dependencies = local_arguments
                        .iter()
                        .flat_map(|(o, environment)| {
                            if *environment {
                                // Invocation borrows the callable slot. The returned
                                // content depends on its environment, independently
                                // of that short-lived access permission.
                                self.installed_loans(state, o.place).held()
                            } else if let Some(field) =
                                self.field_dependencies(model, state, callee, arguments, o)
                            {
                                field
                            } else if composed_result
                                && let Some(loans) = self.related_local(model, state, o)
                            {
                                // MODE-1: relations compose. A data result
                                // related to a local view reaches what the
                                // view reaches, never the local's own
                                // storage: it holds the local's loans.
                                loans
                            } else {
                                self.dependencies(model, state, o)
                            }
                        })
                        .collect();
                    Some((returned_environment, sources, result_dependencies))
                } else {
                    None
                };
                if let Callee::Indirect { value: o, .. } = callee {
                    self.read(model, state, o, report);
                }
                // MODE-2: a `@preserves_unique` callee installs content from its
                // taken and `mut` arguments only, and a bodyless core callee
                // also from a borrowed one the ledger does not mark unretained,
                // which must then be plain or a handle.
                let ledger = matches!(callee, Callee::Definition { path, .. }
                    if path.package_is("core", "1"));
                // Its proof covers other `mut` arguments aliased; a bodyless
                // core callee has none, so those must be unique too.
                let unique_inputs = arguments.iter().all(|a| {
                    let borrowed = a.value.usage == UseKind::Borrow;
                    let retained = ledger && !self.callee_unretained(callee, a.parameter);
                    borrowed && !retained
                        || a.value.usage == UseKind::Mut && !ledger
                        || !needs_release(self, self.place_type(a.value.place), self.t)
                        || super::handle_root(self.t, self.place_type(a.value.place))
                        || !borrowed && self.operand_unique(state, &a.value)
                });
                let parametric = self.parametric_unique(state, callee, arguments);
                let awaited_unique = self.awaited_unique(callee, arguments);
                let unique_arguments: Vec<bool> = arguments
                    .iter()
                    .map(|a| self.operand_unique(state, &a.value))
                    .collect();
                for (a, unique) in arguments.iter().zip(unique_arguments) {
                    // A `mut` argument handed back unique owns every block it
                    // reaches, so it holds no share of any argument (MODE-2).
                    // A plain hand-back cannot install a counted share, even
                    // when the argument is a projection of a counted owner.
                    if a.value.usage == UseKind::Mut
                        && !self.bound(
                            self.t,
                            self.place_type(a.value.place),
                            hirsel_types::Bound::Plain,
                            &self.generic_declarations(),
                        )
                        && !protocol
                        && !self.callee_preserves_state(callee, a.parameter, false)
                        && !self.mut_requires_unique(callee, a.parameter)
                        && !(unique_inputs
                            && unique
                            && self.callee_preserves_unique(callee, a.parameter))
                    {
                        // A checked callee may weaken this root, but its fixed
                        // nested contracts must survive the mutable hand-back.
                        let root = self.body.places[a.value.place.0 as usize].root.0 as usize;
                        state.shared.set(root, true);
                        state.borrow_only.set(root, false);
                        if !self.body.places[a.value.place.0 as usize]
                            .projection
                            .is_empty()
                            && self.t.row(self.place_type(a.value.place)).modes.uniqueness
                                == hirsel_types::Uniqueness::Unique
                        {
                            state.nested_shared.set(root, true);
                        }
                        self.set_borrowed_shares(
                            model,
                            state,
                            a.value.place,
                            loan_content::Content::whole(argument_shares.clone()),
                            false,
                        );
                    }
                    let transport = seal_root == Some(a.value.place)
                        || model
                            .parts(a.value.place)
                            .any(|part| !state.origins.expired[part].is_empty())
                            && self.taken_brandless_data(callee, a);
                    self.read_operand(model, state, &a.value, false, false, transport, report);
                }
                let mut mutated = Vec::new();
                for argument in arguments {
                    let place = argument.value.place;
                    let row = self.t.get(self.place_type(place)).unwrap();
                    if argument.value.usage == UseKind::Mut
                        && !self.callee_preserves_state(callee, argument.parameter, false)
                    {
                        let receipts = self.crossing_receipts(self.t, self.place_type(place));
                        let access = &self.body.places[place.0 as usize];
                        let prefix = crossing::path(&access.projection);
                        let mut parts = crossing::Parts::default();
                        for (path, _) in receipts {
                            parts.set(&path, true);
                        }
                        if prefix.is_empty() {
                            state.crossing_parts[access.root.0 as usize] = parts;
                        } else {
                            state.crossing_parts[access.root.0 as usize].install(&prefix, &parts);
                        }
                        state.crossing_parts[access.root.0 as usize].restrict(model.crossing_depth);
                        // The callee may replace current content according to
                        // this fixed receiving contract, not its old producer.
                        self.set_current_sendable(
                            state,
                            place,
                            hirsel_types::crossing_requirement(row)
                                == hirsel_types::Sendability::Sendable
                                || self.bound(
                                    self.t,
                                    self.place_type(place),
                                    hirsel_types::Bound::Plain,
                                    &self.generic_declarations(),
                                ),
                        );
                    }
                    let keeps_identity = matches!(
                        row.kind,
                        TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::Slots,
                            ..
                        }
                    );
                    if argument.value.usage == UseKind::Mut
                        && !keeps_identity
                        && !self.callee_preserves_state(callee, argument.parameter, true)
                    {
                        // Mutation expires retained values, but an owning storage
                        // identity remains the current owner for newly made brands.
                        // A copied alias has no transferred owner and must detach.
                        let root = model.roots[self.body.places[place.0 as usize].root.0 as usize];
                        let owned_identities: BTreeSet<_> = self
                            .origin_identity(model, state, place)
                            .into_iter()
                            .filter(|identity| {
                                state.origins.owners[root]
                                    .iter()
                                    .any(|owner| super::flow_origins::contains(owner, identity))
                            })
                            .collect();
                        self.end_origin(model, state, place);
                        mutated.push(place);
                        if let Some(origin) = &self.body.places[place.0 as usize].binding {
                            if !origin.fields.is_empty()
                                && self.refresh_copied_prefix(
                                    model,
                                    state,
                                    &self.body.places[place.0 as usize],
                                    origin,
                                )
                            {
                                continue;
                            }
                            Self::forget_written_partition(
                                model,
                                state,
                                &self.body.places[place.0 as usize],
                            );
                            state.origins.introduce(origin);
                            for identity in &owned_identities {
                                state.origins.introduce(identity);
                            }
                            if origin.fields.is_empty() {
                                let identities = if owned_identities.is_empty() {
                                    BTreeSet::from([origin.clone()])
                                } else {
                                    owned_identities
                                };
                                let index = model.mapping[place.0 as usize];
                                state.origins.identities[index] = identities.clone().into();
                                state.origins.ended.remove(&origin.root);
                                state
                                    .origins
                                    .bindings
                                    .insert(origin.root.clone(), identities);
                            }
                        }
                    } else if argument.value.usage == UseKind::Take {
                        self.end_origin(model, state, place);
                    }
                }
                for place in mutated {
                    self.mutable_origin_handback(model, state, place);
                }
                if report && !state.audit {
                    if let Some(tail) = self
                        .body
                        .tail_sites
                        .iter()
                        .find(|t| t.call_block == self.site.block.unwrap())
                    {
                        let loop_roots: BTreeSet<_> = tail
                            .owned_loop_arguments
                            .iter()
                            .map(|(_, p)| self.body.places[p.0 as usize].root.0 as usize)
                            .collect();
                        // A constructor tail's earlier fields stay pending: they
                        // are parts of the partial result (LANG-10).
                        let fields: BTreeSet<usize> = match tail.kind {
                            TailKind::ConstructorHole(_) => {
                                crate::tail::chain_fields(self.body, tail.call_block)
                                    .into_iter()
                                    .map(|root| root as usize)
                                    .collect()
                            }
                            TailKind::SelfTail => BTreeSet::new(),
                        };
                        let pending_owner = self.body.locals.iter().enumerate().any(|(root, _)| {
                            state.places[model.roots[root]] & LIVE != 0
                                && (state.must[root].has(MustFacts::OWNED)
                                    || state.ownership_conflicts[root])
                                && !loop_roots.contains(&root)
                                && !fields.contains(&root)
                                && model.release[root]
                        });
                        let frame_borrow = arguments.iter().any(|a| {
                            matches!(a.value.usage, UseKind::Borrow | UseKind::Mut)
                                && self.sources(model, state, &a.value).iter().any(|source| {
                                    let root = model.places[*source].root.0 as usize;
                                    !model.borrowed[root] && !loop_roots.contains(&root)
                                })
                        });
                        if pending_owner || frame_borrow {
                            self.flow_error(
                                true,
                                ViolationKind::WrongKind {
                                    entity: "tail witness",
                                    index: tail.call_block.0,
                                },
                            );
                        }
                    }
                }
                // Transfers have happened: caller's failure list must exclude
                // Take temporaries, and its result is not yet initialized.
                self.failure_cleanup(model, state, *on_failure, report, true);
                let (Some(destination), Some((returned_environment, sources, result_dependencies))) =
                    (destination, result)
                else {
                    return;
                };
                self.reset_unique(state, destination);
                self.set_fresh_receipt(
                    state,
                    destination,
                    !protocol || protocol_unique || protocol_fresh,
                );
                // MODE-2: a `@preserves_unique` taken input passes its
                // uniqueness on to the result.
                let promised = unique_inputs
                    && arguments.iter().any(|a| {
                        a.value.usage == UseKind::Take
                            && self.callee_preserves_unique(callee, a.parameter)
                    });
                let inferred = match callee {
                    Callee::Definition { path, .. }
                        if hirsel_types::unique_summary::same_module(
                            path,
                            self.definition.path.path(),
                        ) =>
                    {
                        self.signatures
                            .get(path)
                            .map(|s| s.metadata.inferred_unique.clone())
                            .unwrap_or_default()
                    }
                    _ => hirsel_types::UniqueSummary::default(),
                };
                let mut inferred_parts = crossing::Parts::default();
                let declared = match callee {
                    Callee::Definition { path, arguments } => {
                        self.signatures.get(path).and_then(|s| {
                            let SignatureKind::Function(function) = &s.kind else {
                                return None;
                            };
                            self.imported_in(
                                function.result,
                                arguments,
                                &self.generic_declarations(),
                            )
                            .map(|result| (result, s.metadata.roles.clone()))
                        })
                    }
                    Callee::Indirect {
                        value, arguments, ..
                    } => match &self.t.row(self.place_type(value.place)).kind {
                        TypeKind::Function(function) => self
                            .imported_in(function.result, arguments, &self.generic_declarations())
                            .map(|result| {
                                (
                                    result,
                                    self.signatures
                                        .get(self.definition.path)
                                        .map(|s| s.metadata.roles.clone())
                                        .unwrap_or_default(),
                                )
                            }),
                        _ => None,
                    },
                };
                if let Some((root, roles)) = declared
                    && matches!(
                        self.t.row(root).kind,
                        TypeKind::Named { .. } | TypeKind::Tuple(_)
                    )
                {
                    for (row, ty) in
                        hirsel_types::unique_summary::result_rows(self.t, root, &roles, &mut |d| {
                            self.signatures.get(d).cloned().map(std::sync::Arc::new)
                        })
                    {
                        if self.t.row(ty).modes.uniqueness == hirsel_types::Uniqueness::Unique {
                            inferred_parts.set(&unique_row_path(row), true);
                        }
                    }
                }
                for row in &inferred.results {
                    inferred_parts.set(&unique_row_path(*row), true);
                }
                self.install_unique_parts(
                    state,
                    destination,
                    &inferred_parts,
                    model.crossing_depth,
                );
                self.set_current_unique(
                    state,
                    destination,
                    inferred.results.contains(&hirsel_types::UniqueResultRow::Root) || promised
                        || parametric
                        || awaited_unique
                        // An owning core cursor transfers parts of its
                        // currently unique owner; the result owns no aliases.
                        // Freshness alone does not prove this for a shared
                        // owner or an unqualified counted payload.
                        || protocol_unique
                        || self
                            .t
                            .get(self.place_type(destination))
                            .unwrap()
                            .modes
                            .uniqueness
                            == hirsel_types::Uniqueness::Unique,
                );
                self.set_current_global(
                    state,
                    destination,
                    self.lifetime(
                        &self
                            .t
                            .get(self.place_type(destination))
                            .unwrap()
                            .modes
                            .relation,
                    )
                    .is_global(),
                );
                self.set_current_sendable(
                    state,
                    destination,
                    self.t
                        .get(self.place_type(destination))
                        .unwrap()
                        .modes
                        .sendability
                        == hirsel_types::Sendability::Sendable,
                );
                self.write(model, state, destination, true, false, report);
                self.set_authority(model, state, destination, authority);
                self.install_tags(model, state, destination, tags::Parts::new());
                let locality = self.lifetime(
                    &self
                        .t
                        .get(self.place_type(destination))
                        .unwrap()
                        .modes
                        .relation,
                );
                let dependencies = if locality.is_global() {
                    BTreeSet::new()
                } else {
                    result_dependencies
                };
                self.set_dependencies(model, state, destination, dependencies);
                // A local(x) result over globally owned storage is Global at
                // this call site; the callee transfers an owned value (MODE-1).
                let sources = if locality.is_global() {
                    BTreeSet::new()
                } else {
                    sources
                };
                self.set_sources(model, state, destination, sources);
                self.set_borrowed_shares(
                    model,
                    state,
                    destination,
                    loan_content::Content::whole(borrowed),
                    true,
                );
                let result_origins = super::flow_origins::remember(
                    self.t,
                    &self.scratch.origin_types[self.site.body.0 as usize],
                    self.place_type(destination),
                    &state.origins.bindings,
                    &|o| self.arena_place(o),
                );
                self.set_origin_proof(model, state, destination, result_origins, BTreeSet::new());
                self.set_environment(model, state, destination, &returned_environment);
            }
            Terminator::Return { value, cleanup } => {
                self.return_creation(model, state, value, report);
                if self.site.body == self.definition.entry {
                    let rows = hirsel_types::unique_summary::result_rows(
                        self.t,
                        self.body.result,
                        &self
                            .signatures
                            .get(self.definition.path)
                            .map(|s| s.metadata.roles.clone())
                            .unwrap_or_default(),
                        &mut |d| self.signatures.get(d).cloned().map(std::sync::Arc::new),
                    );
                    let parts = self.operand_unique_parts(state, value);
                    for claimed in &self.definition.inferred_unique.results {
                        let valid =
                            rows.iter()
                                .find(|(row, _)| row == claimed)
                                .is_some_and(|(_, ty)| {
                                    !needs_release(self, *ty, self.t)
                                        || super::handle_root(self.t, *ty)
                                        || self.operand_unique(state, value)
                                        || parts.get(&unique_row_path(*claimed)) == Some(true)
                                });
                        if !valid {
                            self.flow_error(
                                report,
                                ViolationKind::TypeMismatch {
                                    entity: "private unique result proof",
                                },
                            );
                        }
                    }
                }
                if model
                    .unique_proof
                    .is_some_and(|root| !model.mutable_parameters[root])
                    && needs_release(self, self.place_type(value.place), self.t)
                    && !super::handle_root(self.t, self.place_type(value.place))
                    && !self.operand_unique(state, value)
                {
                    self.flow_error(
                        report,
                        ViolationKind::TypeMismatch {
                            entity: UNIQUE_PROOF_FAILED,
                        },
                    );
                }
                if !state.audit {
                    let types = self.t;
                    let actual = self.transferred_type(state, value, types);
                    self.returned.insert(
                        self.site.block.unwrap().0,
                        types.get(actual).unwrap().clone(),
                    );
                }
                self.nested_crossing_receipt(state, self.t, self.body.result, value, report);
                let result = self.t.get(self.body.result).unwrap();
                let result_lifetime = self.lifetime(&result.modes.relation);
                let named: Option<BTreeSet<_>> = match result_lifetime.named_parameters() {
                    // An environment-local body row is a closure's or function
                    // type's `in env` result (#72): inputs and environment
                    // storage endure across this return; this body's own frame
                    // never does.
                    _ if result_lifetime.has_environment() => Some(
                        self.body
                            .locals
                            .iter()
                            .enumerate()
                            .filter(|(_, local)| {
                                matches!(local.storage, Storage::Parameter | Storage::Capture)
                            })
                            .map(|(root, _)| root)
                            .collect(),
                    ),
                    Some(parameters) => Some(
                        parameters
                            .iter()
                            .filter_map(|p| match p {
                                hirsel_types::ParameterRef::Body { scope, parameter }
                                    if *scope == self.body.parameter_scope =>
                                {
                                    self.body
                                        .parameters
                                        .get(parameter.0 as usize)
                                        .map(|l| l.0 as usize)
                                }
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => None,
                };
                if let Some(allowed) = &named {
                    let actual = self.sources(model, state, value);
                    if !actual.is_subset(allowed) {
                        self.flow_error(report, ViolationKind::InvalidResultSource(value.place));
                    }
                }
                if result_lifetime.is_global()
                    && (!self.sources(model, state, value).is_empty()
                        || !self.dependencies(model, state, value).is_empty())
                {
                    self.flow_error(report, ViolationKind::InvalidResultSource(value.place));
                }
                if needs_release(self, self.place_type(value.place), self.t)
                    && matches!(value.usage, UseKind::Borrow | UseKind::Mut)
                {
                    self.flow_error(report, ViolationKind::BorrowedMove(value.place));
                }
                let returned_origin = model.mapping[value.place.0 as usize];
                if super::flow_origins::canonical(
                    &mut self.t,
                    self.place_type(value.place),
                    &state.origins.proofs[returned_origin],
                    &|o| self.arena_place(o),
                )
                .is_err()
                {
                    self.flow_error(report, ViolationKind::InvalidOrigin(value.place));
                }

                // Core library borrowed `for` protocols compose an owned yield back to its
                // original source, without bounds on unnamed preparation/cursor
                // storage. Data excludes borrowed callable/resource environments.
                let value_root = self.body.places[value.place.0 as usize].root.0 as usize;
                let owned_share = named.is_some()
                    && value.usage == UseKind::Take
                    && state.must[value_root].has(MustFacts::OWNED)
                    && !state.ownership_conflicts[value_root]
                    && self.bound(
                        self.t,
                        self.place_type(value.place),
                        hirsel_types::Bound::Data,
                        &self.generic_declarations(),
                    );

                // Read loans on this body's own places end when it returns
                // (MODE-1): a loan on an own place holding only arena content
                // bounds nothing past the return.
                let returned_dependencies: Vec<_> = self
                    .dependencies(model, state, value)
                    .into_iter()
                    .filter(|dependency| {
                        let source = model.loans[*dependency].1;
                        let root = self.body.places[source.0 as usize].root.0 as usize;
                        if owned_share
                            && model.protocol_holders.contains(&root)
                            && state.must[root].has(MustFacts::OWNED)
                            && !state.ownership_conflicts[root]
                            && named.as_ref().is_some_and(|allowed| {
                                self.sources(
                                    model,
                                    state,
                                    &Operand {
                                        place: source,
                                        usage: UseKind::Take,
                                    },
                                )
                                .is_subset(allowed)
                            })
                        {
                            return false;
                        }
                        !(matches!(
                            self.body.locals[root].storage,
                            Storage::Local | Storage::Temporary
                        ) && self
                            .placed(&self.t.get(self.place_type(source)).unwrap().modes.relation)
                            && !self.taken_storage(model, source)
                            && self.current_content_global(
                                state,
                                &Operand {
                                    place: source,
                                    usage: UseKind::Borrow,
                                },
                            ))
                    })
                    .collect();
                let returning_shares = self.operand_shares(state, value).held();
                self.read(model, state, value, report);
                // MODE-2: a result keeps the borrow-only shares it derives from.
                *model.returning_shares.borrow_mut() = returning_shares;
                self.cleanup(model, state, *cleanup, true, report, true);
                model.returning_shares.borrow_mut().clear();
                if model
                    .contents(returned_origin)
                    .any(|i| !state.origins.expired[i].is_empty())
                    || !self
                        .environment(model, state, value.place)
                        .expired()
                        .is_empty()
                {
                    self.flow_error(report, ViolationKind::InvalidOrigin(value.place));
                }
                for dependency in returned_dependencies {
                    let (loan, source, _) = model.loans[dependency];
                    // A result declared local to a parameter is as local as
                    // that argument was. A taken parameter's storage ends
                    // with this frame; the value it shared does not (MODE-1).
                    let root = self.body.places[source.0 as usize].root.0 as usize;
                    if named.as_ref().is_some_and(|named| named.contains(&root)) {
                        continue;
                    }
                    if state.places[model.mapping[source.0 as usize]] != LIVE {
                        self.flow_error(
                            report,
                            ViolationKind::LoanOutlivesSource {
                                loan,
                                place: source,
                            },
                        );
                    }
                }
                self.leaks(model, state, report);
            }
            Terminator::Fail { cleanup, .. } => {
                self.cleanup(model, state, *cleanup, true, report, true);
                self.leaks(model, state, report);
            }
            Terminator::Unreachable => {
                // Exhaustive switches omit their impossible default edge from
                // the analysis. Every remaining reachable sentinel is invalid.
                if !state.audit {
                    self.flow_error(report, ViolationKind::ReachableUnreachable);
                    self.leaks(model, state, report);
                }
            }
            Terminator::Goto(_) => (),
        }
    }
    /// A place holding a taken parameter's storage: the parameter itself, or
    /// a temporary copied or moved from one.
    fn taken_storage(&self, model: &Model, place: PlaceId) -> bool {
        // Unnamed storage (a temporary, or a slot no source binding names,
        // such as a kept temporary) is followed to what it was moved, copied
        // or made unique from, and a closure call's result to the closure's captures,
        // which it may share (#80). Named storage holds its own value.
        let mut pending = vec![self.body.places[place.0 as usize].root];
        let mut seen = BTreeSet::new();
        while let Some(root) = pending.pop() {
            if !seen.insert(root.0) {
                continue;
            }
            match self.body.locals[root.0 as usize].storage {
                Storage::Parameter => {
                    // A taken parameter whose content is global and unplaced
                    // is its own share, as the source checker judges it.
                    let relation = &self
                        .t
                        .get(self.body.locals[root.0 as usize].ty)
                        .unwrap()
                        .modes
                        .relation;
                    if !model.borrowed[root.0 as usize]
                        && (!self.lifetime(relation).is_global() || self.placed(relation))
                    {
                        return true;
                    }
                    continue;
                }
                Storage::Temporary => {}
                Storage::Local
                    if !self.body.places.iter().any(|p| {
                        p.root == root
                            && p.binding.as_ref().is_some_and(|o| {
                                matches!(
                                    o.root,
                                    hirsel_types::OriginRoot::Local(..)
                                        | hirsel_types::OriginRoot::Parameter(_)
                                )
                            })
                    }) => {}
                _ => continue,
            }
            let defines = |place: &PlaceId| self.body.places[place.0 as usize].root == root;
            for block in &self.body.blocks {
                for instruction in &block.instructions {
                    let InstructionKind::Assign { destination, value } = instruction.kind else {
                        continue;
                    };
                    if !defines(&destination) {
                        continue;
                    }
                    match &self.body.rvalues[value.0 as usize] {
                        Rvalue::Use(operand)
                        | Rvalue::Unary {
                            op: hirsel_syntax::UnaryOp::Unique,
                            value: operand,
                        } => pending.push(self.body.places[operand.place.0 as usize].root),
                        Rvalue::Closure { captures, .. } => pending.extend(
                            captures
                                .iter()
                                .map(|c| self.body.places[c.place.0 as usize].root),
                        ),
                        _ => {}
                    }
                }
                if let Terminator::Call {
                    callee: Callee::Indirect { value, .. },
                    returns: Some(CallReturn { destination, .. }),
                    ..
                } = &block.terminator
                {
                    if defines(destination) {
                        pending.push(self.body.places[value.place.0 as usize].root);
                    }
                }
            }
        }
        false
    }
    fn leaks(&mut self, model: &Model, state: &State, report: bool) {
        for i in 0..self.body.locals.len() {
            if model.release[i]
                && state.ownership_conflicts[i]
                && state.places[model.roots[i]] & LIVE != 0
            {
                self.flow_error(
                    report,
                    ViolationKind::BorrowedMove(PlaceId(model.roots[i] as u32)),
                );
            }
            if model.release[i]
                && state.must[i].has(MustFacts::OWNED)
                && state.places[model.roots[i]] & LIVE != 0
            {
                self.flow_error(report, ViolationKind::LeakedSlot(LocalId(i as u32)));
            }
        }
    }
}

#[cfg(test)]
mod nominal_limit_tests {
    use super::*;
    use crate::support::{Fixture, Table, id, interner, ty};
    use hirsel_syntax::ContentHash;
    use hirsel_types::*;

    #[test]
    fn partially_moved_cleanup_checks_every_remaining_field_past_128_nominals() {
        for depth in [128, 129, 200] {
            let mut f = Fixture::unit();
            let unit = f.definition.types.id(0).unwrap();
            let resource = id("R");
            f.signatures.insert(
                resource,
                Signature {
                    definition: resource,
                    metadata: SignatureMetadata::default(),
                    fingerprint: ContentHash::default(),
                    kind: SignatureKind::Type(Box::new(TypeDefinition {
                        loop_source: None,
                        format: None,
                        generics: vec![],
                        representation: Some(unit),
                        bounds: vec![],
                        authority: AuthoritySummary::inert(),
                        capability: None,
                        implementation_authority: None,
                        serializable: false,
                        serializable_check: None,
                        layout: Layout::default(),
                        resource: true,
                        distinct: false,
                        drop: None,
                        fact: None,
                        captures: None,
                        destructor_allocation: Allocation::MayAllocate,
                    })),
                },
            );
            let leaf = f.definition.types.push(ty(TypeKind::Named {
                definition: resource,
                arguments: vec![].into(),
            }));
            let mut root = leaf;
            let mut outer = id("D0");
            for n in 0..depth {
                outer = id(&format!("D{n}"));
                let representation = f.definition.types.push(ty(TypeKind::Record(
                    vec![hirsel_types::Field {
                        name: "value".into(),
                        ty: root,
                        growable: false,
                    }]
                    .into(),
                )));
                f.signatures.insert(
                    outer,
                    Signature {
                        definition: outer,
                        metadata: SignatureMetadata::default(),
                        fingerprint: ContentHash::default(),
                        kind: SignatureKind::Type(Box::new(TypeDefinition {
                            loop_source: None,
                            format: None,
                            generics: vec![],
                            representation: Some(representation),
                            bounds: vec![],
                            authority: AuthoritySummary::inert(),
                            capability: None,
                            implementation_authority: None,
                            serializable: false,
                            serializable_check: None,
                            layout: Layout::default(),
                            resource: false,
                            distinct: false,
                            drop: None,
                            fact: None,
                            captures: None,
                            destructor_allocation: Allocation::NoAlloc,
                        })),
                    },
                );
                root = f.definition.types.push(ty(TypeKind::Named {
                    definition: outer,
                    arguments: vec![].into(),
                }));
            }
            let holder = f.slot(
                TypeKind::Named {
                    definition: outer,
                    arguments: vec![].into(),
                },
                Storage::Local,
            );
            let mut moved = f.definition.bodies[0].places[holder as usize].clone();
            moved.projection = vec![Projection::Field(0); depth].into();
            moved.ty = leaf;
            let moved_id = f.definition.bodies[0].places.len();
            f.definition.bodies[0].places.push(moved);
            let declarations = vec![GenericDeclarations::default()];
            let scratch = scratch::Scratch::new(f.definition.bodies.len());
            let memo = TypeMemo::new();
            let reader = crate::input::Reader::new(interner(), &f.signatures);
            let mut checker = Checker {
                definition: &f.definition,
                t: interner(),
                memo: &memo,
                body: &f.definition.bodies[0],
                signatures: &reader,
                declarations: &declarations,
                scratch: &scratch,
                contract: false,
                site: Site {
                    body: BodyId(0),
                    block: None,
                    instruction: None,
                },
                errors: vec![],
                captured: BTreeMap::new(),
                returned: BTreeMap::new(),
                static_data: None,
                proof_errors: std::cell::RefCell::new(Vec::new()),
                unique_parts_cache: Default::default(),
            };
            let model = Model::new(&mut checker);
            let lifetime_global = |ty: TypeId| {
                checker
                    .lifetime(&interner().row(ty).modes.relation)
                    .is_global()
            };
            let mut state = model.initial(
                checker.body,
                interner(),
                &checker.scratch.origin_types[checker.site.body.0 as usize],
                &lifetime_global,
                &|o| checker.arena_place(o),
                &[],
            );
            let root_local = checker.body.places[holder as usize].root.0 as usize;
            for &index in &model.root_parts[root_local] {
                state.places.set(index, LIVE);
            }
            let leaf_index = model.mapping[moved_id];
            state.places.set(leaf_index, MOVED);
            assert!(!checker.remaining_drop_allocates(&model, &state, PlaceId(holder)));
            state.places.set(leaf_index, LIVE);
            assert!(checker.remaining_drop_allocates(&model, &state, PlaceId(holder)));
            assert!(checker.proof_errors.borrow().is_empty());
        }
    }
}

#[cfg(test)]
mod header_name_tests {
    use super::*;
    use crate::support::{Fixture, Table, interner, scoped, ty};
    use hirsel_types::*;

    #[test]
    fn declared_header_names_are_constant_without_certifying_data_origins() {
        for (header, ordinal, valid) in [(true, 0, true), (false, 0, false), (true, 99, false)] {
            let mut f = Fixture::unit();
            let parent_scope = f.definition.bodies[0].parameter_scope;
            let owner = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
            f.definition.bodies[0]
                .parameters
                .push(hirsel_tir::LocalId(owner));
            let mut row = ty(TypeKind::Primitive(Primitive::Bytes));
            row.brand.alternatives.push(Origin {
                root: OriginRoot::Parameter(ParameterRef::body(parent_scope, ParamId(ordinal))),
                fields: vec![].into(),
            });
            let data = f.definition.types.push(row);
            let unit = f.definition.types.id(0).unwrap();
            let root = if header {
                f.definition.types.push(ty(TypeKind::Function(FunctionType {
                    state_promises: vec![].into(),
                    generics: vec![].into(),
                    parameters: vec![Parameter {
                        name: None,
                        label: None,
                        passing: PassingMode::Borrow,
                        ty: data,
                    }]
                    .into(),
                    result: unit,
                    error: None,
                    allocation: Allocation::NoAlloc,
                    authority: AuthoritySummary::inert(),
                    uses: Uses::Unrestricted,
                    contracts: vec![].into(),
                    incomplete: false,
                    foreign: None,
                })))
            } else {
                data
            };
            let mut child = f.definition.bodies[0].clone();
            child.locals.truncate(1);
            child.places.truncate(1);
            child.parameters.clear();
            child.parameter_scope = scoped(NameSite::Expression(hirsel_syntax::ExprId(0)));
            child.locals.push(Local {
                ty: root,
                storage: Storage::Local,
                scope: ScopeId(0),
                span: hirsel_span::Span::default(),
            });
            child.places.push(Place {
                root: hirsel_tir::LocalId(1),
                projection: vec![].into(),
                ty: root,
                loan: None,
                binding: None,
            });
            f.definition.bodies.push(child);
            let declarations = vec![GenericDeclarations::default(); 2];
            let scratch = scratch::Scratch::new(f.definition.bodies.len());
            let memo = TypeMemo::new();
            let reader = crate::input::Reader::new(interner(), &f.signatures);
            let mut checker = Checker {
                definition: &f.definition,
                t: interner(),
                memo: &memo,
                body: &f.definition.bodies[1],
                signatures: &reader,
                declarations: &declarations,
                scratch: &scratch,
                contract: false,
                site: Site {
                    body: BodyId(1),
                    block: None,
                    instruction: None,
                },
                errors: vec![],
                captured: BTreeMap::new(),
                returned: BTreeMap::new(),
                static_data: None,
                proof_errors: std::cell::RefCell::new(Vec::new()),
                unique_parts_cache: Default::default(),
            };
            let model = Model::new(&mut checker);
            let state = model.initial(
                checker.body,
                interner(),
                &checker.scratch.origin_types[checker.site.body.0 as usize],
                &|t| {
                    checker
                        .lifetime(&interner().row(t).modes.relation)
                        .is_global()
                },
                &|o| checker.arena_place(o),
                &[],
            );
            let store = checker.t;
            let canonical =
                super::flow_origins::canonical(store, root, &state.origins.bindings, &|o| {
                    checker.arena_place(o)
                });
            assert_eq!(
                canonical.is_ok(),
                valid,
                "header={header}, ordinal={ordinal}: {canonical:?}"
            );
            if let Ok(canonical) = canonical {
                assert!(interner().semantic_eq(root, canonical));
            }
        }
    }

    #[test]
    fn captured_arena_names_preserve_bounds_without_granting_data_evidence() {
        for (arena, ordinal, valid) in [(true, 0, true), (false, 0, false), (true, 99, false)] {
            let mut f = Fixture::unit();
            let parent_scope = f.definition.bodies[0].parameter_scope;
            let owner = if arena {
                crate::support::arena_slot(&mut f, Storage::Parameter)
            } else {
                f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter)
            };
            f.definition.bodies[0]
                .parameters
                .push(hirsel_tir::LocalId(owner));
            let mut row = ty(TypeKind::Primitive(Primitive::Bytes));
            let origin = Origin {
                root: OriginRoot::Parameter(ParameterRef::body(parent_scope, ParamId(ordinal))),
                fields: vec![].into(),
            };
            row.modes.relation = if arena && ordinal == 0 {
                Relation::place(origin).union(&Relation::environment())
            } else {
                row.brand.alternatives.push(origin);
                Relation::environment()
            };
            let root = f.definition.types.push(row);
            let mut child = f.definition.bodies[0].clone();
            child.locals.truncate(1);
            child.places.truncate(1);
            child.parameters.clear();
            child.parameter_scope = scoped(NameSite::Expression(hirsel_syntax::ExprId(0)));
            child.locals.push(Local {
                ty: root,
                storage: Storage::Capture,
                scope: ScopeId(0),
                span: hirsel_span::Span::default(),
            });
            child.places.push(Place {
                root: hirsel_tir::LocalId(1),
                projection: vec![].into(),
                ty: root,
                loan: None,
                binding: None,
            });
            f.definition.bodies.push(child);
            let declarations = vec![GenericDeclarations::default(); 2];
            let scratch = scratch::Scratch::new(f.definition.bodies.len());
            let memo = TypeMemo::new();
            let reader = crate::input::Reader::new(interner(), &f.signatures);
            let mut checker = Checker {
                definition: &f.definition,
                t: interner(),
                memo: &memo,
                body: &f.definition.bodies[1],
                signatures: &reader,
                declarations: &declarations,
                scratch: &scratch,
                contract: false,
                site: Site {
                    body: BodyId(1),
                    block: None,
                    instruction: None,
                },
                errors: vec![],
                captured: BTreeMap::new(),
                returned: BTreeMap::new(),
                static_data: None,
                proof_errors: std::cell::RefCell::new(Vec::new()),
                unique_parts_cache: Default::default(),
            };
            let model = Model::new(&mut checker);
            let state = model.initial(
                checker.body,
                interner(),
                &checker.scratch.origin_types[checker.site.body.0 as usize],
                &|t| {
                    checker
                        .lifetime(&interner().row(t).modes.relation)
                        .is_global()
                },
                &|o| checker.arena_place(o),
                &[super::super::flow_origins::CapturedOrigin::default()],
            );
            let store = checker.t;
            let canonical =
                super::flow_origins::canonical(store, root, &state.origins.bindings, &|o| {
                    checker.arena_place(o)
                });
            assert_eq!(
                canonical.is_ok(),
                valid,
                "arena={arena}, ordinal={ordinal}: {canonical:?}"
            );
            assert!(!state.must[1].has(MustFacts::SENDABLE));
            assert!(!state.must[1].has(MustFacts::GLOBAL));
            assert!(state.origins.proofs[model.roots[1]].is_empty());
            if let Ok(canonical) = canonical {
                assert!(interner().semantic_eq(root, canonical));
            }
        }
    }
}
