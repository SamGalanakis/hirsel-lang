//! Memos over the session interner, split by whether they depend on context
//! (design §1a). `TypeMemo` holds pure transformations keyed by every explicit
//! input and lives for the session (one for I, one for V). `ContextMemo`
//! belongs to one query: it is bound to that query's generic declarations and
//! signature lookup, and a hit replays the lookups its entry observed into the
//! query, so the query's recorded reads are those of a cache-free evaluation.
use crate::encoding::{Encoder, EncodingError, canonical_encoding};
use crate::*;
use hirsel_intern::IdHasher;
use hirsel_syntax::{Bound, ContentHash};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash};
use std::sync::{PoisonError, RwLock};

const SHARDS: usize = 16;

/// A sharded concurrent map: the structural memo is read by every worker.
struct Shards<K, V> {
    shards: [RwLock<HashMap<K, V, IdHasher>>; SHARDS],
}
impl<K: Eq + Hash, V: Clone> Shards<K, V> {
    fn new() -> Self {
        Self {
            shards: std::array::from_fn(|_| RwLock::new(HashMap::default())),
        }
    }
    fn shard(&self, key: &K) -> &RwLock<HashMap<K, V, IdHasher>> {
        &self.shards[(IdHasher::default().hash_one(key) >> 60) as usize % SHARDS]
    }
    fn get_or(&self, key: K, compute: impl FnOnce() -> V) -> V {
        let shard = self.shard(&key);
        if let Some(value) = shard
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return value.clone();
        }
        // Computed outside the lock: the computation may consult the memo.
        let value = compute();
        shard
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_insert(value)
            .clone()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SubstitutionKey {
    ty: TypeId,
    binder: Option<GenericBinder>,
    arguments: Box<[GenericArgument]>,
    authorities: Box<[AuthoritySummary]>,
    roles: NominalRoles,
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct InstanceKey {
    view: ViewDigest,
    arguments: Box<[GenericArgument]>,
    authorities: Box<[AuthoritySummary]>,
    roles: NominalRoles,
}

/// Structural memos only: every entry is a pure function of its key, so the
/// memo is shared session-wide by one checker. I and V each own one.
pub struct TypeMemo {
    substitutions: Shards<SubstitutionKey, Result<TypeId, GraphError>>,
    instances: Shards<InstanceKey, Result<Instantiated, GraphError>>,
    projections: Shards<(TypeId, TypeId, bool), TypeId>,
}
impl Default for TypeMemo {
    fn default() -> Self {
        Self::new()
    }
}
impl TypeMemo {
    pub fn new() -> Self {
        Self {
            substitutions: Shards::new(),
            instances: Shards::new(),
            projections: Shards::new(),
        }
    }
    pub(crate) fn substitute(
        &self,
        ty: TypeId,
        s: &Substitution<'_>,
        compute: impl FnOnce() -> Result<TypeId, GraphError>,
    ) -> Result<TypeId, GraphError> {
        if s.binder.is_none() && s.roles.result_type.is_none() {
            // Transport without a binder is the identity on a valid graph.
            return compute();
        }
        self.substitutions.get_or(
            SubstitutionKey {
                ty,
                binder: s.binder,
                arguments: s.arguments.into(),
                authorities: s.authorities.into(),
                roles: s.roles.clone(),
            },
            compute,
        )
    }
}
impl std::fmt::Debug for TypeMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TypeMemo")
    }
}

/// The exact content of one signature view: a hash over every field, the
/// private shape and implementation ceiling included. Two views with equal
/// public fingerprints but different private representations differ here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewDigest(pub ContentHash);

/// The `ViewDigest` of `view`, computed when the view enters the snapshot.
pub fn view_digest(t: &TypeInterner, view: &Signature) -> Result<ViewDigest, EncodingError> {
    let mut bytes = canonical_encoding(t, view, &[])?;
    // The ceiling is excluded from public encodings; a view still binds it.
    if let SignatureKind::Type(declaration) = &view.kind {
        let mut e = Encoder::default();
        e.option(
            declaration.implementation_authority.as_ref(),
            |e, ceiling| e.authority(ceiling),
        );
        bytes.extend_from_slice(&e.bytes);
    }
    Ok(ViewDigest(ContentHash::of_parts(
        "hirsel.signature-view.v1",
        [bytes.as_slice(), view.fingerprint.0.as_slice()],
    )))
}

/// A signature's kind with every type edge substituted at its own binder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instantiated {
    pub kind: SignatureKind,
}

/// `sig` at `arguments`: its header (or constant type, or representation)
/// substituted for its own binder. Keyed by the view's exact digest and the
/// actuals, never by its DefId or public fingerprint, so an edited resident
/// signature or private shape cannot hit a stale entry.
pub fn instantiate(
    t: &TypeInterner,
    m: &TypeMemo,
    sig: &Signature,
    view: ViewDigest,
    s: &Substitution<'_>,
) -> Result<Instantiated, GraphError> {
    let key = InstanceKey {
        view,
        arguments: s.arguments.into(),
        authorities: s.authorities.into(),
        roles: s.roles.clone(),
    };
    m.instances.get_or(key, || {
        let s = Substitution {
            binder: Some(GenericBinder::Bound(0)),
            ..*s
        };
        let kind = match &sig.kind {
            SignatureKind::Function(f) => {
                SignatureKind::Function(crate::graph::substitute_header(t, m, f, &s)?)
            }
            SignatureKind::Const { ty, generics } => SignatureKind::Const {
                ty: substitute(t, m, *ty, &s)?,
                generics: generics.clone(),
            },
            SignatureKind::Type(declaration) => {
                let mut declaration = declaration.clone();
                declaration.representation = declaration
                    .representation
                    .map(|r| substitute(t, m, r, &s))
                    .transpose()?;
                SignatureKind::Type(declaration)
            }
        };
        Ok(Instantiated { kind })
    })
}

/// OWN-8: a part read through its owner is bounded by both relations unless
/// it is Plain. `plain` is the contextual Plain proof of `part`
/// (`ContextMemo::plain`); the transformation itself is structural.
pub fn project(t: &TypeInterner, m: &TypeMemo, owner: TypeId, part: TypeId, plain: bool) -> TypeId {
    if plain {
        return part;
    }
    m.projections.get_or((owner, part, plain), || {
        let (Some(row), Some(owner)) = (t.get(part), t.get(owner)) else {
            return part;
        };
        let relation = row.modes.relation.union(&owner.modes.relation);
        if relation == row.modes.relation {
            return part;
        }
        // Projection introduces an access lifetime, not a counted share.
        // The operand's use and ownership flow decide any later invalidation.
        let mut row = row.clone();
        row.modes.relation = relation;
        t.intern(row)
    })
}

/// The proof mode of `ContextMemo::prove`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProofFlags {
    pub check_root_locality: bool,
    pub use_evidence: bool,
    pub shared_storage: bool,
    /// MEM-11: `Shared`, `Mutex` and `Atomic` handles count as detached parts.
    pub handles: bool,
}

/// An owned-payload set interned in one `ContextMemo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PayloadSetId(u32);

/// The policy of one directional comparison. `payloads` are the owned payload
/// rows of the actual along which a view may weaken nested evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompatPolicy {
    pub forget_brands: bool,
    pub payloads: Option<PayloadSetId>,
    /// Nested rows may promise more in reading positions; false for hand-back.
    pub received: bool,
}
impl CompatPolicy {
    /// Ordinary reception: no brand forgetting, no payload view.
    pub const RECEIVED: Self = Self {
        forget_brands: false,
        payloads: None,
        received: true,
    };
    /// `mut` hand-back: every nested contract is exact.
    pub const HANDBACK: Self = Self {
        forget_brands: false,
        payloads: None,
        received: false,
    };
}

/// A memoized contextual result and the lookups it observed.
#[derive(Clone)]
struct Recorded<T> {
    value: Result<T, QueryError>,
    reads: Box<[DefId]>,
}

/// The signature lookup of a query: missing declarations are None.
pub type Lookup<'q> = dyn FnMut(DefId) -> Result<Option<SignatureRef>, QueryError> + 'q;

/// Contextual memos of one query, bound to its generic declarations and its
/// signature lookup (and so to its snapshot). Entries die with the query.
pub struct ContextMemo<'q> {
    t: &'q TypeInterner,
    generics: &'q GenericDeclarations,
    lookup: &'q mut Lookup<'q>,
    /// Every lookup this memo's evaluations performed, in order; an entry
    /// records the range its evaluation appended.
    log: Vec<DefId>,
    proofs: HashMap<(TypeId, Bound, ProofFlags), Recorded<Sendability>>,
    plain: HashMap<TypeId, Recorded<bool>>,
    compatible: HashMap<(TypeId, TypeId, TypeRelation, CompatPolicy), Recorded<bool>>,
    payload_ids: HashMap<Vec<TypeId>, PayloadSetId>,
    payload_sets: Vec<HashSet<TypeId>>,
    hits: u64,
}

impl<'q> ContextMemo<'q> {
    pub fn new(
        t: &'q TypeInterner,
        generics: &'q GenericDeclarations,
        lookup: &'q mut Lookup<'q>,
    ) -> Self {
        Self {
            t,
            generics,
            lookup,
            log: vec![],
            proofs: HashMap::new(),
            plain: HashMap::new(),
            compatible: HashMap::new(),
            payload_ids: HashMap::new(),
            payload_sets: vec![],
            hits: 0,
        }
    }
    pub fn types(&self) -> &'q TypeInterner {
        self.t
    }
    pub fn generics(&self) -> &'q GenericDeclarations {
        self.generics
    }
    /// Memo hits so far, for the lane report.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Look up `definition` through the query's provider, recording the read.
    pub fn signature(&mut self, definition: DefId) -> Result<Option<SignatureRef>, QueryError> {
        self.log.push(definition);
        (self.lookup)(definition)
    }

    fn replay(&mut self, reads: &[DefId]) {
        self.hits += 1;
        for &definition in reads {
            // The entry's value already reflects this read's result; replay
            // records the dependency in the owning query.
            let _ = self.signature(definition);
        }
    }

    /// Run `f` with a lookup that records into this memo's log.
    fn observed<T>(
        &mut self,
        f: impl FnOnce(
            &'q TypeInterner,
            &'q GenericDeclarations,
            &mut dyn FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
        ) -> Result<T, QueryError>,
    ) -> Recorded<T> {
        let start = self.log.len();
        let (t, generics) = (self.t, self.generics);
        let log = &mut self.log;
        let lookup = &mut *self.lookup;
        let value = f(t, generics, &mut |definition| {
            log.push(definition);
            lookup(definition)
        });
        Recorded {
            value,
            reads: self.log[start..].into(),
        }
    }

    /// `ty` satisfies `bound` (Sendable: its crossing summary) under `flags`.
    pub fn prove(
        &mut self,
        ty: TypeId,
        bound: Bound,
        flags: ProofFlags,
    ) -> Result<Sendability, QueryError> {
        let key = (ty, bound, flags);
        if let Some(entry) = self.proofs.get(&key).cloned() {
            self.replay(&entry.reads);
            return entry.value;
        }
        let entry = self.observed(|t, generics, lookup| {
            crate::properties::prove(
                t,
                ty,
                bound,
                generics,
                flags.check_root_locality,
                flags.use_evidence,
                flags.shared_storage,
                flags.handles,
                &mut |d| lookup(d),
            )
        });
        self.proofs.insert(key, entry.clone());
        entry.value
    }

    /// Whether `part` is Plain: the contextual half of `projected_type`.
    pub fn plain(&mut self, part: TypeId) -> Result<bool, QueryError> {
        if let Some(entry) = self.plain.get(&part).cloned() {
            self.replay(&entry.reads);
            return entry.value;
        }
        let entry = self.observed(|t, generics, lookup| {
            structural_property(t, part, StructuralProperty::Plain, generics, &mut |d| {
                lookup(d)
            })
        });
        self.plain.insert(part, entry.clone());
        entry.value
    }

    /// OWN-8 projection of `part` through `owner`: the Plain proof here, the
    /// transformation in the structural memo.
    pub fn projected(
        &mut self,
        m: &TypeMemo,
        owner: TypeId,
        part: TypeId,
    ) -> Result<TypeId, QueryError> {
        if !self.t.contains(part) || !self.t.contains(owner) {
            return Ok(part);
        }
        let relation = self
            .t
            .row(part)
            .modes
            .relation
            .union(&self.t.row(owner).modes.relation);
        if relation == self.t.row(part).modes.relation {
            return Ok(part);
        }
        let plain = self.plain(part)?;
        Ok(project(self.t, m, owner, part, plain))
    }

    /// Intern an owned-payload set for a `CompatPolicy`.
    pub fn payload_set(&mut self, ids: impl IntoIterator<Item = TypeId>) -> PayloadSetId {
        let set: HashSet<TypeId> = ids.into_iter().collect();
        let mut key: Vec<TypeId> = set.iter().copied().collect();
        key.sort_by_key(|id| format!("{id:?}"));
        if let Some(id) = self.payload_ids.get(&key) {
            return *id;
        }
        let id = PayloadSetId(self.payload_sets.len() as u32);
        self.payload_sets.push(set);
        self.payload_ids.insert(key, id);
        id
    }

    /// The directional relation: `actual` received where `expected` stands
    /// (CLO-2, MODE-1/2/4, MEM-5). Variance reverses through callable
    /// parameters, `mut` parameters and nominal arguments stay exact, stronger
    /// promises are accepted only when receiving, and hand-back is exact.
    pub fn compatible(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
        policy: CompatPolicy,
    ) -> Result<bool, QueryError> {
        if relation == TypeRelation::Exact {
            return Ok(self.t.semantic_eq(expected, actual));
        }
        let key = (expected, actual, relation, policy);
        if let Some(entry) = self.compatible.get(&key).cloned() {
            self.replay(&entry.reads);
            return entry.value;
        }
        let start = self.log.len();
        let value = self.compatible_uncached(expected, actual, relation, policy);
        let entry = Recorded {
            value,
            reads: self.log[start..].into(),
        };
        self.compatible.insert(key, entry.clone());
        entry.value
    }

    fn compatible_uncached(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
        policy: CompatPolicy,
    ) -> Result<bool, QueryError> {
        let t = self.t;
        let Some(expected_row) = t.get(expected).cloned() else {
            return Ok(false);
        };
        let Some(mut actual_row) = t.get(actual).cloned() else {
            return Ok(false);
        };
        if expected_row.allocation_target.is_some()
            && expected_row.allocation_target != actual_row.allocation_target
        {
            return Ok(false);
        }
        actual_row.allocation_target = expected_row.allocation_target;
        let plain = self.plain(actual)?;
        let received = policy.received;
        // A `mut` hand-back is written back through the parameter, so storage
        // that promised less than the parameter admits would return holding it:
        // the callable promise, the erased state's captures and the cleanup
        // summary are exact at the root as they are below it. A core operation
        // that only advances an erased state declares its parameter at the
        // argument's own captures, so it meets this rule by construction.
        let handed_back = received
            || (value_promises(&actual_row, &mut expected_row.clone())
                && actual_row.modes.multiplicity == expected_row.modes.multiplicity
                && (plain
                    || crossing_requirement(&actual_row) == crossing_requirement(&expected_row)));
        let accepts = handed_back
            && value_promises(&expected_row, &mut actual_row)
            && receives_relation(
                &expected_row.modes.relation,
                &actual_row.modes.relation,
                relation,
            )
            && (expected_row.modes.uniqueness == Uniqueness::Aliased
                || actual_row.modes.uniqueness == Uniqueness::Unique
                || plain)
            && (expected_row.modes.multiplicity == Multiplicity::Once
                || actual_row.modes.multiplicity == Multiplicity::Many)
            && receives_crossing(
                &crossing_requirement(&expected_row),
                &actual_row.modes.sendability,
            );
        if !accepts {
            return Ok(false);
        }
        // Root brands may forget identity or join known alternatives. Empty actual
        // evidence cannot manufacture a branded origin; nested brands stay invariant.
        if !expected_row.brand.alternatives.is_empty()
            && (actual_row.brand.alternatives.is_empty()
                || !actual_row
                    .brand
                    .alternatives
                    .iter()
                    .all(|o| expected_row.brand.alternatives.contains(o)))
        {
            return Ok(false);
        }
        actual_row.brand = expected_row.brand.clone();
        actual_row.sendability_requirement = expected_row.sendability_requirement;
        actual_row.modes = expected_row.modes;
        let payloads = policy.payloads.map(|id| &self.payload_sets[id.0 as usize]);
        Ok(crate::graph::directional_equal(
            t,
            expected,
            actual_row,
            policy.forget_brands,
            payloads,
            received,
        )
        .unwrap_or(false))
    }

    /// `compatible` with ordinary reception (main's `compatible_type`).
    pub fn compatible_type(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
    ) -> Result<bool, QueryError> {
        self.compatible(expected, actual, relation, CompatPolicy::RECEIVED)
    }

    /// The relation for a `mut` argument (main's `compatible_handback_type`):
    /// the callee writes the caller's storage back through its parameter, so
    /// every contract, root and nested, is exact.
    pub fn handback(&mut self, expected: TypeId, actual: TypeId) -> Result<bool, QueryError> {
        self.compatible(
            expected,
            actual,
            TypeRelation::Parameter,
            CompatPolicy::HANDBACK,
        )
    }

    /// The non-writing view policy of `actual` (main's `compatible_view_type_in`):
    /// Data may forget nested brands, and owned payload edges may weaken nested
    /// uniqueness. Payload classification reads declarations.
    pub fn view_policy(&mut self, actual: TypeId) -> Result<CompatPolicy, QueryError> {
        let data = self.prove_bound(actual, Bound::Data)?;
        // Only owned payload edges are covariant. A callable's header, phantom
        // argument or interior-mutable cell remains the same contract even when
        // the containing value is taken. Nominal Data excludes hidden callbacks.
        let mut payloads = HashSet::new();
        let mut pending = vec![actual];
        let mut seen = HashSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(row) = self.t.get(id) else { continue };
            let owned = match &row.kind {
                TypeKind::Tuple(_)
                | TypeKind::Box(_)
                | TypeKind::Record(_)
                | TypeKind::Variant(_) => true,
                TypeKind::Builtin { kind, .. } => matches!(
                    kind,
                    BuiltinType::Array
                        | BuiltinType::List
                        | BuiltinType::Map
                        | BuiltinType::Set
                        | BuiltinType::Inline
                        | BuiltinType::Simd
                        | BuiltinType::Packed
                        | BuiltinType::Shared
                        | BuiltinType::Slots
                ),
                TypeKind::Named { .. } => self.prove_bound(id, Bound::Data)?,
                _ => false,
            };
            if owned {
                payloads.insert(id);
                pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
            }
        }
        Ok(CompatPolicy {
            forget_brands: data,
            payloads: Some(self.payload_set(payloads)),
            received: true,
        })
    }

    /// Data/Key structural equality proof (`structural_equality`), recorded.
    fn prove_bound(&mut self, ty: TypeId, bound: Bound) -> Result<bool, QueryError> {
        self.observed(|t, generics, lookup| {
            structural_equality(t, ty, bound, generics, &mut |d| lookup(d))
        })
        .value
    }

    /// The non-writing view relation (main's `compatible_view_type_in`).
    pub fn view(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
    ) -> Result<bool, QueryError> {
        if relation == TypeRelation::Exact {
            return self.compatible_type(expected, actual, relation);
        }
        let policy = self.view_policy(actual)?;
        self.compatible(expected, actual, relation, policy)
    }

    /// Replacement of caller-owned storage must supply a global value
    /// (main's `compatible_storage_type`).
    pub fn storage(&mut self, expected: TypeId, actual: TypeId) -> Result<bool, QueryError> {
        if !self
            .t
            .get(actual)
            .is_some_and(|row| row.modes.relation.is_global())
        {
            return Ok(false);
        }
        self.compatible_type(expected, actual, TypeRelation::Value)
    }
}
