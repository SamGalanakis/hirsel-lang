//! Independent stage validators.
//!
//! `validate` checks one raw `LirUnit<S>` against the layout table and the
//! operation manifest and, only if every check passes, seals it into a
//! `ValidatedUnit<S>` with a receipt. Nothing here reads a pass's liveness,
//! availability, shape or uniqueness answer, or `OwnershipFacts`: every fact is
//! re-derived from the unit's operations and the layout records, as the TIR
//! validator re-derives its facts from TIR. Destruction comes from layout drop
//! programs. The checks are ordinary code, so they run in release builds too.
//!
//! The phases:
//! - `structure`: vocabulary, arity and types, SSA definitions and dominance,
//!   block arguments, address typing and preparation, edges and block kinds,
//!   placement and the static facts later phases read (`Facts`).
//! - `flow`: the CFG over normal and failure edges, dominators, the loop
//!   nesting forest's feedback edges and the worklist solver. Convergence is a
//!   fixpoint of monotone transfers over finite lattices; there is no
//!   iteration cutoff.
//! - `ownership`: slot liveness, initialization masks and reaching versions,
//!   loans and borrowed views, compiler-local regions, the cleanup nesting
//!   order, and the exact holder-credit replay over normal and failure edges.
//! - `tokens`: reuse-token provenance, compatibility and feedback edges.
//! - `ready`: what the Ready stage must have discharged.
//! - `discharge`: PS-13, a discharged Value unit against its reference
//!   import (`validate_discharge`).
//!
//! Readings this module relies on:
//! - Cleanup order reads scope nesting from `LirUnit::scopes`: a release in a
//!   scope follows releases in that scope or its descendants only.
//! - Temporaries of one scope release in descending slot order, as names do.
//! - A `mut` call defines its root slot's write-back version on both of its
//!   edges and invalidates views of that storage.
//! - A capture slot's environment content is named by its entry in
//!   `LirUnit::captures`, so capture reads are version-checked like any slot.
//! - A failing operation has disposed of the owned operands it takes (its
//!   fields, token, captures or replacement value), as call entry transfers
//!   Take actuals, and commits nothing else; release, reset, drop and region
//!   exit finish their cleanup before failing.
//! - Every block is reachable, and every edge and failure edge is named by
//!   exactly one site.
//! - A Plain static (a `Const`, `ReadStatic` or `FunctionRef` result held
//!   Plain: a literal slice) is backed by an immortal static block: consuming
//!   it needs no credit, and it is never a reuse donor.
mod aliasing;
mod linkage;
mod products;
pub use linkage::{CheckedLinkage, check_arena_scoped_links};
pub use products::{NativeCache, NativeEnvironment, NativeOutput};
mod arena;
pub use arena::check_calls as check_result_arena_calls;
mod declaration_reads;
mod discharge;
pub use discharge::validate_discharge;
mod flat_open;
mod flat_walk;
mod flow;
mod holes;
mod ownership;
mod ready;
mod receipt_custody;
mod scoped_transport;
mod sealed;
mod sealed_payload;
mod structure;
mod tokens;
pub use scoped_transport::{
    ScopedQualification, check_calls as check_scoped_calls, check_heap_links as check_scoped_links,
    qualify as qualify_scoped_transport,
};

use crate::effect::Opcode;
use crate::ir::{
    AddressId, BlockId, CleanupAnchorId, EdgeId, FailureId, LirTypeId, LirUnit, LoanId,
    OperationRef, RegionInstanceId, SchemaError, ScopeId, SlotId, ValueId, ValueRepr,
};
use crate::layout_types::{
    CopyRecipe, DropStep, LayoutId, LayoutKind, LayoutRecord, LayoutTable, ScalarKind, SizeSpec,
    TargetSpec,
};
use crate::stage::{NativeReceipt, RawReceipt, Stage, StageKind, ValidatedUnit};
use hirsel_core_contract::Manifest;

/// The independently supplied declarations a unit is checked against. The
/// layout table is checked against `target` by `layout_validate`
/// before any unit check reads it.
pub struct Inputs<'a> {
    pub target: &'a TargetSpec,
    pub layouts: &'a LayoutTable,
    pub manifest: &'a Manifest,
    /// `Sync` so that one `Inputs` serves units validated on several threads.
    pub operations: &'a (dyn crate::operation::OperationDeclarations + Sync),
    pub discharge: Option<&'a crate::discharge::Discharge>,
    /// The program-wide facts every unit's check and receipt read, computed once per
    /// `Inputs` rather than once per unit. Sound because the inputs are borrowed:
    /// nothing they name can change while this `Inputs` lives.
    pub memo: Memo,
}

/// The facts of an [`Inputs`] that do not depend on the unit checked: the layout
/// table's check against the target and the digests a receipt binds. Each is
/// computed on first use.
#[derive(Default)]
pub struct Memo {
    table: std::sync::OnceLock<Result<(), Vec<crate::layout_validate::Violation>>>,
    layouts: std::sync::OnceLock<hirsel_syntax::ContentHash>,
    manifest: std::sync::OnceLock<hirsel_syntax::ContentHash>,
}

impl Inputs<'_> {
    /// `layout_validate::validate_table` of the layout table against the target.
    pub fn table_check(&self) -> &Result<(), Vec<crate::layout_validate::Violation>> {
        self.memo
            .table
            .get_or_init(|| crate::layout_validate::validate_table(self.layouts, self.target))
    }
    /// The layout table's digest.
    pub fn layouts_digest(&self) -> hirsel_syntax::ContentHash {
        *self.memo.layouts.get_or_init(|| self.layouts.digest())
    }
    /// The operation manifest's digest.
    pub fn manifest_digest(&self) -> hirsel_syntax::ContentHash {
        *self
            .memo
            .manifest
            .get_or_init(|| crate::snapshot::digest("manifest", self.manifest))
    }
}

/// The independently supplied inputs reached by one unit. Unlike a planned
/// object key, this evidence is computed from the actual rows checked here.
///
/// Borrowed from Cranelift's function stencil/ISA cache split
/// (`cranelift/codegen/src/incremental_cache.rs:147-205`): hash a function's
/// dependencies, never its compilation's unrelated functions. Differs because
/// a native receipt must independently reject a producer's stale LayoutKeys;
/// it binds actual records and local ids, not producer-supplied content keys.
#[derive(Clone, Copy)]
pub struct UnitInputs<'a> {
    pub target: &'a TargetSpec,
    pub layouts: &'a LayoutTable,
    pub manifest: &'a Manifest,
}

impl<'a> From<&Inputs<'a>> for UnitInputs<'a> {
    fn from(inputs: &Inputs<'a>) -> Self {
        Self {
            target: inputs.target,
            layouts: inputs.layouts,
            manifest: inputs.manifest,
        }
    }
}

/// A unit's direct FnAbis and transitive layout closure, in local id order.
/// The ids are meaningful only with the table supplied to `UnitInputs`.
#[derive(Debug, Eq, PartialEq)]
pub struct InputClosure {
    pub layouts: Vec<LayoutId>,
    pub abis: Vec<crate::FnAbiId>,
}

impl UnitInputs<'_> {
    /// Enumerate only rows this unit can read, rejecting every foreign id.
    /// Cycles are visited once, including edges in scan/drop programs.
    pub fn closure<S: Stage>(&self, unit: &LirUnit<S>) -> Result<InputClosure, String> {
        self.closure_with_index(unit, &crate::LayoutIndex::new(self.layouts))
    }

    /// The closure using the build's borrow-bound lookup index.
    pub fn closure_with_index<S: Stage>(
        &self,
        unit: &LirUnit<S>,
        index: &crate::LayoutIndex<'_>,
    ) -> Result<InputClosure, String> {
        if !index.matches(self.layouts) {
            return Err("layout index names another table".into());
        }
        let (mut pending, abis) = crate::objects::references(unit);
        let mut reached = std::collections::BTreeSet::new();
        for id in &abis {
            let abi = self
                .layouts
                .fn_abis
                .get(id.0 as usize)
                .ok_or_else(|| format!("the unit names FnAbi {}, outside the table", id.0))?;
            pending.extend(abi.parameters.iter().map(|p| p.layout));
            pending.extend(abi.result.iter().map(|r| r.layout));
        }
        while let Some(id) = pending.pop() {
            if !reached.insert(id) {
                continue;
            }
            let record = self
                .layouts
                .layout(id)
                .ok_or_else(|| format!("the unit names layout {}, outside the table", id.0))?;
            pending.extend(record.references());
            pending.extend(index.implicit(id));
        }
        Ok(InputClosure {
            layouts: reached.into_iter().collect(),
            abis,
        })
    }

    /// Exact checked-input evidence. No scan, allocation or hash is proportional
    /// to the size of the program's table or manifest. Emission recomputes this
    /// from its own inputs; copying a UnitKey cannot substitute for the check.
    pub fn digest<S: Stage>(
        &self,
        unit: &LirUnit<S>,
    ) -> Result<hirsel_syntax::ContentHash, String> {
        self.digest_with_index(unit, &crate::LayoutIndex::new(self.layouts))
    }

    /// Exact evidence using the build's index; no program-wide work per unit.
    pub fn digest_with_index<S: Stage>(
        &self,
        unit: &LirUnit<S>,
        index: &crate::LayoutIndex<'_>,
    ) -> Result<hirsel_syntax::ContentHash, String> {
        let closure = self.closure_with_index(unit, index)?;
        let mut parts = vec![
            self.target.digest().0.to_vec(),
            self.layouts.target.0.to_vec(),
            self.layouts.layout_version.to_le_bytes().to_vec(),
            self.layouts.abi_version.to_le_bytes().to_vec(),
            crate::INTERFACE_VERSION.to_le_bytes().to_vec(),
            crate::LIR_VERSION.to_le_bytes().to_vec(),
            crate::VALIDATOR_VERSION.to_le_bytes().to_vec(),
            hirsel_core_contract::depth::SEMANTICS.as_bytes().to_vec(),
            (closure.layouts.len() as u64).to_le_bytes().to_vec(),
        ];
        for id in closure.layouts {
            parts.push(id.0.to_le_bytes().to_vec());
            parts.push(hirsel_codec::to_text(&self.layouts.layouts[id.0 as usize]).into_bytes());
        }
        parts.push((closure.abis.len() as u64).to_le_bytes().to_vec());
        for id in closure.abis {
            parts.push(id.0.to_le_bytes().to_vec());
            parts.push(hirsel_codec::to_text(&self.layouts.fn_abis[id.0 as usize]).into_bytes());
        }
        parts.push((unit.tables.operations.len() as u64).to_le_bytes().to_vec());
        for operation in &unit.tables.operations {
            parts.push(hirsel_codec::to_text(operation).into_bytes());
            parts.push(hirsel_codec::to_text(&self.manifest.row(operation).cloned()).into_bytes());
        }
        Ok(hirsel_syntax::ContentHash::of_parts(
            "hirsel.checked-unit-inputs.v1",
            parts.iter().map(Vec::as_slice),
        ))
    }
}

/// Checks and identities shared by units that borrow the same immutable inputs.
/// No unit or producer analysis is retained: each stage still reconstructs all
/// of its structural and ownership facts. The borrows prevent a table, target
/// or manifest mutation while its cached result can be used.
///
/// A context is Sync even when a worker's operation declarations are not.
/// Operations and discharge evidence are always read from that check's Inputs.
#[derive(Debug)]
pub struct ValidationContext<'a> {
    layouts: &'a LayoutTable,
    target: &'a TargetSpec,
    manifest: &'a Manifest,
    layout_errors: std::sync::OnceLock<Vec<crate::layout_validate::Violation>>,
    index: crate::LayoutIndex<'a>,
}

impl<'a> ValidationContext<'a> {
    pub fn new(inputs: &Inputs<'a>) -> Self {
        Self {
            layouts: inputs.layouts,
            target: inputs.target,
            manifest: inputs.manifest,
            layout_errors: std::sync::OnceLock::new(),
            index: crate::LayoutIndex::new(inputs.layouts),
        }
    }

    pub fn index(&self) -> &crate::LayoutIndex<'a> {
        &self.index
    }

    /// Check the immutable table once per build, including builds with no units.
    /// Unit checks preserve their schema-first diagnostic ordering.
    pub fn preflight(&self) -> Result<(), &[crate::layout_validate::Violation]> {
        let errors = self.layout_errors.get_or_init(|| {
            crate::layout_validate::validate_table(self.layouts, self.target)
                .err()
                .unwrap_or_default()
        });
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.as_slice())
        }
    }

    fn matches(&self, inputs: &Inputs<'_>) -> bool {
        std::ptr::eq(self.layouts, inputs.layouts)
            && std::ptr::eq(self.target, inputs.target)
            && std::ptr::eq(self.manifest, inputs.manifest)
    }

    /// Validate and seal a fresh stage input, sharing only immutable program
    /// checks. Different input borrows take the ordinary independent path.
    pub fn validate<S: Stage>(
        &self,
        unit: LirUnit<S>,
        inputs: &Inputs<'_>,
    ) -> Result<ValidatedUnit<S>, Box<Rejected<S>>> {
        if !self.matches(inputs) {
            return validate(unit, inputs);
        }
        let violations = self.check(&unit, inputs);
        if !violations.is_empty() {
            return Err(Box::new(Rejected { unit, violations }));
        }
        let raw = RawReceipt {
            key: unit.header.key,
            stage: S::KIND,
            lir_version: unit.header.lir_version,
            unit: unit.digest(),
            validator_version: crate::VALIDATOR_VERSION,
        };
        let evidence = UnitInputs::from(inputs)
            .digest_with_index(&unit, &self.index)
            .expect("the independent validator checked every input reference");
        Ok(ValidatedUnit::seal(
            unit,
            NativeReceipt::issue(raw, evidence),
        ))
    }

    /// Every violation, with exactly the same ordering as `check`.
    pub fn check<S: Stage>(&self, unit: &LirUnit<S>, inputs: &Inputs<'_>) -> Vec<Violation> {
        if !self.matches(inputs) {
            return check(unit, inputs);
        }
        check_with_context(unit, inputs, self, false)
    }
}

/// A rejected unit, returned with every violation found.
#[derive(Debug)]
pub struct Rejected<S> {
    pub unit: LirUnit<S>,
    pub violations: Vec<Violation>,
}

/// Where a violation was found. `index` is an instruction ordinal within the
/// block; the block's instruction count names its terminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Site {
    pub block: Option<BlockId>,
    pub index: Option<u32>,
}

impl Site {
    pub const UNIT: Self = Self {
        block: None,
        index: None,
    };
    pub fn block(block: BlockId) -> Self {
        Self {
            block: Some(block),
            index: None,
        }
    }
    pub fn at(block: BlockId, index: u32) -> Self {
        Self {
            block: Some(block),
            index: Some(index),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Violation {
    pub site: Site,
    pub kind: ViolationKind,
}

/// One broken obligation. Each variant names one rule.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ViolationKind {
    /// Typed seal descriptors, source events, or affine transfer disagree.
    Sealed(String),
    /// Stage, version, digest or index identity (`LirUnit::check_identity`).
    Schema(String),
    /// An opcode outside the stage's vocabulary.
    Vocabulary(Opcode),
    /// Explicit finalization belongs only to Ready cleanup (LIR 13).
    FinalizationStage(Opcode),
    /// The layout table fails its independent validation (`layout_validate`).
    Layout(String),
    /// A type's representation cannot have its layout's kind.
    Representation(LirTypeId),
    /// A value's declared holding is impossible for its definition.
    Holding(ValueId),
    /// An independently reconstructed scoped origin reaches a retaining operation.
    ScopedRetention {
        value: ValueId,
        origin: ValueId,
        operation: &'static str,
    },
    Undefined(ValueId),
    Redefined(ValueId),
    /// A use the definition does not dominate.
    NotDominated(ValueId),
    /// Operand types or layouts disagree.
    Type(&'static str),
    /// Operand or argument counts disagree.
    Arity(&'static str),
    AddressType(AddressId),
    AddressCycle(AddressId),
    /// An address used without a dominating `Address` preparation.
    AddressUnprepared(AddressId),
    AddressPreparedTwice(AddressId),
    /// A use of a dynamic address after a write to its root slot, on some
    /// path since its preparation, at or above a counted block it
    /// dereferences or a tag or bounds it checks: the location prepared may
    /// no longer be the place (validator version 6).
    StaleAddress {
        address: AddressId,
        slot: SlotId,
    },
    /// A write through a block reference value rather than a slot.
    WriteThroughValue(AddressId),
    /// An edge or failure edge named by more than one site, or by none.
    EdgeUse(EdgeId),
    FailureUse(FailureId),
    UnreachableBlock(BlockId),
    /// A transfer between block kinds the cleanup protocol forbids.
    BlockKind(&'static str),
    /// `Unreachable` without an exhaustive tag switch proving it.
    UnprovedUnreachable,
    /// A storage version name used as an operand.
    VersionAsValue(ValueId),
    /// A `reaching` operand that is not the slot's unique reaching version.
    StaleVersion {
        slot: SlotId,
        reaching: ValueId,
    },
    /// A version join argument that is not its predecessor's reaching version.
    VersionJoin {
        slot: SlotId,
        value: ValueId,
    },
    /// A drop flag that is not a bool block parameter named once, or an
    /// argument to one that is neither a constant matching the slot's state
    /// on its edge nor the slot's current flag (LIR schema revision 8).
    InitFlag {
        slot: SlotId,
        value: ValueId,
    },
    DeadStorage(SlotId),
    Uninitialized(SlotId),
    /// A part (or the whole) that is or may be moved out already.
    MovedPart(SlotId),
    StoreToLive(SlotId),
    /// Storage ending, or the function exiting, with owned content still in it.
    LeakedStorage(SlotId),
    /// An inline resource out of `Live -> Finalizing -> FieldsPending` order:
    /// a `FinalizeBegin` of a place that is not Live, a `FinalizeEnd` of one
    /// that is not Finalizing, a field taken out of a resource with a
    /// finalizer before its `FinalizeEnd`, or storage ending while Finalizing.
    FinalizeState(SlotId),
    /// A write, move or drop of a borrowed parameter's storage.
    BorrowedStorage(SlotId),
    /// A caller `mut` parameter not fully initialized at an exit.
    MutExit(SlotId),
    /// A call's `mut` argument at this address whose place the callee also reaches
    /// through another `mut` argument, or through a loan held by another argument or
    /// by the callee value (OWN-3).
    CallAlias(AddressId),
    LoanState(LoanId),
    LoanMismatch(LoanId),
    /// A borrowed value used where its loan is not definitely active.
    OutsideLoan(ValueId),
    /// A borrowed read whose loan does not cover the place read.
    LoanCoverage(ValueId),
    /// A returned borrowed holder is not related to a declared Borrow parameter.
    BorrowedReturn(ValueId),
    LoanOpenAtExit(LoanId),
    /// A holder moved, dropped or released while a loan on it is active.
    HolderUnderLoan(LoanId),
    /// A view used after its backing storage was written, moved or released.
    StaleView(ValueId),
    /// A consumption or `Dup` of a value that holds no credit here.
    NoCredit(ValueId),
    /// A credit still held at an exit, or leaving the scope of its value.
    Leak(ValueId),
    /// Predecessors disagree on a value's credits at a join.
    JoinCredits {
        block: BlockId,
        value: ValueId,
    },
    /// A borrowed value passed into an owned join parameter without a credit.
    BorrowIntoOwned(ValueId),
    /// An owned credit passed into a borrowed join parameter.
    OwnedIntoBorrowed(ValueId),
    /// `Dup` of a value that is not a counted block reference.
    DupUncounted(ValueId),
    /// A value with user-visible destruction released before Ready lowered
    /// its semantic drop, i.e. away from its cleanup anchor.
    ResourceOutsideAnchor(ValueId),
    /// RC-ROOT (S-14): before Ready, a drop or release of content that
    /// reaches a `Shared` root under a TIR cleanup anchor away from that
    /// anchor's cleanup (outside a `Cleanup` or `FailureCleanup` block), or
    /// under a disposal or state-release anchor outside a failure cleanup.
    RootOutsideAnchor(CleanupAnchorId),
    /// RC-ROOT (S-14): a semantic copy (`Share`) of content that reaches a
    /// `Shared` root made by moving it out of its holder: the copy's Dup was
    /// cancelled against that holder's release, which moves the holder's end
    /// away from its cleanup anchor.
    RootCopyCancelled(ValueId),
    /// RC-ROOT (S-14): a construction of content that reaches a `Shared` root
    /// placed in a compiler-local region or arena, which frees it at its own
    /// end rather than at the value's cleanup anchor.
    RootInRegion(ValueId),
    /// A cleanup anchor no reachable operation discharges.
    SkippedAnchor(CleanupAnchorId),
    /// A release out of TIR's nested cleanup order.
    CleanupOrder(SlotId),
    /// A scope that is its own ancestor in `LirUnit::scopes`.
    ScopeCycle(ScopeId),
    RegionState(RegionInstanceId),
    UseAfterRegionExit(ValueId),
    RegionEscape(ValueId),
    /// Placement absent from Placed on, present before, or inconsistent.
    Placement(&'static str),
    /// A token used other than by reuse, disposal or a token join.
    TokenUse(ValueId),
    /// A token credit live across a feedback edge of the loop nesting forest.
    TokenFeedback(EdgeId),
    TokenOwner(ValueId),
    TokenCapacity(ValueId),
    TokenAlignment(ValueId),
    /// A donor that is not an owned counted heap block of known domain.
    TokenDonor(ValueId),
    /// Token join arguments of different provenance.
    TokenJoin(ValueId),
    /// A live token missing from a failure edge's disposal list.
    TokenFailure(ValueId),
    /// A field of an owned value taken twice or used after its move, or the
    /// value consumed or duplicated while a field is moved out.
    MovedValuePart(ValueId),
    /// A release or reset whose live mask does not clear exactly the fields
    /// moved out of the value (static masks only).
    ReleaseMask(ValueId),
    /// A reuse skipping the store of a field without proof that the token's
    /// storage already holds the operand at that physical slot.
    KeptField {
        value: ValueId,
        field: u32,
    },
    UnknownOperation(OperationRef),
    /// An operation without a native implementation, at Ready.
    UnsupportedOperation(OperationRef),
    UnresolvedDestination,
    /// A call shape the callee declaration does not admit.
    CallShape(&'static str),
}

/// Validate a raw unit and seal it. Every phase runs on every stage; Ready
/// adds its discharge checks.
pub fn validate<S: Stage>(
    unit: LirUnit<S>,
    inputs: &Inputs<'_>,
) -> Result<ValidatedUnit<S>, Box<Rejected<S>>> {
    ValidationContext::new(inputs).validate(unit, inputs)
}

/// One non-generic entry per stage, so the stage-generic checker is
/// instantiated here, once, rather than in every crate that seals a unit.
pub fn validate_value(
    unit: LirUnit<crate::stage::Value>,
    inputs: &Inputs<'_>,
) -> Result<ValidatedUnit<crate::stage::Value>, Box<Rejected<crate::stage::Value>>> {
    validate(unit, inputs)
}
pub fn validate_placed(
    unit: LirUnit<crate::stage::Placed>,
    inputs: &Inputs<'_>,
) -> Result<ValidatedUnit<crate::stage::Placed>, Box<Rejected<crate::stage::Placed>>> {
    validate(unit, inputs)
}
pub fn validate_rc(
    unit: LirUnit<crate::stage::Rc>,
    inputs: &Inputs<'_>,
) -> Result<ValidatedUnit<crate::stage::Rc>, Box<Rejected<crate::stage::Rc>>> {
    validate(unit, inputs)
}
pub fn validate_reuse(
    unit: LirUnit<crate::stage::Reuse>,
    inputs: &Inputs<'_>,
) -> Result<ValidatedUnit<crate::stage::Reuse>, Box<Rejected<crate::stage::Reuse>>> {
    validate(unit, inputs)
}
pub fn validate_ready(
    unit: LirUnit<crate::stage::Ready>,
    inputs: &Inputs<'_>,
) -> Result<ValidatedUnit<crate::stage::Ready>, Box<Rejected<crate::stage::Ready>>> {
    validate(unit, inputs)
}

/// Every violation of `unit`, sorted. Later phases run only on a unit whose
/// structure is sound, since they index by its facts.
pub fn check<S: Stage>(unit: &LirUnit<S>, inputs: &Inputs<'_>) -> Vec<Violation> {
    ValidationContext::new(inputs).check(unit, inputs)
}

/// Strict per-unit scoped obligations, without linked-module evidence.
/// An ordinary stage receipt alone does not establish these obligations.
pub fn check_scoped_transport<S: Stage>(unit: &LirUnit<S>, inputs: &Inputs<'_>) -> Vec<Violation> {
    check_with_context(unit, inputs, &ValidationContext::new(inputs), true)
}

fn check_with_context<S: Stage>(
    unit: &LirUnit<S>,
    inputs: &Inputs<'_>,
    context: &ValidationContext<'_>,
    strict_scoped: bool,
) -> Vec<Violation> {
    if let Err(error) = unit.check_identity(inputs.layouts) {
        return vec![Violation {
            site: Site::UNIT,
            kind: ViolationKind::Schema(schema_text(&error)),
        }];
    }
    if let Err(errors) = context.preflight() {
        return errors
            .iter()
            .map(|e| Violation {
                site: Site::UNIT,
                kind: ViolationKind::Layout(format!(
                    "{:?} {:?}: {}",
                    e.subject, e.obligation, e.detail
                )),
            })
            .collect();
    }
    // Read independently even when unavailable. Native output replay must
    // notice a newly demanded source boundary instead of reusing an ordinary
    // entry whose ABI silently omitted every companion.
    if let Some(source) = inputs
        .operations
        .receipt_source(&unit.header.definition, unit.header.instance)
    {
        let abi = &inputs.layouts.fn_abis[unit.header.abi.0 as usize];
        let error = source.check_structure().err().or_else(|| {
            (source.original.definition != unit.header.definition
                || source.instance != unit.header.instance
                || source.implementation_revision != unit.header.implementation_revision
                || abi.receipts.as_ref() != Some(&source.contract))
            .then_some("receipt source differs from the selected entry or complete ABI contract")
        });
        if let Some(error) = error {
            return vec![Violation {
                site: Site::UNIT,
                kind: ViolationKind::Schema(error.into()),
            }];
        }
    }
    if let Some(entry) = &unit.tables.receipt_entry {
        let checked = inputs
            .layouts
            .fn_abis
            .get(unit.header.abi.0 as usize)
            .ok_or("receipt entry lacks its ABI")
            .and_then(|abi| entry.check(abi, unit, inputs.layouts));
        let error = checked
            .err()
            .unwrap_or("receipt entry requires independent source and custody validation");
        return vec![Violation {
            site: Site::UNIT,
            kind: ViolationKind::Schema(error.into()),
        }];
    }
    if unit.slots.iter().any(|slot| {
        matches!(
            slot.kind,
            crate::ir::SlotKind::ReceiptParameter(_)
                | crate::ir::SlotKind::ReceiptCapture(_)
                | crate::ir::SlotKind::ReceiptResult
        )
    }) {
        return vec![Violation {
            site: Site::UNIT,
            kind: ViolationKind::Schema("receipt slots lack an entry contract".into()),
        }];
    }
    for call in &unit.tables.calls {
        if let Some(abi) = inputs.layouts.fn_abis.get(call.abi.0 as usize)
            && let Err(error) =
                crate::receipt_transport::check_call_contract(abi, call, &unit.pools)
        {
            return vec![Violation {
                site: Site::UNIT,
                kind: ViolationKind::Schema(error.into()),
            }];
        }
    }
    let (_, abis) = crate::objects::references(unit);
    for id in abis {
        if let Some(abi) = inputs.layouts.fn_abis.get(id.0 as usize)
            && let Err(error) = crate::abi::check_implemented_transport(abi)
        {
            return vec![Violation {
                site: Site::UNIT,
                kind: ViolationKind::Schema(error.into()),
            }];
        }
    }
    let cx = Cx {
        unit,
        layouts: Layouts(inputs.layouts),
        manifest: inputs.manifest,
        stage: S::KIND,
        target: inputs.target,
        operations: inputs.operations,
        discharge: inputs.discharge,
    };
    let mut report = Report::default();
    let mut roots = std::collections::BTreeSet::new();
    for plan in &unit.tables.structural {
        let checked = plan.check(inputs.layouts).and_then(|()| {
            if !roots.insert(plan.root_layout()) {
                return Err("duplicate structural root layout".into());
            }
            let expected = inputs
                .operations
                .structural_access(&plan.source, inputs.layouts, inputs.target)
                .map_err(|e| format!("structural declaration reconstruction: {e:?}"))?;
            if &expected != plan {
                return Err("structural projection differs from independent declarations".into());
            }
            Ok(())
        });
        if let Err(error) = checked {
            report.add(Site::UNIT, ViolationKind::Schema(error));
        }
    }
    if inputs.operations.requires_structural_reference() {
        for instruction in &unit.instructions {
            let layout = match *instruction {
                crate::ir::Inst::Compare { layout, .. } | crate::ir::Inst::Hash { layout, .. } => {
                    layout
                }
                _ => continue,
            };
            if !roots.contains(&layout) {
                report.add(
                    Site::UNIT,
                    ViolationKind::Schema(
                        "structural operand has no independent access plan".into(),
                    ),
                );
            }
        }
    }
    let facts = structure::check(&cx, &mut report);
    if let Some(facts) = facts.filter(|_| report.0.is_empty()) {
        for result in [
            crate::pretransfer::check_edges(
                unit,
                inputs.layouts,
                inputs.operations.pretransfer_adapters(),
            ),
            crate::pretransfer::check_adapter(
                unit,
                inputs.layouts,
                inputs.operations.pretransfer_adapters(),
            )
            .map(|_| ()),
        ] {
            if let Err(error) = result {
                report.add(Site::UNIT, ViolationKind::Schema(error));
            }
        }
        if unit.header.origin == crate::ir::UnitOrigin::Library {
            if let Err(error) = crate::invocations::check_library_with_layouts(
                unit,
                inputs.operations,
                inputs.layouts,
            ) {
                report.add(Site::UNIT, ViolationKind::Schema(error));
            }
        }
        if let Some(reference) = inputs.operations.invocations(&unit.header.definition) {
            let selected = inputs
                .operations
                .invocation_body(&unit.header.definition, unit.header.instance)
                .ok_or_else(|| "independent source body selection is missing".to_owned());
            if let Err(error) = selected.and_then(|body| {
                crate::invocations::check_with_layouts(
                    unit,
                    reference,
                    body,
                    inputs.operations,
                    inputs.layouts,
                )
            }) {
                report.add(Site::UNIT, ViolationKind::Schema(error));
            }
        }
        if unit.header.origin == crate::ir::UnitOrigin::Source
            && inputs.operations.requires_invocation_reference()
            && inputs
                .operations
                .invocations(&unit.header.definition)
                .is_none()
        {
            report.add(
                Site::UNIT,
                ViolationKind::Schema("source invocation reference is missing".into()),
            );
        }
        if unit.header.origin == crate::ir::UnitOrigin::Source
            && inputs.operations.requires_structural_reference()
        {
            if let Err(error) = crate::structural_access::check_key_order(unit) {
                report.add(Site::UNIT, ViolationKind::Schema(error));
            }
        }
        sealed::check(&cx, &facts, &mut report);
        sealed_payload::check(&cx, &mut report);
        receipt_custody::check(&cx, &mut report);
        ownership::check(&cx, &facts, &mut report);
        holes::check(&cx, &facts, &mut report);
        aliasing::check(&cx, &facts, &mut report);
        if strict_scoped
            || scoped_transport::has_data_contract(&cx)
            || unit
                .instructions
                .iter()
                .any(|inst| matches!(inst, crate::ir::Inst::BorrowSealedRoot { .. }))
        {
            scoped_transport::check(&cx, &facts, &mut report);
        }
        arena::check(&cx, &facts, &mut report);
        if cx.stage == StageKind::Ready {
            ready::check(&cx, &mut report);
        }
    }
    let mut violations = report.0;
    violations.sort();
    violations.dedup();
    violations
}

fn schema_text(error: &SchemaError) -> String {
    format!("{error:?}")
}

/// The checking context: the unit, its layout table and manifest, and its stage.
pub(crate) struct Cx<'a, S> {
    pub unit: &'a LirUnit<S>,
    pub layouts: Layouts<'a>,
    pub manifest: &'a Manifest,
    pub stage: StageKind,
    pub target: &'a TargetSpec,
    pub operations: &'a dyn crate::operation::OperationDeclarations,
    pub discharge: Option<&'a crate::discharge::Discharge>,
}

impl<S> Cx<'_, S> {
    pub fn at_least(&self, stage: StageKind) -> bool {
        self.stage >= stage
    }
    pub fn repr(&self, value: ValueId) -> ValueRepr {
        self.unit.types[self.unit.values[value.index()].ty.index()].repr
    }
    pub fn value_layout(&self, value: ValueId) -> LayoutId {
        self.unit.types[self.unit.values[value.index()].ty.index()].layout
    }
    /// Whether this type has an ownership obligation. Private metadata holders
    /// are separate from language block/Shared credits. Slots' plain versions
    /// name their contents rather than acquiring another holder.
    pub fn owns_type(&self, ty: LirTypeId) -> bool {
        let ty = &self.unit.types[ty.index()];
        match ty.repr {
            ValueRepr::PayloadReceipt => true,
            ValueRepr::Block | ValueRepr::Token | ValueRepr::RegionMark | ValueRepr::Hole => true,
            ValueRepr::Unit
            | ValueRepr::Scalar(_)
            | ValueRepr::InitMask { .. }
            | ValueRepr::Uncounted
            | ValueRepr::Arena => false,
            ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle => {
                !self.layouts.trivial(ty.layout)
            }
        }
    }
    pub fn owns(&self, value: ValueId) -> bool {
        self.owns_type(self.unit.values[value.index()].ty)
    }
}

/// Read-only queries over the layout table. A layout id inside a record that
/// falls outside the table answers conservatively; `structure` reports it.
#[derive(Clone, Copy)]
pub(crate) struct Layouts<'a>(pub &'a LayoutTable);

impl<'a> Layouts<'a> {
    pub fn get(&self, id: LayoutId) -> Option<&'a LayoutRecord> {
        self.0.layout(id)
    }
    /// No destruction work: an empty drop program and no finalizer.
    pub fn trivial(&self, id: LayoutId) -> bool {
        self.get(id)
            .is_some_and(|l| l.drop.is_empty() && l.finalizer.is_none())
    }
    pub fn copy(&self, id: LayoutId) -> Option<CopyRecipe> {
        self.get(id).map(|l| l.copy)
    }
    /// Two scalar layouts of one kind: the same bits, size, alignment and
    /// ABI. A table can hold several (a type's own `u8` and the `u8` a
    /// Simd or String layout names inside), and a value of one may stand
    /// for the other.
    pub fn same_scalar(&self, a: LayoutId, b: LayoutId) -> bool {
        match (self.get(a).map(|l| &l.kind), self.get(b).map(|l| &l.kind)) {
            (Some(LayoutKind::Scalar(x)), Some(LayoutKind::Scalar(y))) => x == y,
            _ => false,
        }
    }
    /// Whether a finalizer is reachable from this layout without crossing a
    /// block reference: through owned inline fields, variants and inline
    /// elements. Borrowed captures do not own their referent's destruction.
    pub fn inline_finalizer(&self, id: LayoutId) -> bool {
        let Some(layout) = self.get(id) else {
            return true;
        };
        if layout.finalizer.is_some() {
            return true;
        }
        match layout.kind {
            LayoutKind::Block { .. } | LayoutKind::Box { .. } | LayoutKind::Shared { .. } => false,
            LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. } => {
                self.inline_finalizer(element)
            }
            _ => {
                let variants = layout.tag.iter().flat_map(|t| &t.variants);
                layout
                    .fields
                    .iter()
                    .chain(variants.flat_map(|v| &v.fields))
                    .filter(|f| !crate::layout::field_is_borrowed(self.0, layout, f))
                    .any(|f| self.inline_finalizer(f.layout))
            }
        }
    }
    /// The payload of a block reference layout.
    pub fn payload(&self, id: LayoutId) -> Option<LayoutId> {
        match self.get(id)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => Some(payload),
            _ => None,
        }
    }
    /// The payload a release of a block value names: a block reference's, or
    /// a `Shared` root's frozen record, which no address projects into.
    pub fn released(&self, id: LayoutId) -> Option<LayoutId> {
        match self.get(id)?.kind {
            LayoutKind::Shared { payload } => Some(payload),
            _ => self.payload(id),
        }
    }
    pub fn fixed_size(&self, id: LayoutId) -> Option<u64> {
        match self.get(id)?.size {
            SizeSpec::Fixed(size) => Some(size),
            SizeSpec::Dynamic(_) => None,
        }
    }
    /// A length-prefixed FlatFormatV0 encoding (a variable field's record): a
    /// dynamic-size Flat record whose one field, its content, lies after a
    /// u64 byte-length prefix (offset 8) or, for a list of variable elements,
    /// after the prefix and a u64 element count (offset 16). The content's
    /// offset and layout.
    pub fn flat_prefixed(&self, id: LayoutId) -> Option<(u64, LayoutId)> {
        let r = self.get(id)?;
        match (&r.kind, &r.size, r.fields.as_slice()) {
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [only])
                if only.ordinal == 0 && matches!(only.offset, 8 | 16) =>
            {
                Some((only.offset, only.layout))
            }
            _ => None,
        }
    }
    /// A flat sequence encoding (List, Array, String, Bytes) and its element
    /// layout: fixed elements follow the prefix at a stride, variable ones
    /// (each its own prefixed encoding) follow the prefix and the count. A
    /// prefixed encoding of one variable value is no sequence.
    pub fn flat_sequence(&self, id: LayoutId) -> Option<LayoutId> {
        match self.flat_prefixed(id)? {
            (16, element) => Some(element),
            (_, element) => self.fixed_size(element).map(|_| element),
        }
    }
    /// The offset in the flat record `record` of its field `field` when an
    /// open element can build it in place (LIR schema revision 24): a byte
    /// sequence encoding after only fixed fields, so at a static offset.
    pub fn open_field(&self, record: LayoutId, field: u32) -> Option<u64> {
        let r = self.get(record)?;
        if r.kind != LayoutKind::Flat {
            return None;
        }
        let f = r.fields.iter().find(|f| f.ordinal == field)?;
        let earlier_fixed = r
            .fields
            .iter()
            .filter(|e| e.ordinal < field)
            .all(|e| self.fixed_size(e.layout).is_some());
        (earlier_fixed && self.flat_bytes(f.layout)).then_some(f.offset)
    }
    /// The FlatFormatV0 encoding of String and Bytes: a byte sequence.
    pub fn flat_bytes(&self, id: LayoutId) -> bool {
        self.flat_sequence(id).is_some_and(|e| {
            self.get(e)
                .is_some_and(|r| r.kind == LayoutKind::Scalar(ScalarKind::U8))
        })
    }
    /// Whether the source value can be written at the given flat encoding.
    /// Sequence elements are checked recursively: scalar bits keep their kind,
    /// byte slices encode their contents, and variable flat records encode the
    /// bytes of their payload. Encoded sources are borrowed for construction.
    pub fn encodes(&self, source: LayoutId, encoding: LayoutId) -> bool {
        self.encodes_in(source, encoding, &mut std::collections::BTreeSet::new())
    }

    fn encodes_in(
        &self,
        source: LayoutId,
        encoding: LayoutId,
        seen: &mut std::collections::BTreeSet<(LayoutId, LayoutId)>,
    ) -> bool {
        if !seen.insert((source, encoding)) {
            return false;
        }
        if source == encoding || self.same_scalar(source, encoding) {
            return self.fixed_size(encoding).is_some();
        }
        let Some(source) = self.get(source) else {
            return false;
        };
        if self.flat_bytes(encoding)
            && matches!(source.kind, LayoutKind::Slice { element }
            if self.get(element).is_some_and(|r| r.kind == LayoutKind::Scalar(ScalarKind::U8)))
        {
            return true;
        }
        match (self.flat_prefixed(encoding), &source.kind) {
            (Some((offset, content)), LayoutKind::Block { payload }) => {
                // A sequential sequence of exactly the encoding's variable
                // elements, each already in its prefixed encoding (LIR schema
                // revision 22): its count and used bytes are the content.
                if let Some(LayoutKind::Sequential { element }) =
                    self.get(*payload).map(|r| &r.kind)
                {
                    return offset == 16
                        && *element == content
                        && self.fixed_size(content).is_none()
                        && self.flat_prefixed(content).is_some_and(|(o, _)| o == 8);
                }
                if let Some(LayoutKind::Sequence { element }) = self.get(*payload).map(|r| &r.kind)
                {
                    self.flat_sequence(encoding).is_some()
                        && self.encodes_in(*element, content, seen)
                } else {
                    offset == 8 && *payload == content && self.fixed_size(content).is_none()
                }
            }
            (Some((8, content)), LayoutKind::Box { payload }) => {
                *payload == content && self.fixed_size(content).is_none()
            }
            _ => false,
        }
    }
    /// Whether structural comparison is defined on every part of this layout:
    /// no callable, iterator, handle, resource, header or raw pointer, the values
    /// `hirsel_interp::structural::compare` rejects. A recursive layout is
    /// comparable when its non-recursive parts are.
    pub fn comparable(&self, id: LayoutId) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        self.comparable_in(id, &mut seen)
    }
    fn comparable_in(&self, id: LayoutId, seen: &mut std::collections::BTreeSet<LayoutId>) -> bool {
        if !seen.insert(id) {
            return true;
        }
        let Some(layout) = self.get(id) else {
            return false;
        };
        let fields = layout.fields.iter().chain(
            layout
                .tag
                .iter()
                .flat_map(|t| &t.variants)
                .flat_map(|v| &v.fields),
        );
        match &layout.kind {
            LayoutKind::Unit | LayoutKind::Path => true,
            LayoutKind::Scalar(scalar) => *scalar != crate::layout_types::ScalarKind::Pointer,
            LayoutKind::Aggregate
            | LayoutKind::Environment { .. }
            | LayoutKind::Variant
            | LayoutKind::Packed
            | LayoutKind::Flat => fields
                .map(|f| f.layout)
                .collect::<Vec<_>>()
                .into_iter()
                .all(|f| self.comparable_in(f, seen)),
            LayoutKind::Inline { element, .. }
            | LayoutKind::Simd { element, .. }
            | LayoutKind::Slice { element }
            | LayoutKind::SoaColumns { element }
            | LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element } => self.comparable_in(*element, seen),
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                self.comparable_in(*payload, seen)
            }
            // Map, Set and SortedMap compare by their library order.
            LayoutKind::Opaque { .. } => true,
            // Shared roots have no equality (GEN-1); a frozen payload is never a value.
            LayoutKind::Shared { .. }
            | LayoutKind::SealedRegion { .. }
            | LayoutKind::Frozen { .. }
            | LayoutKind::Handle
            | LayoutKind::HostCapability { .. }
            | LayoutKind::Arena
            | LayoutKind::Callable { .. }
            | LayoutKind::Iterator { .. }
            | LayoutKind::Resource
            | LayoutKind::Header
            | LayoutKind::PayloadReceipt { .. } => false,
        }
    }
    /// Whether `Inst::Hash` is defined on every part of this layout (LIR
    /// schema revision 7): exactly the layouts `Compare Eq` is defined on,
    /// since equal values must hash equal.
    pub fn hashable(&self, id: LayoutId) -> bool {
        self.comparable(id)
    }
    /// Whether destroying a value of this layout is user-visible: its drop
    /// program, at any depth, runs a finalizer or closes a handle. Derived from
    /// the drop programs alone; a counted child's payload is
    /// found through the field at the released offset.
    pub fn destructible(&self, id: LayoutId) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        self.destructible_in(id, &mut seen)
    }
    fn destructible_in(
        &self,
        id: LayoutId,
        seen: &mut std::collections::BTreeSet<LayoutId>,
    ) -> bool {
        if !seen.insert(id) {
            return false;
        }
        let Some(layout) = self.get(id) else {
            return true;
        };
        if layout.finalizer.is_some() {
            return true;
        }
        // A block reference's own drop program releases the reference; what
        // that runs is its payload's destruction.
        match layout.kind {
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => return self.destructible_in(payload, seen),
            // Callable storage contains a pointer, not the environment's type.
            // A visible environment supplies its actual descriptor; an erased
            // environment or iterator can run arbitrary language destruction.
            LayoutKind::Callable {
                environment: Some(environment),
            } => {
                return self.destructible_in(environment, seen);
            }
            LayoutKind::Callable { environment: None } => return !layout.drop.is_empty(),
            LayoutKind::Iterator { .. } => return true,
            _ => {}
        }
        self.steps_destructible(layout, &layout.drop, seen)
    }
    fn steps_destructible(
        &self,
        layout: &LayoutRecord,
        steps: &[DropStep],
        seen: &mut std::collections::BTreeSet<LayoutId>,
    ) -> bool {
        steps.iter().any(|step| match step {
            DropStep::Finalize | DropStep::CloseHandle { .. } => true,
            // Restoring counts is invisible; the release that follows is a Child.
            DropStep::Thaw { .. }
            | DropStep::ReleaseSealed { .. }
            | DropStep::ReleaseReceipt { .. } => false,
            DropStep::Child { layout, .. }
            | DropStep::ElementsReverse {
                element: layout, ..
            }
            | DropStep::RepeatReverse {
                element: layout, ..
            } => self.destructible_in(*layout, seen),
            DropStep::ColumnsReverse(columns) => columns
                .columns
                .iter()
                .any(|c| self.destructible_in(c.layout, seen)),
            DropStep::ActiveVariant { variants, .. } => variants
                .iter()
                .any(|(_, steps)| self.steps_destructible(layout, steps, seen)),
            DropStep::Environment { offset }
            | DropStep::Release { offset }
            | DropStep::ReleaseRoot { offset } => {
                // Lifted steps may lie inside callable or aggregate fields.
                // Check every covering variant field: equal physical offsets
                // do not prove equal destruction behavior.
                let variants = layout.tag.iter().flat_map(|t| &t.variants);
                let mut covering = layout
                    .fields
                    .iter()
                    .chain(variants.flat_map(|v| &v.fields))
                    .filter(|f| {
                        f.offset <= *offset
                            && self
                                .fixed_size(f.layout)
                                .is_some_and(|size| *offset - f.offset < size)
                    })
                    .peekable();
                covering.peek().is_none() || covering.any(|f| self.destructible_in(f.layout, seen))
            }
        })
    }
    /// Whether this layout's drop program reaches a `Shared` root at any
    /// depth (RC-ROOT, S-14): a `Shared` root itself, or a payload,
    /// environment, child, element, column or variant step that does. An
    /// erased environment or an iterator may hold one. Derived from the
    /// layout records alone, independently of the passes' answer.
    pub fn root_bearing(&self, id: LayoutId) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        self.root_bearing_in(id, &mut seen)
    }
    fn root_bearing_in(
        &self,
        id: LayoutId,
        seen: &mut std::collections::BTreeSet<LayoutId>,
    ) -> bool {
        if !seen.insert(id) {
            return false;
        }
        let Some(layout) = self.get(id) else {
            return true;
        };
        match layout.kind {
            LayoutKind::Shared { .. } | LayoutKind::Iterator { .. } => true,
            // A static callable's (an Entry's) environment is never counted.
            LayoutKind::Callable { environment: None } => !layout.drop.is_empty(),
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                self.root_bearing_in(payload, seen)
            }
            LayoutKind::Callable {
                environment: Some(environment),
            } => self.root_bearing_in(environment, seen),
            _ => self.steps_root_bearing(layout, &layout.drop, seen),
        }
    }
    fn steps_root_bearing(
        &self,
        layout: &LayoutRecord,
        steps: &[DropStep],
        seen: &mut std::collections::BTreeSet<LayoutId>,
    ) -> bool {
        steps.iter().any(|step| match step {
            DropStep::Finalize | DropStep::CloseHandle { .. } | DropStep::ReleaseReceipt { .. } => {
                false
            }
            DropStep::ReleaseSealed { .. } => true,
            DropStep::Thaw { .. } | DropStep::ReleaseRoot { .. } => true,
            DropStep::Child { layout, .. }
            | DropStep::ElementsReverse {
                element: layout, ..
            }
            | DropStep::RepeatReverse {
                element: layout, ..
            } => self.root_bearing_in(*layout, seen),
            DropStep::ColumnsReverse(columns) => columns
                .columns
                .iter()
                .any(|c| self.root_bearing_in(c.layout, seen)),
            DropStep::ActiveVariant { variants, .. } => variants
                .iter()
                .any(|(_, steps)| self.steps_root_bearing(layout, steps, seen)),
            DropStep::Release { offset } | DropStep::Environment { offset } => {
                // Every field whose bytes cover the offset, of any variant: a
                // lifted step names a word inside a child aggregate.
                let variants = layout.tag.iter().flat_map(|t| &t.variants);
                let mut covering = layout
                    .fields
                    .iter()
                    .chain(variants.flat_map(|v| &v.fields))
                    .filter(|f| {
                        f.offset <= *offset
                            && self
                                .fixed_size(f.layout)
                                .is_some_and(|size| *offset - f.offset < size)
                    })
                    .peekable();
                covering.peek().is_none() || covering.any(|f| self.root_bearing_in(f.layout, seen))
            }
        })
    }
}

/// Violations collected by every phase.
#[derive(Default)]
pub(crate) struct Report(pub Vec<Violation>);

impl Report {
    pub fn add(&mut self, site: Site, kind: ViolationKind) {
        self.0.push(Violation { site, kind });
    }
}
