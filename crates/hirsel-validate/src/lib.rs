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

use hashed::{HashMap, HashSet};
use hirsel_intern::work::{self, Counter};
use hirsel_syntax::{DefId, DefPath};
use hirsel_tir::*;
use hirsel_types::{
    Destination, GenericArgument, QueryError, Signature, TypeId, TypeInterner, TypeKind, TypeMemo,
    UseKind,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod arena_scope;
mod bit_rows;
mod hashed {
    pub(super) type HashMap<K, V> = std::collections::HashMap<K, V, hirsel_intern::IdHasher>;
    pub(super) type HashSet<K> = std::collections::HashSet<K, hirsel_intern::IdHasher>;
}
mod cleanup_proof;
mod cpu;
mod declaration;
mod effects;
mod flow;
mod flow_origins;
mod generics;
mod indexed;
mod input;
mod views;
pub use views::{
    CheckedEnvironment, EnvironmentStamp, PackageViews, SignatureInputs, View, ViewTable,
};
mod instances;
mod invocations;
pub use invocations::invocation_coverage;
mod matching;
mod origin_dependencies;
mod origin_types;
mod proofs;
mod receipts;
pub use receipts::{
    AcceptedArtifact, Artifact, ExecutionReceipt, Installation, ReplayEnvironment,
    SourceInvocation, StoredDefinitionOutput, VerifiedDefinition, VerifiedDefinitionOutput,
    VerifiedInvocations, VerifierCache, VerifierRows, validate_cached, validate_definition,
    validate_with_receipt,
};
mod protocol;
mod scope_locals;
mod scoped_transport;
mod scratch;
mod sealed;
mod serialization;
mod shared;
mod static_data;
pub mod storage;
mod tail;
mod typing;
mod variant_brands;
use input::Reader;
pub use input::{
    IdentifiedSignature, SignatureLookup, SignatureMap, UniverseErrors, VRead, ValidatedSignatures,
    ValidationDigest, ViewInput, ViewVerdict, check_view, digest, errors_digest, read_holds,
};
pub use instances::{
    InstanceViolation, validate_instance_bodies, validate_instance_catalog, validate_instance_uses,
};
pub use matching::{MAX_MATCH_WORK, tooling_limit_error};

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
/// Every view V reads is appended to `reads` (`input`).
pub fn validate(
    definition: &Definition,
    t: &TypeInterner,
    signatures: &dyn SignatureLookup,
    reads: &mut Vec<VRead>,
) -> Validation {
    // Opt-in per-definition profiling, independent of the driver's phase
    // accounting (including V invoked by import and compile-time execution).
    let started = profile_start();
    let reader = Reader::new(t, signatures);
    let mut validation = validate_input(definition, t, &reader);
    validation.violations.extend(reader.input_errors());
    reader.finish(reads);
    if let Some(started) = started {
        eprintln!(
            "hirsel-validate-profile\t{}\t{}\t{}\t{}",
            definition.path,
            started.elapsed().as_nanos(),
            definition.types.len(),
            validation.violations.len()
        );
    }
    validation
}

/// Independent scoped proof qualification in addition to ordinary V acceptance.
/// Unsupported borrowed generic/erased forwarding keeps its safe heap behavior
/// under `validate`, but cannot satisfy this stronger proof boundary.
pub fn qualify_scoped_transport(
    definition: &Definition,
    t: &TypeInterner,
    signatures: &dyn SignatureLookup,
    reads: &mut Vec<VRead>,
) -> Validation {
    let reader = Reader::new(t, signatures);
    let mut result = validate_input(definition, t, &reader);
    if result.violations.is_empty() {
        result
            .violations
            .extend(scoped_transport::check(definition, t, &reader));
    }
    result.violations.extend(reader.input_errors());
    reader.finish(reads);
    result
}

fn profile_start() -> Option<std::time::Instant> {
    static PROFILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    PROFILE
        .get_or_init(|| std::env::var_os("HIRSEL_VALIDATE_PROFILE").is_some())
        .then(std::time::Instant::now)
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
    t: &TypeInterner,
    d: DefId,
    signature: &Signature,
    signatures: &Reader<'_>,
) -> Option<Violation> {
    work::count(Counter::ValidateSignatureRows);
    let path = d.path();
    // A view's verdict is its own: it shares no memo with a body or with I.
    let memo = TypeMemo::new();
    if d != signature.definition
        || !typing::signature_valid(t, signature)
        || !carrier_rows_valid(
            t,
            &hirsel_types::closure(t, hirsel_types::signature_types(t, signature)),
            signatures,
        )
        || !declaration::valid(t, signature, signatures)
        || !protocol::declaration(t, &memo, signature, signatures)
        || !protocol::selectors(t, &memo, signature, signatures)
    {
        return Some(invalid_signature(path));
    }
    match serialization::declaration(t, &memo, signature, signatures) {
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
fn validate_input(
    definition: &Definition,
    t: &TypeInterner,
    signatures: &Reader<'_>,
) -> Validation {
    // V's own structural memo: it shares no memo or verdict with inference.
    let memo = TypeMemo::new();
    let mut errors = validate_structure(definition, t);
    if !errors.is_empty() {
        return Validation { violations: errors };
    }
    if !graph_valid(t, &definition.types) || !carrier_rows_valid(t, &definition.types, signatures) || definition.types.iter().map(|&id| t.row(id)).any(|t| {
        t.allocation_target.is_some() && !matches!(&t.kind, TypeKind::Named { definition, .. } if signatures.get(*definition).is_some_and(|s| t.is_heap_target(&s.metadata.roles)))
    }) {
        errors.push(Violation { body: None, block: None, instruction: None, kind: ViolationKind::InvalidTypeGraph });
    }
    if !errors.is_empty() {
        return Validation { violations: errors };
    }
    errors.extend(signatures.universe_errors().iter().cloned());
    // Reconstruct tracked bodies from the declaration and the entry's own clause producers.
    // Contracts inside ordinary nested closures cannot inherit the enclosing declaration.
    let mut caller_bodies = Vec::new();
    if signatures
        .get(definition.path)
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
    errors.extend(protocol::receipts(t, &memo, definition, signatures));
    errors.extend(protocol::unproved_promises(t, definition, signatures));
    if !errors.is_empty() {
        return Validation { violations: errors };
    }
    let Some(declarations) = typing::declaration_tables(t, definition, signatures) else {
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
    let scratch = scratch::Scratch::new(definition.bodies.len());
    for (index, body) in definition.bodies.iter().enumerate() {
        let mut checker = Checker {
            definition,
            body,
            t,
            memo: &memo,
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
            unique_parts_cache: Default::default(),
        };
        let mut witnessed = BTreeSet::new();
        for tail in &body.tail_sites {
            let property = |ty, counted| {
                let mut lookup = |d: DefId| {
                    Ok::<_, hirsel_types::QueryError>(
                        signatures.get(d).cloned().map(std::sync::Arc::new),
                    )
                };
                if counted {
                    hirsel_types::structural_equality(
                        t,
                        ty,
                        hirsel_types::Bound::Data,
                        &declarations[index],
                        &mut lookup,
                    )
                    .unwrap_or_else(|error| checker.proof_failure(error, false))
                } else {
                    hirsel_types::structural_property(
                        t,
                        ty,
                        hirsel_types::StructuralProperty::Plain,
                        &declarations[index],
                        &mut lookup,
                    )
                    .unwrap_or_else(|error| checker.proof_failure(error, false))
                }
            };
            let ordinary =
                |p: &hirsel_syntax::DefPath| crate::tail::ordinary_constructor(signatures, p);
            let verified = match tail.kind {
                TailKind::SelfTail => crate::tail::verify_self_tail(
                    t,
                    definition,
                    BodyId(index as u32),
                    tail,
                    &property,
                ),
                TailKind::ConstructorHole(_) => crate::tail::verify_constructor_hole(
                    t,
                    definition,
                    BodyId(index as u32),
                    tail,
                    &property,
                    &ordinary,
                ),
            };
            if !witnessed.insert(tail.call_block.0) || !verified {
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
    // A private-summary mismatch rejects the definition, but does not make
    // its already type-checked bodies unsafe to analyze. Keep independent
    // source, loan and cleanup violations visible beside the forged claim.
    let inferred_header = signatures.get(definition.path);
    if inferred_header.map_or(!definition.inferred_unique.is_empty(), |s| {
        s.metadata.inferred_unique != definition.inferred_unique
    }) {
        errors.push(Violation {
            body: None,
            block: None,
            instruction: None,
            kind: ViolationKind::TypeMismatch {
                entity: "unproved private uniqueness summary",
            },
        });
    }
    // Closure environments participate in the same finite flow proof. Compute
    // entry snapshots before reporting, so body arena order has no significance.
    // A single entry body with no closure producer has no environment edges.
    // Its ordinary reporting pass proves the same flow below; there is no
    // capture snapshot to discover in a preliminary copy of that fixed point.
    let entry_only = definition.bodies.len() == 1
        && definition.contracts.is_empty()
        && definition.bodies[0]
            .rvalues
            .iter()
            .all(|value| !matches!(value, Rvalue::Closure { .. }));
    let mut environments = BTreeMap::<u32, Vec<flow_origins::CapturedOrigin>>::new();
    let mut pending = VecDeque::new();
    environments.insert(definition.entry.0, vec![]);
    pending.push_back(definition.entry.0);
    for (_, body) in &definition.contracts {
        environments.insert(body.0, vec![]);
        pending.push_back(body.0);
    }
    while let Some(index) = pending.pop_front() {
        if entry_only {
            continue;
        }
        let body = &definition.bodies[index as usize];
        let mut checker = Checker {
            definition,
            body,
            t,
            memo: &memo,
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
            unique_parts_cache: Default::default(),
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
            t,
            memo: &memo,
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
            unique_parts_cache: Default::default(),
        };
        if let Some(captures) = environments.get(&(index as u32)) {
            checker.flow(captures, true);
            // Preserve the preliminary pass's early tooling-limit outcome.
            if entry_only && !checker.proof_errors.borrow().is_empty() {
                return Validation {
                    violations: checker.proof_errors.into_inner(),
                };
            }
            if index as u32 == definition.entry.0 {
                for parameter in unique_promises(t, signatures.get(definition.path), signatures) {
                    let mut proof = Checker {
                        definition,
                        body,
                        t,
                        memo: &memo,
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
                        unique_parts_cache: Default::default(),
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
    errors.extend(scratch.closure_receipts.into_inner().into_iter().map(
        |(body, block, instruction)| Violation {
            body: Some(BodyId(body)),
            block: block.map(BlockId),
            instruction,
            kind: ViolationKind::TypeMismatch {
                entity: "closure sendability",
            },
        },
    ));
    if errors.is_empty() {
        match invocations::coverage_with_declarations(t, definition, signatures, &declarations) {
            Ok(coverage) => {
                for (body, block, _, restart) in coverage {
                    // A restart is a self tail or a constructor-tail site; the
                    // witness must be published, whichever it is (LANG-10).
                    if restart
                        && !definition.bodies[body as usize]
                            .tail_sites
                            .iter()
                            .any(|site| site.owner_body.0 == body && site.call_block.0 == block)
                    {
                        errors.push(Violation {
                            body: Some(BodyId(body)),
                            block: Some(BlockId(block)),
                            instruction: None,
                            kind: ViolationKind::WrongKind {
                                entity: "missing tail witness",
                                index: block,
                            },
                        });
                    }
                }
            }
            Err(_) => errors.push(Violation {
                body: None,
                block: None,
                instruction: None,
                kind: ViolationKind::InvalidTypeGraph,
            }),
        }
    }
    if errors.is_empty() && !definition.inferred_unique.nonretaining.is_empty() {
        errors.extend(scoped_transport::check(definition, t, signatures));
    }
    Validation { violations: errors }
}

/// Explicit and SEM-3-implied `@preserves_unique` selections (MODE-2).
fn unique_promises(
    t: &TypeInterner,
    signature: Option<&Signature>,
    signatures: &Reader<'_>,
) -> Vec<hirsel_types::ParamId> {
    let Some(hirsel_types::SignatureKind::Function(f)) = signature.map(|s| &s.kind) else {
        return Vec::new();
    };
    let mut promised: Vec<_> = signature
        .into_iter()
        .flat_map(|s| s.metadata.inferred_unique.preserves_unique.iter().copied())
        .collect();
    promised.extend(
        f.state_promises
            .iter()
            .filter(|p| p.preserve_unique && p.selector.fields.is_empty())
            .filter_map(|p| match p.selector.parameter {
                hirsel_types::ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } => Some(parameter),
                _ => None,
            })
            .collect::<Vec<_>>(),
    );
    promised.sort();
    promised.dedup();
    if signature.is_some_and(|s| format_association(t, s, signatures))
        && !promised.contains(&hirsel_types::ParamId(1))
    {
        promised.push(hirsel_types::ParamId(1));
    }
    promised
}

/// Reconstruct SEM-3's implied promise from declaration snapshots, independent
/// of source annotations and of the producer's ownership decisions.
fn format_association(t: &TypeInterner, signature: &Signature, signatures: &Reader<'_>) -> bool {
    let hirsel_types::SignatureKind::Function(f) = &signature.kind else {
        return false;
    };
    if f.foreign.is_some() {
        return false;
    }
    let Some(receiver) = f.parameters.first() else {
        return false;
    };
    let Some(TypeKind::Named { definition, .. }) = t.get(receiver.ty).map(|r| &r.kind) else {
        return false;
    };
    signatures.get(*definition).is_some_and(|s| {
        matches!(&s.kind, hirsel_types::SignatureKind::Type(t)
            if t.format.as_ref() == Some(signature.definition.path()))
    })
}

/// Lowering-test helper: make invariant failures readable at their publication
/// boundary, without introducing a dependency on the interpreter.
#[track_caller]
pub fn assert_valid(definition: &Definition, t: &TypeInterner, signatures: &dyn SignatureLookup) {
    let result = validate(definition, t, signatures, &mut vec![]);
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
    /// The session interner: the definition's rows, signature rows and V's
    /// own temporaries.
    t: &'a TypeInterner,
    /// V's structural memo, never inference's (design §1a).
    memo: &'a TypeMemo,
    signatures: &'a Reader<'a>,
    declarations: &'a [hirsel_types::GenericDeclarations],
    scratch: &'a scratch::Scratch,
    contract: bool,
    site: Site,
    errors: Vec<Violation>,
    captured: BTreeMap<u32, Vec<flow_origins::CapturedOrigin>>,
    returned: BTreeMap<u32, hirsel_types::Type>,
    static_data: Option<static_data::Values>,
    proof_errors: std::cell::RefCell<Vec<Violation>>,
    unique_parts_cache: std::cell::RefCell<std::collections::HashMap<TypeId, bool>>,
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
    /// Run `f` with a contextual memo over `generics` whose lookups are V's
    /// own recorded signature reads.
    fn context<R>(
        &self,
        generics: &hirsel_types::GenericDeclarations,
        f: impl FnOnce(&mut hirsel_types::ContextMemo<'_>) -> R,
    ) -> R {
        let mut lookup = |d: DefId| {
            Ok::<_, QueryError>(self.signatures.get(d).cloned().map(std::sync::Arc::new))
        };
        f(&mut hirsel_types::ContextMemo::new(
            self.t,
            generics,
            &mut lookup,
        ))
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
        store: &TypeInterner,
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
            .is_some_and(|h| hirsel_types::is_arena_type(self.t, ty, &h.arena))
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
                .and_then(|l| self.t.get(l.ty))
                .is_some_and(|t| matches!(&t.kind, TypeKind::Named { definition, .. } if *definition.path() == heap.arena))
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

/// Only an erased carrier declaration gives a nominal row erased evidence (CAP-2).
fn carrier_rows_valid(t: &TypeInterner, rows: &[TypeId], signatures: &Reader<'_>) -> bool {
    rows.iter().all(|&id| match (&t.row(id).kind, &t.row(id).erased_captures) {
        (TypeKind::Named { definition, .. }, Some(_)) => signatures
            .get(*definition)
            .is_some_and(|s| matches!(&s.kind, hirsel_types::SignatureKind::Type(d) if d.captures.is_some())),
        _ => true,
    })
}
/// V's own walk of a closure table: every row and edge is in the table (the
/// caller checked membership), and the graph is acyclic with no recovery row.
/// Iterative DFS also bounds recursion before invoking recursive shared helpers.
fn graph_valid(t: &TypeInterner, rows: &[TypeId]) -> bool {
    let members: crate::hashed::HashSet<TypeId> = rows.iter().copied().collect();
    let mut colour = crate::hashed::HashMap::<TypeId, u8>::default();
    for &root in rows {
        let mut stack = vec![(root, false)];
        while let Some((id, leaving)) = stack.pop() {
            if !members.contains(&id) || !t.contains(id) {
                return false;
            }
            if leaving {
                colour.insert(id, 2);
                continue;
            }
            match colour.get(&id) {
                Some(1) => return false,
                Some(2) => continue,
                _ => (),
            }
            colour.insert(id, 1);
            let row = t.row(id);
            // A nominal row's evidence is checked against its declaration by
            // `carrier_rows_valid`.
            if row.erased_captures.is_some()
                && !matches!(
                    row.kind,
                    TypeKind::Function(_)
                        | TypeKind::Named { .. }
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
pub fn validate_structure(definition: &Definition, t: &TypeInterner) -> Vec<Violation> {
    // TypeIds carry no provenance: a row belongs to this definition exactly
    // when its closure table lists it.
    let members: crate::hashed::HashSet<TypeId> = definition
        .types
        .iter()
        .copied()
        .filter(|&id| t.contains(id))
        .collect();
    let owned_type = |id: TypeId| members.contains(&id);
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
        let mut cursor_carriers = BTreeSet::new();
        for cursor in &body.borrow_cursors {
            refs.push((None, None, "local", cursor.carrier.0));
            refs.push((None, None, "rvalue", cursor.initializer.0));
            for place in cursor.cover.iter().flatten() {
                refs.push((None, None, "place", place.0));
            }
            let declarations = body
                .blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .filter(|instruction| {
                    matches!(instruction.kind, InstructionKind::Assign { destination, value }
                    if value == cursor.initializer && body.places.get(destination.0 as usize)
                        .is_some_and(|p| p.root == cursor.carrier && p.projection.is_empty()))
                })
                .count();
            if !cursor_carriers.insert(cursor.carrier.0) || declarations != 1 {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::WrongKind {
                        entity: "borrow cursor declaration",
                        index: cursor.carrier.0,
                    },
                );
            }
        }
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
                    // CT-4 placement: a call charge ending its block precedes a
                    // source call (never a compiler contract call), and an
                    // iteration charge opens a loop body block. One event may
                    // have several sites (an or-pattern arm lowers per alternative).
                    InstructionKind::Charge(event) => {
                        let misplaced = match event.kind {
                            hirsel_tir::SourceEventKind::Call => {
                                i + 1 == block.instructions.len()
                                    && !matches!(
                                        block.terminator,
                                        Terminator::Call {
                                            callee: Callee::Definition { .. }
                                                | Callee::Indirect {
                                                    forward_caller: false,
                                                    ..
                                                },
                                            ..
                                        }
                                    )
                            }
                            hirsel_tir::SourceEventKind::Iteration => {
                                i != 0 || b == body.entry.0 as usize
                            }
                        };
                        if misplaced {
                            add(
                                bid,
                                block_id,
                                ins,
                                ViolationKind::WrongKind {
                                    entity: "charge placement",
                                    index: event.source,
                                },
                            );
                        }
                    }
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
        for ty in type_roots {
            if !owned_type(ty) {
                add(bid, None, None, ViolationKind::InvalidType(ty));
            }
        }
    }
    // Check the table's graph edges once; each body's root IDs were checked above.
    for &id in definition.types.iter() {
        if !owned_type(id) {
            add(None, None, None, ViolationKind::InvalidType(id));
            continue;
        }
        for edge in t.row(id).kind.graph_edges() {
            if !owned_type(edge.ty) {
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
pub(crate) fn handle_root(store: &TypeInterner, ty: TypeId) -> bool {
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
