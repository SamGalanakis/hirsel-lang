//! P2 SSA import ("CFG obligations and mandatory loops"): one
//! instance view of validated TIR becomes one `LirUnit<Value>`.
//!
//! Mutable TIR locals become explicit slots; the immutable value a slot holds
//! after each write is a fresh version name, reads name the reaching version,
//! and merge blocks join versions through block parameters. Operands are read
//! in TIR source order and keep their declaration ordinals (`CallArgument.
//! parameter`, `ConstructField.field`); nothing rematches labels.
//!
//! Every potentially failing operation gets an explicit failure successor. A
//! failure successor is the TIR instruction's cleanup chain expanded into
//! `FailureCleanup` blocks, specialized by the abstract slot and loan state at
//! that point: a `drop?` of a slot that is definitely empty there is a no-op and
//! is not emitted, a `drop?` of a slot that is definitely full loses its `?`.
//! Normal-path cleanup (`RunCleanup`, `Return`) runs in `Cleanup` blocks, and a
//! drop that fails there continues the rest of its list, then the enclosing
//! failure cleanup, so the list always finishes (`RunCleanup` may fail and still
//! finishes). `Return` extracts its owned result before cleanup and a cleanup
//! failure disposes of that result before propagating. Owned values pending
//! when an operation fails (an earlier operand's copy, an extracted result) are
//! passed to the failure block and disposed through a scratch slot at a
//! synthetic cleanup anchor.
//!
//! Anchors: a TIR cleanup action anchors at `(body, cleanup, ordinal)`; the
//! synthetic drops anchor at reserved cleanup ordinals (`INSTRUCTION_DROP`,
//! `LOOP_RELEASE`, `SCRATCH_DISPOSAL`), so no anchor names a TIR action it does
//! not perform. Allocation-class failures (construction, copies, closures)
//! carry no language rule: allocation exhaustion is environmental.
use crate::instance::{Actuals, InstanceError, InstanceId, Instances, Program, substitute};
use crate::normalize::{Declined, Facts, Home, LoopPlan, LoopSite, NormalizeError};
use hirsel_core_contract::{OperationId, Passing};
use hirsel_lir::abi_types::{CallableShape, FnAbiId};
use hirsel_lir::effect::Effects;
use hirsel_lir::ir::{
    AddressBase, AddressId, AddressInfo, ArgumentValue, BinaryOp, Block, BlockId, BlockKind,
    CallArgument, CallId, CallSite, Callee, CaptureValue, CleanupAnchor, CleanupAnchorId,
    ClosureCode, ClosureSite, CompareOp, ConstantData, ConstantId, ConstantValue, ConstructSite,
    ConstructorKind, DestinationClass, Edge, EdgeId, FailureEdge, FailureId, FieldValue, Holding,
    Inst, LirType, LirTypeId, LirUnit, LoanId, LoanInfo, OperationRef, OriginKind, PoolRange,
    ProjectionStep, ReadMode, ScopeId, ScopeInfo, SiteRange, SlotId, SlotInfo, SlotKind,
    SourceAnchor, SourceAnchorId, SwitchArm, Symbol, SymbolId, SymbolKind, Terminator, UnaryOp,
    UnitHeader, UnitOrigin, ValueId, ValueInfo, ValueRepr,
};
use hirsel_lir::layout::{LayoutBuilder, LayoutError};
use hirsel_lir::layout_types::{
    CopyRecipe, LayoutId, LayoutKind, LayoutTable, SizeSpec, TargetSpec,
};
use hirsel_lir::persistent::{Element, PVec};
use hirsel_lir::stage::{StageKind, Value};
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir as tir;
use hirsel_types::{GenericArgument, SignatureKind, TypeId, TypeStore, UseKind};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Cleanup-anchor ordinals reserved for drops TIR states outside a cleanup chain.
pub const INSTRUCTION_DROP: u32 = u32::MAX;
/// The release of a self-tail loop's internal owned slot (LANG-10).
pub const LOOP_RELEASE: u32 = u32::MAX - 1;
/// The disposal of an owned value pending when an operation failed.
pub const SCRATCH_DISPOSAL: u32 = u32::MAX - 2;
/// A release derived from the state where TIR names no cleanup (an operation
/// TIR cannot make fail: a prologue preparation or a match test).
pub const STATE_RELEASE: u32 = u32::MAX - 3;

#[derive(Clone, Debug)]
pub enum ImportError {
    Instance(InstanceError),
    Layout(DefPath, LayoutError),
    /// A TIR form the importer cannot express yet, named.
    Unsupported(String),
    /// TIR that contradicts its own validated invariants.
    Malformed(String),
}

impl From<InstanceError> for ImportError {
    fn from(e: InstanceError) -> Self {
        Self::Instance(e)
    }
}

mod flat_builder;
mod flat_field;
mod flat_views;
mod materialize;

type R<T> = Result<T, ImportError>;

fn unsupported<T>(what: impl Into<String>) -> R<T> {
    Err(ImportError::Unsupported(what.into()))
}
fn malformed<T>(what: impl Into<String>) -> R<T> {
    Err(ImportError::Malformed(what.into()))
}

/// A lowered program: its instances, their loop plans and declined
/// published sites, one Value unit per instance, and the layout table the
/// units name.
pub struct Lowered {
    pub instances: Instances,
    pub plans: Vec<Option<LoopPlan>>,
    pub declined: Vec<(InstanceId, Declined)>,
    pub units: Vec<LirUnit<Value>>,
    pub table: LayoutTable,
    /// The static data of every constant a unit reads (`ReadStatic`).
    pub statics: Vec<hirsel_lir::statics::StaticData>,
    /// Signature/placement proof owner of every unit, including specializations.
    pub unit_origins: Vec<InstanceId>,
    /// Source bodies available for inlining in every calling partition.
    pub inline_instances: std::collections::BTreeSet<ContentHash>,
}

#[derive(Clone, Debug)]
pub enum LowerError {
    Instance(InstanceError),
    Normalize(InstanceId, NormalizeError),
    Import(InstanceId, ImportError),
    /// A library body's builder failed.
    Library(InstanceId, crate::library::LibraryError),
    /// A constant a unit reads has no static encoding (or no value).
    Static(DefPath, crate::statics::StaticError),
}

/// Expand, normalize and import a whole program for one target.
pub fn lower(program: &Program, target: &TargetSpec) -> Result<Lowered, LowerError> {
    lower_with_options(program, target, &crate::flat_walk::Options::default())
}

/// Import with a matrix-selectable variable-flat loop rewrite.
pub fn lower_with_options(
    program: &Program,
    target: &TargetSpec,
    flat_walk: &crate::flat_walk::Options,
) -> Result<Lowered, LowerError> {
    // Results built only to be appended are built in place (append_into).
    let fused = crate::append_into::fuse(program);
    let program = &fused;
    // The library's native representations lie over the program's owners.
    let owners = crate::library::owners(&program.signatures).over(&program.owners);
    let mut layouts = LayoutBuilder::new(target, &owners);
    let mut instances =
        crate::instance::expand(program, &mut layouts).map_err(LowerError::Instance)?;
    let mut plans = Vec::new();
    let mut declined = Vec::new();
    let mut units = Vec::new();
    // Importing a unit or building a library body may require further
    // library instances, which join the end of the list.
    let mut required = Vec::new();
    let mut next = 0;
    while next < instances.len() {
        let id = InstanceId(next as u32);
        next += 1;
        if instances.roles[id.index()].is_library() {
            plans.push(None);
            units.push(
                crate::library::build(program, &mut layouts, &mut instances, id)
                    .map_err(|e| LowerError::Library(id, e))?,
            );
            continue;
        }
        let (plan, no) = plan_instance(program, &instances, id, &mut layouts)
            .map_err(|e| LowerError::Normalize(id, e))?;
        declined.extend(no.into_iter().map(|d| (id, d)));
        let mut unit = import(
            program,
            &instances,
            id,
            plan.as_ref(),
            &mut layouts,
            &mut required,
        )
        .map_err(|e| LowerError::Import(id, e))?;
        let proven = crate::spare_push::prove(&unit, &|l| layouts.record(l).kind.clone());
        crate::spare_push::commit(&mut unit, &proven, &mut required, &layouts)
            .map_err(LowerError::Instance)?;
        let fields = crate::spare_push::prove_fields(&unit);
        crate::spare_push::commit_fields(&mut unit, &fields);
        plans.push(plan);
        units.push(unit);
        for request in required.drain(..) {
            crate::library::instantiate(&mut layouts, &mut instances, request)
                .map_err(LowerError::Instance)?;
        }
    }
    let statics = statics(program, &units, &mut layouts)?;
    // Units name the table by digest: seal them once no layout is added.
    // The block header is the layout of a reuse token, which a
    // later stage creates after sealing.
    layouts.header();
    let table = layouts.finish();
    let digest = table.digest();
    for unit in &mut units {
        unit.header.layouts = digest;
        unit.header.target = table.target;
    }
    let units = units
        .into_iter()
        .map(|u| crate::flat_walk::rewrite(u, &table, flat_walk))
        .collect();
    let mut lowered = Lowered {
        instances,
        plans,
        declined,
        units,
        table,
        statics,
        unit_origins: Vec::new(),
        inline_instances: Default::default(),
    };
    let specialized = crate::closure_calls::specialize(&mut lowered);
    lowered.unit_origins = specialized.origins;
    lowered.inline_instances = specialized.inline;
    crate::arena_credit::summarize(&mut lowered.units, &mut lowered.table);
    Ok(lowered)
}

/// The definition of every static empty sequence; its instance names the
/// sequence's payload layout.
pub fn empty_sequence() -> DefPath {
    crate::library::core_path("$intrinsic.EmptySequence")
}

/// The instance of the static empty sequence of the payload `record`.
fn empty_sequence_instance(record: &hirsel_lir::layout_types::LayoutRecord) -> ContentHash {
    hirsel_lir::snapshot::digest("empty-sequence", record)
}

/// The static data of each constant the units read, once per constant, and
/// of each static empty sequence: its zero count, at its payload layout.
fn statics(
    program: &Program,
    units: &[LirUnit<Value>],
    layouts: &mut LayoutBuilder<'_>,
) -> Result<Vec<hirsel_lir::statics::StaticData>, LowerError> {
    let mut read = BTreeMap::new();
    let mut empty = BTreeMap::new();
    let empty_path = empty_sequence();
    for unit in units {
        for inst in &unit.instructions {
            if let Inst::ReadStatic { symbol, result } = inst {
                let symbol = &unit.tables.symbols[symbol.index()];
                if symbol.definition == empty_path {
                    let ty = unit.values[result.index()].ty;
                    let LayoutKind::Block { payload } =
                        layouts.record(unit.types[ty.index()].layout).kind
                    else {
                        unreachable!("an empty sequence read as a block")
                    };
                    empty.insert(symbol.instance, (symbol.clone(), payload));
                } else if symbol.kind == SymbolKind::Constant {
                    read.insert(symbol.definition.clone(), symbol.clone());
                }
            }
        }
    }
    let empty = empty
        .into_values()
        .map(|(symbol, payload)| {
            let record = layouts.record(payload);
            // Even an empty sequence contains its complete header and alignment
            // padding before element zero: a counted sequence's count and capacity
            // (runtime ABI 5, both zero), Sequential Packed storage's count and
            // used-byte count. LLVM and H retain this exact static extent.
            let extent = 16usize.next_multiple_of(record.align as usize);
            Ok(hirsel_lir::statics::StaticData::plain(
                symbol,
                payload,
                vec![0; extent],
            ))
        })
        .collect::<Vec<_>>();
    empty
        .into_iter()
        .chain(read.into_values().map(|symbol| {
            let definition = symbol.definition.clone();
            let static_error = |e| LowerError::Static(definition.clone(), e);
            let Some(value) = program.constants.get(&symbol.definition) else {
                return Err(static_error(crate::statics::StaticError(
                    "a constant without a materialized value".into(),
                )));
            };
            crate::statics::encode(layouts, symbol, value).map_err(static_error)
        }))
        .collect()
}

/// The loop plan of one instance, with layout facts of its own types.
pub fn plan_instance(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    layouts: &mut LayoutBuilder<'_>,
) -> Result<(Option<LoopPlan>, Vec<Declined>), NormalizeError> {
    let view = instances.view(id);
    let def: &tir::Definition = &view.definition;
    let actuals = &instances.actuals[id.index()];
    let mut store = actuals.store.clone();
    let owner = crate::instance::import_actuals(actuals, &mut store)
        .map_err(|e| NormalizeError::Instance(InstanceError::Substitution(def.path.clone(), e)))?;
    let abi = &layouts.table().fn_abis[instances.abis[id.index()].0 as usize];
    let passing: Vec<Passing> = abi.parameters.iter().map(|p| p.convention).collect();
    let mut failed = None;
    let mut facts = |ty: TypeId| -> Facts {
        let none = Facts {
            plain: false,
            copyable: false,
        };
        let Ok(t) = substitute(&def.types, ty, actuals, &owner, &mut store, false) else {
            failed.get_or_insert("substitution");
            return none;
        };
        let Ok(layout) = layouts.layout(&store, t) else {
            failed.get_or_insert("layout");
            return none;
        };
        let r = layouts.record(layout);
        let trivial = r.drop.is_empty() && r.finalizer.is_none();
        Facts {
            plain: trivial && r.copy == CopyRecipe::Bits,
            copyable: r.copy != CopyRecipe::NotCopyable,
        }
    };
    crate::normalize::plan(program, instances, id, &passing, &mut facts)
}

/// Tri-state facts joined at merges (definitely no, definitely yes, either).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum Tri {
    No,
    Yes,
    Maybe,
}

impl Tri {
    fn join(self, other: Self) -> Self {
        if self == other { self } else { Tri::Maybe }
    }
    fn may(self) -> bool {
        self != Tri::No
    }
}

/// `Element` flags. Dead storage, an inactive loan, no view: what most of a
/// state is.
const DEAD: u8 = 1;
/// Joined with a `DEAD` element, this one stays as it is.
const ABSORBS: u8 = 2;
/// Storage holding nothing: uninitialized on every path.
const EMPTY: u8 = 4;

impl Element for Tri {
    fn flags(&self) -> u8 {
        match self {
            Tri::No => DEAD | ABSORBS,
            Tri::Maybe => ABSORBS,
            Tri::Yes => 0,
        }
    }
    fn absorbs(own: u8, other: u8) -> bool {
        own & ABSORBS != 0 && other & DEAD != 0
    }
}

/// One field-path step as the validator compares them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum Step {
    Payload,
    Field(u32),
    Variant(u32, u32),
    Index,
}

fn overlap(a: &[Step], b: &[Step]) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| x == y || (*x == Step::Index && *y == Step::Index))
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct SlotState {
    live: Tri,
    init: Tri,
    moved: BTreeMap<Vec<Step>, Tri>,
}

impl SlotState {
    const DEAD: Self = Self {
        live: Tri::No,
        init: Tri::No,
        moved: BTreeMap::new(),
    };
    /// Whether the whole content may still be present (for a whole drop).
    fn present(&self) -> Tri {
        if self.init == Tri::No || self.moved.get(&Vec::new()) == Some(&Tri::Yes) {
            Tri::No
        } else {
            self.init
        }
    }
}

impl SlotState {
    fn join(&mut self, other: &SlotState) {
        self.live = self.live.join(other.live);
        self.init = self.init.join(other.init);
        let keys: BTreeSet<_> = self
            .moved
            .keys()
            .chain(other.moved.keys())
            .cloned()
            .collect();
        for key in keys {
            let l = self.moved.get(&key).copied().unwrap_or(Tri::No);
            let r = other.moved.get(&key).copied().unwrap_or(Tri::No);
            self.moved.insert(key, l.join(r));
        }
    }
}

impl Element for SlotState {
    fn flags(&self) -> u8 {
        let dead = self.live == Tri::No && self.init == Tri::No && self.moved.is_empty();
        // Joined with dead storage, a `Yes` becomes `Maybe` and nothing else
        // changes.
        let absorbs = self.live != Tri::Yes
            && self.init != Tri::Yes
            && self.moved.values().all(|t| *t != Tri::Yes);
        u8::from(dead) * DEAD + u8::from(absorbs) * ABSORBS + u8::from(self.init == Tri::No) * EMPTY
    }
    fn absorbs(own: u8, other: u8) -> bool {
        own & ABSORBS != 0 && other & DEAD != 0
    }
}

/// The abstract state at one program point: per slot, liveness, initialization
/// and moved parts; per loan, whether it is active. It mirrors exactly what the
/// LIR validator derives, so a state-specialized cleanup is valid. Clones share
/// structure (`PVec`), so keeping the state of every edge, comparing two and
/// hashing one cost what changed between them.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct State {
    slots: PVec<SlotState>,
    loans: PVec<Tri>,
    /// Per TIR local, whether a joined view temporary holds a view.
    views: PVec<Tri>,
}

impl State {
    fn join(&mut self, other: &State) {
        // Slots and loans created later in a pass are dead and inactive here.
        fn padded<T: Element>(a: &mut PVec<T>, b: &PVec<T>, fill: T) -> PVec<T> {
            if a.len() < b.len() {
                a.resize(b.len(), fill.clone());
            }
            let mut b = b.clone();
            if b.len() < a.len() {
                b.resize(a.len(), fill);
            }
            b
        }
        let slots = padded(&mut self.slots, &other.slots, SlotState::DEAD);
        self.slots.join(&slots, SlotState::join);
        let loans = padded(&mut self.loans, &other.loans, Tri::No);
        self.loans.join(&loans, |a, b| *a = a.join(*b));
        self.views.join(&other.views, |a, b| *a = a.join(*b));
    }
    fn live(&mut self, slot: SlotId) {
        self.slots[slot.index()] = SlotState {
            live: Tri::Yes,
            init: Tri::No,
            moved: BTreeMap::new(),
        };
    }
    fn dead(&mut self, slot: SlotId) {
        self.slots[slot.index()] = SlotState::DEAD;
    }
    /// A write of `path` (Store/Replace): the part is present again.
    fn write(&mut self, slot: SlotId, path: &[Step]) {
        let s = &mut self.slots[slot.index()];
        if path.is_empty() {
            s.init = Tri::Yes;
            s.moved.clear();
        } else {
            s.moved
                .retain(|q, _| !(q.len() >= path.len() && overlap(q, path)));
        }
    }
    /// A move or drop of `path`.
    fn take(&mut self, slot: SlotId, path: &[Step]) {
        let s = &mut self.slots[slot.index()];
        if path.is_empty() {
            s.init = Tri::No;
            s.moved.clear();
        } else {
            s.moved
                .retain(|q, _| !(q.len() > path.len() && overlap(q, path)));
            s.moved.insert(path.to_vec(), Tri::Yes);
        }
    }
}

/// A resolved TIR place: its slot root, validator steps and LIR projection.
#[derive(Clone, Debug)]
struct PlaceRef {
    slot: SlotId,
    steps: Vec<Step>,
    projection: Vec<ProjectionStep<tir::LocalId>>,
    ty: LirTypeId,
    /// No index or variant step and no block crossing: preparable once.
    fixed: bool,
    /// The place lies inside a block's payload (an element of a list, a
    /// field behind a box) and is named through a temporary (a call's or a
    /// block's result, perhaps a view of a block-local it extends), which
    /// nothing writes again.
    crosses: bool,
    /// The loan a borrowed read of the place is inside.
    loan: Option<tir::LoanId>,
    /// A variable-size part of a flat value: `ty` is its length-prefixed
    /// encoding, this its source type, which only flat-aware operations
    /// produce (LIR schema revision 4).
    source: Option<(LirTypeId, TypeId)>,
    /// The site of the origin binding that named this place: an index or tag
    /// check of it fails there, as in the interpreter (docs/spec/diagnostics.md: the operation
    /// naming the place, not the enclosing statement).
    site: Option<Span>,
}

/// What is known of a place at one reaching version of its slot. Facts flow
/// only along single-predecessor TIR edges, so wherever one holds, the point
/// that established it dominates.
#[derive(Clone, Debug)]
enum Fact {
    /// After a variant switch: the place holds one of `variants`.
    Variant {
        slot: SlotId,
        projection: Vec<ProjectionStep<tir::LocalId>>,
        version: ValueId,
        variants: Vec<u32>,
    },
    /// A field of a proven variant, prepared once at its arm's `BindFields`.
    Prepared {
        slot: SlotId,
        projection: Vec<ProjectionStep<tir::LocalId>>,
        version: ValueId,
        address: AddressId,
    },
}

/// Narrow what `facts` know of a place's variant to `allowed`.
fn narrow(
    facts: &mut Vec<Fact>,
    slot: SlotId,
    projection: &[ProjectionStep<tir::LocalId>],
    version: ValueId,
    allowed: &[u32],
) {
    for fact in facts.iter_mut() {
        if let Fact::Variant {
            slot: s,
            projection: q,
            version: v,
            variants,
        } = fact
            && *s == slot
            && *v == version
            && q == projection
        {
            variants.retain(|variant| allowed.contains(variant));
            return;
        }
    }
    facts.push(Fact::Variant {
        slot,
        projection: projection.to_vec(),
        version,
        variants: allowed.to_vec(),
    });
}

/// Whether a pattern matches every value of its place's type: a failed match
/// of a variant pattern over such fields can only be its own tag test.
fn irrefutable(m: &tir::MatchTable, id: tir::MatchPatternId) -> bool {
    use tir::MatchPatternKind as P;
    match &m.patterns[id.0 as usize].kind {
        P::Wildcard | P::Binding { .. } | P::Literal(tir::Constant::Unit) => true,
        P::Tuple(fields) | P::Record { fields, .. } => {
            fields.iter().all(|(_, sub)| irrefutable(m, *sub))
        }
        P::Literal(_) | P::Variant { .. } => false,
    }
}

/// One cleanup action still to run on an exit or failure path.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum Action {
    /// A TIR cleanup node, anchored at its `node_anchors` entry.
    Tir { node: u32 },
    /// Release an owned slot no TIR action names, anchored at a reserved
    /// `(cleanup, action)` pair.
    Release { slot: SlotId, anchor: (u32, u32) },
    /// End a loan left active where TIR names no cleanup.
    EndLoan { loan: LoanId },
}

/// One block of a failure-cleanup chain: loan ends, then a drop (the last
/// link has none and propagates). It names everything the block's content
/// and its validity depend on: the instructions, and the state of the
/// dropped slot before the drop. An action that does nothing leaves no trace.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct Link {
    ends: Vec<(LoanId, bool)>,
    drop: Option<(AddressId, bool, CleanupAnchorId, SlotState)>,
}

/// Builder-side block, laid out into the unit's arenas by `finish`.
struct BBlock {
    kind: BlockKind,
    params: Vec<ValueId>,
    /// Version joins, after `params`: one parameter per slot.
    phis: Vec<(SlotId, ValueId)>,
    /// Joined views, after `phis`: one parameter per joined view temporary.
    view_phis: Vec<(usize, ValueId)>,
    insts: Vec<Inst>,
    term: Option<Terminator>,
    anchor: SourceAnchorId,
}

struct BEdge {
    target: BlockId,
    args: Vec<ValueId>,
    /// Slot versions at the edge's source, for the target's version joins.
    versions: PVec<Option<ValueId>>,
    /// Joined views at the edge's source, for the target's view joins.
    views: PVec<Option<ValueId>>,
}

/// Arenas of one emission pass.
#[derive(Default)]
struct Out {
    types: Vec<LirType>,
    type_index: HashMap<LirType, LirTypeId>,
    values: Vec<ValueInfo>,
    slots: Vec<SlotInfo>,
    addresses: Vec<AddressInfo>,
    fixed_addresses: HashMap<(SlotId, Vec<ProjectionStep>), AddressId>,
    blocks: Vec<BBlock>,
    edges: Vec<BEdge>,
    tables: hirsel_lir::ir::SideTables,
    pools: hirsel_lir::ir::Pools,
    symbol_index: HashMap<Symbol, SymbolId>,
    operation_index: HashMap<OperationId, OperationRef>,
    source_anchor_index: HashMap<SourceAnchor, SourceAnchorId>,
    cleanup_anchor_index: HashMap<CleanupAnchor, CleanupAnchorId>,
    constant_index: HashMap<ConstantData, ConstantId>,
    parameters: Vec<ValueId>,
}

/// What a failure successor must do besides cleanup.
struct FailureContext {
    /// The TIR cleanup chain the current operation fails into; None derives
    /// the cleanup from the state (operations TIR cannot make fail).
    cleanup: Option<tir::CleanupId>,
    /// Actions that run before that chain (the rest of a normal cleanup list).
    before: Vec<Action>,
    /// Owned values the operation has not consumed yet.
    pending: Vec<ValueId>,
}

/// The importer as an inline lowering sees it: the calling unit at the call
/// being replaced. A lowering declines
/// (returns false) before it emits anything.
pub struct ImportCx<'i> {
    im: &'i mut (dyn Inline + 'i),
}

/// The importer operations an inline lowering is built from.
trait Inline {
    fn operand(&mut self, operand: &tir::Operand) -> R<ValueId>;
    fn read_field(&mut self, value: ValueId, field: u32) -> R<ValueId>;
    fn place_address(&mut self, place: tir::PlaceId) -> R<(AddressId, ValueId)>;
    fn tir_layout(&mut self, ty: TypeId) -> R<LayoutId>;
    fn place_layout(&mut self, place: tir::PlaceId) -> R<LayoutId>;
    fn layout_record(&self, layout: LayoutId) -> &hirsel_lir::layout_types::LayoutRecord;
    fn new_value(&mut self, layout: LayoutId, holding: Option<Holding>) -> ValueId;
    fn literal(&mut self, layout: LayoutId, value: ConstantValue) -> ValueId;
    fn emit(&mut self, inst: Inst);
    fn consume(&mut self, value: ValueId);
    fn pend(&mut self, value: ValueId);
    fn failure(&mut self, rule: Option<String>) -> R<FailureId>;
    fn finish(&mut self, destination: tir::PlaceId, next: tir::BlockId, value: ValueId) -> R<()>;
    fn intern_symbol(&mut self, symbol: &Symbol) -> SymbolId;
    fn iterator_closure(
        &mut self,
        call: &crate::library::CallShape<'_>,
        helper: &DefPath,
    ) -> R<ValueId>;
    fn container_len(&mut self, call: &crate::library::CallShape<'_>) -> R<()>;
    fn construct_inline(
        &mut self,
        layout: LayoutId,
        payload: LayoutId,
        constructor: ConstructorKind,
        values: &[ValueId],
    ) -> R<ValueId>;
    fn iterator_next(&mut self, call: &crate::library::CallShape<'_>) -> R<()>;
    fn direct_call(
        &mut self,
        symbol: SymbolId,
        abi: FnAbiId,
        arguments: Vec<CallArgument>,
    ) -> R<Option<ValueId>>;
}

impl Inline for Importer<'_, '_> {
    fn construct_inline(
        &mut self,
        layout: LayoutId,
        payload: LayoutId,
        constructor: ConstructorKind,
        values: &[ValueId],
    ) -> R<ValueId> {
        let ty = self.intern(layout);
        let result = self.fresh(ty);
        let fields = PoolRange::push(
            &mut self.out.pools.fields,
            values.iter().enumerate().map(|(i, &value)| FieldValue {
                field: i as u32,
                value,
            }),
        );
        let construct = hirsel_lir::ir::ConstructId(self.out.tables.constructs.len() as u32);
        let anchor = self.anchor_here();
        self.out.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            constructor,
            layout: payload,
            fields,
            destination: DestinationClass::Inferred,
            placement: None,
            anchor,
        });
        for &value in values {
            self.consumed(value);
        }
        let failure = self.fail(None)?;
        self.push(Inst::Construct {
            result,
            construct,
            failure,
        });
        if self.holds(result) {
            self.failure.pending.push(result);
        }
        Ok(result)
    }
    fn container_len(&mut self, call: &crate::library::CallShape<'_>) -> R<()> {
        let argument = &call.arguments[0].value;
        let mut place = self.place(argument.place)?;
        let layout = self.layout_of(place.ty);
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) =
            self.layouts.record(layout).kind
        else {
            return malformed("container length of a nonblock");
        };
        let entries = self.layouts.record(payload).fields[0].layout;
        place.steps.push(Step::Field(0));
        place.projection.push(ProjectionStep::Field(0));
        place.fixed = false;
        place.ty = self.intern(entries);
        let address = self.address(&place)?;
        let value = self.operand_at(UseKind::Borrow, &place, address, place.loan)?;
        let layout = self.place_layout(call.destination)?;
        let ty = self.intern(layout);
        let result = self.value(ty, Holding::Plain);
        self.push(Inst::SeqLen { result, value });
        self.finish(call.destination, call.next, result)
    }
    fn operand(&mut self, operand: &tir::Operand) -> R<ValueId> {
        Importer::operand(self, operand)
    }
    fn read_field(&mut self, value: ValueId, field: u32) -> R<ValueId> {
        let layout = self.layout_of(self.value_type(value));
        let shape = match self.layouts.record(layout).kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => layout,
        };
        let layout = self
            .layouts
            .record(shape)
            .fields
            .iter()
            .find(|f| f.ordinal == field)
            .ok_or_else(|| ImportError::Unsupported(format!("field {field} of {shape:?}")))?
            .layout;
        let ty = self.intern(layout);
        let projection = PoolRange::push(
            &mut self.out.pools.projections,
            [ProjectionStep::Field(field)],
        );
        let address = AddressId(self.out.addresses.len() as u32);
        let (base, reaching) = self.out.blocks[self.current.index()]
            .insts
            .iter()
            .find_map(|i| match *i {
                Inst::Read {
                    result,
                    address,
                    reaching,
                    ..
                } if result == value => Some((AddressBase::Address(address), reaching)),
                _ => None,
            })
            .unwrap_or((AddressBase::Value(value), value));
        self.out.addresses.push(AddressInfo {
            base,
            projection,
            ty,
        });
        let failure = self.fail(None)?;
        self.push(Inst::Address { address, failure });
        let holding = self.out.values[value.index()].holding;
        let result = self.value(ty, holding);
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode: ReadMode::Borrow,
        });
        Ok(result)
    }
    fn place_address(&mut self, place: tir::PlaceId) -> R<(AddressId, ValueId)> {
        let p = self.place(place)?;
        let address = self.address(&p)?;
        Ok((address, self.reaching(p.slot)?))
    }
    fn tir_layout(&mut self, ty: TypeId) -> R<LayoutId> {
        let t = self.ty(ty)?;
        self.layout(t)
    }
    fn place_layout(&mut self, place: tir::PlaceId) -> R<LayoutId> {
        let Some(p) = self.body.places.get(place.0 as usize) else {
            return malformed(format!("place {} outside the body", place.0));
        };
        self.tir_layout(p.ty)
    }
    fn layout_record(&self, layout: LayoutId) -> &hirsel_lir::layout_types::LayoutRecord {
        self.layouts.record(layout)
    }
    fn new_value(&mut self, layout: LayoutId, holding: Option<Holding>) -> ValueId {
        let ty = self.intern(layout);
        match holding {
            Some(holding) => self.value(ty, holding),
            None => self.fresh(ty),
        }
    }
    fn literal(&mut self, layout: LayoutId, value: ConstantValue) -> ValueId {
        let ty = self.intern(layout);
        let constant = self.constant(ty, value);
        let result = self.new_value(layout, Some(Holding::Plain));
        self.push(Inst::Const { result, constant });
        result
    }
    fn emit(&mut self, inst: Inst) {
        self.push(inst);
    }
    fn consume(&mut self, value: ValueId) {
        self.consumed(value);
    }
    fn pend(&mut self, value: ValueId) {
        if self.holds(value) {
            self.failure.pending.push(value);
        }
    }
    fn failure(&mut self, rule: Option<String>) -> R<FailureId> {
        self.fail(rule)
    }
    fn finish(&mut self, destination: tir::PlaceId, next: tir::BlockId, value: ValueId) -> R<()> {
        let dest = self.place(destination)?;
        let address = self.address(&dest)?;
        self.store(&dest, address, value)?;
        let e = self.enter(next);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }
    fn intern_symbol(&mut self, symbol: &Symbol) -> SymbolId {
        self.symbol(symbol.kind, &symbol.definition, symbol.instance)
    }
    fn iterator_closure(
        &mut self,
        call: &crate::library::CallShape<'_>,
        helper: &DefPath,
    ) -> R<ValueId> {
        let body = crate::library::body_index(helper).expect("registered iterator thunk");
        let mut request = self.library_request(
            body,
            &crate::library::core_path("iter.next"),
            &call.generics[1..],
        )?;
        request.path = helper.clone();
        let mut callable = request.actuals.store.get(request.callable).unwrap().clone();
        let hirsel_types::TypeKind::Function(ref mut function) = callable.kind else {
            unreachable!()
        };
        function.parameters = Vec::new().into();
        request.callable = request.actuals.store.push(callable);
        request.shape = CallableShape::IterNext;
        let mut fields = Vec::new();
        let mut values = Vec::new();
        // Keep the actual captured callable type, including its invocation ABI,
        // rather than reconstructing it from the operation's receiving type.
        for argument in call.arguments {
            let source = self.body.places[argument.value.place.0 as usize].ty;
            let ty = self.ty(source)?;
            let imported = hirsel_types::substitute_type(
                &self.store,
                ty,
                &mut request.actuals.store,
                Default::default(),
            )
            .map_err(|e| ImportError::Instance(InstanceError::Substitution(helper.clone(), e)))?;
            fields.push((imported, Passing::Take));
            values.push(CaptureValue {
                value: self.operand(&argument.value)?,
                usage: hirsel_lir::ir::CaptureUse::Take,
            });
        }
        // The capture types complete the helper instance's identity. The first
        // two actuals are the physical capture types; its result is already closed.
        request.actuals.arguments = fields
            .iter()
            .map(|(ty, _)| GenericArgument::Type(*ty))
            .collect();
        request.environment = Some(
            self.layouts
                .environment(&hirsel_lir::layout::EnvironmentShape {
                    definition: helper,
                    body: 0,
                    store: &request.actuals.store,
                    captures: &fields,
                    sizes: &[],
                    movable: &[],
                })
                .map_err(|e| ImportError::Layout(helper.clone(), e))?,
        );
        let environment = request.environment.unwrap();
        let direct = self.require(request)?;
        let code = self.symbol(SymbolKind::Function, helper, direct.symbol.instance);
        let anchor = self.anchor_here();
        let captures = PoolRange::push(&mut self.out.pools.captures, values.iter().copied());
        let closure = hirsel_lir::ir::ClosureId(self.out.tables.closures.len() as u32);
        self.out.tables.closures.push(ClosureSite {
            code: Some(ClosureCode {
                symbol: code,
                abi: direct.abi,
            }),
            environment,
            captures,
            destination: DestinationClass::Inferred,
            placement: None,
            anchor,
        });
        for c in values {
            self.consumed(c.value);
        }
        let layout = self.place_layout(call.destination)?;
        let result = self.new_value(layout, None);
        let failure = self.fail(None)?;
        self.push(Inst::Closure {
            result,
            closure,
            failure,
        });
        if self.holds(result) {
            self.failure.pending.push(result);
        }
        Ok(result)
    }

    fn iterator_next(&mut self, call: &crate::library::CallShape<'_>) -> R<()> {
        let value = call.arguments[0].value.clone();
        // The iterator's words stay in its exclusive source place. Only the
        // environment changes; call its step through that borrowed value.
        let callee = self.operand(&value)?;
        let body =
            crate::library::body_index(&crate::library::native_path("iter_from_step")).unwrap();
        let mut request = self.library_request(body, call.path, call.generics)?;
        let mut callable = request.actuals.store.get(request.callable).unwrap().clone();
        let hirsel_types::TypeKind::Function(ref mut function) = callable.kind else {
            unreachable!()
        };
        function.parameters = Vec::new().into();
        let callable = request.actuals.store.push(callable);
        let abi = self
            .layouts
            .fn_abi(&request.actuals.store, callable, CallableShape::IterNext)
            .map_err(|e| ImportError::Layout(self.def.path.clone(), e))?;
        let fn_abi = self.layouts.table().fn_abis[abi.0 as usize].clone();
        let result = self.new_value(fn_abi.result.unwrap().layout, None);
        self.consumed(callee);
        let failure = self.fail(None)?;
        let anchor = self.anchor_here();
        let id = CallId(self.out.tables.calls.len() as u32);
        self.out.tables.calls.push(CallSite {
            operation_arguments: Vec::new(),
            callee: Callee::Indirect(callee),
            abi,
            arguments: PoolRange::EMPTY,
            result: Some(result),
            destination: fn_abi.destination.then_some(DestinationClass::Inferred),
            declared: CALL_EFFECTS,
            anchor,
        });
        let after = self.block(BlockKind::Body);
        let normal = self.edge(after, Vec::new());
        self.terminate(Terminator::Call {
            call: id,
            normal: Some(normal),
            failure,
        });
        self.current = after;
        if self.holds(result) {
            self.failure.pending.push(result);
        }
        self.finish(call.destination, call.next, result)
    }

    fn direct_call(
        &mut self,
        symbol: SymbolId,
        abi: FnAbiId,
        arguments: Vec<CallArgument>,
    ) -> R<Option<ValueId>> {
        let fn_abi = self.layouts.table().fn_abis[abi.0 as usize].clone();
        for a in &arguments {
            if let ArgumentValue::Value(v) = a.value {
                self.consumed(v);
            }
        }
        let result = fn_abi.result.as_ref().map(|r| {
            let ty = self.intern(r.layout);
            self.fresh(ty)
        });
        let failure = self.fail(None)?;
        let anchor = self.anchor_here();
        let arguments = PoolRange::push(&mut self.out.pools.call_arguments, arguments);
        let call = CallId(self.out.tables.calls.len() as u32);
        self.out.tables.calls.push(CallSite {
            operation_arguments: Vec::new(),
            callee: Callee::Direct(symbol),
            abi,
            arguments,
            result,
            destination: fn_abi.destination.then_some(DestinationClass::Inferred),
            declared: CALL_EFFECTS,
            anchor,
        });
        let after = self.block(BlockKind::Body);
        let normal = self.edge(after, Vec::new());
        self.terminate(Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        });
        self.current = after;
        if let Some(v) = result
            && self.holds(v)
        {
            self.failure.pending.push(v);
        }
        Ok(result)
    }
}

impl ImportCx<'_> {
    pub fn read_field(&mut self, value: ValueId, field: u32) -> R<ValueId> {
        self.im.read_field(value, field)
    }
    /// The value of a call argument as the call would pass it: a `borrow`
    /// argument is a view under its TIR loan, a `take` argument is moved
    /// out of its place.
    pub fn operand(&mut self, operand: &tir::Operand) -> R<ValueId> {
        self.im.operand(operand)
    }
    /// The address of a TIR place (a `mut` argument's, say) and the
    /// version of its root slot that reaches the call.
    pub fn place_address(&mut self, place: tir::PlaceId) -> R<(AddressId, ValueId)> {
        self.im.place_address(place)
    }
    /// The layout of a TIR place: the call's destination, say.
    pub fn place_layout(&mut self, place: tir::PlaceId) -> R<LayoutId> {
        self.im.place_layout(place)
    }
    pub fn record(&self, layout: LayoutId) -> &hirsel_lir::layout_types::LayoutRecord {
        self.im.layout_record(layout)
    }
    /// A freshly produced value of `layout`: owned if the layout carries a
    /// credit, plain otherwise.
    pub fn fresh(&mut self, layout: LayoutId) -> ValueId {
        self.im.new_value(layout, None)
    }
    pub fn value(&mut self, layout: LayoutId, holding: Holding) -> ValueId {
        self.im.new_value(layout, Some(holding))
    }
    /// A literal produced by an inline lowering (including a unit result).
    pub fn constant(&mut self, layout: LayoutId, value: ConstantValue) -> ValueId {
        self.im.literal(layout, value)
    }
    /// Append an instruction at the call.
    pub fn push(&mut self, inst: Inst) {
        self.im.emit(inst);
    }
    /// An owned value an instruction just consumed: the call's failure
    /// cleanup no longer releases it.
    pub fn consumed(&mut self, value: ValueId) {
        self.im.consume(value);
    }
    /// An owned value the lowering produced and has not stored yet: a
    /// later failure of the call releases it.
    pub fn pending(&mut self, value: ValueId) {
        self.im.pend(value);
    }
    /// A failure edge at the call, into the call's own cleanup.
    pub fn failure(&mut self, rule: Option<&str>) -> R<FailureId> {
        self.im.failure(rule.map(str::to_owned))
    }
    /// Store the call's result and continue after the call. The last thing
    /// a lowering does.
    pub fn finish(&mut self, call: &crate::library::CallShape<'_>, value: ValueId) -> R<()> {
        self.im.finish(call.destination, call.next, value)
    }
    pub fn symbol(&mut self, symbol: &Symbol) -> SymbolId {
        self.im.intern_symbol(symbol)
    }
    /// Construct an inline library value, retaining failure ownership until stored.
    pub fn construct(
        &mut self,
        layout: LayoutId,
        payload: LayoutId,
        constructor: ConstructorKind,
        values: &[ValueId],
    ) -> R<ValueId> {
        self.im
            .construct_inline(layout, payload, constructor, values)
    }
    /// Read field zero's sequence count under the caller's existing loan.
    pub fn container_len(&mut self, call: &crate::library::CallShape<'_>) -> R<()> {
        self.im.container_len(call)
    }
    /// Construct an erased iterator environment and require its step body.
    pub fn iterator_closure(
        &mut self,
        call: &crate::library::CallShape<'_>,
        helper: &DefPath,
    ) -> R<ValueId> {
        self.im.iterator_closure(call, helper)
    }
    /// Advance the exclusive iterator place with one indirect step call.
    pub fn iterator_next(&mut self, call: &crate::library::CallShape<'_>) -> R<()> {
        self.im.iterator_next(call)
    }
}

pub fn import(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    plan: Option<&LoopPlan>,
    layouts: &mut LayoutBuilder<'_>,
    required: &mut Vec<crate::library::Request>,
) -> R<LirUnit<Value>> {
    let view = instances.view(id);
    let def: &tir::Definition = &view.definition;
    let body = &def.bodies[view.body.0 as usize];
    let actuals = &instances.actuals[id.index()];
    let mut store = actuals.store.clone();
    let store_actuals = crate::instance::import_actuals(actuals, &mut store)
        .map_err(|e| InstanceError::Substitution(def.path.clone(), e))?;
    let mut im = Importer {
        program,
        instances,
        layouts,
        id,
        plan,
        def,
        body,
        body_id: view.body.0,
        actuals,
        store,
        store_actuals,
        type_memo: HashMap::new(),
        required,
        finalizer_memo: std::collections::HashSet::new(),
        out: Out::default(),
        local_slot: Vec::new(),
        aliases: view_temporaries(body),
        observations: discarded_callable_reads(body, &def.types),
        node_anchors: node_anchors(body),
        joined: joined_views(body),
        views: PVec::default(),
        entry_views: Vec::new(),
        loop_slots: Vec::new(),
        loan_map: Vec::new(),
        elided_loans: body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| {
                if let tir::InstructionKind::BeginLoan {
                    id: loan, place, ..
                } = i.kind
                    && instances
                        .elided
                        .get(&id)
                        .is_some_and(|locals| locals.contains(&body.places[place.0 as usize].root))
                {
                    Some(loan)
                } else {
                    None
                }
            })
            .collect(),
        builders: flat_builder::Builders::default(),
        fields: flat_field::Fields::default(),
        tir_block: Vec::new(),
        call_block: tir::BlockId(0),
        versioned: Vec::new(),
        constant_version: PVec::default(),
        prologue: BlockId(0),
        prologue_failure: None,
        unreachable: None,
        propagate: None,
        current: BlockId(0),
        state: State {
            slots: PVec::default(),
            loans: PVec::default(),
            views: PVec::default(),
        },
        versions: PVec::default(),
        failure: FailureContext {
            cleanup: None,
            before: Vec::new(),
            pending: Vec::new(),
        },
        chain_memo: HashMap::new(),
        incoming: Vec::new(),
        entry_versions: Vec::new(),
        facts: Vec::new(),
        entry_facts: Vec::new(),
        tir_preds: Vec::new(),
        rpo: Vec::new(),
        capture_versions: Vec::new(),
        outer_scope: false,
        span: Span::default(),
        statement: (0, 0),
        origin_sites: HashMap::new(),
    };
    im.builders = im.builder_webs()?;
    let loans: Vec<_> = im.builders.loans.iter().copied().collect();
    im.elided_loans.extend(loans);
    im.fields = im.field_webs()?;
    let loans: Vec<_> = im.fields.loans.iter().copied().collect();
    im.elided_loans.extend(loans);
    im.cfg()?;
    let mut entry: Vec<Option<State>> = vec![None; body.blocks.len()];
    // Abstract states only grow (towards Maybe), so this reaches a fixpoint.
    for _ in 0..1000 {
        let used = im.pass(&entry)?;
        if converged(&used, &entry) {
            return im.finish();
        }
        entry = used;
    }
    malformed("abstract state did not converge")
}

/// Whether two passes agree on every block's entry state. The states of one
/// pass share structure with each other and none with the other pass's, so
/// the comparison remembers the parts it has already found equal.
fn converged(used: &[Option<State>], entry: &[Option<State>]) -> bool {
    let (mut slots, mut loans, mut views) = Default::default();
    used.len() == entry.len()
        && used.iter().zip(entry).all(|pair| match pair {
            (Some(a), Some(b)) => {
                a.slots.matches(&b.slots, &mut slots)
                    && a.loans.matches(&b.loans, &mut loans)
                    && a.views.matches(&b.views, &mut views)
            }
            (None, None) => true,
            _ => false,
        })
}

struct Importer<'a, 'o> {
    program: &'a Program,
    instances: &'a Instances,
    layouts: &'a mut LayoutBuilder<'o>,
    id: InstanceId,
    plan: Option<&'a LoopPlan>,
    def: &'a tir::Definition,
    body: &'a tir::Body,
    body_id: u32,
    actuals: &'a Actuals,
    /// The instance's working store: its actuals plus substituted TIR types.
    store: TypeStore,
    store_actuals: Vec<GenericArgument>,
    type_memo: HashMap<TypeId, TypeId>,
    /// Library instances this unit requires; `lower` registers them.
    required: &'a mut Vec<crate::library::Request>,
    /// Store types already searched for library resources.
    finalizer_memo: std::collections::HashSet<TypeId>,
    out: Out,
    local_slot: Vec<Option<SlotId>>,
    /// A view temporary: the place it views, read through instead of storage.
    aliases: Vec<Option<tir::PlaceId>>,
    /// Unread callable borrow temporaries evaluate their source without a store.
    observations: Vec<bool>,
    /// Per TIR cleanup node, the `(cleanup, ordinal)` its drop anchors at.
    node_anchors: Vec<(u32, u32)>,
    /// A joined view temporary: assigned views of places under loans on
    /// several paths, never stored. Its value is SSA, joined at merges into a
    /// borrowed block parameter covered by the union of its incoming loans.
    joined: Vec<bool>,
    /// Per TIR local, the joined view it holds here.
    views: PVec<Option<ValueId>>,
    /// Per single-predecessor TIR block, the joined views its edge carries.
    entry_views: Vec<Option<PVec<Option<ValueId>>>>,
    /// Self-tail loop slot of each TIR parameter ordinal (LANG-10).
    loop_slots: Vec<Option<SlotId>>,
    loan_map: Vec<LoanId>,
    elided_loans: std::collections::HashSet<tir::LoanId>,
    /// Lists built in their flat encoding (LIR schema revision 22).
    builders: flat_builder::Builders,
    /// Byte lists built in place in a builder's element (revision 24).
    fields: flat_field::Fields,
    tir_block: Vec<Option<BlockId>>,
    /// The TIR block whose call terminator is being lowered.
    call_block: tir::BlockId,
    /// Slots written somewhere after entry: candidates for version joins.
    versioned: Vec<bool>,
    /// The single version of a slot written only at entry (parameters).
    constant_version: PVec<Option<ValueId>>,
    prologue: BlockId,
    prologue_failure: Option<BlockId>,
    unreachable: Option<BlockId>,
    propagate: Option<BlockId>,
    current: BlockId,
    state: State,
    versions: PVec<Option<ValueId>>,
    failure: FailureContext,
    /// Failure-cleanup blocks by what they run and where they continue.
    chain_memo: HashMap<(Link, Option<BlockId>), BlockId>,
    /// Per TIR block, the join of states reaching it in this pass.
    incoming: Vec<Option<State>>,
    /// Per single-predecessor TIR block, the versions its edge carries.
    entry_versions: Vec<Option<PVec<Option<ValueId>>>>,
    /// Facts holding here, and per single-predecessor TIR block, those its edge carries.
    facts: Vec<Fact>,
    entry_facts: Vec<Option<Vec<Fact>>>,
    tir_preds: Vec<u32>,
    rpo: Vec<tir::BlockId>,
    /// Per capture ordinal, the version naming its environment content.
    capture_versions: Vec<ValueId>,
    /// Whether a scratch slot lives in the synthetic scope enclosing the
    /// body's scopes (ordinal `body.scopes.len()`).
    outer_scope: bool,
    /// Site and TIR statement of the operation being imported.
    span: Span,
    statement: (u32, u32),
    /// Per TIR place an origin event named, the site of the first one.
    origin_sites: HashMap<tir::PlaceId, Span>,
}

/// The anchor of each TIR cleanup node: its position in the first cleanup
/// (by id) whose chain runs it. Chains share suffixes, so one action has a
/// `(cleanup, ordinal)` in every chain through it; naming it by one of them
/// lets every failure site that runs it share the block that does.
/// Each node is walked once.
fn node_anchors(body: &tir::Body) -> Vec<(u32, u32)> {
    let mut out: Vec<Option<(u32, u32)>> = vec![None; body.cleanup_nodes.len()];
    for (c, cleanup) in body.cleanups.iter().enumerate() {
        let mut next = cleanup.head;
        let mut ordinal = 0;
        while let Some(node) = next {
            let slot = &mut out[node.0 as usize];
            if slot.is_some() {
                break;
            }
            *slot = Some((c as u32, ordinal));
            ordinal += 1;
            next = body.cleanup_nodes[node.0 as usize].parent;
        }
    }
    out.into_iter()
        .map(|a| a.unwrap_or((u32::MAX, 0)))
        .collect()
}

/// Temporaries that only ever hold a view: assigned once by a borrowed use of
/// a place under a loan (`_1 = use(Borrow _0@L0)`), never written, moved or
/// lent mutably afterwards, and viewing no computed element (an index could
/// change before a later read). Their reads go to the viewed place itself.
fn view_temporaries(body: &tir::Body) -> Vec<Option<tir::PlaceId>> {
    let (assigned, disqualified) = view_assignments(body);
    let n = body.locals.len();
    let mut out: Vec<Option<tir::PlaceId>> = (0..n)
        .map(|l| match assigned[l].as_slice() {
            [Some(viewed)] if view_storage(body, l) && !disqualified[l] => Some(*viewed),
            _ => None,
        })
        .collect();
    // A view of a view is fine; a cycle is not possible in validated TIR, but
    // never follow one.
    for l in 0..n {
        let mut seen = BTreeSet::new();
        let mut at = l;
        while let Some(p) = out[at] {
            if !seen.insert(at) {
                out[l] = None;
                break;
            }
            at = body.places[p.0 as usize].root.0 as usize;
        }
    }
    out
}

/// Temporaries a view temporary would be but for being assigned a view on
/// more than one path (`if c { &a.f } else { &b.f }`): joined views.
fn joined_views(body: &tir::Body) -> Vec<bool> {
    let (assigned, disqualified) = view_assignments(body);
    (0..body.locals.len())
        .map(|l| {
            assigned[l].len() > 1
                && assigned[l].iter().all(Option::is_some)
                && view_storage(body, l)
                && !disqualified[l]
        })
        .collect()
}

fn view_storage(body: &tir::Body, l: usize) -> bool {
    matches!(
        body.locals[l].storage,
        tir::Storage::Temporary | tir::Storage::Local
    )
}

/// A discarded callable view still observes bounds/tags at its assignment.
/// Its borrowed SSA result needs no owning storage or credit. Function locals
/// cannot be index components in validated TIR; every other use is counted here.
fn discarded_callable_reads(body: &tir::Body, types: &TypeStore) -> Vec<bool> {
    use tir::InstructionKind as K;
    let mut assigned = vec![0u32; body.locals.len()];
    let mut candidate = vec![false; body.locals.len()];
    let mut used = vec![false; body.locals.len()];
    let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    for rv in &body.rvalues {
        for o in rvalue_operands(rv) {
            used[root(o.place)] = true;
        }
    }
    for block in &body.blocks {
        for i in &block.instructions {
            match &i.kind {
                K::Assign { destination, value } => {
                    let r = root(*destination);
                    assigned[r] += 1;
                    candidate[r] = body.places[destination.0 as usize].projection.is_empty()
                        && body.locals[r].storage == tir::Storage::Temporary
                        && matches!(
                            types.get(body.locals[r].ty).map(|t| &t.kind),
                            Some(hirsel_types::TypeKind::Function(_))
                        )
                        && matches!(&body.rvalues[value.0 as usize], tir::Rvalue::Use(o)
                            if o.usage == UseKind::Borrow);
                }
                K::BindOrigin { source, place, .. } => {
                    used[root(*source)] = true;
                    used[root(*place)] = true;
                }
                K::BindFields { source, fields } => {
                    used[root(*source)] = true;
                    for p in fields {
                        used[root(*p)] = true;
                    }
                }
                K::Replace {
                    destination,
                    value,
                    displaced,
                    ..
                } => {
                    for p in [*destination, value.place, *displaced] {
                        used[root(p)] = true;
                    }
                }
                K::Snapshot { destination, value } => {
                    used[root(*destination)] = true;
                    used[root(value.place)] = true;
                }
                K::Defuse { place }
                | K::EndOrigin(place)
                | K::BeginLoan { place, .. }
                | K::Drop { place, .. } => used[root(*place)] = true,
                K::Check { condition, .. } => used[root(condition.place)] = true,
                K::StorageLive(_)
                | K::StorageDead { .. }
                | K::EndScope(_)
                | K::EndLoan { .. }
                | K::RunCleanup(_) => {}
            }
        }
        match &block.terminator {
            tir::Terminator::Call {
                callee,
                arguments,
                returns,
                ..
            } => {
                if let Some(tir::CallReturn { destination, .. }) = returns {
                    used[root(*destination)] = true;
                }
                if let tir::Callee::Indirect { value, .. } = callee {
                    used[root(value.place)] = true;
                }
                for a in arguments {
                    used[root(a.value.place)] = true;
                }
            }
            tir::Terminator::Return { value, .. }
            | tir::Terminator::Branch {
                condition: value, ..
            }
            | tir::Terminator::Switch { value, .. } => used[root(value.place)] = true,
            _ => {}
        }
    }
    for table in &body.matches {
        used[root(table.scrutinee)] = true;
        for p in &table.patterns {
            used[root(p.place)] = true;
        }
    }
    for node in &body.cleanup_nodes {
        if let tir::CleanupAction::Drop { place, .. } = node.action {
            used[root(place)] = true;
        }
    }
    candidate
        .into_iter()
        .enumerate()
        .map(|(i, yes)| yes && assigned[i] == 1 && !used[i])
        .collect()
}

/// Per local, the view each assignment gives it (None: not a view under a
/// loan), and whether any other use rules a view temporary out.
#[allow(clippy::type_complexity)]
fn view_assignments(body: &tir::Body) -> (Vec<Vec<Option<tir::PlaceId>>>, Vec<bool>) {
    use tir::InstructionKind as K;
    let n = body.locals.len();
    let mut assigned: Vec<Vec<Option<tir::PlaceId>>> = vec![Vec::new(); n];
    let mut disqualified = vec![false; n];
    let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    let whole = |p: tir::PlaceId| body.places[p.0 as usize].projection.is_empty();
    let operand = |o: &tir::Operand, disqualified: &mut Vec<bool>| {
        if matches!(o.usage, UseKind::Take | UseKind::Mut) {
            disqualified[root(o.place)] = true;
        }
    };
    for block in &body.blocks {
        for i in &block.instructions {
            match &i.kind {
                K::Assign { destination, value } => {
                    let r = root(*destination);
                    match &body.rvalues[value.0 as usize] {
                        tir::Rvalue::Use(o)
                            if o.usage == UseKind::Borrow && whole(*destination) =>
                        {
                            let viewed = &body.places[o.place.0 as usize];
                            let computed = viewed
                                .projection
                                .iter()
                                .any(|p| matches!(p, tir::Projection::Index(_)));
                            assigned[r]
                                .push((viewed.loan.is_some() && !computed).then_some(o.place));
                        }
                        other => {
                            assigned[r].push(None);
                            for o in rvalue_operands(other) {
                                operand(o, &mut disqualified);
                            }
                        }
                    }
                }
                K::Replace {
                    destination,
                    value,
                    displaced,
                    ..
                } => {
                    disqualified[root(*destination)] = true;
                    disqualified[root(*displaced)] = true;
                    operand(value, &mut disqualified);
                }
                K::Snapshot { destination, value } => {
                    disqualified[root(*destination)] = true;
                    operand(value, &mut disqualified);
                }
                K::Check { condition, .. } => operand(condition, &mut disqualified),
                K::BeginLoan {
                    place,
                    mutable: true,
                    ..
                } => disqualified[root(*place)] = true,
                _ => {}
            }
        }
        match &block.terminator {
            tir::Terminator::Call {
                callee,
                arguments,
                returns,
                ..
            } => {
                if let Some(tir::CallReturn { destination, .. }) = returns {
                    disqualified[root(*destination)] = true;
                }
                if let tir::Callee::Indirect { value, .. } = callee {
                    operand(value, &mut disqualified);
                }
                for a in arguments {
                    operand(&a.value, &mut disqualified);
                }
            }
            tir::Terminator::Return { value, .. }
            | tir::Terminator::Branch {
                condition: value, ..
            }
            | tir::Terminator::Switch { value, .. } => operand(value, &mut disqualified),
            _ => {}
        }
    }
    (assigned, disqualified)
}

/// A joined view parameter is covered by the union of the loans of the views
/// it joins (joined ones transitively) and declares the least of them; one of
/// plain views is plain.
fn join_view_holdings(values: &mut [ValueInfo], incoming: &BTreeMap<ValueId, Vec<ValueId>>) {
    let mut loans: BTreeMap<ValueId, BTreeSet<LoanId>> = BTreeMap::new();
    loop {
        let mut changed = false;
        for (&p, args) in incoming {
            let mut union = loans.get(&p).cloned().unwrap_or_default();
            for a in args {
                match (incoming.contains_key(a), values[a.index()].holding) {
                    (true, _) => union.extend(loans.get(a).into_iter().flatten()),
                    (false, Holding::Borrowed(l)) => {
                        union.insert(l);
                    }
                    (false, _) => {}
                }
            }
            if loans.get(&p) != Some(&union) {
                loans.insert(p, union);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (p, union) in loans {
        if let Some(&least) = union.first() {
            values[p.index()].holding = Holding::Borrowed(least);
        }
    }
}

fn rvalue_operands(value: &tir::Rvalue) -> Vec<&tir::Operand> {
    match value {
        tir::Rvalue::Use(o) | tir::Rvalue::Unary { value: o, .. } => vec![o],
        tir::Rvalue::Binary { left, right, .. } => vec![left, right],
        tir::Rvalue::Construct { fields, .. } => fields.iter().map(|f| &f.value).collect(),
        tir::Rvalue::Closure { captures, .. } => captures.iter().collect(),
        _ => Vec::new(),
    }
}

/// The successors of a TIR block (Match: each candidate arm's guard or target).
fn tir_successors(body: &tir::Body, block: &tir::Block) -> Vec<tir::BlockId> {
    use tir::Terminator as T;
    match &block.terminator {
        T::Goto(b) => vec![*b],
        T::Branch { yes, no, .. } => vec![*yes, *no],
        T::Switch {
            arms, otherwise, ..
        } => arms.iter().map(|a| a.1).chain([*otherwise]).collect(),
        T::Call { returns, .. } => returns.iter().map(|r| r.next).collect(),
        T::Match { table, from } => body.matches[table.0 as usize].arms[*from as usize..]
            .iter()
            .map(|arm| arm.guard.unwrap_or(arm.target))
            .collect(),
        T::Return { .. } | T::Fail { .. } | T::Unreachable => Vec::new(),
    }
}

fn lir_binary(op: hirsel_syntax::BinaryOp) -> Option<BinaryOp> {
    use hirsel_syntax::BinaryOp as S;
    Some(match op {
        S::Add => BinaryOp::Add,
        S::Sub => BinaryOp::Sub,
        S::Mul => BinaryOp::Mul,
        S::Div => BinaryOp::Div,
        S::Rem => BinaryOp::Rem,
        S::Shl => BinaryOp::Shl,
        S::Shr => BinaryOp::Shr,
        S::BitAnd => BinaryOp::BitAnd,
        S::BitOr => BinaryOp::BitOr,
        S::BitXor => BinaryOp::BitXor,
        S::Eq => BinaryOp::Eq,
        S::Ne => BinaryOp::Ne,
        S::Lt => BinaryOp::Lt,
        S::Le => BinaryOp::Le,
        S::Gt => BinaryOp::Gt,
        S::Ge => BinaryOp::Ge,
        S::And | S::Or | S::Range | S::RangeInclusive => return None,
    })
}

pub(crate) fn repr_of(kind: &LayoutKind) -> ValueRepr {
    match kind {
        LayoutKind::Unit => ValueRepr::Unit,
        LayoutKind::Scalar(k) => ValueRepr::Scalar(*k),
        LayoutKind::Block { .. } | LayoutKind::Box { .. } => ValueRepr::Block,
        LayoutKind::Callable { .. } => ValueRepr::Callable,
        LayoutKind::Handle => ValueRepr::Handle,
        LayoutKind::Path => ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
        LayoutKind::Arena => ValueRepr::Arena,
        LayoutKind::Header => ValueRepr::Token,
        _ => ValueRepr::Aggregate,
    }
}

/// Whether `path` is the core library's `module.name`.
fn core_operation(path: &DefPath, module: &str, name: &str) -> bool {
    let item = &path.0;
    item.package.name == "core" && item.modules == [module] && item.name == name
}

/// A length-prefixed FlatFormatV0 encoding: a dynamic-size Flat record whose
/// one field, its content, follows the u64 byte-length prefix (offset 8) or
/// the prefix and the element count of a list of variable elements (16).
fn flat_prefixed(layouts: &LayoutBuilder<'_>, layout: LayoutId) -> Option<(u64, LayoutId)> {
    let r = layouts.record(layout);
    match (&r.kind, &r.size, r.fields.as_slice()) {
        (LayoutKind::Flat, SizeSpec::Dynamic(_), [only]) if matches!(only.offset, 8 | 16) => {
            Some((only.offset, only.layout))
        }
        _ => None,
    }
}

/// The element of a flat sequence encoding (List, Array, String, Bytes).
fn flat_sequence(layouts: &LayoutBuilder<'_>, layout: LayoutId) -> Option<LayoutId> {
    match flat_prefixed(layouts, layout)? {
        (16, element) => Some(element),
        (_, element) => flat_fixed(layouts, element).then_some(element),
    }
}

fn flat_fixed(layouts: &LayoutBuilder<'_>, layout: LayoutId) -> bool {
    matches!(layouts.record(layout).size, SizeSpec::Fixed(_))
}

fn site(span: Span) -> SiteRange {
    SiteRange {
        start: span.start,
        end: span.end,
    }
}

impl Importer<'_, '_> {
    /// TIR reachability, predecessor counts and reverse postorder. A normalized
    /// self-tail site's back edge targets the entry block.
    fn cfg(&mut self) -> R<()> {
        let n = self.body.blocks.len();
        let successors = |b: tir::BlockId| -> Vec<tir::BlockId> {
            if let Some(plan) = self.plan
                && plan.site(b).is_some()
            {
                return vec![self.body.entry];
            }
            tir_successors(self.body, &self.body.blocks[b.0 as usize])
        };
        let mut seen = vec![false; n];
        let mut post = Vec::new();
        let mut stack = vec![(self.body.entry, 0usize)];
        seen[self.body.entry.0 as usize] = true;
        while let Some((b, i)) = stack.pop() {
            let succ = successors(b);
            if i < succ.len() {
                stack.push((b, i + 1));
                let s = succ[i];
                if (s.0 as usize) >= n {
                    return malformed("successor outside the body");
                }
                if !seen[s.0 as usize] {
                    seen[s.0 as usize] = true;
                    stack.push((s, 0));
                }
            } else {
                post.push(b);
            }
        }
        post.reverse();
        self.rpo = post;
        self.tir_preds = vec![0; n];
        for &b in &self.rpo {
            for s in successors(b) {
                self.tir_preds[s.0 as usize] += 1;
            }
        }
        Ok(())
    }

    // --- types and values ---

    /// A TIR type substituted into the instance store.
    fn ty(&mut self, ty: TypeId) -> R<TypeId> {
        if let Some(&t) = self.type_memo.get(&ty) {
            return Ok(t);
        }
        let t = substitute(
            &self.def.types,
            ty,
            self.actuals,
            &self.store_actuals,
            &mut self.store,
            false,
        )
        .map_err(|e| InstanceError::Substitution(self.def.path.clone(), e))?;
        self.type_memo.insert(ty, t);
        Ok(t)
    }

    fn layout(&mut self, store_ty: TypeId) -> R<LayoutId> {
        let layout = self
            .layouts
            .layout(&self.store, store_ty)
            .map_err(|e| ImportError::Layout(self.def.path.clone(), e))?;
        self.library_finalizers(store_ty);
        Ok(layout)
    }

    /// Require the drop body of every library resource inside a type this
    /// unit lays out: cleanup lowering calls it where the value is dropped.
    fn library_finalizers(&mut self, store_ty: TypeId) {
        let mut todo = vec![store_ty];
        while let Some(ty) = todo.pop() {
            if !self.finalizer_memo.insert(ty) {
                continue;
            }
            let Some(row) = self.store.get(ty) else {
                continue;
            };
            let arguments = |arguments: &[GenericArgument], todo: &mut Vec<TypeId>| {
                todo.extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(*t),
                    GenericArgument::Size(_) => None,
                }));
            };
            match &row.kind {
                hirsel_types::TypeKind::Tuple(items) => todo.extend(items.iter().copied()),
                hirsel_types::TypeKind::Record(fields) => todo.extend(fields.iter().map(|f| f.ty)),
                hirsel_types::TypeKind::Variant(variants) => {
                    todo.extend(variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1)));
                }
                hirsel_types::TypeKind::Box(inner) => todo.push(*inner),
                hirsel_types::TypeKind::Builtin { arguments: a, .. } => arguments(a, &mut todo),
                hirsel_types::TypeKind::Named {
                    definition,
                    arguments: a,
                } => {
                    arguments(a, &mut todo);
                    if let Some(hirsel_lir::layout::OwnerRepresentation::Defined(record)) =
                        self.layouts.owners().nominals.get(definition)
                        && let Some((finalizer, _)) = &record.finalizer
                        && let Some(body) = crate::library::body_index(finalizer)
                    {
                        let actuals = Actuals {
                            binder: (!a.is_empty()).then_some(crate::library::BINDER),
                            store: self.store.clone(),
                            arguments: a.to_vec(),
                            outer: Vec::new(),
                        };
                        self.required.push(crate::library::Request::finalizer(
                            body, finalizer, definition, &actuals,
                        ));
                    }
                }
                hirsel_types::TypeKind::Primitive(_)
                | hirsel_types::TypeKind::Parameter(_)
                | hirsel_types::TypeKind::Function(_)
                | hirsel_types::TypeKind::Error => {}
            }
        }
    }

    /// Require a library instance and name it as a direct callee. `lower`
    /// registers the instance once this unit is imported.
    fn require(&mut self, request: crate::library::Request) -> R<crate::library::builder::Direct> {
        let key = request.key(self.layouts)?;
        let abi = self
            .layouts
            .fn_abi(&request.actuals.store, request.callable, request.shape)
            .map_err(|e| ImportError::Layout(request.path.clone(), e))?;
        let symbol = Symbol {
            kind: SymbolKind::Function,
            definition: request.path.clone(),
            instance: Some(key.digest()),
        };
        self.required.push(request);
        Ok(crate::library::builder::Direct { symbol, abi })
    }

    /// The instance of the library body of operation `path` under the
    /// call's generic arguments (rows of the definition's store).
    fn library_request(
        &mut self,
        body: usize,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> R<crate::library::Request> {
        let callee = crate::instance::callee_actuals(
            self.program.binders.get(path).copied(),
            &self.def.types,
            arguments,
            self.actuals,
            false,
            path,
        )?;
        Ok(crate::library::Request::operation(
            self.program,
            body,
            path,
            callee.store,
            callee.arguments,
        )?)
    }

    fn intern(&mut self, layout: LayoutId) -> LirTypeId {
        let repr = repr_of(&self.layouts.record(layout).kind);
        self.intern_repr(repr, layout)
    }

    fn intern_repr(&mut self, repr: ValueRepr, layout: LayoutId) -> LirTypeId {
        let ty = LirType { repr, layout };
        if let Some(&id) = self.out.type_index.get(&ty) {
            return id;
        }
        let id = LirTypeId(self.out.types.len() as u32);
        self.out.types.push(ty.clone());
        self.out.type_index.insert(ty, id);
        id
    }

    /// The LIR type of a TIR type.
    fn lir_type(&mut self, ty: TypeId) -> R<LirTypeId> {
        let t = self.ty(ty)?;
        let layout = self.layout(t)?;
        Ok(self.intern(layout))
    }

    fn layout_of(&self, ty: LirTypeId) -> LayoutId {
        self.out.types[ty.index()].layout
    }

    fn record(&self, ty: LirTypeId) -> &hirsel_lir::layout_types::LayoutRecord {
        self.layouts.record(self.layout_of(ty))
    }

    /// Whether values of this type carry a holder credit (the validator's rule).
    fn owns(&self, ty: LirTypeId) -> bool {
        let t = &self.out.types[ty.index()];
        match t.repr {
            ValueRepr::Block | ValueRepr::Token | ValueRepr::RegionMark => true,
            ValueRepr::Unit
            | ValueRepr::Scalar(_)
            | ValueRepr::InitMask { .. }
            | ValueRepr::Uncounted
            | ValueRepr::Arena => false,
            ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle => {
                let r = self.layouts.record(t.layout);
                !(r.drop.is_empty() && r.finalizer.is_none())
            }
        }
    }

    /// Plain bits: a bit copy, no credit.
    fn bits(&self, ty: LirTypeId) -> bool {
        self.record(ty).copy == CopyRecipe::Bits && !self.owns(ty)
    }

    fn value(&mut self, ty: LirTypeId, holding: Holding) -> ValueId {
        let id = ValueId(self.out.values.len() as u32);
        self.out.values.push(ValueInfo { ty, holding });
        id
    }

    /// A freshly produced value: owned if its type carries a credit.
    fn fresh(&mut self, ty: LirTypeId) -> ValueId {
        let holding = if self.owns(ty) {
            Holding::Owned
        } else {
            Holding::Plain
        };
        self.value(ty, holding)
    }

    /// An owned many-call callable received where TIR expects the once-call callable of
    /// the same environment (MODE-3: a once binding holds whatever it
    /// received) is weakened explicitly, so LIR types stay exact.
    fn weaken(&mut self, value: ValueId, ty: LirTypeId) -> ValueId {
        let from = self.value_type(value);
        let callable = |t: LirTypeId| {
            let record = self.record(t);
            match record.kind {
                LayoutKind::Callable { environment }
                    if self.out.types[t.index()].repr == ValueRepr::Callable =>
                {
                    Some((environment, record.copy))
                }
                _ => None,
            }
        };
        let weakens = matches!(
            (callable(from), callable(ty)),
            (Some((a, CopyRecipe::Dup)), Some((b, CopyRecipe::NotCopyable))) if a == b
        );
        if !weakens {
            return value;
        }
        let result = self.fresh(ty);
        self.push(Inst::Weaken { result, value });
        for pending in &mut self.failure.pending {
            if *pending == value {
                *pending = result;
            }
        }
        result
    }

    fn value_type(&self, v: ValueId) -> LirTypeId {
        self.out.values[v.index()].ty
    }

    fn holds(&self, v: ValueId) -> bool {
        self.out.values[v.index()].holding == Holding::Owned
    }

    // --- blocks and edges ---

    fn block(&mut self, kind: BlockKind) -> BlockId {
        let anchor = self.anchor_here();
        let id = BlockId(self.out.blocks.len() as u32);
        self.out.blocks.push(BBlock {
            kind,
            params: Vec::new(),
            phis: Vec::new(),
            view_phis: Vec::new(),
            insts: Vec::new(),
            term: None,
            anchor,
        });
        id
    }

    fn push(&mut self, inst: Inst) {
        self.out.blocks[self.current.index()].insts.push(inst);
    }

    fn terminate(&mut self, term: Terminator) {
        let block = &mut self.out.blocks[self.current.index()];
        debug_assert!(block.term.is_none(), "block terminated twice");
        block.term = Some(term);
    }

    fn edge(&mut self, target: BlockId, args: Vec<ValueId>) -> EdgeId {
        let id = EdgeId(self.out.edges.len() as u32);
        self.out.edges.push(BEdge {
            target,
            args,
            versions: self.versions.clone(),
            views: self.views.clone(),
        });
        id
    }

    /// End the current block with a jump and continue in `next`.
    fn goto(&mut self, next: BlockId) {
        let e = self.edge(next, Vec::new());
        self.terminate(Terminator::Jump(e));
        self.current = next;
    }

    /// Continue in a fresh block of `kind` (single predecessor).
    fn split(&mut self, kind: BlockKind) {
        let next = self.block(kind);
        self.goto(next);
    }

    fn symbol(
        &mut self,
        kind: SymbolKind,
        definition: &DefPath,
        instance: Option<ContentHash>,
    ) -> SymbolId {
        let s = Symbol {
            kind,
            definition: definition.clone(),
            instance,
        };
        if let Some(&id) = self.out.symbol_index.get(&s) {
            return id;
        }
        let id = SymbolId(self.out.tables.symbols.len() as u32);
        self.out.tables.symbols.push(s.clone());
        self.out.symbol_index.insert(s, id);
        id
    }

    fn operation(&mut self, op: &OperationId) -> OperationRef {
        if let Some(&id) = self.out.operation_index.get(op) {
            return id;
        }
        let id = OperationRef(self.out.tables.operations.len() as u32);
        self.out.tables.operations.push(op.clone());
        self.out.operation_index.insert(op.clone(), id);
        id
    }

    fn constant(&mut self, ty: LirTypeId, value: ConstantValue) -> ConstantId {
        let data = ConstantData { ty, value };
        if let Some(&id) = self.out.constant_index.get(&data) {
            return id;
        }
        let id = ConstantId(self.out.tables.constants.len() as u32);
        self.out.tables.constants.push(data.clone());
        self.out.constant_index.insert(data, id);
        id
    }

    fn anchor_here(&mut self) -> SourceAnchorId {
        let a = SourceAnchor {
            body: self.body_id,
            block: self.statement.0,
            statement: self.statement.1,
            site: site(self.span),
        };
        if let Some(&id) = self.out.source_anchor_index.get(&a) {
            return id;
        }
        let id = SourceAnchorId(self.out.tables.source_anchors.len() as u32);
        self.out.tables.source_anchors.push(a.clone());
        self.out.source_anchor_index.insert(a, id);
        id
    }

    fn cleanup_anchor(&mut self, cleanup: u32, action: u32, span: Span) -> CleanupAnchorId {
        let a = CleanupAnchor {
            body: self.body_id,
            cleanup,
            action,
            site: site(span),
        };
        if let Some(&id) = self.out.cleanup_anchor_index.get(&a) {
            return id;
        }
        let id = CleanupAnchorId(self.out.tables.cleanup_anchors.len() as u32);
        self.out.tables.cleanup_anchors.push(a.clone());
        self.out.cleanup_anchor_index.insert(a, id);
        id
    }

    fn new_slot(&mut self, ty: LirTypeId, kind: SlotKind, scope: ScopeId) -> SlotId {
        let id = SlotId(self.out.slots.len() as u32);
        self.out.slots.push(SlotInfo { ty, kind, scope });
        // Indexed by id, as `new_loan` is: a later pass's state may already
        // name the slot.
        if self.state.slots.len() <= id.index() {
            self.state.slots.resize(id.index() + 1, SlotState::DEAD);
        }
        self.state.slots[id.index()] = SlotState::DEAD;
        self.versions.push(None);
        self.versioned.push(false);
        self.constant_version.push(None);
        id
    }
}

impl Importer<'_, '_> {
    /// One emission pass from the given TIR entry states (None: not yet known).
    /// Returns every block's entry state joined over all its edges, which
    /// equals `prev` at the fixpoint, so each emitted block assumed a state
    /// covering every edge into it.
    fn pass(&mut self, prev: &[Option<State>]) -> R<Vec<Option<State>>> {
        self.out = Out::default();
        let locals = self.body.locals.len();
        self.state = State {
            slots: PVec::default(),
            loans: PVec::default(),
            views: PVec::filled(locals, Tri::No),
        };
        self.views = PVec::filled(locals, None);
        self.versions = PVec::default();
        self.versioned.clear();
        self.constant_version = PVec::default();
        self.chain_memo.clear();
        self.outer_scope = false;
        self.unreachable = None;
        self.propagate = None;
        self.prologue_failure = None;
        self.failure = FailureContext {
            cleanup: None,
            before: Vec::new(),
            pending: Vec::new(),
        };
        let n = self.body.blocks.len();
        self.incoming = vec![None; n];
        self.entry_versions = vec![None; n];
        self.entry_views = vec![None; n];
        self.entry_facts = vec![None; n];
        self.span = self.body.blocks[self.body.entry.0 as usize].span;
        self.statement = (self.body.entry.0, 0);
        self.setup()?;
        let mut used = vec![None; n];
        let rpo = self.rpo.clone();
        for t in rpo {
            let mut entry = self.incoming[t.0 as usize].clone();
            if let Some(p) = &prev[t.0 as usize] {
                match &mut entry {
                    Some(e) => e.join(p),
                    None => entry = Some(p.clone()),
                }
            }
            let Some(entry) = entry else {
                return malformed(format!("block {} has no incoming state", t.0));
            };
            used[t.0 as usize] = Some(entry.clone());
            self.emit_block(t, entry)?;
        }
        // Back edges reach their targets after those were emitted: the next
        // pass starts from every edge's state.
        for (u, incoming) in used.iter_mut().zip(&self.incoming) {
            if let (Some(u), Some(i)) = (u.as_mut(), incoming) {
                u.join(i);
            }
        }
        Ok(used)
    }

    /// Slots, parameters, loans and the prologue.
    fn setup(&mut self) -> R<()> {
        let body = self.body;
        let view = self.instances.view(self.id);
        let homes: Vec<Home> = match self.plan {
            Some(plan) => plan.homes.clone(),
            None => vec![Home::Parameter; body.parameters.len()],
        };
        let parameter_of: HashMap<u32, u32> = body
            .parameters
            .iter()
            .enumerate()
            .map(|(k, l)| (l.0, k as u32))
            .collect();
        let mut capture = 0u32;
        // Locals receiving displaced content of a Replace.
        let displaced: BTreeSet<u32> = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| match &i.kind {
                tir::InstructionKind::Replace { displaced, .. } => {
                    Some(body.places[displaced.0 as usize].root.0)
                }
                _ => None,
            })
            .collect();
        self.local_slot.clear();
        self.loop_slots = vec![None; body.parameters.len()];
        let mut parameter_slots: Vec<Option<SlotId>> = vec![
                None;
                self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize]
                    .parameters
                    .len()
            ];
        let direct_parameters: HashMap<_, _> = self
            .instances
            .direct_captures
            .get(&self.id)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(ordinal, capture)| (capture.local, (ordinal, capture.layout)))
            .collect();
        for (i, local) in body.locals.iter().enumerate() {
            if self
                .instances
                .elided
                .get(&self.id)
                .is_some_and(|locals| locals.contains(&tir::LocalId(i as u32)))
                || self.builders.elements.contains(&tir::LocalId(i as u32))
                || self.fields.locals.contains(&tir::LocalId(i as u32))
            {
                self.local_slot.push(None);
                continue;
            }
            let direct = direct_parameters.get(&tir::LocalId(i as u32)).copied();
            let ty = if let Some((_, layout)) = direct {
                self.intern(layout)
            } else if let Some(&layout) = self.builders.locals.get(&tir::LocalId(i as u32)) {
                self.intern(layout)
            } else if let Some(&layout) = self
                .instances
                .data_locals
                .get(&(self.id, tir::LocalId(i as u32)))
            {
                self.intern(layout)
            } else {
                self.lir_type(local.ty)?
            };
            let scope = ScopeId(local.scope.0);
            let slot = match local.storage {
                tir::Storage::Parameter => {
                    let Some(&k) = parameter_of.get(&(i as u32)) else {
                        return malformed(format!("parameter local {i} is not a parameter"));
                    };
                    match homes[k as usize] {
                        Home::Parameter => {
                            let s = self.new_slot(ty, SlotKind::Parameter(k), scope);
                            parameter_slots[k as usize] = Some(s);
                            s
                        }
                        Home::Loop => {
                            let s = self.new_slot(ty, SlotKind::LoopCarried(k), scope);
                            self.loop_slots[k as usize] = Some(s);
                            s
                        }
                    }
                }
                tir::Storage::Local => self.new_slot(ty, SlotKind::Local, scope),
                tir::Storage::Temporary if displaced.contains(&(i as u32)) => {
                    self.new_slot(ty, SlotKind::Displaced, scope)
                }
                tir::Storage::Temporary => self.new_slot(ty, SlotKind::Temporary, scope),
                tir::Storage::Capture => {
                    if let Some((ordinal, _)) = direct {
                        let k = body.parameters.len() + ordinal;
                        let slot = self.new_slot(ty, SlotKind::Parameter(k as u32), scope);
                        parameter_slots[k] = Some(slot);
                        slot
                    } else {
                        capture += 1;
                        self.new_slot(ty, SlotKind::Capture(capture - 1), scope)
                    }
                }
                tir::Storage::ReturnSlot => self.new_slot(ty, SlotKind::ReturnSlot, scope),
            };
            self.local_slot.push(Some(slot));
        }
        if let Some(environment) = self.instances.environments[self.id.index()]
            && let LayoutKind::Environment { .. } = self.layouts.record(environment).kind
        {
            let word = self
                .layouts
                .record(environment)
                .fields
                .last()
                .expect("live word")
                .layout;
            let ty = self.intern(word);
            self.new_slot(ty, SlotKind::Capture(capture), ScopeId(0));
        }
        self.flat_copies()?;
        // A loop-carried borrowed parameter still arrives in its caller's storage.
        let abi =
            self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize].clone();
        for (k, home) in homes.iter().enumerate() {
            if *home == Home::Loop && abi.parameters[k].convention == Passing::Borrow {
                let local = body.parameters[k];
                let ty = self.out.slots[self.slot_for(local.0 as usize)?.index()].ty;
                let scope = ScopeId(body.locals[local.0 as usize].scope.0);
                parameter_slots[k] = Some(self.new_slot(ty, SlotKind::Parameter(k as u32), scope));
            }
        }
        let _ = view;
        // Declared parameters, in instance ordinal order.
        self.out.parameters.clear();
        for (k, p) in abi.parameters.iter().enumerate() {
            let ty = self.intern(p.layout);
            let value = match parameter_slots[k] {
                Some(slot) => {
                    let v = self.value(ty, Holding::Plain);
                    self.constant_version[slot.index()] = Some(v);
                    self.versions[slot.index()] = Some(v);
                    v
                }
                None => self.fresh(ty),
            };
            self.out.parameters.push(value);
        }
        let mut captures = Vec::new();
        for s in 0..self.out.slots.len() {
            let slot = &self.out.slots[s];
            if let SlotKind::Capture(i) = slot.kind {
                let ty = slot.ty;
                let v = self.value(ty, Holding::Plain);
                self.constant_version[s] = Some(v);
                self.versions[s] = Some(v);
                captures.push((i, v));
            }
        }
        captures.sort();
        self.capture_versions = captures.into_iter().map(|(_, v)| v).collect();
        if let Some(environment) = self.instances.environments[self.id.index()]
            && matches!(
                self.layouts.record(environment).kind,
                LayoutKind::Environment { .. }
            )
        {
            let ordinal = self
                .layouts
                .record(environment)
                .fields
                .last()
                .expect("live word")
                .ordinal;
            let slot = SlotId(
                self.out
                    .slots
                    .iter()
                    .position(|s| s.kind == SlotKind::Capture(ordinal))
                    .expect("live word slot") as u32,
            );
            self.out.tables.init_flags.push(hirsel_lir::ir::InitFlag {
                path: PoolRange::EMPTY,
                value: self.capture_versions[ordinal as usize],
                slot,
                environment: Some(environment),
            });
        }
        for (s, slot) in self.out.slots.iter().enumerate() {
            let entered = matches!(slot.kind, SlotKind::Parameter(_) | SlotKind::Capture(_));
            self.state.slots[s] = SlotState {
                live: if entered || slot.kind == SlotKind::ReturnSlot {
                    Tri::Yes
                } else {
                    Tri::No
                },
                init: if entered { Tri::Yes } else { Tri::No },
                moved: BTreeMap::new(),
            };
        }
        self.mark_versioned();
        if self.out.tables.environment().is_some() {
            let flag = self
                .out
                .tables
                .init_flags
                .iter()
                .find(|flag| flag.environment.is_some())
                .expect("live word");
            self.versioned[flag.slot.index()] = true;
        }
        // The prologue: fixed-address preparations, then loop-slot entry.
        self.prologue = self.block(BlockKind::Body);
        self.current = self.prologue;
        self.loans()?;
        self.tir_block = vec![None; body.blocks.len()];
        for &t in &self.rpo.clone() {
            let b = self.block(BlockKind::Body);
            self.tir_block[t.0 as usize] = Some(b);
        }
        self.split(BlockKind::Body);
        if self.plan.is_some() {
            self.enter_loop(&parameter_slots)?;
        }
        let entry = self.body.entry;
        let e = self.enter(entry);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// Slots that some operation after entry writes, so reads at a merge may
    /// see different versions.
    fn mark_versioned(&mut self) {
        use tir::InstructionKind as K;
        let body = self.body;
        let root = |p: &tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
        let mut written = BTreeSet::new();
        for block in &body.blocks {
            for i in &block.instructions {
                match &i.kind {
                    K::Assign { destination, .. } | K::Snapshot { destination, .. } => {
                        written.insert(root(destination));
                    }
                    K::Replace {
                        destination,
                        displaced,
                        ..
                    } => {
                        written.insert(root(destination));
                        written.insert(root(displaced));
                    }
                    _ => {}
                }
            }
            if let tir::Terminator::Call {
                returns, arguments, ..
            } = &block.terminator
            {
                if let Some(tir::CallReturn { destination, .. }) = returns {
                    written.insert(root(destination));
                }
                for a in arguments {
                    if a.value.usage == UseKind::Mut {
                        written.insert(root(&a.value.place));
                    }
                }
            }
        }
        for local in written {
            if let Some(slot) = self.local_slot[local] {
                self.versioned[slot.index()] = true;
            }
        }
        for slot in self.loop_slots.clone().into_iter().flatten() {
            self.versioned[slot.index()] = true;
        }
    }

    /// One LIR loan per TIR loan. A loan of a fixed place names its prologue
    /// address; a loan of a computed place (an element, a variant field)
    /// names the address prepared at its one `BeginLoan`.
    fn loans(&mut self) -> R<()> {
        let body = self.body;
        let mut begins: BTreeMap<u32, (tir::PlaceId, bool, u32)> = BTreeMap::new();
        let mut max = None::<u32>;
        for block in &body.blocks {
            for i in &block.instructions {
                if let tir::InstructionKind::BeginLoan {
                    id, place, mutable, ..
                } = &i.kind
                {
                    begins.entry(id.0).or_insert((*place, *mutable, 0)).2 += 1;
                }
            }
        }
        for place in &body.places {
            if let Some(l) = place.loan {
                max = Some(max.map_or(l.0, |m: u32| m.max(l.0)));
            }
        }
        if let Some((&last, _)) = begins.iter().next_back() {
            max = Some(max.map_or(last, |m| m.max(last)));
        }
        self.loan_map.clear();
        let Some(max) = max else {
            return Ok(());
        };
        for l in 0..=max {
            let Some(&(place, mutable, count)) = begins.get(&l) else {
                // A loan named only by places, never begun here: no LIR loan.
                self.loan_map.push(LoanId(u32::MAX));
                continue;
            };
            if self.elided_place(place) {
                self.loan_map.push(LoanId(u32::MAX));
                continue;
            }
            let p = self.place(place)?;
            let address = if p.fixed {
                self.fixed_address(&p)?
            } else if count == 1 {
                // Prepared at the BeginLoan.
                UNPREPARED
            } else {
                return unsupported(format!(
                    "loan {l} of a computed place begun at {count} sites"
                ));
            };
            let id = self.new_loan(LoanInfo { address, mutable }, Tri::No);
            self.loan_map.push(id);
        }
        Ok(())
    }

    fn loan(&self, l: tir::LoanId) -> R<LoanId> {
        match self.loan_map.get(l.0 as usize) {
            Some(&id) if id.0 != u32::MAX => Ok(id),
            _ => malformed(format!("loan {} has no BeginLoan", l.0)),
        }
    }

    /// Jump into TIR block `t`, recording the state and versions it receives.
    fn enter(&mut self, t: tir::BlockId) -> EdgeId {
        let target = self.tir_block[t.0 as usize].expect("reachable TIR block");
        let e = self.edge(target, Vec::new());
        let slot = &mut self.incoming[t.0 as usize];
        match slot {
            Some(s) => s.join(&self.state),
            None => *slot = Some(self.state.clone()),
        }
        self.entry_versions[t.0 as usize] = Some(self.versions.clone());
        self.entry_views[t.0 as usize] = Some(self.views.clone());
        self.entry_facts[t.0 as usize] = Some(self.facts.clone());
        e
    }

    fn merge(&self, t: tir::BlockId) -> bool {
        let extra = u32::from(t == self.body.entry);
        self.tir_preds[t.0 as usize] + extra >= 2
    }

    fn emit_block(&mut self, t: tir::BlockId, entry: State) -> R<()> {
        let b = self.tir_block[t.0 as usize].expect("reachable TIR block");
        let block = &self.body.blocks[t.0 as usize];
        self.current = b;
        self.span = block.span;
        self.statement = (t.0, 0);
        let anchor = self.anchor_here();
        self.out.blocks[b.index()].anchor = anchor;
        self.state = entry;
        if self.merge(t) {
            let mut phis = Vec::new();
            let mut versions = self.constant_version.clone();
            // Empty storage joins no version: only the slots holding something.
            let held: Vec<usize> = self
                .state
                .slots
                .lacking(EMPTY)
                .into_iter()
                .filter(|&(s, st)| {
                    s < versions.len()
                        && self.versioned[s]
                        && st.live == Tri::Yes
                        && st.init == Tri::Yes
                })
                .map(|(s, _)| s)
                .collect();
            for s in held {
                let ty = self.out.slots[s].ty;
                let p = self.value(ty, Holding::Plain);
                phis.push((SlotId(s as u32), p));
                versions[s] = Some(p);
            }
            self.out.blocks[b.index()].phis = phis;
            self.versions = versions;
            // A joined view held on every edge joins into a parameter whose
            // loan `finish` reads off its incoming views.
            let mut view_phis = Vec::new();
            let mut views = PVec::filled(self.body.locals.len(), None);
            let held: Vec<usize> = self
                .state
                .views
                .lacking(DEAD)
                .into_iter()
                .filter(|&(l, view)| self.joined[l] && *view == Tri::Yes)
                .map(|(l, _)| l)
                .collect();
            for l in held {
                let ty = self.out.slots[self.slot_for(l)?.index()].ty;
                let p = self.value(ty, Holding::Plain);
                view_phis.push((l, p));
                views[l] = Some(p);
            }
            self.out.blocks[b.index()].view_phis = view_phis;
            self.views = views;
            self.facts = Vec::new();
        } else {
            self.versions = self.entry_versions[t.0 as usize]
                .clone()
                .unwrap_or_else(|| self.constant_version.clone());
            // Blocks emitted since the edge's snapshot may have made slots; theirs
            // hold no version here, and `new_slot` indexes by id.
            let slots = self.out.slots.len();
            if self.versions.len() < slots {
                self.versions.resize(slots, None);
            }
            self.views = self.entry_views[t.0 as usize]
                .clone()
                .unwrap_or_else(|| PVec::filled(self.body.locals.len(), None));
            self.facts = self.entry_facts[t.0 as usize].clone().unwrap_or_default();
        }
        for (j, instruction) in block.instructions.iter().enumerate() {
            self.statement = (t.0, j as u32);
            self.span = instruction.span;
            self.failure = FailureContext {
                cleanup: Some(instruction.on_failure),
                before: Vec::new(),
                pending: Vec::new(),
            };
            self.instruction(&instruction.kind)?;
        }
        self.statement = (t.0, block.instructions.len() as u32);
        self.span = block.span;
        self.failure = FailureContext {
            cleanup: None,
            before: Vec::new(),
            pending: Vec::new(),
        };
        self.terminator(t, &block.terminator)
    }
}

impl Importer<'_, '_> {
    // --- places and addresses ---

    fn slot_for(&self, local: usize) -> R<SlotId> {
        self.local_slot
            .get(local)
            .copied()
            .flatten()
            .ok_or_else(|| {
                ImportError::Malformed(format!(
                    "a runtime use of elided generic function local {local}"
                ))
            })
    }

    fn elided_place(&self, place: tir::PlaceId) -> bool {
        let root = self.body.places[place.0 as usize].root;
        self.instances
            .elided
            .get(&self.id)
            .is_some_and(|locals| locals.contains(&root))
            || self.builders.elements.contains(&root)
            || self.fields.locals.contains(&root)
    }

    fn elided_loan(&self, loan: tir::LoanId) -> bool {
        self.elided_loans.contains(&loan)
    }

    /// Resolve a TIR place to its slot, steps and type.
    fn place(&mut self, id: tir::PlaceId) -> R<PlaceRef> {
        let Some(place) = self.body.places.get(id.0 as usize) else {
            return malformed(format!("place {} outside the body", id.0));
        };
        let temporary = self.out.slots[self.slot_for(place.root.0 as usize)?.index()].kind
            == SlotKind::Temporary;
        // A view temporary reads through the place it views.
        let mut root = place.root;
        let mut projection: Vec<tir::Projection> = place.projection.iter().cloned().collect();
        let mut loan = place.loan;
        while let Some(base) = self.aliases[root.0 as usize] {
            let b = &self.body.places[base.0 as usize];
            let mut full: Vec<tir::Projection> = b.projection.iter().cloned().collect();
            full.extend(projection);
            projection = full;
            loan = loan.or(b.loan);
            root = b.root;
        }
        if self.joined[root.0 as usize] {
            return unsupported("a part of a joined view");
        }
        let slot = self.slot_for(root.0 as usize)?;
        let mut steps = Vec::new();
        let mut fixed = true;
        let mut crosses = false;
        let mut flat = false;
        let mut layout = self.layout_of(self.out.slots[slot.index()].ty);
        let mut lowered = Vec::new();
        for p in &projection {
            if let LayoutKind::Box { payload } = self.layouts.record(layout).kind {
                if *p != tir::Projection::Field(0) {
                    return malformed("a Box projection without its payload step");
                }
                fixed = false;
                crosses = temporary;
                layout = payload;
                steps.push(Step::Payload);
                lowered.push(ProjectionStep::Payload);
                continue;
            }
            if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
                self.layouts.record(layout).kind
            {
                fixed = false;
                crosses = temporary;
                layout = payload;
            }
            // A variable flat value inside a flat value is behind its length
            // prefix: its parts are parts of the prefixed encoding's content.
            if let Some((8, content)) = flat_prefixed(self.layouts, layout)
                && !flat_fixed(self.layouts, content)
            {
                steps.push(Step::Field(0));
                lowered.push(ProjectionStep::Field(0));
                layout = content;
            }
            // A container's public element place is entries[i], in its private record.
            if matches!(p, tir::Projection::Index(_))
                && matches!(&self.layouts.record(layout).kind, LayoutKind::Opaque { owner } if ["Map", "Set", "SortedMap"].iter().any(|name| *owner == crate::library::core_path(name)))
            {
                steps.push(Step::Field(0));
                lowered.push(ProjectionStep::Field(0));
                layout = self.layouts.record(layout).fields[0].layout;
                if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
                    self.layouts.record(layout).kind
                {
                    layout = payload;
                }
            }
            lowered.push(match *p {
                tir::Projection::Field(field) => ProjectionStep::Field(field),
                tir::Projection::VariantField { variant, field } => {
                    ProjectionStep::VariantField { variant, field }
                }
                tir::Projection::Index(index) => ProjectionStep::Index(index),
            });
            let record = self.layouts.record(layout);
            if matches!(record.kind, LayoutKind::Flat) {
                // A part of a flat value is its FlatFormatV0 encoding at its
                // cursor, which follows earlier variable fields' prefixes: its
                // address is prepared where it is used.
                fixed = false;
                flat = true;
            }
            let next = match p {
                tir::Projection::Field(f) => {
                    steps.push(Step::Field(*f));
                    record
                        .fields
                        .iter()
                        .find(|x| x.ordinal == *f)
                        .map(|x| x.layout)
                }
                tir::Projection::VariantField { variant, field } => {
                    fixed = false;
                    steps.push(Step::Variant(*variant, *field));
                    record
                        .tag
                        .iter()
                        .flat_map(|t| &t.variants)
                        .find(|v| v.ordinal == *variant)
                        .and_then(|v| v.fields.iter().find(|x| x.ordinal == *field))
                        .map(|x| x.layout)
                }
                tir::Projection::Index(_) => {
                    fixed = false;
                    steps.push(Step::Index);
                    match record.kind {
                        // Element i of a flat sequence, at its cursor.
                        LayoutKind::Flat => flat_sequence(self.layouts, layout),
                        LayoutKind::Inline { element, .. }
                        | LayoutKind::Simd { element, .. }
                        | LayoutKind::Slice { element }
                        | LayoutKind::SoaColumns { element }
                        | LayoutKind::Sequence { element }
                        // Element i of a sequential Packed sequence, walked to (MEM-13).
                        | LayoutKind::Sequential { element } => Some(element),
                        _ => None,
                    }
                }
            };
            let Some(next) = next else {
                return unsupported(format!(
                    "projection {p:?} of a {:?} layout",
                    self.layouts.record(layout).kind
                ));
            };
            layout = next;
        }
        let ty = if place.projection.is_empty() {
            if let Some(&layout) = self.instances.data_locals.get(&(self.id, root)) {
                self.intern(layout)
            } else if let Some(&layout) = self.builders.locals.get(&root) {
                self.intern(layout)
            } else {
                self.lir_type(place.ty)?
            }
        } else {
            self.lir_type(place.ty)?
        };
        if flat && self.layout_of(ty) != layout {
            // Only a fixed-size part has its own type's layout; a variable one
            // is a length-prefixed encoding, read by flat-aware operations.
            if flat_fixed(self.layouts, layout) {
                return unsupported("a fixed flat record part at its source type");
            }
            return Ok(PlaceRef {
                slot,
                steps,
                projection: lowered,
                ty: self.intern(layout),
                fixed,
                crosses,
                loan,
                source: Some((ty, self.ty(place.ty)?)),
                site: self.origin_sites.get(&id).copied(),
            });
        }
        if self.layout_of(ty) != layout {
            return malformed(format!(
                "place {} has type layout {:?} but its projection reaches {:?}",
                id.0,
                self.layouts.record(self.layout_of(ty)).kind,
                self.layouts.record(layout).kind
            ));
        }
        Ok(PlaceRef {
            slot,
            steps,
            projection: lowered,
            ty,
            fixed,
            crosses,
            loan,
            source: None,
            site: self.origin_sites.get(&id).copied(),
        })
    }

    /// The prologue-prepared address of a fixed place.
    fn fixed_address(&mut self, p: &PlaceRef) -> R<AddressId> {
        let steps: Vec<ProjectionStep> = p
            .projection
            .iter()
            .map(|x| match x {
                ProjectionStep::Field(f) => ProjectionStep::Field(*f),
                _ => unreachable!("a fixed place has only field steps"),
            })
            .collect();
        let key = (p.slot, steps.clone());
        if let Some(&a) = self.out.fixed_addresses.get(&key) {
            return Ok(a);
        }
        let projection = PoolRange::push(&mut self.out.pools.projections, steps);
        let a = AddressId(self.out.addresses.len() as u32);
        self.out.addresses.push(AddressInfo {
            base: AddressBase::Slot(p.slot),
            projection,
            ty: p.ty,
        });
        self.out.fixed_addresses.insert(key, a);
        let failure = self.prologue_failure_edge()?;
        self.out.blocks[self.prologue.index()]
            .insts
            .push(Inst::Address {
                address: a,
                failure,
            });
        Ok(a)
    }

    /// A failure edge from the prologue: nothing has run yet, so its cleanup
    /// releases what the caller handed over (owned parameters, as slots or as
    /// values). Field-path preparations never fire it, but each names one.
    fn prologue_failure_edge(&mut self) -> R<FailureId> {
        let pending: Vec<ValueId> = self
            .out
            .parameters
            .iter()
            .copied()
            .filter(|&v| self.holds(v))
            .collect();
        let target = match self.prologue_failure {
            Some(b) => b,
            None => {
                let b = self.block(BlockKind::FailureCleanup);
                self.prologue_failure = Some(b);
                let saved = self.save();
                self.state = self.initial_state();
                let actions = self.state_actions();
                self.current = b;
                self.fill_dispose(&pending, &actions, false)?;
                self.restore(saved);
                b
            }
        };
        let anchor = self.anchor_here();
        let edge = self.edge(target, pending);
        Ok(self.failure_edge(edge, None, anchor))
    }

    fn save(&self) -> (BlockId, State, PVec<Option<ValueId>>) {
        (self.current, self.state.clone(), self.versions.clone())
    }

    /// Restore an emission point; slots created meanwhile stay, dead.
    fn restore(&mut self, saved: (BlockId, State, PVec<Option<ValueId>>)) {
        (self.current, self.state, self.versions) = saved;
        self.pad();
    }

    fn pad(&mut self) {
        let n = self.out.slots.len();
        self.state.slots.resize(n, SlotState::DEAD);
        self.versions.resize(n, None);
        let l = self.out.tables.loans.len();
        self.state.loans.resize(l, Tri::No);
    }

    /// The state at function entry.
    fn initial_state(&self) -> State {
        State {
            slots: self
                .out
                .slots
                .iter()
                .map(|slot| {
                    let entered =
                        matches!(slot.kind, SlotKind::Parameter(_) | SlotKind::Capture(_));
                    SlotState {
                        live: if entered || slot.kind == SlotKind::ReturnSlot {
                            Tri::Yes
                        } else {
                            Tri::No
                        },
                        init: if entered { Tri::Yes } else { Tri::No },
                        moved: BTreeMap::new(),
                    }
                })
                .collect(),
            loans: PVec::filled(self.out.tables.loans.len(), Tri::No),
            views: PVec::filled(self.body.locals.len(), Tri::No),
        }
    }

    fn failure_edge(
        &mut self,
        cleanup: EdgeId,
        rule: Option<String>,
        anchor: SourceAnchorId,
    ) -> FailureId {
        let id = FailureId(self.out.tables.failures.len() as u32);
        self.out.tables.failures.push(FailureEdge {
            cleanup,
            rule,
            anchor,
            dispose: PoolRange::EMPTY,
        });
        id
    }

    /// The address of a place, prepared here when it is computed. Index
    /// operands are read first; a bounds or tag check fails along the current
    /// failure successor with its rule, at the site of the origin binding
    /// that named the place when there was one.
    fn address(&mut self, p: &PlaceRef) -> R<AddressId> {
        if p.fixed {
            return self.fixed_address(p);
        }
        let (a, rule) = self.named_address(p)?;
        let span = self.span;
        self.span = p.site.unwrap_or(span);
        let failure = self.fail(rule.map(String::from));
        self.span = span;
        self.push(Inst::Address {
            address: a,
            failure: failure?,
        });
        Ok(a)
    }

    /// The address an origin event names. The event binds a name and never
    /// accesses the place (the interpreter's `BindOrigin` computes it without
    /// failing), so a dynamic address is not prepared: an index or tag check
    /// runs where the place is accessed, after the operands evaluated before
    /// that access, such as an assignment's right-hand value.
    fn origin_address(&mut self, p: &PlaceRef) -> R<AddressId> {
        if p.fixed {
            return self.fixed_address(p);
        }
        Ok(self.named_address(p)?.0)
    }

    /// A dynamic place's address and the rule its preparation checks, with
    /// its index operands read here; nothing prepares it.
    fn named_address(&mut self, p: &PlaceRef) -> R<(AddressId, Option<&'static str>)> {
        let mut steps = Vec::new();
        let mut rule = None;
        for x in &p.projection {
            steps.push(match x {
                ProjectionStep::Payload => ProjectionStep::Payload,
                ProjectionStep::Field(f) => ProjectionStep::Field(*f),
                ProjectionStep::VariantField { variant, field } => {
                    rule.get_or_insert("MEM-13");
                    ProjectionStep::VariantField {
                        variant: *variant,
                        field: *field,
                    }
                }
                ProjectionStep::Index(local) => {
                    rule = Some("MEM-14");
                    let index = self.read_local_bits(*local)?;
                    ProjectionStep::Index(index)
                }
                ProjectionStep::Slot(_) | ProjectionStep::FlatAt { .. } => {
                    return malformed("library addressing in a TIR place");
                }
            });
        }
        let projection = PoolRange::push(&mut self.out.pools.projections, steps);
        let a = AddressId(self.out.addresses.len() as u32);
        self.out.addresses.push(AddressInfo {
            base: AddressBase::Slot(p.slot),
            projection,
            ty: p.ty,
        });
        Ok((a, rule))
    }

    /// Read a whole plain local (an index operand).
    fn read_local_bits(&mut self, local: tir::LocalId) -> R<ValueId> {
        let slot = self.slot_for(local.0 as usize)?;
        let ty = self.out.slots[slot.index()].ty;
        let p = PlaceRef {
            slot,
            steps: Vec::new(),
            projection: Vec::new(),
            ty,
            fixed: true,
            crosses: false,
            loan: None,
            source: None,
            site: None,
        };
        let address = self.fixed_address(&p)?;
        let reaching = self.reaching(slot)?;
        let result = self.value(ty, Holding::Plain);
        let mode = if self.bits(ty) {
            ReadMode::Bits
        } else {
            return unsupported("a non-plain index operand");
        };
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode,
        });
        Ok(result)
    }

    /// The reaching version of a slot.
    fn reaching(&self, slot: SlotId) -> R<ValueId> {
        match self.versions[slot.index()] {
            Some(v) => Ok(v),
            None => malformed(format!(
                "read of slot {} ({:?}) with no reaching version",
                slot.0,
                self.out.slots[slot.index()].kind
            )),
        }
    }

    // --- operands ---

    /// Read an operand at its use. `Copy` makes an owned copy (a plain read
    /// for bits), `Take` moves the content out, `Borrow` reads a view inside
    /// the operand's loan. A copy may fail; owned values read earlier in the
    /// same operation are pending on that failure.
    fn operand(&mut self, op: &tir::Operand) -> R<ValueId> {
        if let Some(view) = self.joined_view(op) {
            return view;
        }
        let place = self.place(op.place)?;
        let address = match self.prepared(&place) {
            Some(address) => address,
            None => self.address(&place)?,
        };
        self.operand_at(op.usage, &place, address, place.loan)
    }

    /// The reaching version of a place's slot when the place can carry facts:
    /// payload, field and variant steps only.
    fn fact_version(&self, p: &PlaceRef) -> Option<ValueId> {
        let steps_ok = p.projection.iter().all(|step| {
            matches!(
                step,
                ProjectionStep::Payload
                    | ProjectionStep::Field(_)
                    | ProjectionStep::VariantField { .. }
            )
        });
        if p.source.is_some() || !steps_ok {
            return None;
        }
        self.versions[p.slot.index()]
    }

    /// The variant match tag tests proved `p` holds at its slot's current version.
    fn proven_variant(&self, p: &PlaceRef) -> Option<u32> {
        let current = self.fact_version(p)?;
        self.facts.iter().find_map(|fact| match fact {
            Fact::Variant {
                slot,
                projection,
                version,
                variants,
            } if *slot == p.slot
                && *version == current
                && *projection == p.projection
                && variants.len() == 1 =>
            {
                Some(variants[0])
            }
            _ => None,
        })
    }

    /// The address prepared at its arm's `BindFields` for an operand read of
    /// `p`, while its slot keeps that version (FACT-3: the arm proved the tag, so
    /// the read needs no second MEM-13 check). A read, even a take, writes nothing.
    fn prepared(&self, p: &PlaceRef) -> Option<AddressId> {
        let current = self.fact_version(p)?;
        self.facts.iter().find_map(|fact| match fact {
            Fact::Prepared {
                slot,
                projection,
                version,
                address,
            } if *slot == p.slot
                && *version == current
                && *projection == p.projection
                && self.out.addresses[address.index()].ty == p.ty =>
            {
                Some(*address)
            }
            _ => None,
        })
    }

    /// A borrowed use of a whole joined view temporary is the view it holds.
    fn joined_view(&self, op: &tir::Operand) -> Option<R<ValueId>> {
        let place = &self.body.places[op.place.0 as usize];
        let l = place.root.0 as usize;
        if !self.joined[l] || !place.projection.is_empty() || op.usage != UseKind::Borrow {
            return None;
        }
        Some(match (self.state.views[l], self.views[l]) {
            (Tri::Yes, Some(v)) => Ok(v),
            _ => unsupported("a joined view not held on every path"),
        })
    }

    fn clear_capture(&mut self, ordinal: u32) -> R<()> {
        let environment = self
            .out
            .tables
            .environment()
            .ok_or_else(|| ImportError::Malformed("capture take without live word".into()))?;
        let record = self.layouts.record(environment);
        let LayoutKind::Environment { movable } = &record.kind else {
            return malformed("capture mask layout");
        };
        let bit = movable
            .iter()
            .position(|&i| i == ordinal)
            .ok_or_else(|| ImportError::Malformed("capture lacks live bit".into()))?;
        let flag_ordinal = record.fields.last().expect("live word").ordinal;
        let slot = SlotId(
            self.out
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Capture(flag_ordinal))
                .expect("live word slot") as u32,
        );
        let ty = self.out.slots[slot.index()].ty;
        let place = PlaceRef {
            slot,
            steps: Vec::new(),
            projection: Vec::new(),
            ty,
            fixed: true,
            crosses: false,
            loan: None,
            source: None,
            site: None,
        };
        let address = self.fixed_address(&place)?;
        let reaching = self.reaching(slot)?;
        let old = self.value(ty, Holding::Plain);
        self.push(Inst::Read {
            result: old,
            address,
            reaching,
            mode: ReadMode::Bits,
        });
        let value = self.value(ty, Holding::Plain);
        self.push(Inst::UpdateMask {
            result: value,
            mask: old,
            field: bit as u32,
            initialized: false,
        });
        let version = self.value(ty, Holding::Plain);
        self.push(Inst::Store {
            address,
            value,
            version,
        });
        self.state.write(slot, &[]);
        self.versions[slot.index()] = Some(version);
        Ok(())
    }

    fn operand_at(
        &mut self,
        usage: UseKind,
        place: &PlaceRef,
        address: AddressId,
        loan: Option<tir::LoanId>,
    ) -> R<ValueId> {
        if let Some((source, source_type)) = place.source {
            if matches!(usage, UseKind::Copy | UseKind::Take) {
                if let LayoutKind::Block { payload } = self.record(source).kind
                    && matches!(
                        self.layouts.record(payload).kind,
                        LayoutKind::Sequence { .. }
                    )
                {
                    return self.materialize_sequence(address, place.ty, source, source_type);
                }
                if !matches!(self.record(source).kind, LayoutKind::Slice { .. })
                    && !matches!(self.record(source).kind, LayoutKind::Block { payload }
                        if self.layouts.record(payload).kind == LayoutKind::Flat)
                {
                    return unsupported("materializing this ordinary value from a flat encoding");
                }
                let result = self.fresh(source);
                let failure = self.fail(None)?;
                self.push(Inst::FlatCopy {
                    result,
                    address,
                    layout: self.layout_of(place.ty),
                    failure,
                });
                if self.holds(result) {
                    self.failure.pending.push(result);
                }
                return Ok(result);
            }
            let loan = match (usage, loan) {
                (UseKind::Borrow, Some(l)) => self.loan(l)?,
                _ => {
                    return unsupported(format!(
                        "a variable-size part of a flat value as a {usage:?} operand"
                    ));
                }
            };
            return self.flat_view(address, loan, source);
        }
        let ty = place.ty;
        let reaching = self.reaching(place.slot)?;
        // The caller keeps what it lends: taking from a borrowed parameter
        // (a copyable capability, `take s` of `s: Dir`) copies it.
        let usage = if usage == UseKind::Take && self.borrowed_storage(place.slot) {
            UseKind::Copy
        } else if usage == UseKind::Take && self.bits(ty) && self.mut_storage(place.slot) {
            // The caller's storage keeps its bits: a plain take is a read.
            UseKind::Copy
        } else if usage == UseKind::Take
            && matches!(
                self.out.slots[place.slot.index()].kind,
                SlotKind::Capture(_)
            )
            && self.record(ty).copy != CopyRecipe::NotCopyable
        {
            // A closure body's captures are its caller's environment, lent
            // for the call as a borrowed parameter is: taking a copyable one
            // (a `once` closure's `take x`) copies it, and the environment
            // still releases it whole.
            UseKind::Copy
        } else if usage == UseKind::Take
            && place.crosses
            && self.record(ty).copy != CopyRecipe::NotCopyable
        {
            // A copyable part taken out of a temporary's block (an element of
            // a call's list result, say) is copied, and the temporary is
            // dropped whole: a take would detach a shared block (OWN-4) only
            // to leave a hole in it that its drop would have to skip. Nothing
            // tells the two apart, as copyable content has no finalizer.
            UseKind::Copy
        } else {
            usage
        };
        match usage {
            UseKind::Copy if self.bits(ty) => {
                let result = self.value(ty, Holding::Plain);
                self.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Bits,
                });
                Ok(result)
            }
            UseKind::Copy => {
                let result = self.fresh(ty);
                let failure = self.fail(None)?;
                self.push(Inst::Copy {
                    result,
                    address,
                    reaching,
                    failure,
                });
                if self.holds(result) {
                    self.failure.pending.push(result);
                }
                Ok(result)
            }
            UseKind::Take => {
                let result = self.fresh(ty);
                self.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Take,
                });
                if let SlotKind::Capture(ordinal) = self.out.slots[place.slot.index()].kind {
                    if !place.steps.is_empty() {
                        return unsupported("partial capture take");
                    }
                    self.clear_capture(ordinal)?;
                }
                self.state.take(place.slot, &place.steps);
                if place.steps.is_empty() {
                    self.versions[place.slot.index()] = None;
                }
                if self.holds(result) {
                    self.failure.pending.push(result);
                }
                Ok(result)
            }
            // A value read through a mutable loan (`x += y` reads `mut x`):
            // a view inside that loan, like a shared one.
            UseKind::Borrow | UseKind::Mut => {
                let holding = if self.owns(ty) {
                    match loan {
                        Some(l) => Holding::Borrowed(self.loan(l)?),
                        None => {
                            return unsupported(
                                "a borrowed read of counted storage outside any loan",
                            );
                        }
                    }
                } else {
                    Holding::Plain
                };
                let result = self.value(ty, holding);
                self.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Borrow,
                });
                Ok(result)
            }
        }
    }

    /// A String or Bytes part of a flat value viewed at its source type under
    /// `loan` (LIR schema revision 4); other variable parts have no view.
    fn flat_view(&mut self, address: AddressId, loan: LoanId, source: LirTypeId) -> R<ValueId> {
        if !matches!(self.record(source).kind, LayoutKind::Slice { .. }) {
            return unsupported(
                "a variable-size part of a flat value other than String or Bytes at its source type",
            );
        }
        let result = self.value(source, Holding::Borrowed(loan));
        self.push(Inst::FlatView { result, address });
        Ok(result)
    }

    /// A new loan of this pass, `active` in the current state. A later pass
    /// starts from states joined over the previous pass's edges, which
    /// already name the loans this pass allocates after a loop head, so the
    /// state is set at the loan's id, never appended.
    fn new_loan(&mut self, info: LoanInfo, active: Tri) -> LoanId {
        let loan = LoanId(self.out.tables.loans.len() as u32);
        self.out.tables.loans.push(info);
        if self.state.loans.len() <= loan.index() {
            self.state.loans.resize(loan.index() + 1, Tri::No);
        }
        self.state.loans[loan.index()] = active;
        loan
    }

    /// A loan the TIR does not name, begun here on a prepared address.
    fn synthetic_loan(&mut self, address: AddressId) -> LoanId {
        let info = LoanInfo {
            address,
            mutable: false,
        };
        let loan = self.new_loan(info, Tri::Yes);
        self.push(Inst::BeginLoan {
            loan,
            address,
            mutable: false,
            if_inactive: false,
        });
        loan
    }

    /// A slot the caller lends: a borrowed parameter in its own storage.
    fn borrowed_storage(&self, slot: SlotId) -> bool {
        let SlotKind::Parameter(k) = self.out.slots[slot.index()].kind else {
            return false;
        };
        let abi = &self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize];
        abi.parameters.get(k as usize).map(|p| p.convention) == Some(Passing::Borrow)
    }

    /// A `mut` parameter's storage, which the caller reads back.
    fn mut_storage(&self, slot: SlotId) -> bool {
        let SlotKind::Parameter(k) = self.out.slots[slot.index()].kind else {
            return false;
        };
        let abi = &self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize];
        abi.parameters.get(k as usize).map(|p| p.convention) == Some(Passing::Mut)
    }

    /// A value consumed by an operation leaves the pending set.
    fn consumed(&mut self, v: ValueId) {
        self.failure.pending.retain(|&p| p != v);
    }
}

impl Importer<'_, '_> {
    // --- failure successors and cleanup chains ---

    /// The TIR actions of a cleanup chain, in execution order.
    fn chain_actions(&self, cleanup: tir::CleanupId) -> R<Vec<Action>> {
        let Some(c) = self.body.cleanups.get(cleanup.0 as usize) else {
            return malformed(format!("cleanup {} outside the body", cleanup.0));
        };
        let mut out = Vec::new();
        let mut next = c.head;
        while let Some(node) = next {
            out.push(Action::Tir { node: node.0 });
            next = self.body.cleanup_nodes[node.0 as usize].parent;
        }
        Ok(out)
    }

    /// An exit's actions: the TIR chain, with the internal loop owners of
    /// borrowed parameters released where parameters are (after inner scopes
    /// and temporaries, before the parameter-scope names).
    fn exit_actions(&self, mut actions: Vec<Action>) -> Vec<Action> {
        let Some(plan) = self.plan else {
            return actions;
        };
        let abi = &self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize];
        let mut releases: Vec<Action> = Vec::new();
        for (k, home) in plan.homes.iter().enumerate().rev() {
            if *home == Home::Loop && abi.parameters[k].convention == Passing::Borrow {
                let slot = self.loop_slots[k].expect("loop slot");
                releases.push(Action::Release {
                    slot,
                    anchor: (LOOP_RELEASE, k as u32),
                });
            }
        }
        releases.sort_by_key(|a| match a {
            Action::Release { slot, .. } => std::cmp::Reverse(slot.0),
            _ => std::cmp::Reverse(0),
        });
        let position = actions
            .iter()
            .position(|a| match a {
                Action::Tir { node, .. } => match self.body.cleanup_nodes[*node as usize].action {
                    tir::CleanupAction::Drop { place, .. } => {
                        let root = self.body.places[place.0 as usize].root;
                        let local = &self.body.locals[root.0 as usize];
                        local.scope.0 == 0 && local.storage != tir::Storage::Temporary
                    }
                    _ => false,
                },
                _ => false,
            })
            .unwrap_or(actions.len());
        actions.splice(position..position, releases);
        actions
    }

    /// Release what the state says is owned, for a failure TIR names no
    /// cleanup for: end active loans, then release in TIR's nested order
    /// (inner scopes first, temporaries before names, later slots first).
    fn state_actions(&self) -> Vec<Action> {
        let abi = &self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize];
        let mut slots: Vec<SlotId> = self
            .state
            .slots
            .lacking(EMPTY)
            .into_iter()
            .map(|(s, _)| SlotId(s as u32))
            .filter(|&s| s.index() < self.out.slots.len())
            .filter(|&s| {
                let info = &self.out.slots[s.index()];
                let owned_here = match info.kind {
                    SlotKind::Parameter(k) => {
                        abi.parameters.get(k as usize).map(|p| p.convention) == Some(Passing::Take)
                    }
                    SlotKind::Capture(_) | SlotKind::ReturnSlot => false,
                    _ => true,
                };
                let st = &self.state.slots[s.index()];
                owned_here && st.live.may() && st.present().may() && self.owns(info.ty)
            })
            .collect();
        slots.sort_by_key(|s| {
            let info = &self.out.slots[s.index()];
            (
                std::cmp::Reverse(info.scope),
                info.kind != SlotKind::Temporary,
                std::cmp::Reverse(s.0),
            )
        });
        // Views end before any holder is released.
        let mut out: Vec<Action> = self
            .state
            .loans
            .lacking(DEAD)
            .into_iter()
            .map(|(l, _)| Action::EndLoan {
                loan: LoanId(l as u32),
            })
            .collect();
        out.extend(slots.into_iter().map(|slot| Action::Release {
            slot,
            anchor: (STATE_RELEASE, slot.0),
        }));
        out
    }

    /// The actions a failure here runs.
    fn failure_actions(&self) -> R<Vec<Action>> {
        let mut actions = self.failure.before.clone();
        match self.failure.cleanup {
            Some(c) => actions.extend(self.chain_actions(c)?),
            None => return Ok(self.state_actions()),
        }
        Ok(self.exit_actions(actions))
    }

    /// Structural equality or order of two non-scalar operands:
    /// `Inst::Compare` over the left operand's layout, failing with MEM-3
    /// Depth (`hirsel_interp::structural::binary`). The comparison only reads,
    /// so an owned operand is read through a view under a synthetic loan; a
    /// taken one is released after the comparison, as the interpreter discards
    /// it, and a copied one is never copied.
    fn compare(
        &mut self,
        op: BinaryOp,
        left: &tir::Operand,
        right: &tir::Operand,
        ty: LirTypeId,
    ) -> R<ValueId> {
        let op = match op {
            BinaryOp::Eq => CompareOp::Eq,
            BinaryOp::Ne => CompareOp::Ne,
            BinaryOp::Lt => CompareOp::Lt,
            BinaryOp::Le => CompareOp::Le,
            BinaryOp::Gt => CompareOp::Gt,
            BinaryOp::Ge => CompareOp::Ge,
            other => return malformed(format!("{other:?} of non-scalar values")),
        };
        let mut loans = Vec::new();
        let mut taken = Vec::new();
        let l = self.compared(left, &mut loans, &mut taken)?;
        let r = self.compared(right, &mut loans, &mut taken)?;
        let layout = self.layout_of(self.value_type(l));
        let result = self.fresh(ty);
        self.reading(loans, taken, |this| {
            let failure = this.fail(Some("MEM-3".to_string()))?;
            this.push(Inst::Compare {
                result,
                op,
                left: l,
                right: r,
                layout,
                failure,
            });
            Ok(())
        })?;
        Ok(result)
    }

    /// Emit an operation that only reads operands `compared` prepared: their
    /// synthetic loans end and the taken slots are released after it, and on
    /// its failure edge before the frame's cleanup.
    fn reading(
        &mut self,
        loans: Vec<LoanId>,
        mut taken: Vec<SlotId>,
        emit: impl FnOnce(&mut Self) -> R<()>,
    ) -> R<()> {
        // Temporaries of one scope release in descending slot order.
        taken.sort_by(|a: &SlotId, b| b.cmp(a));
        let release: Vec<Action> = loans
            .into_iter()
            .map(|loan| Action::EndLoan { loan })
            .chain(taken.into_iter().map(|slot| Action::Release {
                slot,
                anchor: (INSTRUCTION_DROP, self.statement_action()),
            }))
            .collect();
        self.failure.before.splice(0..0, release.iter().cloned());
        emit(self)?;
        self.failure.before.drain(0..release.len());
        for (index, action) in release.iter().enumerate() {
            match self.action(action)? {
                Step2::Inline(inst) => self.push(inst),
                Step2::Drop(drop) => {
                    drop.apply(self);
                    // A failed source release must still drain the remaining
                    // construction temporaries, which TIR does not name.
                    let remaining = &release[index + 1..];
                    self.failure.before.splice(0..0, remaining.iter().cloned());
                    let failure = self.fail(None)?;
                    self.failure.before.drain(0..remaining.len());
                    self.push(Inst::Drop {
                        address: drop.address,
                        if_live: drop.if_live,
                        anchor: drop.anchor,
                        failure,
                    });
                }
                Step2::Skip => {}
            }
        }
        Ok(())
    }

    /// One comparison operand as a read-only value: a borrowed or plain
    /// operand as usual, an owned copied or taken one as a view under a
    /// synthetic loan (`loans`). A taken owner moves into operation-owned
    /// temporary storage and releases after the operation (`taken`). Those
    /// temporaries follow all source staging slots: failure releases the
    /// operands before any kept source or intermediate copy in TIR cleanup.
    fn compared(
        &mut self,
        op: &tir::Operand,
        loans: &mut Vec<LoanId>,
        taken: &mut Vec<SlotId>,
    ) -> R<ValueId> {
        if let Some(view) = self.joined_view(op) {
            return view;
        }
        let p = self.place(op.place)?;
        if let Some((source, _)) = p.source {
            // A variable part of a flat value is read in place, never copied.
            if op.usage == UseKind::Take {
                return unsupported("a taken part of a flat value as a comparison operand");
            }
            let address = self.address(&p)?;
            let loan = match p.loan {
                Some(l) if op.usage == UseKind::Borrow => self.loan(l)?,
                _ => {
                    let loan = self.synthetic_loan(address);
                    loans.push(loan);
                    loan
                }
            };
            return self.flat_view(address, loan, source);
        }
        if !matches!(op.usage, UseKind::Take | UseKind::Copy) || !self.owns(p.ty) {
            return self.operand(op);
        }
        let take = op.usage == UseKind::Take && !self.borrowed_storage(p.slot);
        if take && (!p.fixed || !p.steps.is_empty()) {
            return unsupported("a taken part of a value as a comparison operand");
        }
        let p = if take {
            let owned = self.operand(op)?;
            let scope = self.out.slots[p.slot.index()].scope;
            let slot = self.new_slot(p.ty, SlotKind::Temporary, scope);
            self.push(Inst::StorageLive { slot });
            self.state.live(slot);
            let place = self.whole(slot);
            let address = self.address(&place)?;
            self.store(&place, address, owned)?;
            place
        } else {
            p
        };
        let address = self.address(&p)?;
        let loan = self.synthetic_loan(address);
        let reaching = self.reaching(p.slot)?;
        let v = self.value(p.ty, Holding::Borrowed(loan));
        self.push(Inst::Read {
            result: v,
            address,
            reaching,
            mode: ReadMode::Borrow,
        });
        loans.push(loan);
        if take {
            taken.push(p.slot);
        }
        Ok(v)
    }

    /// A failure successor for the operation at hand: dispose pending owned
    /// values, run the failure cleanup, propagate.
    fn fail(&mut self, rule: Option<String>) -> R<FailureId> {
        let actions = self.failure_actions()?;
        let pending = self.failure.pending.clone();
        let target = if pending.is_empty() {
            self.chain(&actions)?
        } else {
            self.dispose(&pending, &actions, false)?
        };
        let anchor = self.anchor_here();
        let edge = self.edge(target, pending);
        Ok(self.failure_edge(edge, rule, anchor))
    }

    /// A failure-cleanup block that receives `pending` owned values and
    /// disposes of them through scratch slots (see `fill_dispose`).
    fn dispose(&mut self, pending: &[ValueId], actions: &[Action], outer: bool) -> R<BlockId> {
        let saved = self.save();
        let block = self.block(BlockKind::FailureCleanup);
        self.current = block;
        self.fill_dispose(pending, actions, outer)?;
        self.restore(saved);
        Ok(block)
    }

    /// Fill the current failure-cleanup block: receive `pending` and move each
    /// into a scratch slot. Values pending when an operation fails go first,
    /// in the innermost scope the cleanup releases. Values pending when a
    /// cleanup drop fails (a returned result) outlive the frame's cleanup:
    /// they go last, in the synthetic scope enclosing the body's scopes, so
    /// cleanup order stays nested.
    fn fill_dispose(&mut self, pending: &[ValueId], actions: &[Action], outer: bool) -> R<()> {
        let block = self.current;
        let innermost = actions
            .iter()
            .find_map(|a| match a {
                Action::Tir { node, .. } => match self.body.cleanup_nodes[*node as usize].action {
                    tir::CleanupAction::Drop { place, .. } => {
                        let root = self.body.places[place.0 as usize].root;
                        self.local_slot[root.0 as usize]
                            .map(|slot| self.out.slots[slot.index()].scope)
                    }
                    _ => None,
                },
                Action::Release { slot, .. } => Some(self.out.slots[slot.index()].scope),
                Action::EndLoan { .. } => None,
            })
            .unwrap_or(ScopeId(0));
        let scope = if outer {
            self.outer_scope = true;
            ScopeId(self.body.scopes.len() as u32)
        } else {
            innermost
        };
        let mut releases = Vec::new();
        for &v in pending {
            let ty = self.value_type(v);
            let param = self.value(ty, Holding::Owned);
            self.out.blocks[block.index()].params.push(param);
            let slot = self.new_slot(ty, SlotKind::Temporary, scope);
            self.push(Inst::StorageLive { slot });
            self.state.live(slot);
            let p = self.whole(slot);
            let address = self.fixed_address(&p)?;
            let version = self.value(ty, Holding::Plain);
            self.push(Inst::Store {
                address,
                value: param,
                version,
            });
            self.state.write(slot, &[]);
            self.versions[slot.index()] = Some(version);
            let n = releases.len() as u32;
            releases.push(Action::Release {
                slot,
                anchor: (SCRATCH_DISPOSAL, n),
            });
        }
        releases.reverse();
        if outer {
            let mut all = actions.to_vec();
            all.extend(releases);
            self.run_failure(&all)
        } else {
            releases.extend(actions.iter().cloned());
            self.run_failure(&releases)
        }
    }

    /// The failure-cleanup chain running `actions` from the current state.
    /// Chains share tails: two failure points meet at the first block from
    /// which both run the same instructions on slots in the same state, so
    /// what else differs between them (their statement temporaries, slots a
    /// prefix has already released) costs no block.
    fn chain(&mut self, actions: &[Action]) -> R<BlockId> {
        let saved = self.save();
        let links = self.links(actions)?;
        let mut next = None;
        for link in links.into_iter().rev() {
            let key = (link, next);
            next = Some(match self.chain_memo.get(&key) {
                Some(&block) => block,
                None => {
                    let block = self.block(BlockKind::FailureCleanup);
                    self.current = block;
                    self.link(&key.0, next);
                    self.chain_memo.insert(key, block);
                    block
                }
            });
        }
        self.restore(saved);
        Ok(next.expect("a chain ends with its propagation"))
    }

    /// Emit `actions` in failure mode into the current block, which keeps
    /// the first link; the rest of the chain is shared (`chain`).
    fn run_failure(&mut self, actions: &[Action]) -> R<()> {
        let block = self.current;
        let mut links = self.links(actions)?.into_iter();
        let first = links.next().expect("a chain ends with its propagation");
        let rest: Vec<Link> = links.collect();
        let mut next = None;
        for link in rest.into_iter().rev() {
            let key = (link, next);
            next = Some(match self.chain_memo.get(&key) {
                Some(&block) => block,
                None => {
                    let block = self.block(BlockKind::FailureCleanup);
                    self.current = block;
                    self.link(&key.0, next);
                    self.chain_memo.insert(key, block);
                    block
                }
            });
        }
        self.current = block;
        self.link(&first, next);
        Ok(())
    }

    /// What `actions` do from the current state, one link per block of the
    /// chain: the loan ends up to a drop and that drop, the last link ending
    /// in the propagation. The state is left as the chain leaves it.
    fn links(&mut self, actions: &[Action]) -> R<Vec<Link>> {
        let mut links = Vec::new();
        let mut ends = Vec::new();
        for action in actions {
            match self.action(action)? {
                Step2::Skip => {}
                Step2::Inline(Inst::EndLoan { loan, if_active }) => ends.push((loan, if_active)),
                Step2::Inline(other) => {
                    return malformed(format!("{other:?} as a cleanup action"));
                }
                Step2::Drop(drop) => {
                    let before = self.state.slots[drop.slot.index()].clone();
                    drop.apply(self);
                    links.push(Link {
                        ends: std::mem::take(&mut ends),
                        drop: Some((drop.address, drop.if_live, drop.anchor, before)),
                    });
                }
            }
        }
        links.push(Link { ends, drop: None });
        Ok(links)
    }

    /// Fill the current block with one link continuing at `next`: a drop that
    /// fails continues the same remaining chain, so both successors meet.
    fn link(&mut self, link: &Link, next: Option<BlockId>) {
        for &(loan, if_active) in &link.ends {
            self.push(Inst::EndLoan { loan, if_active });
        }
        let (Some((address, if_live, anchor, _)), Some(next)) = (&link.drop, next) else {
            self.terminate(Terminator::Propagate);
            return;
        };
        let source = self.anchor_here();
        let fe = self.edge(next, Vec::new());
        let failure = self.failure_edge(fe, None, source);
        self.push(Inst::Drop {
            address: *address,
            if_live: *if_live,
            anchor: *anchor,
            failure,
        });
        let e = self.edge(next, Vec::new());
        self.terminate(Terminator::Jump(e));
    }

    /// Emit `actions` on a normal path into `Cleanup` blocks, ending with the
    /// current block still open. A drop that fails runs the rest of the list,
    /// then `tail`, disposing the pending values first.
    fn run_normal(&mut self, actions: &[Action], tail: &[Action]) -> R<()> {
        let mut opened = false;
        for (i, action) in actions.iter().enumerate() {
            let step = self.action(action)?;
            if matches!(step, Step2::Skip) {
                continue;
            }
            if !opened {
                self.split(BlockKind::Cleanup);
                opened = true;
            }
            match step {
                Step2::Skip => {}
                Step2::Inline(inst) => self.push(inst),
                Step2::Drop(drop) => {
                    let (address, if_live, anchor) = drop.parts();
                    drop.apply(self);
                    let mut rest: Vec<Action> = actions[i + 1..].to_vec();
                    rest.extend(tail.iter().cloned());
                    let pending = self.failure.pending.clone();
                    let target = if pending.is_empty() {
                        self.chain(&rest)?
                    } else {
                        self.dispose(&pending, &rest, true)?
                    };
                    let source = self.anchor_here();
                    let fe = self.edge(target, pending);
                    let failure = self.failure_edge(fe, None, source);
                    self.push(Inst::Drop {
                        address,
                        if_live,
                        anchor,
                        failure,
                    });
                }
            }
        }
        Ok(())
    }

    /// What one action does from the current state: nothing (its slot or loan
    /// is definitely inactive), an inline loan end, or a drop.
    fn action(&mut self, action: &Action) -> R<Step2> {
        let (slot, steps, anchor_at, span) = match action {
            Action::EndLoan { loan } => {
                return Ok(match self.state.loans[loan.index()] {
                    Tri::No => Step2::Skip,
                    t => {
                        self.state.loans[loan.index()] = Tri::No;
                        Step2::Inline(Inst::EndLoan {
                            loan: *loan,
                            if_active: t == Tri::Maybe,
                        })
                    }
                });
            }
            Action::Release { slot, anchor } => {
                // Plain content needs no release; storage end covers it.
                if !self.owns(self.out.slots[slot.index()].ty) {
                    return Ok(Step2::Skip);
                }
                (*slot, Vec::new(), *anchor, self.span)
            }
            Action::Tir { node } => match &self.body.cleanup_nodes[*node as usize].action {
                tir::CleanupAction::EndLoan { loan, .. } => {
                    if self.elided_loan(*loan) {
                        return Ok(Step2::Skip);
                    }
                    let loan = self.loan(*loan)?;
                    return self.action(&Action::EndLoan { loan });
                }
                tir::CleanupAction::Drop { place, .. } => {
                    if self.elided_place(*place) {
                        return Ok(Step2::Skip);
                    }
                    let tp = &self.body.places[place.0 as usize];
                    let root = tp.root.0 as usize;
                    if tp.projection.is_empty()
                        && (self.aliases[root].is_some() || self.joined[root])
                    {
                        // A view temporary owns nothing.
                        return Ok(Step2::Skip);
                    }
                    let p = self.place(*place)?;
                    if matches!(self.out.slots[p.slot.index()].kind, SlotKind::Capture(_)) {
                        // A closure's captures are its caller's environment,
                        // lent for the call: the environment releases them.
                        return Ok(Step2::Skip);
                    }
                    if !p.fixed {
                        return unsupported("a cleanup drop of a computed place");
                    }
                    let root = self.body.places[place.0 as usize].root;
                    let span = self.body.locals[root.0 as usize].span;
                    (p.slot, p.steps, self.node_anchors[*node as usize], span)
                }
                other => {
                    return unsupported(format!(
                        "cleanup action {other:?} (M2; the TIR validator reports it unsupported)"
                    ));
                }
            },
        };
        let st = &self.state.slots[slot.index()];
        let presence = if st.live == Tri::No {
            Tri::No
        } else if steps.is_empty() {
            match st.present() {
                Tri::Yes if st.live == Tri::Yes => Tri::Yes,
                Tri::No => Tri::No,
                _ => Tri::Maybe,
            }
        } else {
            let moved = st
                .moved
                .iter()
                .filter(|(q, _)| overlap(q, &steps))
                .map(|(q, t)| (q.len() <= steps.len(), *t))
                .collect::<Vec<_>>();
            if st.init == Tri::No || moved.iter().any(|&(covers, t)| covers && t == Tri::Yes) {
                Tri::No
            } else if st.init == Tri::Yes
                && st.live == Tri::Yes
                && moved.iter().all(|&(_, t)| t == Tri::No)
            {
                Tri::Yes
            } else {
                Tri::Maybe
            }
        };
        if presence == Tri::No {
            return Ok(Step2::Skip);
        }
        let ty = self.out.slots[slot.index()].ty;
        let place = PlaceRef {
            slot,
            steps: steps.clone(),
            projection: steps
                .iter()
                .map(|s| match s {
                    Step::Payload => ProjectionStep::Payload,
                    Step::Field(f) => ProjectionStep::Field(*f),
                    _ => unreachable!("fixed cleanup places have field steps only"),
                })
                .collect(),
            ty,
            fixed: true,
            crosses: false,
            loan: None,
            source: None,
            site: None,
        };
        let place = if steps.is_empty() {
            place
        } else {
            // The part's own type.
            let mut layout = self.layout_of(ty);
            for s in &steps {
                let Step::Field(f) = s else { unreachable!() };
                layout = self
                    .layouts
                    .record(layout)
                    .fields
                    .iter()
                    .find(|x| x.ordinal == *f)
                    .map(|x| x.layout)
                    .expect("field of a resolved place");
            }
            PlaceRef {
                ty: self.intern(layout),
                ..place
            }
        };
        let address = self.fixed_address(&place)?;
        let anchor = self.cleanup_anchor(anchor_at.0, anchor_at.1, span);
        Ok(Step2::Drop(DropStep {
            address,
            if_live: presence != Tri::Yes,
            anchor,
            slot,
            steps,
        }))
    }
}

enum Step2 {
    Skip,
    Inline(Inst),
    Drop(DropStep),
}

struct DropStep {
    address: AddressId,
    if_live: bool,
    anchor: CleanupAnchorId,
    slot: SlotId,
    steps: Vec<Step>,
}

impl DropStep {
    fn parts(&self) -> (AddressId, bool, CleanupAnchorId) {
        (self.address, self.if_live, self.anchor)
    }
    fn apply(&self, im: &mut Importer<'_, '_>) {
        im.state.take(self.slot, &self.steps);
        if self.steps.is_empty() {
            im.versions[self.slot.index()] = None;
        }
    }
}

fn st_maybe(s: &SlotState) -> bool {
    s.init != Tri::Yes || s.moved.values().any(|t| t.may())
}

/// The address of a computed-place loan before its `BeginLoan` is emitted.
const UNPREPARED: AddressId = AddressId(u32::MAX);

/// What a call is declared to do: everything but control (callees are opaque
/// here; effect inference narrows later stages).
pub(crate) const CALL_EFFECTS: Effects = Effects::READS
    .union(Effects::WRITES)
    .union(Effects::OWNS)
    .union(Effects::MAY_FAIL)
    .union(Effects::ALLOCATES)
    .union(Effects::OBSERVABLE)
    .union(Effects::BARRIER);

impl Importer<'_, '_> {
    fn destination_class(&self, d: &hirsel_types::Destination) -> R<DestinationClass> {
        use hirsel_types::Destination as D;
        Ok(match d {
            D::Heap => DestinationClass::Heap,
            D::Inferred => DestinationClass::Inferred,
            D::Region(_) => DestinationClass::Region,
            D::Caller(parameters) => {
                DestinationClass::Caller(parameters.iter().map(|p| p.parameter().0).collect())
            }
            D::Arena(origin) => DestinationClass::Arena(self.arena_slot(origin)?),
        })
    }

    /// The slot holding the arena token a relation names: a whole parameter
    /// or binding of this body.
    fn arena_slot(&self, origin: &hirsel_types::Origin) -> R<SlotId> {
        use hirsel_types::{OriginRoot, ParameterRef};
        if !origin.fields.is_empty() {
            return unsupported("an arena destination naming a field path");
        }
        let body = self.body;
        let parameter = match &origin.root {
            OriginRoot::Parameter(ParameterRef::Body { scope, parameter })
                if *scope == body.parameter_scope =>
            {
                body.parameters.get(parameter.0 as usize).copied()
            }
            _ => None,
        };
        let root = parameter.or_else(|| {
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
        match root {
            Some(root) => self.slot_for(root.0 as usize),
            None => unsupported("an arena destination naming storage outside this body"),
        }
    }

    fn whole(&self, slot: SlotId) -> PlaceRef {
        PlaceRef {
            slot,
            steps: Vec::new(),
            projection: Vec::new(),
            ty: self.out.slots[slot.index()].ty,
            fixed: true,
            crosses: false,
            loan: None,
            source: None,
            site: None,
        }
    }

    /// Store `value` at a prepared address; the root slot gets a new version.
    /// Plain content still present (a plain `take` of `mut` storage kept its
    /// bits) is overwritten through a scratch displaced slot.
    fn store(&mut self, place: &PlaceRef, address: AddressId, value: ValueId) -> R<()> {
        let ty = self.out.slots[place.slot.index()].ty;
        let st = &self.state.slots[place.slot.index()];
        let present = st.init != Tri::No
            && !st.moved.iter().any(|(q, t)| {
                *t == Tri::Yes && q.len() <= place.steps.len() && overlap(q, &place.steps)
            });
        if present && self.bits(place.ty) && self.mut_storage(place.slot) {
            let scratch = self.new_slot(
                place.ty,
                SlotKind::Displaced,
                self.out.slots[place.slot.index()].scope,
            );
            self.push(Inst::StorageLive { slot: scratch });
            self.state.live(scratch);
            let version = self.value(ty, Holding::Plain);
            let scratch_ty = self.out.slots[scratch.index()].ty;
            let displaced_version = self.value(scratch_ty, Holding::Plain);
            let failure = self.fail(None)?;
            self.push(Inst::Replace {
                address,
                value,
                displaced: scratch,
                version,
                displaced_version,
                if_live: st_maybe(&self.state.slots[place.slot.index()]),
                failure,
            });
            self.consumed(value);
            self.state.write(place.slot, &place.steps);
            self.versions[place.slot.index()] = Some(version);
            self.push(Inst::StorageDead {
                slot: scratch,
                if_live: false,
            });
            self.state.dead(scratch);
            return Ok(());
        }
        let version = self.value(ty, Holding::Plain);
        self.push(Inst::Store {
            address,
            value,
            version,
        });
        self.consumed(value);
        self.state.write(place.slot, &place.steps);
        self.versions[place.slot.index()] = Some(version);
        Ok(())
    }

    /// A per-statement anchor for drops TIR states as instructions.
    fn statement_action(&self) -> u32 {
        (self.statement.0 << 16) | (self.statement.1 & 0xffff)
    }

    /// Whether an instruction only maintains a view temporary's own storage,
    /// which does not exist: its reads go to the viewed place.
    /// Maintain a joined view temporary, which has no storage: an assignment
    /// holds the assigned view as its SSA value, a storage end or drop lets it
    /// go, and an origin binding of it names nothing. Whether `kind` was one.
    fn on_joined(&mut self, kind: &tir::InstructionKind) -> R<bool> {
        use tir::InstructionKind as K;
        let joined = |p: &tir::PlaceId| {
            let place = &self.body.places[p.0 as usize];
            let l = place.root.0 as usize;
            (self.joined[l] && place.projection.is_empty()).then_some(l)
        };
        match kind {
            K::StorageLive(local) => Ok(self.joined[local.0 as usize]),
            K::StorageDead { place, .. } | K::Drop { place, .. } => match joined(place) {
                Some(l) => {
                    self.state.views[l] = Tri::No;
                    self.views[l] = None;
                    Ok(true)
                }
                None => Ok(false),
            },
            K::Assign { destination, value } => {
                let Some(l) = joined(destination) else {
                    return Ok(false);
                };
                let tir::Rvalue::Use(o) = &self.body.rvalues[value.0 as usize] else {
                    return malformed("a joined view assigned other than a view");
                };
                let v = self.operand(o)?;
                if self.value_type(v) != self.out.slots[self.slot_for(l)?.index()].ty {
                    return unsupported("a joined view of a place of another type");
                }
                self.state.views[l] = Tri::Yes;
                self.views[l] = Some(v);
                Ok(true)
            }
            K::BindOrigin { source, place, .. } => match (joined(source), joined(place)) {
                (_, Some(_)) => Ok(true),
                (Some(_), None) => unsupported("an origin bound from a joined view"),
                (None, None) => Ok(false),
            },
            _ => Ok(false),
        }
    }

    fn on_view(&self, kind: &tir::InstructionKind) -> bool {
        use tir::InstructionKind as K;
        let view = |p: &tir::PlaceId| {
            let place = &self.body.places[p.0 as usize];
            place.projection.is_empty() && self.aliases[place.root.0 as usize].is_some()
        };
        match kind {
            K::StorageLive(local) => self.aliases[local.0 as usize].is_some(),
            K::StorageDead { place, .. } | K::Drop { place, .. } => view(place),
            K::Assign { destination, .. } => view(destination),
            _ => false,
        }
    }

    fn instruction(&mut self, kind: &tir::InstructionKind) -> R<()> {
        use tir::InstructionKind as K;
        if let K::Assign { destination, value } = kind {
            let root = self.body.places[destination.0 as usize].root;
            if let Some(&(list, field)) = self.fields.opens.get(&root) {
                return self.field_open(list, field);
            }
            if let Some(&list) = self.builders.heads.get(&root) {
                let open = self.fields.closes.get(&root).copied();
                return self.flat_append(list, *value, open);
            }
        }
        let elided = match kind {
            K::StorageLive(local) => self.local_slot[local.0 as usize].is_none(),
            K::StorageDead { place, .. } | K::Drop { place, .. } | K::EndOrigin(place) => {
                self.elided_place(*place)
            }
            K::BeginLoan { id, place, .. } => self.elided_place(*place) || self.elided_loan(*id),
            K::Assign { destination, .. } | K::Snapshot { destination, .. } => {
                self.elided_place(*destination)
            }
            K::BindOrigin { source, place, .. } => {
                self.elided_place(*source) || self.elided_place(*place)
            }
            K::EndLoan { loan, .. } => self.elided_loan(*loan),
            _ => false,
        };
        if elided {
            return Ok(());
        }
        match kind {
            K::StorageLive(local) if self.observations[local.0 as usize] => return Ok(()),
            K::Assign { destination, value }
                if self.observations[self.body.places[destination.0 as usize].root.0 as usize] =>
            {
                let tir::Rvalue::Use(o) = &self.body.rvalues[value.0 as usize] else {
                    return malformed("discarded callable read is not a borrowed use");
                };
                self.operand(o)?;
                return Ok(());
            }
            _ => {}
        }
        if self.on_view(kind) {
            return Ok(());
        }
        if self.on_joined(kind)? {
            return Ok(());
        }
        match kind {
            K::StorageLive(local) => {
                let slot = self.slot_for(local.0 as usize)?;
                self.push(Inst::StorageLive { slot });
                self.state.live(slot);
                self.versions[slot.index()] = None;
            }
            K::StorageDead { place, .. } => {
                let p = self.place(*place)?;
                if !p.steps.is_empty() {
                    return malformed("storage end of a part");
                }
                let live = self.state.slots[p.slot.index()].live;
                if live != Tri::No {
                    self.push(Inst::StorageDead {
                        slot: p.slot,
                        if_live: live == Tri::Maybe,
                    });
                    self.state.dead(p.slot);
                    self.versions[p.slot.index()] = None;
                }
            }
            K::EndScope(scope) => self.push(Inst::EndScope {
                scope: ScopeId(scope.0),
            }),
            K::Assign { destination, value } => {
                let rvalue = &self.body.rvalues[value.0 as usize];
                let p = self.place(*destination)?;
                let v = self.rvalue(rvalue, p.ty)?;
                let v = self.weaken(v, p.ty);
                let address = self.address(&p)?;
                self.store(&p, address, v)?;
            }
            K::Snapshot { destination, value } => {
                let p = self.place(*destination)?;
                let v = self.operand(value)?;
                let v = self.weaken(v, p.ty);
                let address = self.address(&p)?;
                self.store(&p, address, v)?;
            }
            K::Replace {
                destination,
                value,
                displaced,
                ..
            } => self.replace(*destination, value, *displaced)?,
            K::BeginLoan {
                id, place, mutable, ..
            } => {
                let loan = self.loan(*id)?;
                let st = self.state.loans[loan.index()];
                if st != Tri::Yes {
                    let mut address = self.out.tables.loans[loan.index()].address;
                    if address == UNPREPARED {
                        let p = self.place(*place)?;
                        address = self.address(&p)?;
                        self.out.tables.loans[loan.index()].address = address;
                    }
                    self.push(Inst::BeginLoan {
                        loan,
                        address,
                        mutable: *mutable,
                        if_inactive: st == Tri::Maybe,
                    });
                    self.state.loans[loan.index()] = Tri::Yes;
                }
            }
            K::EndLoan { loan, .. } => {
                let loan = self.loan(*loan)?;
                if let Step2::Inline(inst) = self.action(&Action::EndLoan { loan })? {
                    self.push(inst);
                }
            }
            K::Drop { place, .. } => {
                let p = self.place(*place)?;
                if !p.fixed {
                    return unsupported("a drop of a computed place");
                }
                let action = Action::Release {
                    slot: p.slot,
                    anchor: (INSTRUCTION_DROP, self.statement_action()),
                };
                let step = if p.steps.is_empty() {
                    self.action(&action)?
                } else {
                    self.part_drop(&p)?
                };
                if let Step2::Drop(drop) = step {
                    drop.apply(self);
                    let failure = self.fail(None)?;
                    self.push(Inst::Drop {
                        address: drop.address,
                        if_live: drop.if_live,
                        anchor: drop.anchor,
                        failure,
                    });
                }
            }
            K::RunCleanup(cleanup) => {
                let actions = self.chain_actions(*cleanup)?;
                let tail = self.failure_actions()?;
                let before = self.current;
                self.run_normal(&actions, &tail)?;
                if self.current != before {
                    self.split(BlockKind::Body);
                }
            }
            K::Check { condition, rule } => {
                let c = self.operand(condition)?;
                let failure = self.fail(Some(rule.to_string()))?;
                self.push(Inst::Check {
                    condition: c,
                    failure,
                });
            }
            K::BindOrigin {
                source,
                place,
                kind,
            } => {
                for id in [*source, *place] {
                    self.origin_sites.entry(id).or_insert(self.span);
                }
                let s = self.place(*source)?;
                let source = self.origin_address(&s)?;
                let p = self.place(*place)?;
                let place = self.origin_address(&p)?;
                self.push(Inst::BindOrigin {
                    source,
                    place,
                    kind: match kind {
                        tir::OriginBindingKind::Alias => OriginKind::Alias,
                        tir::OriginBindingKind::Fresh => OriginKind::Fresh,
                    },
                });
            }
            K::BindFields { source, fields } => {
                let s = self.place(*source)?;
                let proven = self.proven_variant(&s);
                let source = self.origin_address(&s)?;
                let mut addresses = Vec::new();
                for f in fields {
                    let p = self.place(*f)?;
                    let field_of_proven = p.slot == s.slot
                        && p.projection.len() == s.projection.len() + 1
                        && p.projection.starts_with(&s.projection)
                        && matches!(
                            p.projection.last(),
                            Some(ProjectionStep::VariantField { variant, .. })
                                if Some(*variant) == proven
                        );
                    let version = self.fact_version(&p);
                    match version {
                        // The tag tests proved the variant: this check cannot
                        // fail, and the arm's reads of the field reuse it.
                        Some(version) if field_of_proven => {
                            let address = self.address(&p)?;
                            self.facts.push(Fact::Prepared {
                                slot: p.slot,
                                projection: p.projection.clone(),
                                version,
                                address,
                            });
                            addresses.push(address);
                        }
                        _ => addresses.push(self.origin_address(&p)?),
                    }
                }
                let fields = PoolRange::push(&mut self.out.pools.addresses, addresses);
                self.push(Inst::BindFields { source, fields });
            }
            K::Defuse { place } => {
                let p = self.place(*place)?;
                let address = self.address(&p)?;
                self.push(Inst::Defuse { address });
            }
            K::EndOrigin(place) => {
                let p = self.place(*place)?;
                let place = self.origin_address(&p)?;
                self.push(Inst::EndOrigin { place });
            }
        }
        Ok(())
    }

    /// A drop of a fixed part: the same state rule as a cleanup drop.
    fn part_drop(&mut self, p: &PlaceRef) -> R<Step2> {
        let span = self.span;
        let anchor = (INSTRUCTION_DROP, self.statement_action());
        // Reuse the cleanup path through a synthetic TIR-free action.
        let st = &self.state.slots[p.slot.index()];
        let covered = st
            .moved
            .iter()
            .any(|(q, t)| q.len() <= p.steps.len() && overlap(q, &p.steps) && *t == Tri::Yes);
        if st.live == Tri::No || st.init == Tri::No || covered {
            return Ok(Step2::Skip);
        }
        let partial = st
            .moved
            .iter()
            .any(|(q, t)| overlap(q, &p.steps) && t.may());
        let sure = st.live == Tri::Yes && st.init == Tri::Yes && !partial;
        let address = self.fixed_address(p)?;
        let anchor = self.cleanup_anchor(anchor.0, anchor.1, span);
        Ok(Step2::Drop(DropStep {
            address,
            if_live: !sure,
            anchor,
            slot: p.slot,
            steps: p.steps.clone(),
        }))
    }

    /// `Replace`: the new value is evaluated, the destination prepared, then
    /// old content moves to the displaced slot and new content is installed
    /// in one step; the displaced value is dropped by the TIR's
    /// following `Drop`.
    fn replace(
        &mut self,
        destination: tir::PlaceId,
        value: &tir::Operand,
        displaced: tir::PlaceId,
    ) -> R<()> {
        let v = self.operand(value)?;
        let p = self.place(destination)?;
        let v = self.weaken(v, p.ty);
        let address = self.address(&p)?;
        let d = self.place(displaced)?;
        if !d.steps.is_empty() {
            return malformed("a displaced part");
        }
        let st = &self.state.slots[p.slot.index()];
        let moved = st
            .moved
            .iter()
            .filter(|(q, _)| overlap(q, &p.steps))
            .map(|(_, t)| *t)
            .fold(Tri::No, Tri::join);
        let present = match (st.init, moved) {
            (Tri::No, _) | (_, Tri::Yes) => Tri::No,
            (Tri::Yes, Tri::No) => Tri::Yes,
            _ => Tri::Maybe,
        };
        if present == Tri::No {
            // Nothing to displace: a plain store; the displaced slot stays empty.
            self.store(&p, address, v)?;
            return Ok(());
        }
        self.consumed(v);
        let failure = self.fail(None)?;
        let ty = self.out.slots[p.slot.index()].ty;
        let version = self.value(ty, Holding::Plain);
        let displaced_ty = self.out.slots[d.slot.index()].ty;
        let displaced_version = self.value(displaced_ty, Holding::Plain);
        self.push(Inst::Replace {
            address,
            value: v,
            displaced: d.slot,
            version,
            displaced_version,
            if_live: present == Tri::Maybe,
            failure,
        });
        self.state.write(p.slot, &p.steps);
        self.versions[p.slot.index()] = Some(version);
        let ds = &mut self.state.slots[d.slot.index()];
        ds.init = if p.steps.is_empty() {
            present
        } else {
            Tri::Yes
        };
        ds.moved.clear();
        self.versions[d.slot.index()] = Some(displaced_version);
        Ok(())
    }

    fn rvalue(&mut self, rvalue: &tir::Rvalue, ty: LirTypeId) -> R<ValueId> {
        use tir::Rvalue as V;
        Ok(match rvalue {
            V::Constant(c) => {
                let value = match c {
                    tir::Constant::Unit => ConstantValue::Unit,
                    tir::Constant::Bool(b) => ConstantValue::Bool(*b),
                    tir::Constant::Integer(n) => ConstantValue::Integer(*n),
                    tir::Constant::F32(x) => ConstantValue::F32(*x),
                    tir::Constant::F64(x) => ConstantValue::F64(*x),
                    tir::Constant::String(s) => ConstantValue::String(s.clone()),
                    tir::Constant::Bytes(b) => ConstantValue::Bytes(b.clone()),
                };
                // A text literal is a static slice: no credit (LIR schema rev 2).
                let literal = matches!(value, ConstantValue::String(_) | ConstantValue::Bytes(_));
                let constant = self.constant(ty, value);
                let result = if literal {
                    self.value(ty, Holding::Plain)
                } else {
                    self.fresh(ty)
                };
                self.push(Inst::Const { result, constant });
                result
            }
            V::ReadConstant { definition } => {
                if let Some(value) = self.program.constants.get(definition)
                    && let tir::ValueKind::Construct {
                        constructor: tir::Constructor::Record(record),
                        fields,
                    } = &value.root.kind
                    && *record == crate::library::core_path("Order")
                    && let [field] = fields.as_slice()
                    && let tir::ValueKind::FunctionReference(function) = &field.kind
                    && crate::library::body_index(function).is_some()
                {
                    let value = value.clone();
                    let tir::ValueKind::Construct { fields, .. } = &value.root.kind else {
                        unreachable!()
                    };
                    let callable = hirsel_types::substitute_type(
                        &value.types,
                        fields[0].ty,
                        &mut self.store,
                        hirsel_types::Substitution {
                            binder: None,
                            roles: &Default::default(),
                            arguments: &[],
                            authorities: &[],
                        },
                    )
                    .map_err(|e| InstanceError::Substitution(definition.clone(), e))?;
                    let callable = self.layout(callable)?;
                    let callable = self.intern(callable);
                    let (symbol, _) = self.direct(function, &[])?;
                    let compare = self.fresh(callable);
                    self.push(Inst::FunctionRef {
                        result: compare,
                        symbol,
                    });
                    let record = self.symbol(SymbolKind::Type, record, None);
                    self.construct_inline(
                        self.layout_of(ty),
                        self.layout_of(ty),
                        ConstructorKind::Record(record),
                        &[compare],
                    )?
                } else {
                    let symbol = self.symbol(SymbolKind::Constant, definition, None);
                    let result = self.fresh(ty);
                    self.push(Inst::ReadStatic { result, symbol });
                    result
                }
            }
            V::FunctionReference {
                definition,
                arguments,
            } => {
                let (symbol, _) = self.direct(definition, arguments)?;
                let result = self.fresh(ty);
                self.push(Inst::FunctionRef { result, symbol });
                result
            }
            V::Size(parameter) => {
                let n = self
                    .instances
                    .view(self.id)
                    .substitution
                    .sizes
                    .iter()
                    .find(|(p, _)| p == parameter)
                    .map(|(_, n)| *n);
                let Some(n) = n else {
                    return malformed("a size parameter without an actual");
                };
                let constant = self.constant(ty, ConstantValue::Integer(u128::from(n)));
                let result = self.fresh(ty);
                self.push(Inst::Const { result, constant });
                result
            }
            V::Use(op) => self.operand(op)?,
            V::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } => self.unique(value, ty)?,
            V::Unary { op, value } => {
                let op = match op {
                    hirsel_syntax::UnaryOp::Neg => UnaryOp::Neg,
                    hirsel_syntax::UnaryOp::Not => UnaryOp::Not,
                    hirsel_syntax::UnaryOp::BitNot => UnaryOp::BitNot,
                    other => return malformed(format!("ownership operator {other:?} as a value")),
                };
                let operand = self.operand(value)?;
                let result = self.fresh(ty);
                if op == UnaryOp::Neg && self.unsigned(ty) {
                    // Negating an unsigned value is `0 - x`: SEM-1 unless x is 0.
                    let constant = self.constant(ty, ConstantValue::Integer(0));
                    let zero = self.fresh(ty);
                    self.push(Inst::Const {
                        result: zero,
                        constant,
                    });
                    let failure = self.fail(Some("SEM-1".to_string()))?;
                    self.push(Inst::Binary {
                        result,
                        op: BinaryOp::Sub,
                        left: zero,
                        right: operand,
                        failure,
                    });
                    return Ok(result);
                }
                let failure = self.fail((op == UnaryOp::Neg).then(|| "SEM-1".to_string()))?;
                self.push(Inst::Unary {
                    result,
                    op,
                    operand,
                    failure,
                });
                result
            }
            V::Binary { op, left, right } => {
                let Some(lop) = lir_binary(*op) else {
                    return unsupported(format!("binary {op:?} as one operation"));
                };
                let side = self.place(left.place)?;
                if !matches!(self.out.types[side.ty.index()].repr, ValueRepr::Scalar(_)) {
                    return self.compare(lop, left, right, ty);
                }
                let l = self.operand(left)?;
                let r = self.operand(right)?;
                let result = self.fresh(ty);
                let arithmetic = matches!(
                    lop,
                    BinaryOp::Add
                        | BinaryOp::Sub
                        | BinaryOp::Mul
                        | BinaryOp::Div
                        | BinaryOp::Rem
                        | BinaryOp::Shl
                        | BinaryOp::Shr
                );
                let failure = self.fail(arithmetic.then(|| "SEM-1".to_string()))?;
                self.push(Inst::Binary {
                    result,
                    op: lop,
                    left: l,
                    right: r,
                    failure,
                });
                result
            }
            V::Construct {
                constructor,
                fields,
                destination,
            } => {
                if matches!(constructor, tir::Constructor::Map | tir::Constructor::Set) {
                    return self.keyed_construct(
                        matches!(constructor, tir::Constructor::Map),
                        fields,
                        self.destination_class(destination)?,
                        ty,
                    );
                }
                let layout = match self.record(ty).kind {
                    LayoutKind::Block { payload } | LayoutKind::Box { payload }
                        if !matches!(self.out.types[ty.index()].repr, ValueRepr::Aggregate) =>
                    {
                        payload
                    }
                    _ => self.layout_of(ty),
                };
                // A flat record encodes each variable field's source value,
                // reading it only (LIR schema revision 4).
                let record = self.layouts.record(layout);
                let encoded: Vec<u32> = if record.kind == LayoutKind::Flat {
                    record
                        .fields
                        .iter()
                        .filter(|d| !flat_fixed(self.layouts, d.layout))
                        .map(|d| d.ordinal)
                        .collect()
                } else {
                    Vec::new()
                };
                let (values, loans, taken) = self.construct_fields(&encoded, fields)?;
                if !encoded.is_empty() {
                    let constructor = match constructor {
                        tir::Constructor::Record(path) => {
                            ConstructorKind::Record(self.symbol(SymbolKind::Type, path, None))
                        }
                        tir::Constructor::Tuple => ConstructorKind::Tuple,
                        other => return malformed(format!("a flat {other:?} construction")),
                    };
                    let destination = self.destination_class(destination)?;
                    // The construction is pending while the encoded sources
                    // are released after it.
                    let mut result = None;
                    self.reading(loans, taken, |this| {
                        let v = this.construct(constructor, layout, values, destination, ty)?;
                        if this.holds(v) {
                            this.failure.pending.push(v);
                        }
                        result = Some(v);
                        Ok(())
                    })?;
                    let v = result.expect("constructed");
                    self.failure.pending.retain(|&p| p != v);
                    return Ok(v);
                }
                let destination = self.destination_class(destination)?;
                // A distinct type over another nominal (`type R = A`) is laid
                // out as a one-field wrapper of it, while TIR builds it from the
                // inner type's own fields: build the inner value, then wrap it.
                if let tir::Constructor::Record(path) = constructor
                    && let Some(inner) = self.wrapped_nominal(path)
                    && let [field] = self.layouts.record(layout).fields.as_slice()
                    && !(values.len() == 1
                        && values[0].field == field.ordinal
                        && self.layout_of(self.value_type(values[0].value)) == field.layout)
                {
                    let inner_layout = field.layout;
                    let inner_ty = self.intern(inner_layout);
                    let symbol = self.symbol(SymbolKind::Type, &inner, None);
                    let v = self.construct(
                        ConstructorKind::Record(symbol),
                        inner_layout,
                        values,
                        destination.clone(),
                        inner_ty,
                    )?;
                    if self.holds(v) {
                        self.failure.pending.push(v);
                    }
                    let symbol = self.symbol(SymbolKind::Type, path, None);
                    return self.construct(
                        ConstructorKind::Record(symbol),
                        layout,
                        vec![FieldValue { field: 0, value: v }],
                        destination,
                        ty,
                    );
                }
                let constructor = match constructor {
                    tir::Constructor::Tuple => ConstructorKind::Tuple,
                    tir::Constructor::Record(path) => {
                        ConstructorKind::Record(self.symbol(SymbolKind::Type, path, None))
                    }
                    tir::Constructor::Variant {
                        definition,
                        variant,
                    } => ConstructorKind::Variant {
                        definition: self.symbol(SymbolKind::Type, definition, None),
                        variant: *variant,
                    },
                    tir::Constructor::Sequence => ConstructorKind::Sequence,
                    tir::Constructor::Map | tir::Constructor::Set => unreachable!("handled above"),
                    tir::Constructor::Box => ConstructorKind::Box,
                };
                self.construct(constructor, layout, values, destination, ty)?
            }
            V::Closure {
                body,
                captures,
                destination,
            } => {
                let data_environment = self
                    .instances
                    .data_environments
                    .get(&(self.id, *body))
                    .copied();
                let mut values = Vec::new();
                for c in captures {
                    if self.elided_place(c.place) {
                        continue;
                    }
                    let usage = match c.usage {
                        UseKind::Borrow => hirsel_lir::ir::CaptureUse::Borrow,
                        UseKind::Take => hirsel_lir::ir::CaptureUse::Take,
                        UseKind::Copy => hirsel_lir::ir::CaptureUse::Copy,
                        UseKind::Mut => return unsupported("a mut capture"),
                    };
                    let v = self.operand(c)?;
                    // A copied capture was copied at its read: the
                    // environment takes that owned copy.
                    let usage = if usage == hirsel_lir::ir::CaptureUse::Copy && self.holds(v) {
                        hirsel_lir::ir::CaptureUse::Take
                    } else {
                        usage
                    };
                    values.push(CaptureValue { value: v, usage });
                }
                if let Some(environment) = data_environment {
                    let fields = values
                        .iter()
                        .enumerate()
                        .map(|(field, c)| FieldValue {
                            field: field as u32,
                            value: c.value,
                        })
                        .collect();
                    return self.construct(
                        ConstructorKind::Tuple,
                        environment,
                        fields,
                        self.destination_class(destination)?,
                        ty,
                    );
                }
                let instance = self
                    .instances
                    .closure(self.id, *body)
                    .ok_or_else(|| InstanceError::MissingInstance(self.def.path.clone()))?;
                let (code, environment) = {
                    let digest = self.instances.view(instance).key.digest();
                    let symbol =
                        self.symbol(SymbolKind::Function, &self.def.path.clone(), Some(digest));
                    let code = ClosureCode {
                        symbol,
                        abi: self.instances.abis[instance.index()],
                    };
                    let environment = self.instances.environments[instance.index()]
                        .expect("closure instances have an environment");
                    (Some(code), environment)
                };
                let anchor = self.anchor_here();
                let captures =
                    PoolRange::push(&mut self.out.pools.captures, values.iter().copied());
                let closure = hirsel_lir::ir::ClosureId(self.out.tables.closures.len() as u32);
                self.out.tables.closures.push(ClosureSite {
                    code,
                    environment,
                    captures,
                    destination: self.destination_class(destination)?,
                    placement: None,
                    anchor,
                });
                for c in &values {
                    self.consumed(c.value);
                }
                let result = self.fresh(ty);
                let failure = self.fail(None)?;
                self.push(Inst::Closure {
                    result,
                    closure,
                    failure,
                });
                result
            }
            V::Shape(_) => {
                let anchor = self.anchor_here();
                self.push(Inst::Unavailable {
                    feature: hirsel_lir::ir::UnavailableFeature::Shape,
                    anchor,
                });
                return unsupported("Shape static data is not materialized");
            }
        })
    }

    /// `unique x`: a deep copy of the source's owned graph (plain content is
    /// a bit copy). The source is read as a view; one without a loan of its
    /// own is viewed through a synthetic loan around the copy.
    fn unique(&mut self, value: &tir::Operand, ty: LirTypeId) -> R<ValueId> {
        let p = self.place(value.place)?;
        let address = self.address(&p)?;
        if self.bits(p.ty) {
            return self.operand_at(UseKind::Copy, &p, address, None);
        }
        let reaching = self.reaching(p.slot)?;
        let tir_loan = p.loan;
        let (holding, synthetic) = match tir_loan {
            Some(l) if self.state.loans[self.loan(l)?.index()] == Tri::Yes => {
                (Holding::Borrowed(self.loan(l)?), None)
            }
            _ => {
                let loan = self.synthetic_loan(address);
                (Holding::Borrowed(loan), Some(loan))
            }
        };
        let view = self.value(p.ty, holding);
        self.push(Inst::Read {
            result: view,
            address,
            reaching,
            mode: ReadMode::Borrow,
        });
        let layout = match self.record(ty).kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => self.layout_of(ty),
        };
        let anchor = self.anchor_here();
        let construct = hirsel_lir::ir::ConstructId(self.out.tables.constructs.len() as u32);
        self.out.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            constructor: ConstructorKind::Tuple,
            layout,
            fields: PoolRange::EMPTY,
            destination: DestinationClass::Inferred,
            placement: None,
            anchor,
        });
        let result = self.fresh(ty);
        if let Some(loan) = synthetic {
            self.failure.before.insert(0, Action::EndLoan { loan });
        }
        let failure = self.fail(None);
        if synthetic.is_some() {
            self.failure.before.remove(0);
        }
        let failure = failure?;
        self.push(Inst::UniqueCopy {
            result,
            value: view,
            construct,
            deep: true,
            failure,
        });
        if let Some(loan) = synthetic {
            self.push(Inst::EndLoan {
                loan,
                if_active: false,
            });
            self.state.loans[loan.index()] = Tri::No;
        }
        if self.holds(result) {
            self.failure.pending.push(result);
        }
        Ok(result)
    }

    /// A construction's field values in source order. A field in `encoded`
    /// is a flat record's variable field: its source is only read, from a
    /// construction-owned temporary or under a loan the caller ends (`loans`,
    /// `taken`, for `reading`) once the construction is built.
    fn construct_fields(
        &mut self,
        encoded: &[u32],
        fields: &[tir::ConstructField],
    ) -> R<(Vec<FieldValue>, Vec<LoanId>, Vec<SlotId>)> {
        let mut values = Vec::new();
        let (mut loans, mut taken) = (Vec::new(), Vec::new());
        let before = (!encoded.is_empty()).then(|| self.failure.before.clone());
        for f in fields {
            let v = if encoded.contains(&f.field) {
                self.compared(&f.value, &mut loans, &mut taken)?
            } else {
                self.operand(&f.value)?
            };
            // Preparing a later operand can fail after earlier encoded
            // sources have entered construction-owned temporaries.
            if let Some(before) = &before {
                let mut prepared: Vec<Action> = loans
                    .iter()
                    .copied()
                    .map(|loan| Action::EndLoan { loan })
                    .collect();
                prepared.extend(taken.iter().rev().copied().map(|slot| Action::Release {
                    slot,
                    anchor: (INSTRUCTION_DROP, self.statement_action()),
                }));
                prepared.extend(before.iter().cloned());
                self.failure.before = prepared;
            }
            values.push(FieldValue {
                field: f.field,
                value: v,
            });
        }
        if let Some(before) = before {
            self.failure.before = before;
        }
        Ok((values, loans, taken))
    }

    /// Push a construction site and build its value; the fields are consumed
    /// even when the construction fails.
    fn construct(
        &mut self,
        constructor: ConstructorKind,
        layout: LayoutId,
        values: Vec<FieldValue>,
        destination: DestinationClass,
        ty: LirTypeId,
    ) -> R<ValueId> {
        // An empty sequence holds no element: it is the static empty
        // sequence of its layout, which the first reserve or growth replaces
        // (`hirsel_rt_v2_seq_grow`), and allocates nothing.
        if constructor == ConstructorKind::Sequence
            && values.is_empty()
            && matches!(
                destination,
                DestinationClass::Heap | DestinationClass::Inferred
            )
            && matches!(
                self.layouts.record(layout).kind,
                LayoutKind::Sequence { .. }
            )
            && matches!(
                self.layouts.record(self.layout_of(ty)).kind,
                LayoutKind::Block { payload } if payload == layout
            )
        {
            let instance = empty_sequence_instance(self.layouts.record(layout));
            let symbol = self.symbol(SymbolKind::Constant, &empty_sequence(), Some(instance));
            let result = self.fresh(ty);
            self.push(Inst::ReadStatic { result, symbol });
            return Ok(result);
        }
        let anchor = self.anchor_here();
        let fields = PoolRange::push(&mut self.out.pools.fields, values.iter().copied());
        let construct = hirsel_lir::ir::ConstructId(self.out.tables.constructs.len() as u32);
        self.out.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            constructor,
            layout,
            fields,
            destination,
            placement: None,
            anchor,
        });
        for f in &values {
            self.consumed(f.value);
        }
        let result = self.fresh(ty);
        let failure = self.fail(None)?;
        self.push(Inst::Construct {
            result,
            construct,
            failure,
        });
        Ok(result)
    }

    /// Container syntax is an empty private record, then ordinary insert
    /// calls. No backend container-constructor vocabulary survives import.
    fn keyed_construct(
        &mut self,
        is_map: bool,
        fields: &[tir::ConstructField],
        destination: DestinationClass,
        ty: LirTypeId,
    ) -> R<ValueId> {
        let layout = self.layout_of(ty);
        let payload = match self.layouts.record(layout).kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => return malformed("container block"),
        };
        let shape: Vec<_> = self
            .layouts
            .record(payload)
            .fields
            .iter()
            .map(|f| (f.ordinal, f.layout))
            .collect();
        let mut values = Vec::new();
        for (field, layout) in shape {
            let sequence = match self.layouts.record(layout).kind {
                LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
                _ => return malformed("container sequence"),
            };
            let sequence_ty = self.intern(layout);
            let value = self.construct(
                ConstructorKind::Sequence,
                sequence,
                vec![],
                destination.clone(),
                sequence_ty,
            )?;
            if self.holds(value) {
                self.failure.pending.push(value);
            }
            values.push(FieldValue { field, value });
        }
        let symbol = self.symbol(
            SymbolKind::Type,
            &crate::library::core_path(if is_map { "Map" } else { "Set" }),
            None,
        );
        let empty = self.construct(
            ConstructorKind::Record(symbol),
            payload,
            values,
            destination,
            ty,
        )?;
        if fields.is_empty() {
            return Ok(empty);
        }
        if self.holds(empty) {
            self.failure.pending.push(empty);
        }
        let scope = self.out.slots[self
            .slot_for(self.body.places[fields[0].value.place.0 as usize].root.0 as usize)?
            .index()]
        .scope;
        let slot = self.new_slot(ty, SlotKind::Temporary, scope);
        self.push(Inst::StorageLive { slot });
        self.state.live(slot);
        let place = self.whole(slot);
        let address = self.address(&place)?;
        self.store(&place, address, empty)?;
        let release = Action::Release {
            slot,
            anchor: (INSTRUCTION_DROP, self.statement_action()),
        };
        self.failure.before.insert(0, release.clone());
        let kind = if is_map {
            hirsel_types::BuiltinType::Map
        } else {
            hirsel_types::BuiltinType::Set
        };
        let candidates: Vec<_> = self
            .store
            .iter()
            .filter_map(|(id, t)| match &t.kind {
                hirsel_types::TypeKind::Builtin { kind: k, arguments } if *k == kind => {
                    Some((id, arguments.to_vec()))
                }
                _ => None,
            })
            .collect();
        let mut actuals = None;
        for (candidate, arguments) in candidates {
            if self.layout(candidate)? == layout {
                actuals = Some(arguments);
                break;
            }
        }
        let actuals = actuals
            .ok_or_else(|| ImportError::Unsupported("container constructor actuals".into()))?;
        let path = crate::library::core_path(if is_map { "map.insert" } else { "set.insert" });
        let body = crate::library::body_index(&path).expect("registered insertion");
        let request = crate::library::Request::operation(
            self.program,
            body,
            &path,
            self.store.clone(),
            actuals,
        )?;
        let direct = self.require(request)?;
        let symbol = self.symbol(
            direct.symbol.kind,
            &direct.symbol.definition,
            direct.symbol.instance,
        );
        for field in fields {
            let value = self.operand(&field.value)?;
            let mut args = Vec::new();
            let pair = if is_map {
                let pair_ty = self.value_type(value);
                let s = self.new_slot(pair_ty, SlotKind::Temporary, scope);
                self.push(Inst::StorageLive { slot: s });
                self.state.live(s);
                let whole = self.whole(s);
                let a = self.address(&whole)?;
                self.store(&whole, a, value)?;
                let release = Action::Release {
                    slot: s,
                    anchor: (INSTRUCTION_DROP, self.statement_action()),
                };
                self.failure.before.insert(0, release.clone());
                let record = self.record(pair_ty).clone();
                let mut key = whole.clone();
                key.steps.push(Step::Field(0));
                key.projection.push(ProjectionStep::Field(0));
                key.ty = self.intern(record.fields[0].layout);
                let key_address = self.address(&key)?;
                let loan = self.synthetic_loan(key_address);
                self.failure.before.insert(0, Action::EndLoan { loan });
                let holding = if self.owns(key.ty) {
                    Holding::Borrowed(loan)
                } else {
                    Holding::Plain
                };
                let k = self.value(key.ty, holding);
                let reaching = self.reaching(s)?;
                self.push(Inst::Read {
                    result: k,
                    address: key_address,
                    reaching,
                    mode: ReadMode::Borrow,
                });
                let mut item = whole;
                item.steps.push(Step::Field(1));
                item.projection.push(ProjectionStep::Field(1));
                item.ty = self.intern(record.fields[1].layout);
                let a = self.address(&item)?;
                let v = self.operand_at(UseKind::Take, &item, a, None)?;
                args.push(CallArgument {
                    parameter: 1,
                    passing: Passing::Borrow,
                    value: ArgumentValue::Value(k),
                });
                args.push(CallArgument {
                    parameter: 2,
                    passing: Passing::Take,
                    value: ArgumentValue::Value(v),
                });
                Some((s, loan, release))
            } else {
                args.push(CallArgument {
                    parameter: 1,
                    passing: Passing::Take,
                    value: ArgumentValue::Value(value),
                });
                None
            };
            let version = self.value(ty, Holding::Plain);
            self.versions[slot.index()] = Some(version);
            args.insert(
                0,
                CallArgument {
                    parameter: 0,
                    passing: Passing::Mut,
                    value: ArgumentValue::Address { address, version },
                },
            );
            Inline::direct_call(self, symbol, direct.abi, args)?;
            if let Some((s, loan, release)) = pair {
                self.failure.before.remove(0);
                self.push(Inst::EndLoan {
                    loan,
                    if_active: false,
                });
                self.state.loans[loan.index()] = Tri::No;
                self.failure.before.remove(0);
                self.run_normal(&[release], &[])?;
                self.push(Inst::StorageDead {
                    slot: s,
                    if_live: false,
                });
                self.state.dead(s);
            }
        }
        self.failure.before.remove(0);
        let result = self.operand_at(UseKind::Take, &place, address, None)?;
        self.push(Inst::StorageDead {
            slot,
            if_live: false,
        });
        self.state.dead(slot);
        self.failure.pending.retain(|&v| v != result);
        Ok(result)
    }

    /// The nominal a distinct type is declared over, if any.
    fn wrapped_nominal(&self, path: &DefPath) -> Option<DefPath> {
        let signature = self.program.signatures.get(path)?;
        let SignatureKind::Type(t) = &signature.kind else {
            return None;
        };
        let representation = t.representation?;
        match &signature.types.get(representation)?.kind {
            hirsel_types::TypeKind::Named { definition, .. } => Some(definition.clone()),
            _ => None,
        }
    }

    /// The symbol and ABI of a direct callee instance.
    fn direct(&mut self, path: &DefPath, arguments: &[GenericArgument]) -> R<(SymbolId, FnAbiId)> {
        if let Some(body) = crate::library::body_index(path) {
            let request = self.library_request(body, path, arguments)?;
            let direct = self.require(request)?;
            return Ok((
                self.symbol(SymbolKind::Function, path, direct.symbol.instance),
                direct.abi,
            ));
        }
        // An operation without a library body is named, as a value, by its
        // forwarding body (`library::value`).
        if self.program.operations.contains_key(path)
            && !self.program.definitions.contains_key(path)
        {
            if !crate::library::value::forwards(path) {
                return unsupported(format!(
                    "{path:?} as a function value: it is lowered inline at each call"
                ));
            }
            let callee = crate::instance::callee_actuals(
                self.program.binders.get(path).copied(),
                &self.def.types,
                arguments,
                self.actuals,
                false,
                path,
            )?;
            let request = crate::library::Request::operation_value(
                self.program,
                path,
                callee.store,
                callee.arguments,
            )?;
            let direct = self.require(request)?;
            return Ok((
                self.symbol(SymbolKind::Function, path, direct.symbol.instance),
                direct.abi,
            ));
        }
        let callee = crate::instance::callee_actuals(
            self.program.binders.get(path).copied(),
            &self.def.types,
            arguments,
            self.actuals,
            false,
            path,
        )?;
        let id = self.instances.find(self.program, path, &callee)?;
        let digest = self.instances.view(id).key.digest();
        Ok((
            self.symbol(SymbolKind::Function, path, Some(digest)),
            self.instances.abis[id.index()],
        ))
    }
}

impl Importer<'_, '_> {
    fn terminator(&mut self, t: tir::BlockId, term: &tir::Terminator) -> R<()> {
        use tir::Terminator as T;
        match term {
            T::Goto(next) => {
                let e = self.enter(*next);
                self.terminate(Terminator::Jump(e));
            }
            T::Branch { condition, yes, no } => {
                let c = self.operand(condition)?;
                let yes = self.enter(*yes);
                let no = self.enter(*no);
                self.terminate(Terminator::Branch {
                    condition: c,
                    yes,
                    no,
                });
            }
            T::Switch {
                value,
                arms,
                otherwise,
            } => {
                let v = self.operand(value)?;
                let arms: Vec<SwitchArm> = arms
                    .iter()
                    .map(|(value, b)| SwitchArm {
                        value: *value,
                        edge: self.enter(*b),
                    })
                    .collect();
                let otherwise = self.enter(*otherwise);
                let arms = PoolRange::push(&mut self.out.pools.switch_arms, arms);
                self.terminate(Terminator::Switch {
                    value: v,
                    arms,
                    otherwise,
                });
            }
            T::Unreachable => self.terminate(Terminator::Unreachable),
            T::Return { value, cleanup } => {
                // Extracting the result may fail (a copy): the return's cleanup runs.
                self.failure.cleanup = Some(*cleanup);
                let v = self.operand(value)?;
                // A many-call closure returned as a once-call one (MODE-3).
                let v = match self.own_abi().result.as_ref().map(|r| r.layout) {
                    Some(layout) => {
                        let ty = self.intern(layout);
                        self.weaken(v, ty)
                    }
                    None => v,
                };
                let actions = self.exit_actions(self.chain_actions(*cleanup)?);
                self.failure = FailureContext {
                    cleanup: None,
                    before: Vec::new(),
                    pending: if self.holds(v) { vec![v] } else { Vec::new() },
                };
                self.run_normal(&actions, &[])?;
                let abi = self.own_abi();
                let result = abi.result.is_some().then_some(v);
                self.terminate(Terminator::Return(result));
            }
            T::Fail { rule, cleanup } => {
                self.failure = FailureContext {
                    cleanup: Some(*cleanup),
                    before: Vec::new(),
                    pending: Vec::new(),
                };
                let failure = self.fail(Some(rule.to_string()))?;
                self.terminate(Terminator::Raise(failure));
            }
            T::Call {
                callee,
                arguments,
                returns,
                on_failure,
            } => {
                self.failure = FailureContext {
                    cleanup: Some(*on_failure),
                    before: Vec::new(),
                    pending: Vec::new(),
                };
                if self.builders.calls.contains(&t) {
                    return self.removed_call(*returns);
                }
                if self.builders.reserves.contains(&t) {
                    return self.builder_reserve(arguments, *returns);
                }
                if let Some(&write) = self.fields.writes.get(&t) {
                    return self.field_write(write, arguments, *returns);
                }
                if self.fields.freezes.contains(&t) {
                    return self.field_freeze(*returns);
                }
                if let Some(site) = self.plan.and_then(|p| p.site(t)).cloned() {
                    return self.back_edge(&site, arguments);
                }
                self.call_block = t;
                self.call(callee, arguments, *returns)?;
            }
            T::Match { table, from } => self.match_from(*table, *from)?,
        }
        Ok(())
    }

    fn own_abi(&self) -> hirsel_lir::abi_types::FnAbi {
        self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize].clone()
    }

    fn call(
        &mut self,
        callee: &tir::Callee,
        arguments: &[tir::CallArgument],
        returns: Option<tir::CallReturn>,
    ) -> R<()> {
        // `list.len` of a list inside a flat value reads its encoding's count
        // in place: a flat list has no ordinary List to lend (LIR schema
        // revision 4).
        if let tir::Callee::Definition { path, .. } = callee
            && let Some(tir::CallReturn { destination, next }) = returns
            && self.program.operations.contains_key(path)
            && core_operation(path, "list", "len")
            && let [argument] = arguments
            && argument.value.usage == UseKind::Borrow
        {
            let p = self.place(argument.value.place)?;
            if p.source.is_some() {
                return self.flat_length(&p, destination, next);
            }
        }
        // A family's inline lowering replaces the call by instructions.
        if let tir::Callee::Definition {
            path,
            arguments: generics,
        } = callee
            && let Some(tir::CallReturn { destination, next }) = returns
            && let Some(lowering) = crate::library::inline_lowering(path)
        {
            let shape = crate::library::CallShape {
                path,
                generics,
                arguments,
                destination,
                next,
            };
            if (lowering.lower)(&mut ImportCx { im: self }, &shape)? {
                return Ok(());
            }
        }
        let mut taken_callee: Option<(LoanId, PlaceRef)> = None;
        let mut capture_args = Vec::new();
        let mut capture_loans = Vec::new();
        let mut operation_arguments = Vec::new();
        let (lir_callee, abi) = match callee {
            tir::Callee::Definition { path, arguments } => {
                if let Some(body) = crate::library::body_index(path) {
                    // A library body: a direct call of its instance.
                    let request = self.library_request(body, path, arguments)?;
                    let direct = self.require(request)?;
                    let symbol = self.symbol(
                        SymbolKind::Function,
                        &direct.symbol.definition,
                        direct.symbol.instance,
                    );
                    (Callee::Direct(symbol), direct.abi)
                } else if let Some(op) = self.program.operations.get(path).cloned() {
                    let reference = self.operation(&op);
                    let (abi, actuals) = self.operation_abi(path, arguments)?;
                    operation_arguments = actuals;
                    (Callee::Operation(reference), abi)
                } else if self.program.definitions.contains_key(path) {
                    let (symbol, abi) = self.direct(path, arguments)?;
                    (Callee::Direct(symbol), abi)
                } else {
                    return unsupported(format!(
                        "a call of {path:?}, neither an operation nor a definition"
                    ));
                }
            }
            tir::Callee::Indirect {
                value,
                arguments: generics,
            } => {
                // Generic nested names call concrete code with only their
                // declaration's ordinary captured data (spec #42).
                let bound = if generics.is_empty() {
                    None
                } else {
                    let Some(&instance) =
                        self.instances.direct_calls.get(&(self.id, self.call_block))
                    else {
                        return Err(InstanceError::MissingInstance(self.def.path.clone()).into());
                    };
                    let digest = self.instances.view(instance).key.digest();
                    let code =
                        self.symbol(SymbolKind::Function, &self.def.path.clone(), Some(digest));
                    Some((code, self.instances.abis[instance.index()]))
                };
                let abi = match bound {
                    Some((_, abi)) => abi,
                    None => {
                        let ty = self.ty(self.body.places[value.place.0 as usize].ty)?;
                        self.layouts
                            .fn_abi(&self.store, ty, CallableShape::Closure)
                            .map_err(|e| ImportError::Layout(self.def.path.clone(), e))?
                    }
                };
                if let Some((code, _)) = bound {
                    if let Some(&instance) =
                        self.instances.direct_calls.get(&(self.id, self.call_block))
                        && let Some(captures) = self.instances.direct_captures.get(&instance)
                    {
                        let p = self.place(value.place)?;
                        let base = self.layouts.table().fn_abis[abi.0 as usize]
                            .parameters
                            .len()
                            - captures.len();
                        for (ordinal, capture) in captures.iter().enumerate() {
                            let mut field = p.clone();
                            field.steps.push(Step::Field(ordinal as u32));
                            field.projection.push(ProjectionStep::Field(ordinal as u32));
                            field.ty = self.intern(capture.layout);
                            let address = self.address(&field)?;
                            let loan = match p.loan {
                                Some(loan) => self.loan(loan)?,
                                None => {
                                    let loan = self.synthetic_loan(address);
                                    capture_loans.push(loan);
                                    loan
                                }
                            };
                            let holding = if self.owns(field.ty) {
                                Holding::Borrowed(loan)
                            } else {
                                Holding::Plain
                            };
                            let result = self.value(field.ty, holding);
                            let reaching = self.reaching(p.slot)?;
                            self.push(Inst::Read {
                                result,
                                address,
                                reaching,
                                mode: ReadMode::Borrow,
                            });
                            let argument = ArgumentValue::Value(result);
                            capture_args.push(CallArgument {
                                parameter: (base + ordinal) as u32,
                                passing: Passing::Borrow,
                                value: argument,
                            });
                        }
                    }
                    (Callee::Direct(code), abi)
                } else {
                    let mut p = self.place(value.place)?;
                    let v = if value.usage == UseKind::Take
                        && self.owns(p.ty)
                        && !self.borrowed_storage(p.slot)
                    {
                        // Extraction creates the invocation's owner after the
                        // argument temporaries. Borrow that owner for the call,
                        // then release it before the caller's cleanup. Keeping it
                        // in its earlier slot would reverse temporary drop order.
                        if !p.fixed {
                            return unsupported("a taken callee in a computed place");
                        }
                        let owned = self.operand(value)?;
                        // A diverging call's owner lives in its callee's scope.
                        let scope = match returns {
                            Some(r) => {
                                let root = self.body.places[r.destination.0 as usize].root;
                                self.out.slots[self.slot_for(root.0 as usize)?.index()].scope
                            }
                            None => self.out.slots[p.slot.index()].scope,
                        };
                        let slot = self.new_slot(p.ty, SlotKind::Temporary, scope);
                        self.push(Inst::StorageLive { slot });
                        self.state.live(slot);
                        p = self.whole(slot);
                        let address = self.address(&p)?;
                        self.store(&p, address, owned)?;
                        let loan = self.synthetic_loan(address);
                        let reaching = self.reaching(p.slot)?;
                        let v = self.value(p.ty, Holding::Borrowed(loan));
                        self.push(Inst::Read {
                            result: v,
                            address,
                            reaching,
                            mode: ReadMode::Borrow,
                        });
                        taken_callee = Some((loan, p));
                        v
                    } else {
                        self.operand(value)?
                    };
                    (Callee::Indirect(v), abi)
                }
            }
        };
        let release = taken_callee.as_ref().map(|(loan, p)| {
            [
                Action::EndLoan { loan: *loan },
                Action::Release {
                    slot: p.slot,
                    anchor: (INSTRUCTION_DROP, self.statement_action()),
                },
            ]
        });
        if let Some(release) = &release {
            self.failure.before.splice(0..0, release.iter().cloned());
        }
        let fn_abi = self.layouts.table().fn_abis[abi.0 as usize].clone();
        let mut args = capture_args;
        let mut writes = Vec::new();
        for a in arguments {
            let passing = fn_abi
                .parameters
                .iter()
                .find(|p| p.source == a.parameter.0)
                .map(|p| (p.convention, p.layout));
            let Some((passing, layout)) = passing else {
                return malformed(format!(
                    "argument for undeclared parameter {}",
                    a.parameter.0
                ));
            };
            let value = if a.value.usage == UseKind::Mut {
                let p = self.place(a.value.place)?;
                let address = self.address(&p)?;
                let ty = self.out.slots[p.slot.index()].ty;
                let version = self.value(ty, Holding::Plain);
                writes.push((p.slot, version));
                ArgumentValue::Address { address, version }
            } else {
                let value = self.operand(&a.value)?;
                let value = if passing == Passing::Take {
                    let ty = self.intern(layout);
                    self.weaken(value, ty)
                } else {
                    value
                };
                ArgumentValue::Value(value)
            };
            args.push(CallArgument {
                parameter: a.parameter.0,
                passing,
                value,
            });
        }
        let dest = returns
            .map(|r| {
                let dest = self.place(r.destination)?;
                let address = self.address(&dest)?;
                Ok::<_, ImportError>((dest, address))
            })
            .transpose()?;
        for a in &args {
            if let ArgumentValue::Value(v) = a.value {
                self.consumed(v);
            }
        }
        if let Callee::Indirect(v) = lir_callee {
            self.consumed(v);
        }
        for &(slot, version) in &writes {
            self.versions[slot.index()] = Some(version);
        }
        let result = fn_abi.result.as_ref().map(|r| {
            let ty = self.intern(r.layout);
            self.fresh(ty)
        });
        let capture_ends: Vec<_> = capture_loans
            .iter()
            .copied()
            .map(|loan| Action::EndLoan { loan })
            .collect();
        self.failure
            .before
            .splice(0..0, capture_ends.iter().cloned());
        let failure = self.fail(None)?;
        let anchor = self.anchor_here();
        let arguments = PoolRange::push(&mut self.out.pools.call_arguments, args);
        let call = CallId(self.out.tables.calls.len() as u32);
        self.out.tables.calls.push(CallSite {
            callee: lir_callee,
            operation_arguments,
            abi,
            arguments,
            result,
            destination: fn_abi.destination.then_some(DestinationClass::Inferred),
            declared: CALL_EFFECTS,
            anchor,
        });
        let (Some(tir::CallReturn { next, .. }), Some((dest, dest_address))) = (returns, dest)
        else {
            // A diverging call has no result and no normal edge (LIR schema
            // revision 19); its failure edge is the frame's only way on.
            self.terminate(Terminator::Call {
                call,
                normal: None,
                failure,
            });
            return Ok(());
        };
        let after = self.block(BlockKind::Body);
        let normal = self.edge(after, Vec::new());
        self.terminate(Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        });
        self.current = after;
        self.failure.before.drain(0..capture_ends.len());
        for loan in capture_loans {
            self.push(Inst::EndLoan {
                loan,
                if_active: false,
            });
            self.state.loans[loan.index()] = Tri::No;
        }

        let value = match result {
            Some(v) => v,
            None => {
                let constant = self.constant(dest.ty, ConstantValue::Unit);
                let v = self.fresh(dest.ty);
                self.push(Inst::Const {
                    result: v,
                    constant,
                });
                v
            }
        };
        if result.is_some() && self.holds(value) {
            self.failure.pending.push(value);
        }
        if let Some(release) = release {
            // Leave the taken callee's cleanup only for this drop's failure.
            self.failure.before.drain(0..2);
            for action in &release {
                match self.action(action)? {
                    Step2::Inline(inst) => self.push(inst),
                    Step2::Drop(drop) => {
                        drop.apply(self);
                        let failure = self.fail(None)?;
                        self.push(Inst::Drop {
                            address: drop.address,
                            if_live: drop.if_live,
                            anchor: drop.anchor,
                            failure,
                        });
                    }
                    Step2::Skip => {}
                }
            }
        }
        self.store(&dest, dest_address, value)?;
        let e = self.enter(next);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// The element count of a flat list part into `destination`.
    fn flat_length(
        &mut self,
        p: &PlaceRef,
        destination: tir::PlaceId,
        next: tir::BlockId,
    ) -> R<()> {
        if flat_sequence(self.layouts, self.layout_of(p.ty)).is_none() {
            return malformed("the length of a flat part that is no sequence");
        }
        let address = self.address(p)?;
        let dest = self.place(destination)?;
        let dest_address = self.address(&dest)?;
        let result = self.value(dest.ty, Holding::Plain);
        self.push(Inst::FlatLength { result, address });
        self.store(&dest, dest_address, result)?;
        let e = self.enter(next);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    fn operation_abi(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> R<(FnAbiId, Vec<hirsel_lir::operation::OperationArgument>)> {
        let mut callee = crate::instance::callee_actuals(
            self.program.binders.get(path).copied(),
            &self.def.types,
            arguments,
            self.actuals,
            false,
            path,
        )?;
        let signature = self
            .program
            .signatures
            .get(path)
            .ok_or_else(|| InstanceError::MissingSignature(path.clone()))?
            .clone();
        let SignatureKind::Function(function) = &signature.kind else {
            return malformed(format!("{path:?} is called but is not a function"));
        };
        let callable = crate::instance::instantiate_function(
            &signature,
            function,
            hirsel_types::Modes::global(),
            &mut callee,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let arguments =
            hirsel_lir::operation::OperationArgument::collect(&callee.store, &callee.arguments)
                .map_err(|e| ImportError::Layout(path.clone(), e))?;
        let abi = self
            .layouts
            .fn_abi(&callee.store, callable, CallableShape::Definition)
            .map_err(|e| ImportError::Layout(path.clone(), e))?;
        Ok((abi, arguments))
    }
}

impl Importer<'_, '_> {
    /// `Match from k`: arms k.. tested in order, each arm's failure continuing
    /// with the next; the remaining arms are exhaustive (TIR match coverage),
    /// so the last is entered without a test.
    fn match_from(&mut self, table: tir::MatchTableId, from: u32) -> R<()> {
        let m = &self.body.matches[table.0 as usize];
        let arms = &m.arms[from as usize..];
        let Some((last, tested)) = arms.split_last() else {
            return malformed("a match continuation with no arms");
        };
        for arm in tested {
            let next = self.block(BlockKind::Body);
            // `next` holds what held before the test, and when only the tag
            // test of a variant pattern can fail, that it holds another variant.
            let mut after = self.facts.clone();
            if let tir::MatchPatternKind::Variant {
                variant, fields, ..
            } = &m.patterns[arm.pattern.0 as usize].kind
                && fields.iter().all(|(_, sub)| irrefutable(m, *sub))
            {
                let p = self.place(m.patterns[arm.pattern.0 as usize].place)?;
                if let Some(version) = self.fact_version(&p)
                    && let Some(tag) = &self.record(p.ty).tag
                {
                    let others: Vec<u32> = tag
                        .variants
                        .iter()
                        .map(|v| v.ordinal)
                        .filter(|v| v != variant)
                        .collect();
                    narrow(&mut after, p.slot, &p.projection, version, &others);
                }
            }
            self.pattern(m, arm.pattern, next)?;
            let e = self.enter(arm.guard.unwrap_or(arm.target));
            self.terminate(Terminator::Jump(e));
            self.current = next;
            self.facts = after;
        }
        let e = self.enter(last.guard.unwrap_or(last.target));
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// Test one pattern; success continues in the current block, failure
    /// jumps to `fail`.
    fn pattern(&mut self, m: &tir::MatchTable, id: tir::MatchPatternId, fail: BlockId) -> R<()> {
        use tir::MatchPatternKind as P;
        let pattern = &m.patterns[id.0 as usize];
        match &pattern.kind {
            P::Wildcard | P::Binding { .. } | P::Literal(tir::Constant::Unit) => {}
            P::Literal(tir::Constant::Bool(b)) => {
                let v = self.pattern_bits(pattern.place)?;
                let cont = self.block(BlockKind::Body);
                let hit = self.edge(cont, Vec::new());
                let miss = self.edge(fail, Vec::new());
                let (yes, no) = if *b { (hit, miss) } else { (miss, hit) };
                self.terminate(Terminator::Branch {
                    condition: v,
                    yes,
                    no,
                });
                self.current = cont;
            }
            P::Literal(tir::Constant::Integer(n)) => {
                let v = self.pattern_bits(pattern.place)?;
                let ty = self.value_type(v);
                let constant = self.constant(ty, ConstantValue::Integer(*n));
                let k = self.fresh(ty);
                self.push(Inst::Const {
                    result: k,
                    constant,
                });
                let bty = self.bool_type()?;
                let c = self.value(bty, Holding::Plain);
                let failure = self.fail(None)?;
                self.push(Inst::Binary {
                    result: c,
                    op: BinaryOp::Eq,
                    left: v,
                    right: k,
                    failure,
                });
                let cont = self.block(BlockKind::Body);
                let yes = self.edge(cont, Vec::new());
                let no = self.edge(fail, Vec::new());
                self.terminate(Terminator::Branch {
                    condition: c,
                    yes,
                    no,
                });
                self.current = cont;
            }
            P::Literal(other) => {
                return unsupported(format!("a {other:?} literal pattern"));
            }
            P::Tuple(fields) | P::Record { fields, .. } => {
                for (_, sub) in fields {
                    self.pattern(m, *sub, fail)?;
                }
            }
            P::Variant {
                variant, fields, ..
            } => {
                self.tag(pattern.place, *variant, fail)?;
                for (_, sub) in fields {
                    self.pattern(m, *sub, fail)?;
                }
            }
        }
        Ok(())
    }

    fn unsigned(&self, ty: LirTypeId) -> bool {
        use hirsel_lir::layout_types::ScalarKind as K;
        matches!(
            self.out.types[ty.index()].repr,
            ValueRepr::Scalar(K::U8 | K::U16 | K::U32 | K::U64 | K::U128)
        )
    }

    /// The LIR type of `bool`, for comparison results.
    fn bool_type(&mut self) -> R<LirTypeId> {
        let ty = self.store.push(hirsel_types::Type::boolean());
        let layout = self.layout(ty)?;
        Ok(self.intern(layout))
    }

    /// A plain read of a pattern place.
    fn pattern_bits(&mut self, place: tir::PlaceId) -> R<ValueId> {
        let p = self.place(place)?;
        if !self.bits(p.ty) {
            return unsupported("a literal pattern on a counted place");
        }
        let address = self.address(&p)?;
        let reaching = self.reaching(p.slot)?;
        let result = self.value(p.ty, Holding::Plain);
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode: ReadMode::Bits,
        });
        Ok(result)
    }

    /// Switch on a variant's tag: every variant is an arm (`variant` continues,
    /// the others fail), so the otherwise edge is unreachable. A counted
    /// variant is read through a synthetic loan ended on every arm.
    fn tag(&mut self, place: tir::PlaceId, variant: u32, fail: BlockId) -> R<()> {
        let p = self.place(place)?;
        let address = self.address(&p)?;
        let reaching = self.reaching(p.slot)?;
        let variants: Vec<u32> = match &self.record(p.ty).tag {
            Some(tag) => tag.variants.iter().map(|v| v.ordinal).collect(),
            None => return malformed("a variant pattern on a layout without a tag"),
        };
        let loan = (!self.bits(p.ty)).then(|| self.synthetic_loan(address));
        let (mode, holding) = match loan {
            Some(l) => (ReadMode::Borrow, Holding::Borrowed(l)),
            None => (ReadMode::Bits, Holding::Plain),
        };
        let value = self.value(p.ty, holding);
        self.push(Inst::Read {
            result: value,
            address,
            reaching,
            mode,
        });
        let cont = self.block(BlockKind::Body);
        let switch = self.current;
        let mut arms = Vec::new();
        for v in variants {
            let target = if v == variant { cont } else { fail };
            let edge = match loan {
                None => self.edge(target, Vec::new()),
                Some(l) => {
                    let trampoline = self.block(BlockKind::Body);
                    let e = self.edge(trampoline, Vec::new());
                    self.current = trampoline;
                    self.push(Inst::EndLoan {
                        loan: l,
                        if_active: false,
                    });
                    let out = self.edge(target, Vec::new());
                    self.terminate(Terminator::Jump(out));
                    self.current = switch;
                    e
                }
            };
            arms.push(SwitchArm { value: v, edge });
        }
        let unreachable = match self.unreachable {
            Some(b) => b,
            None => {
                let b = self.block(BlockKind::Body);
                self.current = b;
                self.terminate(Terminator::Unreachable);
                self.current = switch;
                self.unreachable = Some(b);
                b
            }
        };
        let otherwise = self.edge(unreachable, Vec::new());
        let arms = PoolRange::push(&mut self.out.pools.switch_arms, arms);
        self.terminate(Terminator::Switch {
            value,
            arms,
            otherwise,
        });
        if let Some(l) = loan {
            self.state.loans[l.index()] = Tri::No;
        }
        self.current = cont;
        if self.fact_version(&p) == Some(reaching) {
            narrow(&mut self.facts, p.slot, &p.projection, reaching, &[variant]);
        }
        Ok(())
    }
}

impl Importer<'_, '_> {
    /// The loop prologue: every loop-carried parameter moves (a taken one) or
    /// is copied (a borrowed one: one count operation) into its loop slot.
    fn enter_loop(&mut self, parameter_slots: &[Option<SlotId>]) -> R<()> {
        let plan = self.plan.expect("a loop plan");
        let mut order: Vec<usize> = (0..plan.homes.len())
            .filter(|&k| plan.homes[k] == Home::Loop)
            .collect();
        // Owned values first, so a failing copy finds them in their slots.
        order.sort_by_key(|&k| parameter_slots[k].is_some());
        for k in order {
            let l = self.loop_slots[k].expect("loop slot");
            self.push(Inst::StorageLive { slot: l });
            self.state.live(l);
            let value = match parameter_slots[k] {
                None => self.out.parameters[k],
                Some(ps) => {
                    let p = self.whole(ps);
                    let address = self.fixed_address(&p)?;
                    self.operand_at(UseKind::Copy, &p, address, None)?
                }
            };
            let lp = self.whole(l);
            let address = self.fixed_address(&lp)?;
            self.store(&lp, address, value)?;
        }
        Ok(())
    }

    /// A verified self-tail site: the arguments become the loop slots' next
    /// contents and control returns to the header. Nothing of the frame is
    /// pending at a verified site, so every other slot ends here.
    fn back_edge(&mut self, site: &LoopSite, arguments: &[tir::CallArgument]) -> R<()> {
        let plan = self.plan.expect("a loop plan");
        let n = plan.homes.len();
        let mut values: Vec<Option<ValueId>> = vec![None; n];
        for a in arguments {
            let k = a.parameter.0 as usize;
            if plan.homes[k] == Home::Parameter || site.owned.iter().any(|(o, _)| *o as usize == k)
            {
                continue;
            }
            let v = match a.value.usage {
                UseKind::Take | UseKind::Copy => self.operand(&a.value)?,
                UseKind::Borrow => {
                    let p = self.place(a.value.place)?;
                    let address = self.address(&p)?;
                    self.operand_at(UseKind::Copy, &p, address, None)?
                }
                UseKind::Mut => return malformed("a mut argument at a verified self-tail site"),
            };
            values[k] = Some(v);
        }
        for l in 0..self.state.loans.len() {
            if let Step2::Inline(inst) = self.action(&Action::EndLoan {
                loan: LoanId(l as u32),
            })? {
                self.push(inst);
            }
        }
        for (k, place) in &site.owned {
            let p = self.place(*place)?;
            let address = self.address(&p)?;
            values[*k as usize] = Some(self.operand_at(UseKind::Take, &p, address, None)?);
        }
        for s in 0..self.out.slots.len() {
            let slot = SlotId(s as u32);
            let info = &self.out.slots[s];
            if !matches!(
                info.kind,
                SlotKind::Local | SlotKind::Temporary | SlotKind::Displaced
            ) {
                continue;
            }
            let st = &self.state.slots[s];
            if st.live == Tri::No {
                continue;
            }
            if self.owns(info.ty) && st.present().may() {
                return malformed(format!(
                    "slot {s} still owns content at the verified self-tail site in block {}",
                    site.call_block.0
                ));
            }
            let if_live = st.live == Tri::Maybe;
            self.push(Inst::StorageDead { slot, if_live });
            self.state.dead(slot);
            self.versions[s] = None;
        }
        for k in 0..n {
            if plan.homes[k] != Home::Loop {
                continue;
            }
            let l = self.loop_slots[k].expect("loop slot");
            let Some(v) = values[k] else {
                return malformed(format!("no argument for parameter {k}"));
            };
            if let Step2::Drop(drop) = self.action(&Action::Release {
                slot: l,
                anchor: (LOOP_RELEASE, k as u32),
            })? {
                drop.apply(self);
                let failure = self.fail(None)?;
                self.push(Inst::Drop {
                    address: drop.address,
                    if_live: drop.if_live,
                    anchor: drop.anchor,
                    failure,
                });
            }
            if self.state.slots[l.index()].present() != Tri::No {
                // Plain content: a store may not overwrite it, so the slot's
                // storage restarts.
                self.push(Inst::StorageDead {
                    slot: l,
                    if_live: false,
                });
                self.push(Inst::StorageLive { slot: l });
                self.state.live(l);
            }
            let lp = self.whole(l);
            let address = self.fixed_address(&lp)?;
            self.store(&lp, address, v)?;
        }
        let e = self.enter(self.body.entry);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// Lay the builder blocks out into the unit's arenas.
    fn finish(mut self) -> R<LirUnit<Value>> {
        let view = self.instances.view(self.id);
        let revision = self
            .program
            .revisions
            .get(&self.def.path)
            .copied()
            .ok_or_else(|| InstanceError::MissingRevision(self.def.path.clone()))?;
        let table = self.layouts.table();
        let mut unit = LirUnit::<Value>::new(UnitHeader {
            stage: StageKind::Value,
            lir_version: hirsel_lir::LIR_VERSION,
            origin: UnitOrigin::Source,
            input: self.program.input.clone(),
            definition: self.def.path.clone(),
            implementation_revision: revision,
            instance: view.key.digest(),
            target: table.target,
            layouts: table.digest(),
            abi: self.instances.abis[self.id.index()],
        });
        let out = std::mem::take(&mut self.out);
        let mut pools = out.pools;
        let mut tables = out.tables;
        let mut values = out.values;
        let mut incoming: BTreeMap<ValueId, Vec<ValueId>> = BTreeMap::new();
        for edge in &out.edges {
            let target = &out.blocks[edge.target.index()];
            let mut args = edge.args.clone();
            for &(slot, _) in &target.phis {
                match edge.versions.get(slot.index()).copied().flatten() {
                    Some(v) => args.push(v),
                    None => {
                        return malformed(format!(
                            "an edge into block {} carries no version of slot {}",
                            edge.target.0, slot.0
                        ));
                    }
                }
            }
            for &(l, p) in &target.view_phis {
                match edge.views.get(l).copied().flatten() {
                    Some(v) => {
                        args.push(v);
                        incoming.entry(p).or_default().push(v);
                    }
                    None => {
                        return malformed(format!(
                            "an edge into block {} carries no view of local {l}",
                            edge.target.0
                        ));
                    }
                }
            }
            tables.edges.push(Edge {
                target: edge.target,
                arguments: PoolRange::push(&mut pools.values, args),
            });
        }
        for (i, block) in out.blocks.into_iter().enumerate() {
            let params = block
                .params
                .iter()
                .copied()
                .chain(block.phis.iter().map(|(_, v)| *v))
                .chain(block.view_phis.iter().map(|(_, v)| *v));
            let parameters = PoolRange::push(&mut pools.values, params);
            let start = unit.instructions.len() as u32;
            unit.instructions.extend(block.insts);
            let Some(term) = block.term else {
                return malformed(format!("block {i} has no terminator"));
            };
            unit.blocks.push(Block {
                kind: block.kind,
                parameters,
                instructions: PoolRange::new(start, unit.instructions.len() as u32 - start),
                anchor: block.anchor,
            });
            unit.terminators.push(term);
        }
        join_view_holdings(&mut values, &incoming);
        unit.types = out.types;
        unit.values = values;
        unit.slots = out.slots;
        unit.addresses = out.addresses;
        unit.entry = self.prologue;
        unit.parameters = out.parameters;
        unit.captures = self.capture_versions.clone();
        let outer = self
            .outer_scope
            .then_some(ScopeId(self.body.scopes.len() as u32));
        unit.scopes = self
            .body
            .scopes
            .iter()
            .map(|s| ScopeInfo {
                parent: s.parent.map(|p| ScopeId(p.0)).or(outer),
            })
            .collect();
        if outer.is_some() {
            unit.scopes.push(ScopeInfo { parent: None });
        }
        unit.tables = tables;
        unit.pools = pools;
        Ok(unit)
    }
}
