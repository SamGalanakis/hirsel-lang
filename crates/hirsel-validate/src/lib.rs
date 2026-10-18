//! Validation of compiler invariants, never source diagnostics.
//!
//! `validate` consumes immutable IR and explicitly owned signature snapshots. It
//! first validates all indices/type graphs, then types, then a finite monotone
//! CFG analysis. Violations are emitted in body/block/instruction order; fixed
//! point iterations do not emit diagnostics. No provider or interpreter runs.
//!
//! Raw in-range arena IDs from another owner cannot be detected (TypeIds do
//! carry provenance and are checked without calling the asserting accessor).
//! Omitted source effects and preservation of source evaluation order cannot be
//! proved from TIR: those are lowering's obligations, tested against source.
//! Plain inline bits need no release (DROP-1); owned counted/resource storage
//! must be moved or explicitly cleaned. Borrowed parameters remain caller-owned.
//! Call/owner origins are replayed from scoped declarations and actual operands;
//! missing or ambiguous identity evidence is a violation.
//! See the crate README for the checks and conservative unsupported cases.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_intern::work::{self, Counter};
use hirsel_syntax::DefPath;
use hirsel_tir::*;
use hirsel_types::{
    Destination, GenericArgument, QueryError, Signature, TypeId, TypeKind, TypeStore, UseKind,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod cleanup_proof;
mod declaration;
mod effects;
mod flow;
mod flow_origins;
mod generics;
mod indexed;
mod instances;
mod matching;
mod origin_dependencies;
mod origin_types;
mod protocol;
mod scope_locals;
mod scratch;
mod serialization;
mod shared;
mod static_data;
mod tail;
mod typing;
mod variant_brands;
pub use instances::{InstanceViolation, validate_instance_bodies};
pub use matching::{MAX_MATCH_WORK, tooling_limit_error};

/// Owned, self-contained snapshots; IDs are never shared between their stores.
pub type Signatures = BTreeMap<DefPath, Signature>;

/// Deterministic compiler-invariant violations from full validation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[must_use]
pub struct Validation {
    pub violations: Vec<Violation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub body: Option<BodyId>,
    pub block: Option<BlockId>,
    /// None denotes the terminator (or a body/arena-level violation).
    pub instruction: Option<u32>,
    pub kind: ViolationKind,
}

/// Compiler errors, deliberately without RuleId, severity or diagnostic spans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViolationKind {
    InvalidId {
        arena: &'static str,
        index: u32,
    },
    InvalidType(TypeId),
    InvalidTypeGraph,
    Duplicate {
        entity: &'static str,
        index: u32,
    },
    WrongKind {
        entity: &'static str,
        index: u32,
    },
    ScopeCycle(ScopeId),
    UnreachableBlock(BlockId),
    UnreferencedBody(BodyId),
    UnreferencedRvalue(RvalueId),
    MissingSignature(DefPath),
    InvalidSignature(DefPath),
    Arity {
        entity: &'static str,
        expected: usize,
        actual: usize,
    },
    TypeMismatch {
        entity: &'static str,
    },
    InvalidOperator,
    InvalidProtocolAssociation,
    /// The declaration seam exists; executable envelope proofs are dependency-owned.
    UnprovedStatePromise,
    InvalidConstructor,
    InvalidUse(PlaceId),
    /// A local result retains storage outside its declared argument sources.
    InvalidResultSource(PlaceId),
    /// Missing, inconsistent, expired or ambiguous binding identity evidence.
    InvalidOrigin(PlaceId),
    AliasedArguments {
        first: u32,
        second: u32,
    },
    InvalidProjection(PlaceId),
    MatchCoverage,
    TypeProofToolingLimit {
        limit: usize,
    },
    MatchToolingLimit {
        limit: u64,
    },
    ReadBeforeInit(PlaceId),
    UseAfterMove(PlaceId),
    DoubleMove(PlaceId),
    WriteToLive(PlaceId),
    BorrowedMove(PlaceId),
    MissingLoan(PlaceId),
    LoanConflict {
        loan: LoanId,
        place: PlaceId,
    },
    LoanOutlivesSource {
        loan: LoanId,
        place: PlaceId,
    },
    InactiveLoan(LoanId),
    ExpiredLoan {
        loan: LoanId,
        place: PlaceId,
    },
    ReachableUnreachable,
    LiveLoanAtExit(LoanId),
    CleanupMismatch {
        cleanup: CleanupId,
        expected: Vec<LocalId>,
        actual: Vec<LocalId>,
    },
    CleanupOrder(CleanupId),
    LeakedSlot(LocalId),
    /// Frozen IR has no RegionId arena; unprovable runtime cleanup fails closed.
    UnsupportedCleanup(CleanupId),
}

/// Full validation. Include the owner's function/resource signature for parameter
/// conventions and all called/constructed nominal signatures. Missing facts are
/// violations, never assumed valid. Call and owner-origin relationships are
/// checked from executable producer edges. Unit bodies without parameters need
/// no owner snapshot; if supplied, its result and parameter types are checked too.
pub fn validate(definition: &Definition, signatures: &Signatures) -> Validation {
    validate_input(definition, signatures, || invalid_signatures(signatures))
}

trait SignatureLookup {
    fn get(&self, path: &DefPath) -> Option<&Signature>;
    fn first_heap(&self) -> Option<hirsel_types::HeapRole>;
}
impl SignatureLookup for Signatures {
    fn get(&self, path: &DefPath) -> Option<&Signature> {
        BTreeMap::get(self, path)
    }
    fn first_heap(&self) -> Option<hirsel_types::HeapRole> {
        self.values().find_map(|s| s.metadata.roles.heap.clone())
    }
}
struct SignatureOverlay<'a> {
    base: &'a ValidatedSignatures,
    replacement: &'a Signature,
}
impl SignatureLookup for SignatureOverlay<'_> {
    fn get(&self, path: &DefPath) -> Option<&Signature> {
        if path == &self.replacement.definition {
            Some(self.replacement)
        } else {
            self.base.get(path)
        }
    }
    fn first_heap(&self) -> Option<hirsel_types::HeapRole> {
        let first = self
            .base
            .heaps
            .iter()
            .find(|(path, _)| *path != &self.replacement.definition);
        match (&self.replacement.metadata.roles.heap, first) {
            (Some(heap), Some((path, _))) if &self.replacement.definition < path => {
                Some(heap.clone())
            }
            (_, Some((_, heap))) => Some(heap.clone()),
            (heap, None) => heap.clone(),
        }
    }
}

/// Immutable complete signature input, independently validated once at creation.
/// Every signature is checked, including rows unreachable from a published body.
/// Reuse is safe only while the owned input is unchanged.
pub struct ValidatedSignatures {
    signatures: Signatures,
    errors: BTreeMap<DefPath, Violation>,
    heaps: BTreeMap<DefPath, hirsel_types::HeapRole>,
}
impl SignatureLookup for ValidatedSignatures {
    fn get(&self, path: &DefPath) -> Option<&Signature> {
        self.signatures.get(path)
    }
    fn first_heap(&self) -> Option<hirsel_types::HeapRole> {
        self.heaps.values().next().cloned()
    }
}
impl ValidatedSignatures {
    pub fn new(signatures: Signatures) -> Self {
        let errors = signature_errors(&signatures);
        let heaps = signatures
            .iter()
            .filter_map(|(path, signature)| {
                signature
                    .metadata
                    .roles
                    .heap
                    .clone()
                    .map(|heap| (path.clone(), heap))
            })
            .collect();
        Self {
            signatures,
            errors,
            heaps,
        }
    }
    /// Compare the whole input, including malformed rows and metadata that an
    /// incorrect producer could omit from its fingerprint.
    pub fn matches(&self, signatures: &Signatures) -> bool {
        &self.signatures == signatures
    }
    pub fn validate(&self, definition: &Definition) -> Validation {
        validate_input(definition, self, || self.errors.values().cloned().collect())
    }
    /// Validate a materialized owner signature without copying the input map.
    /// The replacement shadows its old signature exactly as map insertion did.
    pub fn validate_with_signature(
        &self,
        definition: &Definition,
        replacement: &Signature,
    ) -> Validation {
        let overlay = SignatureOverlay {
            base: self,
            replacement,
        };
        validate_input(definition, &overlay, || {
            let mut errors = self.errors.clone();
            errors.remove(&replacement.definition);
            if let Some(error) = signature_error(&replacement.definition, replacement, &overlay) {
                errors.insert(replacement.definition.clone(), error);
            }
            errors.into_values().collect()
        })
    }
}
fn invalid_signature(path: &DefPath) -> Violation {
    Violation {
        body: None,
        block: None,
        instruction: None,
        kind: ViolationKind::InvalidSignature(path.clone()),
    }
}
fn signature_error(
    path: &DefPath,
    signature: &Signature,
    signatures: &dyn SignatureLookup,
) -> Option<Violation> {
    work::count(Counter::ValidateSignatureRows);
    if path != &signature.definition
        || !typing::signature_valid(signature)
        || !declaration::valid(signature, signatures)
        || !protocol::declaration(signature, signatures)
        || !protocol::selectors(signature, signatures)
    {
        return Some(invalid_signature(path));
    }
    match serialization::declaration(signature, signatures) {
        Ok(true) => None,
        Ok(false) => Some(invalid_signature(path)),
        Err(_) => Some(Violation {
            body: None,
            block: None,
            instruction: None,
            kind: ViolationKind::TypeProofToolingLimit {
                limit: hirsel_types::MAX_POLYMORPHIC_EXPANSIONS,
            },
        }),
    }
}
fn signature_errors(signatures: &Signatures) -> BTreeMap<DefPath, Violation> {
    signatures
        .iter()
        .filter_map(|(path, signature)| {
            signature_error(path, signature, signatures).map(|error| (path.clone(), error))
        })
        .collect()
}
fn invalid_signatures(signatures: &Signatures) -> Vec<Violation> {
    signature_errors(signatures).into_values().collect()
}
fn validate_input(
    definition: &Definition,
    signatures: &dyn SignatureLookup,
    signature_errors: impl FnOnce() -> Vec<Violation>,
) -> Validation {
    let mut errors = validate_structure(definition);
    if !errors.is_empty() {
        return Validation { violations: errors };
    }
    if !graph_valid(&definition.types) || definition.types.iter().any(|(_, t)| {
        t.allocation_target.is_some() && !matches!(&t.kind, TypeKind::Named { definition, .. } if signatures.get(definition).is_some_and(|s| t.is_heap_target(&s.metadata.roles)))
    }) {
        errors.push(Violation { body: None, block: None, instruction: None, kind: ViolationKind::InvalidTypeGraph });
    }
    if !errors.is_empty() {
        return Validation { violations: errors };
    }
    errors.extend(signature_errors());
    // Reconstruct tracked bodies from the declaration and the entry's own clause producers.
    // Contracts inside ordinary nested closures cannot inherit the enclosing declaration.
    let mut caller_bodies = Vec::new();
    if signatures
        .get(&definition.path)
        .is_some_and(|s| s.metadata.track_caller)
    {
        caller_bodies.push(definition.entry);
        let contracts: BTreeSet<_> = definition
            .contracts
            .iter()
            .map(|(_, body)| body.0)
            .collect();
        caller_bodies.extend(
            definition.bodies[definition.entry.0 as usize]
                .rvalues
                .iter()
                .filter_map(|value| {
                    if let Rvalue::Closure { body, .. } = value
                        && contracts.contains(&body.0)
                    {
                        Some(*body)
                    } else {
                        None
                    }
                }),
        );
        caller_bodies.sort_unstable_by_key(|body| body.0);
        caller_bodies.dedup();
    }
    if definition.caller_bodies != caller_bodies {
        errors.push(Violation {
            body: None,
            block: None,
            instruction: None,
            kind: ViolationKind::TypeMismatch {
                entity: "caller attribution",
            },
        });
    }
    errors.extend(protocol::receipts(definition, signatures));
    errors.extend(protocol::unproved_promises(definition, signatures));
    if !errors.is_empty() {
        return Validation { violations: errors };
    }
    let Some(declarations) = typing::declaration_tables(definition, signatures) else {
        return Validation {
            violations: vec![Violation {
                body: None,
                block: None,
                instruction: None,
                kind: ViolationKind::InvalidTypeGraph,
            }],
        };
    };
    // Clause purity also applies to executable lambdas nested inside a clause.
    let mut contract_bodies: BTreeSet<u32> =
        definition.contracts.iter().map(|(_, b)| b.0).collect();
    let mut pending: Vec<_> = contract_bodies.iter().copied().collect();
    while let Some(index) = pending.pop() {
        for rvalue in &definition.bodies[index as usize].rvalues {
            if let Rvalue::Closure { body, .. } = rvalue {
                if contract_bodies.insert(body.0) {
                    pending.push(body.0);
                }
            }
        }
    }
    let scratch = scratch::Scratch::new(&definition.types, definition.bodies.len());
    for (index, body) in definition.bodies.iter().enumerate() {
        let mut checker = Checker {
            definition,
            body,
            signatures,
            declarations: &declarations,
            scratch: &scratch,
            contract: contract_bodies.contains(&(index as u32)),
            site: Site {
                body: BodyId(index as u32),
                block: None,
                instruction: None,
            },
            errors: Vec::new(),
            captured: BTreeMap::new(),
            returned: BTreeMap::new(),
            static_data: None,
            proof_errors: std::cell::RefCell::new(Vec::new()),
        };
        let mut witnessed = BTreeSet::new();
        for tail in &body.tail_sites {
            let property = |ty, counted| {
                let mut lookup = |p: &DefPath| {
                    Ok::<_, hirsel_types::QueryError>(
                        signatures.get(p).cloned().map(std::sync::Arc::new),
                    )
                };
                if counted {
                    hirsel_types::structural_equality(
                        &definition.types,
                        ty,
                        hirsel_types::Bound::Data,
                        &declarations[index],
                        &mut lookup,
                    )
                    .unwrap_or_else(|error| checker.proof_failure(error, false))
                } else {
                    hirsel_types::structural_property(
                        &definition.types,
                        ty,
                        hirsel_types::StructuralProperty::Plain,
                        &declarations[index],
                        &mut lookup,
                    )
                    .unwrap_or_else(|error| checker.proof_failure(error, false))
                }
            };
            if !witnessed.insert(tail.call_block.0)
                || !crate::tail::verify_self_tail(definition, BodyId(index as u32), tail, &property)
            {
                checker.error(ViolationKind::WrongKind {
                    entity: "tail witness",
                    index: tail.call_block.0,
                });
            }
        }
        checker.types();
        errors.extend(checker.proof_errors.into_inner());
        errors.extend(checker.errors);
    }
    // A nested body's entry conventions come from its closure producer. Do not
    // run flow in any body until every producer has passed type validation.
    if !errors.is_empty() {
        return Validation { violations: errors };
    }
    // Closure environments participate in the same finite flow proof. Compute
    // entry snapshots before reporting, so body arena order has no significance.
    let mut environments = BTreeMap::<u32, Vec<flow_origins::CapturedOrigin>>::new();
    let mut pending = VecDeque::new();
    environments.insert(definition.entry.0, vec![]);
    pending.push_back(definition.entry.0);
    for (_, body) in &definition.contracts {
        environments.insert(body.0, vec![]);
        pending.push_back(body.0);
    }
    while let Some(index) = pending.pop_front() {
        let body = &definition.bodies[index as usize];
        let mut checker = Checker {
            definition,
            body,
            signatures,
            declarations: &declarations,
            scratch: &scratch,
            contract: contract_bodies.contains(&(index as u32)),
            site: Site {
                body: BodyId(index),
                block: None,
                instruction: None,
            },
            errors: vec![],
            captured: BTreeMap::new(),
            returned: BTreeMap::new(),
            static_data: None,
            proof_errors: std::cell::RefCell::new(Vec::new()),
        };
        checker.flow(&environments[&index], false);
        if !checker.proof_errors.borrow().is_empty() {
            return Validation {
                violations: checker.proof_errors.into_inner(),
            };
        }
        for (nested, captured) in checker.captured {
            let changed = if let Some(previous) = environments.get_mut(&nested) {
                flow_origins::join_captures(previous, &captured)
            } else {
                environments.insert(nested, captured);
                true
            };
            if changed && !pending.contains(&nested) {
                pending.push_back(nested);
            }
        }
    }
    for (index, body) in definition.bodies.iter().enumerate() {
        let mut checker = Checker {
            definition,
            body,
            signatures,
            declarations: &declarations,
            scratch: &scratch,
            contract: contract_bodies.contains(&(index as u32)),
            site: Site {
                body: BodyId(index as u32),
                block: None,
                instruction: None,
            },
            errors: vec![],
            captured: BTreeMap::new(),
            returned: BTreeMap::new(),
            static_data: None,
            proof_errors: std::cell::RefCell::new(Vec::new()),
        };
        if let Some(captures) = environments.get(&(index as u32)) {
            checker.flow(captures, true);
            if index as u32 == definition.entry.0 {
                for parameter in unique_promises(signatures.get(&definition.path), signatures) {
                    let mut proof = Checker {
                        definition,
                        body,
                        signatures,
                        declarations: &declarations,
                        scratch: &scratch,
                        contract: false,
                        site: Site {
                            body: BodyId(index as u32),
                            block: None,
                            instruction: None,
                        },
                        errors: vec![],
                        captured: BTreeMap::new(),
                        returned: BTreeMap::new(),
                        static_data: None,
                        proof_errors: std::cell::RefCell::new(Vec::new()),
                    };
                    proof.flow_with(captures, true, Some(parameter));
                    checker.errors.extend(proof.errors.into_iter().filter(|v| {
                        matches!(v.kind, ViolationKind::TypeMismatch { entity }
                            if entity == flow::UNIQUE_PROOF_FAILED)
                    }));
                }
            }
        }
        errors.extend(checker.proof_errors.into_inner());
        errors.extend(checker.errors);
    }
    Validation { violations: errors }
}

/// Explicit and SEM-3-implied `@preserves_unique` selections (MODE-2).
fn unique_promises(
    signature: Option<&Signature>,
    signatures: &dyn SignatureLookup,
) -> Vec<hirsel_types::ParamId> {
    let Some(hirsel_types::SignatureKind::Function(f)) = signature.map(|s| &s.kind) else {
        return Vec::new();
    };
    let mut promised: Vec<_> = f
        .state_promises
        .iter()
        .filter(|p| p.preserve_unique && p.selector.fields.is_empty())
        .filter_map(|p| match p.selector.parameter {
            hirsel_types::ParameterRef::Bound {
                depth: 0,
                parameter,
            } => Some(parameter),
            _ => None,
        })
        .collect();
    if signature.is_some_and(|s| format_association(s, signatures))
        && !promised.contains(&hirsel_types::ParamId(1))
    {
        promised.push(hirsel_types::ParamId(1));
    }
    promised
}

/// Reconstruct SEM-3's implied promise from declaration snapshots, independent
/// of source annotations and of the producer's ownership decisions.
fn format_association(signature: &Signature, signatures: &dyn SignatureLookup) -> bool {
    let hirsel_types::SignatureKind::Function(f) = &signature.kind else {
        return false;
    };
    if f.foreign.is_some() {
        return false;
    }
    let Some(receiver) = f.parameters.first() else {
        return false;
    };
    let Some(TypeKind::Named { definition, .. }) =
        signature.types.get(receiver.ty).map(|r| &r.kind)
    else {
        return false;
    };
    signatures.get(definition).is_some_and(|s| {
        matches!(&s.kind, hirsel_types::SignatureKind::Type(t)
            if t.format.as_ref() == Some(&signature.definition))
    })
}

/// Lowering-test helper: make invariant failures readable at their publication
/// boundary, without introducing a dependency on the interpreter.
#[track_caller]
pub fn assert_valid(definition: &Definition, signatures: &Signatures) {
    let result = validate(definition, signatures);
    assert!(
        result.violations.is_empty(),
        "invalid typed IR: {:#?}",
        result.violations
    );
}

#[derive(Clone, Copy)]
struct Site {
    body: BodyId,
    block: Option<BlockId>,
    instruction: Option<u32>,
}
struct Checker<'a> {
    definition: &'a Definition,
    body: &'a Body,
    signatures: &'a dyn SignatureLookup,
    declarations: &'a [hirsel_types::GenericDeclarations],
    scratch: &'a scratch::Scratch<'a>,
    contract: bool,
    site: Site,
    errors: Vec<Violation>,
    captured: BTreeMap<u32, Vec<flow_origins::CapturedOrigin>>,
    returned: BTreeMap<u32, hirsel_types::Type>,
    static_data: Option<static_data::Values>,
    proof_errors: std::cell::RefCell<Vec<Violation>>,
}
impl<'a> Checker<'a> {
    fn proof_failure<T>(&self, error: hirsel_types::QueryError, fallback: T) -> T {
        // These pure snapshot callbacks cannot fail. Only the shared declared
        // non-regular expansion budget can produce a query failure here.
        assert!(matches!(error, hirsel_types::QueryError::Operational(_)));
        let violation = Violation {
            body: Some(self.site.body),
            block: self.site.block,
            instruction: self.site.instruction,
            kind: ViolationKind::TypeProofToolingLimit {
                limit: hirsel_types::MAX_POLYMORPHIC_EXPANSIONS,
            },
        };
        let mut errors = self.proof_errors.borrow_mut();
        if !errors.contains(&violation) {
            errors.push(violation);
        }
        fallback
    }
    fn working_store(&self) -> scratch::Region<'a> {
        self.scratch.store()
    }
    /// The lifetime sources of a body row's relation: every reach except its
    /// arena places, which are where its blocks live (PROV-4).
    fn lifetime(&self, relation: &hirsel_types::Relation) -> hirsel_types::Relation {
        relation.retain(|r| !matches!(r, hirsel_types::Reach::Place(o) if self.arena_place(o)))
    }
    /// A body row's relation as this frame sees it. A place naming one of
    /// this body's parameters that is not an arena lies inside the frame's
    /// environment: a borrowed parameter's written relation adds to its own
    /// root (S2), and both are lent by the caller for the call.
    fn frame_relation(&self, relation: &hirsel_types::Relation) -> hirsel_types::Relation {
        relation.map(|r| match r {
            hirsel_types::Reach::Place(o)
                if !self.arena_place(o)
                    && matches!(&o.root, hirsel_types::OriginRoot::Parameter(
                        hirsel_types::ParameterRef::Body { scope, .. }) if *scope == self.body.parameter_scope) =>
            {
                vec![hirsel_types::Reach::Environment]
            }
            r => vec![r.clone()],
        })
    }
    /// A body row's placement: its arena places and regions only.
    fn placement(&self, relation: &hirsel_types::Relation) -> hirsel_types::Relation {
        relation.retain(|r| match r {
            hirsel_types::Reach::Place(o) => self.arena_place(o),
            hirsel_types::Reach::Region(_) => true,
            hirsel_types::Reach::Environment => false,
        })
    }
    /// Whether a body row's relation places its blocks in an arena or region.
    fn placed(&self, relation: &hirsel_types::Relation) -> bool {
        relation.scoped(|o| self.arena_place(o))
    }
    /// The lifetime sources of a callable's result relation: every reach
    /// except the callable's own Arena parameters (PROV-4).
    fn signature_lifetime(
        &self,
        store: &TypeStore,
        function: &hirsel_types::FunctionType,
        relation: &hirsel_types::Relation,
    ) -> hirsel_types::Relation {
        let Some(heap) = self.signatures.first_heap() else {
            return relation.clone();
        };
        relation.retain(|r| {
            !matches!(r, hirsel_types::Reach::Place(o)
                if hirsel_types::arena_parameter(store, function, o, 0, &heap.arena))
        })
    }
    /// Whether a value of this body row is the registry's Arena.
    fn arena_type(&self, ty: TypeId) -> bool {
        self.signatures
            .first_heap()
            .is_some_and(|h| hirsel_types::is_arena_type(&self.definition.types, ty, &h.arena))
    }
    /// Whether a relation place is an Arena binding (PROV-4): a parameter or a
    /// bound place, typed with the registry's Arena, of this body or of the
    /// body that encloses it (a closure's captured content keeps the arena its
    /// enclosing body named). A field of an Arena token does not denote the
    /// same allocator.
    fn arena_place(&self, origin: &hirsel_types::Origin) -> bool {
        if !origin.fields.is_empty() {
            return false;
        }
        let Some(heap) = self.signatures.first_heap() else {
            return false;
        };
        let arena = |body: &Body, local: LocalId| {
            body.locals
                .get(local.0 as usize)
                .and_then(|l| self.definition.types.get(l.ty))
                .is_some_and(|t| matches!(&t.kind, TypeKind::Named { definition, .. } if *definition == heap.arena))
        };
        std::iter::once(self.body)
            .chain(&self.definition.bodies)
            .any(|body| {
                let local = match &origin.root {
                    hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                        scope,
                        parameter,
                    }) if *scope == body.parameter_scope => {
                        body.parameters.get(parameter.0 as usize).copied()
                    }
                    _ => None,
                }
                .or_else(|| {
                    body.places
                        .iter()
                        .find(|p| {
                            p.projection.is_empty()
                                && p.binding
                                    .as_ref()
                                    .is_some_and(|b| b.root == origin.root && b.fields.is_empty())
                        })
                        .map(|p| p.root)
                });
                local.is_some_and(|local| arena(body, local))
            })
    }
    fn error(&mut self, kind: ViolationKind) {
        let v = Violation {
            body: Some(self.site.body),
            block: self.site.block,
            instruction: self.site.instruction,
            kind,
        };
        if !self.errors.contains(&v) {
            self.errors.push(v);
        }
    }
}

fn owned_type(store: &TypeStore, id: TypeId) -> bool {
    store.id(id.index()) == Some(id)
}

// Check provenance before shared helpers (TypeStore::get asserts on foreign IDs).
// Iterative DFS also bounds recursion before invoking recursive shared importers.
fn graph_valid(store: &TypeStore) -> bool {
    let mut colour = vec![0; store.len()];
    for (root, _) in store.iter() {
        let mut stack = vec![(root, false)];
        while let Some((id, leaving)) = stack.pop() {
            if !owned_type(store, id) {
                return false;
            }
            if leaving {
                colour[id.index()] = 2;
                continue;
            }
            match colour[id.index()] {
                1 => return false,
                2 => continue,
                _ => (),
            }
            colour[id.index()] = 1;
            let row = store.get(id).unwrap();
            if row.erased_captures.is_some()
                && !matches!(
                    row.kind,
                    TypeKind::Function(_)
                        | TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::Iter,
                            ..
                        }
                )
            {
                return false;
            }
            if matches!(row.kind, TypeKind::Error) {
                return false;
            }
            if let TypeKind::Builtin { kind, arguments } = &row.kind {
                use hirsel_types::BuiltinType as B;
                let kinds: &[bool] = match kind {
                    B::Map | B::SortedMap => &[false, false],
                    B::Inline | B::Simd => &[false, true],
                    B::FnPtr => &[],
                    _ => &[false],
                };
                let valid = if *kind == B::FnPtr {
                    !arguments.is_empty()
                        && arguments
                            .iter()
                            .all(|a| matches!(a, GenericArgument::Type(_)))
                } else {
                    arguments.len() == kinds.len()
                        && arguments
                            .iter()
                            .zip(kinds)
                            .all(|(a, size)| *size == matches!(a, GenericArgument::Size(_)))
                };
                if !valid {
                    return false;
                }
            }
            stack.push((id, true));
            for child in row.kind.graph_edges().into_iter().rev() {
                stack.push((child.ty, false));
            }
        }
    }
    true
}

/// Cheap interpreter boundary check. Checks all references, provenance and
/// scopes, but makes no signature, reachability, type or ownership claim. The
/// strict API remains the lowering/publication oracle. One terminator is encoded
/// by Block's type, rather than counted by this function.
pub fn validate_structure(definition: &Definition) -> Vec<Violation> {
    let mut errors = Vec::new();
    let mut add = |body: Option<BodyId>, block: Option<BlockId>, instruction: Option<u32>, kind| {
        let v = Violation {
            body,
            block,
            instruction,
            kind,
        };
        if !errors.contains(&v) {
            errors.push(v);
        }
    };
    if definition.entry.0 as usize >= definition.bodies.len() {
        add(
            None,
            None,
            None,
            ViolationKind::InvalidId {
                arena: "body",
                index: definition.entry.0,
            },
        );
    }
    // Caller body references are checked even at the structural-only interpreter boundary.
    for body in &definition.caller_bodies {
        if body.0 as usize >= definition.bodies.len() {
            add(
                None,
                None,
                None,
                ViolationKind::InvalidId {
                    arena: "body",
                    index: body.0,
                },
            );
        }
    }
    let mut ordinals = BTreeSet::new();
    for (ordinal, body) in &definition.contracts {
        if !ordinals.insert(*ordinal) {
            add(
                None,
                None,
                None,
                ViolationKind::Duplicate {
                    entity: "contract",
                    index: *ordinal,
                },
            );
        }
        if body.0 as usize >= definition.bodies.len() {
            add(
                None,
                None,
                None,
                ViolationKind::InvalidId {
                    arena: "body",
                    index: body.0,
                },
            );
        }
    }
    for (bi, body) in definition.bodies.iter().enumerate() {
        let bid = Some(BodyId(bi as u32));
        let mut refs = Vec::new();
        refs.push((None, None, "block", body.entry.0));
        let mut type_roots = vec![body.result];
        let mut parameters = BTreeSet::new();
        for p in &body.parameters {
            refs.push((None, None, "local", p.0));
            if !parameters.insert(p.0) {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::Duplicate {
                        entity: "parameter",
                        index: p.0,
                    },
                );
            }
            if let Some(l) = body.locals.get(p.0 as usize) {
                if !matches!(l.storage, Storage::Parameter | Storage::Capture) {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::WrongKind {
                            entity: "parameter",
                            index: p.0,
                        },
                    );
                }
            }
        }
        for (i, l) in body.locals.iter().enumerate() {
            type_roots.push(l.ty);
            refs.push((None, None, "scope", l.scope.0));
            if l.storage == Storage::Parameter && !parameters.contains(&(i as u32)) {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::WrongKind {
                        entity: "unbound parameter",
                        index: i as u32,
                    },
                );
            }
            // Only a closure body has an environment; flow starts captures
            // live, so an entry capture would read storage nobody supplied.
            if l.storage == Storage::Capture && bi == definition.entry.0 as usize {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::WrongKind {
                        entity: "entry capture",
                        index: i as u32,
                    },
                );
            }
        }
        for p in &body.places {
            type_roots.push(p.ty);
            refs.push((None, None, "local", p.root.0));
            for projection in &p.projection {
                if let Projection::Index(l) = projection {
                    refs.push((None, None, "local", l.0));
                }
            }
        }
        for (i, s) in body.scopes.iter().enumerate() {
            if let Some(p) = s.parent {
                refs.push((None, None, "scope", p.0));
            }
            let mut seen = BTreeSet::new();
            let mut next = Some(ScopeId(i as u32));
            while let Some(id) = next {
                if !seen.insert(id.0) {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::ScopeCycle(ScopeId(i as u32)),
                    );
                    break;
                }
                next = body.scopes.get(id.0 as usize).and_then(|s| s.parent);
            }
        }
        let mut loans = BTreeSet::new();
        let mut loan_refs: Vec<_> = body
            .places
            .iter()
            .filter_map(|p| p.loan.map(|id| (None, None, id)))
            .collect();
        for (b, block) in body.blocks.iter().enumerate() {
            let block_id = Some(BlockId(b as u32));
            for (i, inst) in block.instructions.iter().enumerate() {
                let ins = Some(i as u32);
                refs.push((block_id, ins, "cleanup", inst.on_failure.0));
                match &inst.kind {
                    InstructionKind::Defuse { place } => {
                        refs.push((block_id, ins, "place", place.0))
                    }
                    InstructionKind::BindOrigin { source, place, .. } => {
                        refs.push((block_id, ins, "place", place.0));
                        refs.push((block_id, ins, "place", source.0));
                    }
                    InstructionKind::BindFields { source, fields } => {
                        refs.push((block_id, ins, "place", source.0));
                        for field in fields {
                            refs.push((block_id, ins, "place", field.0));
                        }
                    }
                    InstructionKind::EndOrigin(place) => {
                        refs.push((block_id, ins, "place", place.0))
                    }
                    InstructionKind::StorageLive(l) => refs.push((block_id, ins, "local", l.0)),
                    InstructionKind::EndScope(scope) => {
                        refs.push((block_id, ins, "scope", scope.0))
                    }
                    InstructionKind::StorageDead { place, .. } => {
                        refs.push((block_id, ins, "place", place.0))
                    }
                    InstructionKind::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } => {
                        refs.push((block_id, ins, "place", destination.0));
                        refs.push((block_id, ins, "place", value.place.0));
                        refs.push((block_id, ins, "place", displaced.0));
                    }
                    InstructionKind::Assign { destination, value } => {
                        refs.push((block_id, ins, "place", destination.0));
                        refs.push((block_id, ins, "rvalue", value.0));
                    }
                    InstructionKind::BeginLoan { id, place, .. } => {
                        if !loans.insert(id.0) {
                            add(
                                bid,
                                block_id,
                                ins,
                                ViolationKind::Duplicate {
                                    entity: "loan",
                                    index: id.0,
                                },
                            );
                        }
                        refs.push((block_id, ins, "place", place.0));
                    }
                    InstructionKind::EndLoan { loan: id, .. } => {
                        loan_refs.push((block_id, ins, *id))
                    }
                    InstructionKind::Drop { place, .. } => {
                        refs.push((block_id, ins, "place", place.0))
                    }
                    InstructionKind::RunCleanup(id) => refs.push((block_id, ins, "cleanup", id.0)),
                    InstructionKind::Check { condition, .. } => {
                        refs.push((block_id, ins, "place", condition.place.0))
                    }
                    InstructionKind::Snapshot { destination, value } => {
                        refs.push((block_id, ins, "place", destination.0));
                        refs.push((block_id, ins, "place", value.place.0));
                    }
                }
            }
            for target in body.successors(&block.terminator) {
                refs.push((block_id, None, "block", target.0));
            }
            let mut operand = |o: &Operand| refs.push((block_id, None, "place", o.place.0));
            match &block.terminator {
                Terminator::Match { table, from } => {
                    if let Some(t) = body.matches.get(table.0 as usize) {
                        if *from as usize > t.arms.len() {
                            add(bid, block_id, None, ViolationKind::MatchCoverage);
                        }
                    } else {
                        add(
                            bid,
                            block_id,
                            None,
                            ViolationKind::InvalidId {
                                arena: "match table",
                                index: table.0,
                            },
                        );
                    }
                }
                Terminator::Branch { condition, .. } => operand(condition),
                Terminator::Switch { value, arms, .. } => {
                    operand(value);
                    let mut tags = BTreeSet::new();
                    for (tag, _) in arms {
                        if !tags.insert(*tag) {
                            add(
                                bid,
                                block_id,
                                None,
                                ViolationKind::Duplicate {
                                    entity: "switch arm",
                                    index: *tag,
                                },
                            );
                        }
                    }
                }
                Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    on_failure,
                } => {
                    if let Callee::Indirect { value: o, .. } = callee {
                        operand(o);
                    }
                    let (Callee::Definition {
                        arguments: generic_arguments,
                        ..
                    }
                    | Callee::Indirect {
                        arguments: generic_arguments,
                        ..
                    }) = callee;
                    type_roots.extend(generic_arguments.iter().filter_map(|a| {
                        if let GenericArgument::Type(t) = a {
                            Some(*t)
                        } else {
                            None
                        }
                    }));
                    for a in arguments {
                        operand(&a.value);
                    }
                    if let Some(r) = returns {
                        refs.push((block_id, None, "place", r.destination.0));
                    }
                    refs.push((block_id, None, "cleanup", on_failure.0));
                }
                Terminator::Return { value, cleanup } => {
                    operand(value);
                    refs.push((block_id, None, "cleanup", cleanup.0));
                }
                Terminator::Fail { cleanup, .. } => {
                    refs.push((block_id, None, "cleanup", cleanup.0))
                }
                _ => (),
            }
        }
        for (table_index, table) in body.matches.iter().enumerate() {
            if !body.blocks.iter().any(|b| matches!(b.terminator, Terminator::Match { table, from: 0 } if table.0 as usize == table_index)) {
                add(bid, None, None, ViolationKind::WrongKind { entity: "match table without entry", index: table_index as u32 });
            }
            refs.push((None, None, "place", table.scrutinee.0));
            let mut used = vec![false; table.patterns.len()];
            for arm in &table.arms {
                refs.push((None, None, "block", arm.target.0));
                if let Some(guard) = arm.guard {
                    refs.push((None, None, "block", guard.0));
                }
                if let Some(u) = used.get_mut(arm.pattern.0 as usize) {
                    *u = true;
                } else {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::InvalidId {
                            arena: "match pattern",
                            index: arm.pattern.0,
                        },
                    );
                }
            }
            for (index, pattern) in table.patterns.iter().enumerate().rev() {
                refs.push((None, None, "place", pattern.place.0));
                let mut fields = BTreeSet::new();
                for (field, child) in pattern.kind.fields() {
                    if !fields.insert(*field) {
                        add(
                            bid,
                            None,
                            None,
                            ViolationKind::Duplicate {
                                entity: "pattern field",
                                index: *field,
                            },
                        );
                    }
                    if child.0 as usize >= index {
                        add(
                            bid,
                            None,
                            None,
                            ViolationKind::InvalidId {
                                arena: "pattern child",
                                index: child.0,
                            },
                        );
                    } else if used[index] {
                        used[child.0 as usize] = true;
                    }
                }
            }
            for (index, used) in used.into_iter().enumerate() {
                if !used {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::WrongKind {
                            entity: "unused match pattern",
                            index: index as u32,
                        },
                    );
                }
            }
        }
        for rvalue in &body.rvalues {
            for o in typing::operands(rvalue) {
                refs.push((None, None, "place", o.place.0));
            }
            match rvalue {
                Rvalue::Closure { body, .. } => refs.push((None, None, "body", body.0)),
                Rvalue::Shape(t) => type_roots.push(*t),
                _ => (),
            }
        }
        for cleanup in &body.cleanups {
            if let Some(head) = cleanup.head {
                if head.0 as usize >= body.cleanup_nodes.len() {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::InvalidId {
                            arena: "cleanup node",
                            index: head.0,
                        },
                    );
                }
            }
        }
        for (index, node) in body.cleanup_nodes.iter().enumerate() {
            if let Some(parent) = node.parent {
                if parent.0 as usize >= index {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::InvalidId {
                            arena: "cleanup parent",
                            index: parent.0,
                        },
                    );
                }
            }
        }
        for c in &body.cleanup_nodes {
            for action in [&c.action] {
                match action {
                    CleanupAction::Drop { place, .. }
                    | CleanupAction::CloseArena(place)
                    | CleanupAction::JoinTasks { scope: place, .. } => {
                        refs.push((None, None, "place", place.0))
                    }
                    CleanupAction::EndLoan { loan: id, .. } => loan_refs.push((None, None, *id)),
                    CleanupAction::ReleaseRegion(_) => (),
                }
            }
        }
        for (block, inst, id) in loan_refs {
            if !loans.contains(&id.0) {
                add(
                    bid,
                    block,
                    inst,
                    ViolationKind::InvalidId {
                        arena: "loan",
                        index: id.0,
                    },
                );
            }
        }
        for (block, inst, arena, index) in refs {
            let len = match arena {
                "local" => body.locals.len(),
                "place" => body.places.len(),
                "scope" => body.scopes.len(),
                "block" => body.blocks.len(),
                "rvalue" => body.rvalues.len(),
                "cleanup" => body.cleanups.len(),
                "body" => definition.bodies.len(),
                _ => unreachable!(),
            };
            if index as usize >= len {
                add(bid, block, inst, ViolationKind::InvalidId { arena, index });
            }
        }
        for t in type_roots {
            if !owned_type(&definition.types, t) {
                add(bid, None, None, ViolationKind::InvalidType(t));
            }
        }
    }
    // Check shared graph edges once; each body's root IDs were checked above.
    for (_, row) in definition.types.iter() {
        for edge in row.kind.graph_edges() {
            if !owned_type(&definition.types, edge.ty) {
                add(None, None, None, ViolationKind::InvalidType(edge.ty));
            }
        }
    }

    errors
}

// The checker's tests run through its public API in tests/it. These unit tests drive
// private internals directly and share the integration suite's fixture builders.
#[cfg(test)]
extern crate self as hirsel_validate;
#[cfg(test)]
mod internal_tests;
#[cfg(test)]
#[path = "../tests/it/support.rs"]
#[allow(dead_code)] // the unit tests use a few of the builders
mod support;

/// A row whose copy is an independent global handle: a `Shared[T]` root
/// (MEM-11) or a run-owned `Mutex` or `Atomic` table entry (MODE-4).
pub(crate) fn handle_root(store: &TypeStore, ty: TypeId) -> bool {
    matches!(
        store.get(ty).map(|row| &row.kind),
        Some(TypeKind::Builtin {
            kind: hirsel_types::BuiltinType::Shared
                | hirsel_types::BuiltinType::Mutex
                | hirsel_types::BuiltinType::Atomic,
            ..
        })
    )
}
