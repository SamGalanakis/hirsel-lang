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
mod arena;
pub use arena::check_calls as check_result_arena_calls;
mod discharge;
pub use discharge::validate_discharge;
mod flat_open;
mod flat_walk;
mod flow;
mod ownership;
mod ready;
mod structure;
mod tokens;

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
    pub operations: &'a dyn crate::operation::OperationDeclarations,
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
    let violations = check(&unit, inputs);
    if !violations.is_empty() {
        return Err(Box::new(Rejected { unit, violations }));
    }
    let raw = RawReceipt {
        input: unit.header.input.clone(),
        stage: S::KIND,
        lir_version: unit.header.lir_version,
        unit: unit.digest(),
        target: inputs.layouts.target,
        layouts: inputs.layouts.digest(),
        manifest: crate::snapshot::digest("manifest", inputs.manifest),
        validator_version: crate::VALIDATOR_VERSION,
    };
    Ok(ValidatedUnit::seal(unit, NativeReceipt::issue(raw)))
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
    if let Err(error) = unit.check_identity(inputs.layouts) {
        return vec![Violation {
            site: Site::UNIT,
            kind: ViolationKind::Schema(schema_text(&error)),
        }];
    }
    if let Err(errors) = crate::layout_validate::validate_table(inputs.layouts, inputs.target) {
        return errors
            .into_iter()
            .map(|e| Violation {
                site: Site::UNIT,
                kind: ViolationKind::Layout(format!(
                    "{:?} {:?}: {}",
                    e.subject, e.obligation, e.detail
                )),
            })
            .collect();
    }
    let cx = Cx {
        unit,
        layouts: Layouts(inputs.layouts),
        manifest: inputs.manifest,
        stage: S::KIND,
        target: inputs.target,
        operations: inputs.operations,
    };
    let mut report = Report::default();
    let facts = structure::check(&cx, &mut report);
    if let Some(facts) = facts.filter(|_| report.0.is_empty()) {
        ownership::check(&cx, &facts, &mut report);
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
    /// Whether a value of this type carries a holder credit: block
    /// references, tokens and region marks always; aggregates, callables and
    /// handles when their layout releases something.
    pub fn owns_type(&self, ty: LirTypeId) -> bool {
        let ty = &self.unit.types[ty.index()];
        match ty.repr {
            ValueRepr::Block | ValueRepr::Token | ValueRepr::RegionMark => true,
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
    /// block reference: through inline fields, variants and inline elements.
    pub fn inline_finalizer(&self, id: LayoutId) -> bool {
        let Some(layout) = self.get(id) else {
            return true;
        };
        if layout.finalizer.is_some() {
            return true;
        }
        match layout.kind {
            LayoutKind::Block { .. } | LayoutKind::Box { .. } => false,
            LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. } => {
                self.inline_finalizer(element)
            }
            _ => {
                let variants = layout.tag.iter().flat_map(|t| &t.variants);
                layout
                    .fields
                    .iter()
                    .chain(variants.flat_map(|v| &v.fields))
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
            LayoutKind::Handle
            | LayoutKind::Arena
            | LayoutKind::Callable { .. }
            | LayoutKind::Iterator { .. }
            | LayoutKind::Resource
            | LayoutKind::Header => false,
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
        if let LayoutKind::Block { payload } | LayoutKind::Box { payload } = layout.kind {
            return self.destructible_in(payload, seen);
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
            DropStep::Release { offset } | DropStep::Environment { offset } => {
                match self.field_at(layout, *offset) {
                    Some(child) => self.destructible_in(child, seen),
                    // An environment or child we cannot see may finalize.
                    None => true,
                }
            }
        })
    }
    /// The layout of the field (of any variant) stored at `offset`.
    fn field_at(&self, layout: &LayoutRecord, offset: u64) -> Option<LayoutId> {
        let variants = layout.tag.iter().flat_map(|t| &t.variants);
        layout
            .fields
            .iter()
            .chain(variants.flat_map(|v| &v.fields))
            .find(|f| f.offset == offset)
            .map(|f| f.layout)
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
