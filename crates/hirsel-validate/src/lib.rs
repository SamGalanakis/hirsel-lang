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
mod row_input;
mod scope_locals;
mod scoped_transport;
mod scratch;
mod sealed;
mod serialization;
mod shared;
mod static_data;
mod structure;
use row_input::{ReadBody, ReadDefinition, ReadMatch};
pub use structure::validate_structure_rows;
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
    let definition = ReadDefinition::live(definition);
    let mut validation = validate_input(&definition, t, &reader);
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

/// Independently validate live or checked indexed TIR. Missing immutable source
/// or type bindings are a read failure. Every semantic obligation and recorded
/// signature read uses the same checker as ordinary live input.
pub fn validate_rows(
    definition: hirsel_product_codec::TirView<'_>,
    t: &TypeInterner,
    signatures: &dyn SignatureLookup,
    reads: &mut Vec<VRead>,
) -> Option<Validation> {
    let definition = ReadDefinition::new(definition)?;
    let reader = Reader::new(t, signatures);
    let mut validation = validate_input(&definition, t, &reader);
    validation.violations.extend(reader.input_errors());
    reader.finish(reads);
    Some(validation)
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
    let definition = ReadDefinition::live(definition);
    let mut result = validate_input(&definition, t, &reader);
    if result.violations.is_empty() {
        result
            .violations
            .extend(scoped_transport::check(&definition, t, &reader));
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
            hirsel_types::closure(t, hirsel_types::signature_types(t, signature)).iter(),
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
    definition: &ReadDefinition<'_>,
    t: &TypeInterner,
    signatures: &Reader<'_>,
) -> Validation {
    // V's own structural memo: it shares no memo or verdict with inference.
    let memo = TypeMemo::new();
    let mut errors = structure::validate_bound(definition, t);
    if !errors.is_empty() {
        return Validation { violations: errors };
    }
    if !graph_valid(t, definition.types.iter()) || !carrier_rows_valid(t, definition.types.iter(), signatures) || definition.types.iter().map(|&id| t.row(id)).any(|t| {
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
struct Checker<'a, 'input> {
    definition: &'a ReadDefinition<'input>,
    body: &'a ReadBody<'input>,
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
impl<'a, 'input> Checker<'a, 'input> {
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
        let arena = |body: &ReadBody<'_>, local: LocalId| {
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
fn carrier_rows_valid<'a>(
    t: &TypeInterner,
    mut rows: impl Iterator<Item = &'a TypeId>,
    signatures: &Reader<'_>,
) -> bool {
    rows.all(|&id| match (&t.row(id).kind, &t.row(id).erased_captures) {
        (TypeKind::Named { definition, .. }, Some(_)) => signatures.get(*definition).is_some_and(
            |s| matches!(&s.kind, hirsel_types::SignatureKind::Type(d) if d.captures.is_some()),
        ),
        _ => true,
    })
}
/// V's own walk of a closure table: every row and edge is in the table (the
/// caller checked membership), and the graph is acyclic with no recovery row.
/// Iterative DFS also bounds recursion before invoking recursive shared helpers.
fn graph_valid<'a>(t: &TypeInterner, rows: impl Iterator<Item = &'a TypeId> + Clone) -> bool {
    let members: crate::hashed::HashSet<TypeId> = rows.clone().copied().collect();
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
/// scopes, but makes no signature, reachability, type or ownership claim.
pub fn validate_structure(definition: &Definition, t: &TypeInterner) -> Vec<Violation> {
    structure::validate_structure_rows(hirsel_product_codec::TirView::Live(definition), t)
        .expect("live TIR rows are present")
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
