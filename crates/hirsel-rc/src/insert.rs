//! Perceus insertion: Placed LIR to Rc LIR.
//!
//! Ownership obligations are linear holder credits over the SSA graph and the
//! slots that hold storage. SSA import already moves every owned value
//! linearly and gives each slot's content one semantic `Drop` at its TIR
//! cleanup anchor, so insertion decides where each slot's credit ends:
//!
//! - **Release at last use.** A slot whose content has no user-visible
//!   destruction is released right after the last use of that content on
//!   every path, normal and failure: memory of a value without user-visible
//!   destruction may be released at its last use. A use is
//!   anything that reads or writes the content or ends a loan on it, so a
//!   holder never ends before the TIR `EndLoan` of a loan it carries, and a
//!   value that only a failure cleanup still needs stays live up to the
//!   failing operation. The release is `read take` plus `Release` when the
//!   content is whole and definitely initialized, else a `drop?` of the slot;
//!   the semantic drops it supersedes are deleted or made conditional.
//!   Content with user-visible destruction keeps its drop at its anchor, and
//!   so does content that reaches a `Shared` root (S-14: every engine then
//!   has the same holders of each root at every operation; `Lay::anchored`).
//!   Borrowed, `mut` and capture slots are never released here.
//! - **Copies.** A semantic `Copy` is the Dup of a non-final owning consumer;
//!   `elide` turns a final one into a move.
//!
//! Liveness, initialization and loans are this module's own analyses; the
//! stage validator replays ownership independently and never reads them.
use crate::PassError;
use crate::import::{INSTRUCTION_DROP, STATE_RELEASE};
use crate::paths::{PathId, Paths, Tris};
use hirsel_core_contract::Passing;
use hirsel_lir::ir::{
    AddressBase, AddressId, AddressInfo, ArgumentValue, Block, BlockId, BlockKind, CallId,
    CaptureUse, CleanupAnchor, CleanupAnchorId, Edge, EdgeId, FailureEdge, FailureId, FlatFieldOp,
    Holding, Inst, LirTypeId, LirUnit, LoanId, MaskRef, PoolRange, ProjectionStep, ReadMode,
    RegionInstanceId, ScopeId, SlotId, SlotKind, SourceAnchorId, Terminator, ValueId, ValueInfo,
    ValueRepr,
};
use hirsel_lir::layout_types::{
    DropStep, LayoutId, LayoutKind, LayoutRecord, LayoutTable, SizeSpec,
};
use hirsel_lir::persistent::{Element, PVec};
use hirsel_lir::stage::{Placed, Rc, Stage};
use std::cell::{Ref, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

/// Lower a Placed unit to Rc: release every non-destructible slot content at
/// its last use. The result still has to pass the Rc validator.
pub fn insert(
    unit: LirUnit<Placed>,
    table: &LayoutTable,
    proofs: &Proofs,
) -> Result<LirUnit<Rc>, PassError> {
    let unit = lower_value_copies(unit.into_stage::<Rc>());
    let unit = copy_on_write(unit, table, proofs)?;
    release_at_last_use(unit, table)
}

/// What the front end proved about a unit's storage, read from its callable
/// type: writes it proved need no detach.
#[derive(Clone, Debug, Default)]
pub struct Proofs {
    /// MEM-5: an accepted `noalloc` body performs no detach not proven
    /// unnecessary, so none of its writes needs one.
    pub noalloc: bool,
    /// Source ordinals of deep-unique (`unique`) taken parameters (MODE-2):
    /// writes rooted in them never meet a shared block.
    pub unique: Vec<u32>,
}

impl Proofs {
    pub fn for_instance(
        instances: &crate::instance::Instances,
        id: crate::instance::InstanceId,
    ) -> Self {
        let store = &*instances.types;
        let f = instances.callable(id);
        Self {
            noalloc: f.allocation == hirsel_types::Allocation::NoAlloc,
            unique: f
                .parameters
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    // An arena block is never written in place: its header is
                    // immortal, so the write detaches it. A relation-free `mut`
                    // parameter may be lent an arena-placed argument (PROV-4);
                    // only a taken one is on the heap.
                    let arena = hirsel_syntax::DefPath::core("Arena");
                    p.passing != hirsel_syntax::PassingMode::Mut
                        && store.get(p.ty).is_some_and(|t| {
                            t.modes.uniqueness == hirsel_types::Uniqueness::Unique
                                && t.modes
                                    .relation
                                    .arenas(|o| {
                                        hirsel_types::arena_parameter(store, f, o, 0, &arena)
                                    })
                                    .is_empty()
                        })
                })
                .map(|(i, _)| i as u32)
                .collect(),
        }
    }
}

/// `insert` without making written blocks unique: the control for the
/// copy-on-write tests, never a pipeline stage.
pub fn insert_without_copy_on_write(
    unit: LirUnit<Placed>,
    table: &LayoutTable,
) -> Result<LirUnit<Rc>, PassError> {
    release_at_last_use(lower_value_copies(unit.into_stage::<Rc>()), table)
}

/// SSA reference copies need no temporary owner or address. The Dup supplies
/// the credit that the independently checked Recast transfers to the result.
fn lower_value_copies(unit: LirUnit<Rc>) -> LirUnit<Rc> {
    let mut editor = Editor::new(unit);
    match rewrite_value_copies(&mut editor, |_, _, _, _| {
        Ok::<_, std::convert::Infallible>(())
    }) {
        Ok(()) => editor.finish(),
        Err(never) => match never {},
    }
}

fn rewrite_value_copies<E>(
    editor: &mut Editor<Rc>,
    mut record: impl FnMut(usize, usize, usize, bool) -> Result<(), E>,
) -> Result<(), E> {
    for b in 0..editor.code.len() {
        let mut lowered = Vec::with_capacity(editor.code[b].len());
        for (index, instruction) in std::mem::take(&mut editor.code[b]).into_iter().enumerate() {
            let at = lowered.len();
            let changed = matches!(instruction, Inst::CopyValue { .. });
            record(b, index, at, changed)?;
            match instruction {
                Inst::CopyValue { result, value } => {
                    let counted = editor.unit.types[editor.unit.values[value.index()].ty.index()]
                        .repr
                        == ValueRepr::Block;
                    if counted {
                        lowered.push(Inst::Dup { value });
                    }
                    lowered.push(Inst::Recast { result, value });
                }
                other => lowered.push(other),
            }
        }
        editor.code[b] = lowered;
    }
    Ok(())
}

/// First RC subpass, with original instruction origins. An observed semantic
/// copy cannot become a transport silently: that expansion needs its own rule.
pub fn lower_value_copies_observed(
    unit: LirUnit<Placed>,
    input: &hirsel_lir::ownership_trace::Plan,
) -> Result<
    (
        LirUnit<Rc>,
        hirsel_lir::ownership_trace::Plan,
        hirsel_lir::ownership_trace::Recipe,
    ),
    String,
> {
    use hirsel_lir::ownership_trace::Point;
    hirsel_lir::ownership_trace::check(&unit, input).map_err(|error| error.to_string())?;
    let before = unit.clone();
    let mut editor = Editor::new(unit.into_stage::<Rc>());
    let mut points: Vec<BTreeMap<u32, u32>> = vec![BTreeMap::new(); editor.code.len()];
    rewrite_value_copies(&mut editor, |b, index, at, changed| {
        if changed
            && let Some(observation) = input.observations.iter().find(|observation| {
                matches!(observation.point, Point::Instruction { block, index: old, .. }
                if block == BlockId(b as u32) && old == index as u32)
            })
        {
            return Err(format!(
                "ownership trace: observed SSA copy {:?} needs an explicit semantic transport",
                observation.marker.id
            ));
        }
        if !changed {
            points[b].insert(at as u32, index as u32);
        }
        Ok(())
    })?;
    let origins: Vec<Vec<Option<u32>>> = editor
        .code
        .iter()
        .enumerate()
        .map(|(b, code)| {
            (0..code.len())
                .map(|index| points[b].get(&(index as u32)).copied())
                .collect()
        })
        .collect();
    crate::observation::finish(&before, input, editor, &origins)
}

/// Each RC subpass supplies and checks a recipe, including unchanged subpasses.
/// Detach rewrites retain coordinates. Releases report their instruction edits
/// and editor compaction; unsupported transports refuse the original plan.
pub fn insert_observed(
    unit: LirUnit<Placed>,
    table: &LayoutTable,
    proofs: &Proofs,
    input: &hirsel_lir::ownership_trace::Plan,
) -> Result<
    (
        LirUnit<Rc>,
        hirsel_lir::ownership_trace::Plan,
        Vec<hirsel_lir::ownership_trace::Recipe>,
    ),
    String,
> {
    let (unit, plan, copies) = lower_value_copies_observed(unit, input)?;
    let before = unit.clone();
    let unit = copy_on_write(unit, table, proofs).map_err(|error| error.to_string())?;
    let (unit, plan, detach) = crate::observation::identity(&before, &plan, unit)?;
    let before = unit.clone();
    let (editor, origins) = release_at_last_use_with::<true>(unit, table, Some(&plan))
        .map_err(|error| error.to_string())?;
    for block in &origins.cloned {
        if plan.observations.iter().any(|row| {
            use hirsel_lir::ownership_trace::Point;
            matches!(row.point,
                Point::Instruction { block: b, .. } | Point::ReleaseDrop { block: b, .. }
                | Point::CallEntered(b) | Point::CallReturned(b) | Point::Return(b)
                if b == *block)
        }) {
            return Err(format!(
                "ownership trace: cloned observed cleanup {block:?} needs a transport recipe"
            ));
        }
    }
    let whole_slot = |mut address: AddressId| -> Option<SlotId> {
        for _ in 0..editor.unit.addresses.len() {
            let info = editor.unit.addresses.get(address.index())?;
            if info.projection.len != 0 {
                return None;
            }
            match info.base {
                AddressBase::Slot(slot) => return Some(slot),
                AddressBase::Address(parent) if parent.0 < address.0 => address = parent,
                _ => return None,
            }
        }
        None
    };
    let mut source_drops = BTreeMap::new();
    for row in &plan.observations {
        if row.marker.kind == hirsel_lir::ownership_trace::Kind::Drop
            && let hirsel_lir::ownership_trace::Location::Address(address) = row.marker.token
            && let Some(slot) = whole_slot(address)
        {
            source_drops.entry(slot).or_insert(row.marker.id);
        }
    }
    let mut generated: Vec<_> = origins
        .generated
        .iter()
        .filter_map(|(&(block, address), &(trigger, anchor, if_live))| {
            let marker = *source_drops.get(&whole_slot(address)?)?;
            Some(hirsel_lir::ownership_trace::CleanupObservation {
                marker,
                point: hirsel_lir::ownership_trace::Point::Instruction {
                    block,
                    index: 0,
                    moment: if if_live {
                        hirsel_lir::ownership_trace::Moment::IfLive
                    } else {
                        hirsel_lir::ownership_trace::Moment::Before
                    },
                },
                token: hirsel_lir::ownership_trace::Location::Address(address),
                site: editor.unit.tables.cleanup_anchors[anchor.index()].site,
                trigger: hirsel_lir::ownership_trace::CleanupOrigin {
                    unit: plan.unit,
                    point: trigger,
                },
            })
        })
        .collect();
    for &(block, index, address, anchor, moment) in &origins.retirements {
        let Some(slot) = whole_slot(address) else {
            continue;
        };
        let source = plan.observations.iter().find(|row| {
            if row.marker.kind != hirsel_lir::ownership_trace::Kind::Drop
                || !matches!(row.marker.token,
                        hirsel_lir::ownership_trace::Location::Address(at)
                        if whole_slot(at) == Some(slot))
            {
                return false;
            }
            let hirsel_lir::ownership_trace::Point::Instruction { block, index, .. } = row.point
            else {
                return false;
            };
            matches!(before.blocks[block.index()].instructions.get(&before.instructions)
                    .and_then(|code| code.get(index as usize)),
                    Some(Inst::Drop { anchor: old, .. }) if *old == anchor)
        });
        let Some(source) = source else { continue };
        let point = moment.map_or(
            hirsel_lir::ownership_trace::Point::ReleaseDrop { block, index },
            |moment| hirsel_lir::ownership_trace::Point::Instruction {
                block,
                index,
                moment,
            },
        );
        generated.push(hirsel_lir::ownership_trace::CleanupObservation {
            marker: source.marker.id,
            point,
            token: hirsel_lir::ownership_trace::Location::Address(address),
            site: editor.unit.tables.cleanup_anchors[anchor.index()].site,
            trigger: hirsel_lir::ownership_trace::CleanupOrigin {
                unit: plan.unit,
                point,
            },
        });
    }
    let (unit, plan, releases) = crate::observation::finish_moved(
        &before,
        &plan,
        editor,
        &origins.rows,
        generated,
        Some(&origins.moves),
    )?;
    Ok((unit, plan, vec![copies, detach, releases]))
}

// ---------------------------------------------------------------------------
// Layout queries

/// Read-only questions about the layout table. A layout id outside the table
/// answers conservatively.
#[derive(Clone, Copy)]
pub(crate) struct Lay<'a>(pub &'a LayoutTable);

impl<'a> Lay<'a> {
    pub fn get(&self, id: LayoutId) -> Option<&'a LayoutRecord> {
        self.0.layout(id)
    }
    pub fn payload(&self, id: LayoutId) -> Option<LayoutId> {
        match self.get(id)?.kind {
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => Some(payload),
            _ => None,
        }
    }
    /// No destruction work at all.
    pub fn trivial(&self, id: LayoutId) -> bool {
        self.get(id)
            .is_some_and(|l| l.drop.is_empty() && l.finalizer.is_none())
    }
    /// Finalizers reached in this value's inline storage need explicit
    /// cleanup phases. A counted reference's payload is destroyed separately;
    /// borrowed fields never destroy their referents.
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
            _ => layout
                .fields
                .iter()
                .chain(
                    layout
                        .tag
                        .iter()
                        .flat_map(|tag| &tag.variants)
                        .flat_map(|v| &v.fields),
                )
                .filter(|field| !hirsel_lir::layout::field_is_borrowed(self.0, layout, field))
                .any(|field| self.inline_finalizer(field.layout)),
        }
    }
    /// Whether destroying a value of this layout is user-visible: its drop
    /// program at any depth runs a finalizer or closes a handle.
    pub fn destructible(&self, id: LayoutId) -> bool {
        self.destructible_in(id, &mut BTreeSet::new())
    }
    fn destructible_in(&self, id: LayoutId, seen: &mut BTreeSet<LayoutId>) -> bool {
        if !seen.insert(id) {
            return false;
        }
        let Some(layout) = self.get(id) else {
            return true;
        };
        if layout.finalizer.is_some() {
            return true;
        }
        match layout.kind {
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => {
                return self.destructible_in(payload, seen);
            }
            LayoutKind::Callable {
                environment: Some(environment),
            } => {
                return self.destructible_in(environment, seen);
            }
            // An erased environment's pointer is storage, not its payload
            // type. It may own resources, so keep its scope-ordered drop; a
            // static callable's (an Entry's) environment is never counted.
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
        seen: &mut BTreeSet<LayoutId>,
    ) -> bool {
        steps.iter().any(|step| match step {
            DropStep::Finalize | DropStep::CloseHandle { .. } => true,
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
            DropStep::Environment { offset } => {
                // Aggregate drop programs lift a child's environment step to
                // this offset. Follow the containing value's layout, including
                // callable words beyond the field's starting offset.
                let variants = layout.tag.iter().flat_map(|t| &t.variants);
                let mut fields = layout
                    .fields
                    .iter()
                    .chain(variants.flat_map(|v| &v.fields))
                    .filter(|f| {
                        f.offset <= *offset
                            && self.get(f.layout).is_some_and(|child| {
                                matches!(child.size, hirsel_lir::layout_types::SizeSpec::Fixed(n)
                                    if *offset - f.offset < n)
                            })
                    })
                    .peekable();
                fields.peek().is_none() || fields.any(|f| self.destructible_in(f.layout, seen))
            }
            DropStep::Release { offset } | DropStep::ReleaseRoot { offset } => self
                .0
                .field_at(layout, *offset)
                .is_none_or(|c| self.destructible_in(c, seen)),
        })
    }
    /// Whether a value of this layout keeps its semantic drop at its TIR
    /// cleanup anchor (S-14): its destruction is user-visible or releases a
    /// `Shared` root. Last-use release, copy-to-move elision, inline and
    /// region placement and reuse donation all leave such values alone.
    pub fn anchored(&self, id: LayoutId) -> bool {
        self.destructible(id) || self.root_bearing(id)
    }
    /// Whether this layout's drop program reaches a `Shared` root at any
    /// depth. An erased environment or iterator counts, as it may hold one.
    pub fn root_bearing(&self, id: LayoutId) -> bool {
        self.root_bearing_in(id, &mut BTreeSet::new())
    }
    fn root_bearing_in(&self, id: LayoutId, seen: &mut BTreeSet<LayoutId>) -> bool {
        if !seen.insert(id) {
            return false;
        }
        let Some(layout) = self.get(id) else {
            return true;
        };
        match layout.kind {
            LayoutKind::Shared { .. } => return true,
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                return self.root_bearing_in(payload, seen);
            }
            LayoutKind::Callable {
                environment: Some(environment),
            } => return self.root_bearing_in(environment, seen),
            LayoutKind::Callable { environment: None } => return !layout.drop.is_empty(),
            LayoutKind::Iterator { .. } => return true,
            _ => {}
        }
        self.steps_root_bearing(layout, &layout.drop, seen)
    }
    fn steps_root_bearing(
        &self,
        layout: &LayoutRecord,
        steps: &[DropStep],
        seen: &mut BTreeSet<LayoutId>,
    ) -> bool {
        steps.iter().any(|step| match step {
            DropStep::Finalize | DropStep::CloseHandle { .. } | DropStep::ReleaseReceipt { .. } => {
                false
            }
            DropStep::Thaw { .. }
            | DropStep::ReleaseRoot { .. }
            | DropStep::ReleaseSealed { .. } => true,
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
                let variants = layout.tag.iter().flat_map(|t| &t.variants);
                let mut fields = layout
                    .fields
                    .iter()
                    .chain(variants.flat_map(|v| &v.fields))
                    .filter(|f| {
                        f.offset <= *offset
                            && self.get(f.layout).is_some_and(|child| {
                                matches!(child.size, SizeSpec::Fixed(n) if *offset - f.offset < n)
                            })
                    })
                    .peekable();
                fields.peek().is_none() || fields.any(|f| self.root_bearing_in(f.layout, seen))
            }
        })
    }
}

/// Whether a value of this type carries a holder credit (the validator's
/// rule): block references, tokens and region marks always;
/// aggregates, callables and handles when their layout releases something.
pub(crate) fn owns_type<S>(unit: &LirUnit<S>, lay: Lay<'_>, ty: LirTypeId) -> bool {
    let ty = &unit.types[ty.index()];
    match ty.repr {
        ValueRepr::PayloadReceipt => true,
        ValueRepr::Block | ValueRepr::Token | ValueRepr::RegionMark | ValueRepr::Hole => true,
        ValueRepr::Unit
        | ValueRepr::Scalar(_)
        | ValueRepr::InitMask { .. }
        | ValueRepr::Uncounted
        | ValueRepr::Arena => false,
        ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle => !lay.trivial(ty.layout),
    }
}

pub(crate) fn owns<S>(unit: &LirUnit<S>, lay: Lay<'_>, value: ValueId) -> bool {
    owns_type(unit, lay, unit.values[value.index()].ty)
}

pub(crate) fn value_layout<S>(unit: &LirUnit<S>, value: ValueId) -> LayoutId {
    unit.types[unit.values[value.index()].ty.index()].layout
}

/// Whether destroying this value's content is user-visible.
pub(crate) fn destructible_type<S>(unit: &LirUnit<S>, lay: Lay<'_>, ty: LirTypeId) -> bool {
    lay.destructible(unit.types[ty.index()].layout)
}

/// Whether this value's content keeps its drop at its TIR cleanup anchor
/// ([`Lay::anchored`]).
pub(crate) fn anchored_type<S>(unit: &LirUnit<S>, lay: Lay<'_>, ty: LirTypeId) -> bool {
    lay.anchored(unit.types[ty.index()].layout)
}

// ---------------------------------------------------------------------------
// Places and static facts

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Step {
    Payload,
    Field(u32),
    Variant(u32, u32),
    Index,
    /// A statically bounded position in fixed inline storage.
    Inline(u64),
    /// An element slot named without a count check (LIR schema revision 7):
    /// no initialization state is kept below it, and it overlaps every
    /// element step.
    Slot,
}

/// Field paths in drop-flag metadata use the same statically bounded inline positions.
pub(crate) fn flag_steps<S>(
    unit: &LirUnit<S>,
    code: &[Vec<Inst>],
    range: PoolRange<ProjectionStep>,
) -> Option<Vec<Step>> {
    range
        .get(&unit.pools.projections)?
        .iter()
        .map(|p| match *p {
            ProjectionStep::Field(f) => Some(Step::Field(f)),
            ProjectionStep::VariantField { variant, field } => Some(Step::Variant(variant, field)),
            ProjectionStep::Index(v) => {
                let constant = code.iter().flatten().find_map(|i| match *i {
                    Inst::Const { result, constant } if result == v => Some(constant),
                    _ => None,
                })?;
                match unit.tables.constants[constant.index()].value {
                    hirsel_lir::ir::ConstantValue::Integer(n) => {
                        u64::try_from(n).ok().map(Step::Inline)
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

impl Step {
    fn element(self) -> bool {
        matches!(self, Step::Index | Step::Inline(_) | Step::Slot)
    }
}

/// Whether the place lies in an element slot named without a count check.
pub(crate) fn through_slot(path: &[Step]) -> bool {
    path.contains(&Step::Slot)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Root {
    Slot(SlotId),
    Value(ValueId),
}

/// A flattened address: root, field path and the path lengths after which
/// the walk dereferences a block reference.
#[derive(Clone, Debug)]
pub struct Place {
    pub root: Root,
    pub path: Vec<Step>,
    pub crossings: Vec<usize>,
}

/// Whether two field paths may name overlapping storage (all elements of a
/// sequence are one place).
pub(crate) fn overlap(a: &[Step], b: &[Step]) -> bool {
    a.iter().zip(b).all(|(x, y)| {
        x == y
            || (x.element() && y.element() && !matches!((x, y), (Step::Inline(_), Step::Inline(_))))
    })
}

fn place_of<S>(unit: &LirUnit<S>, lay: Lay<'_>, address: AddressId) -> Option<Place> {
    let mut chain = vec![address];
    while let AddressBase::Address(base) = unit.addresses[chain.last()?.index()].base {
        if chain.contains(&base) {
            return None;
        }
        chain.push(base);
    }
    let mut place: Option<(Place, LayoutId)> = None;
    for &id in chain.iter().rev() {
        let info = &unit.addresses[id.index()];
        let (mut p, mut layout) = match (info.base, place.take()) {
            (AddressBase::Slot(slot), _) => (
                Place {
                    root: Root::Slot(slot),
                    path: Vec::new(),
                    crossings: Vec::new(),
                },
                unit.types[unit.slots[slot.index()].ty.index()].layout,
            ),
            (AddressBase::Value(value), _) => (
                Place {
                    root: Root::Value(value),
                    path: Vec::new(),
                    crossings: if lay.payload(value_layout(unit, value)).is_some() {
                        vec![0]
                    } else {
                        Vec::new()
                    },
                },
                lay.payload(value_layout(unit, value))
                    .unwrap_or_else(|| value_layout(unit, value)),
            ),
            (AddressBase::Address(_), base) => base?,
        };
        for step in info.projection.get(&unit.pools.projections)? {
            if *step == ProjectionStep::Payload {
                let LayoutKind::Box { payload } = lay.get(layout)?.kind else {
                    return None;
                };
                p.crossings.push(p.path.len());
                p.path.push(Step::Payload);
                layout = payload;
                continue;
            }
            if let Some(payload) = lay.payload(layout) {
                p.crossings.push(p.path.len());
                if matches!(lay.get(layout)?.kind, LayoutKind::Box { .. }) {
                    p.path.push(Step::Payload);
                }
                layout = payload;
            }
            let record = lay.get(layout)?;
            let (next, step) = match *step {
                ProjectionStep::Payload => unreachable!("handled before implicit dereference"),
                ProjectionStep::Field(f) => (
                    record.fields.iter().find(|x| x.ordinal == f)?.layout,
                    Step::Field(f),
                ),
                ProjectionStep::VariantField { variant, field } => (
                    record
                        .tag
                        .iter()
                        .flat_map(|t| &t.variants)
                        .find(|v| v.ordinal == variant)?
                        .fields
                        .iter()
                        .find(|x| x.ordinal == field)?
                        .layout,
                    Step::Variant(variant, field),
                ),
                ProjectionStep::Index(index) => (
                    match record.kind {
                        LayoutKind::Flat => record.fields.first()?.layout,
                        LayoutKind::Inline { element, .. }
                        | LayoutKind::Simd { element, .. }
                        | LayoutKind::Slice { element }
                        | LayoutKind::SoaColumns { element }
                        | LayoutKind::Sequence { element }
                        | LayoutKind::Sequential { element } => element,
                        _ => return None,
                    },
                    match record.kind {
                        LayoutKind::Inline { count, .. } => unit
                            .instructions
                            .iter()
                            .find_map(|inst| match *inst {
                                Inst::Const { result, constant } if result == index => {
                                    match unit.tables.constants[constant.index()].value {
                                        hirsel_lir::ir::ConstantValue::Integer(n)
                                            if n < u128::from(count) =>
                                        {
                                            Some(Step::Inline(n as u64))
                                        }
                                        _ => None,
                                    }
                                }
                                _ => None,
                            })
                            .unwrap_or(Step::Index),
                        _ => Step::Index,
                    },
                ),
                ProjectionStep::Slot(_) => (
                    match record.kind {
                        LayoutKind::Sequence { element } | LayoutKind::SoaColumns { element } => {
                            element
                        }
                        _ => return None,
                    },
                    Step::Slot,
                ),
                // Physical cursor addressing retains the logical element path.
                // Last-use release must keep its owner alive through every read.
                ProjectionStep::FlatAt { .. } => (
                    match record.kind {
                        LayoutKind::Flat => record.fields.first()?.layout,
                        LayoutKind::Sequential { element } => element,
                        _ => return None,
                    },
                    Step::Index,
                ),
            };
            p.path.push(step);
            layout = next;
        }
        place = Some((p, layout));
    }
    place.map(|(p, _)| p)
}

/// Facts read from the unit's operations and declarations, never from a
/// pass's own answers.
pub struct Facts {
    pub places: Vec<Option<Place>>,
    /// Per declared parameter: its convention and its slot, if any.
    pub parameters: Vec<(Passing, Option<SlotId>)>,
    receipt_parameters: BTreeMap<SlotId, Passing>,
    /// The slot whose content a version name denotes.
    pub version_of: Vec<Option<SlotId>>,
    /// Per block, its version-join parameters as (ordinal, slot).
    pub phis: Vec<Vec<(usize, SlotId)>>,
    /// Plain statics: consumed without a credit.
    pub statics: Vec<bool>,
    /// The slot a borrowed view reads, directly or through another view.
    pub backing: Vec<Option<SlotId>>,
    /// Direct borrowed tag reads, with their inline path and observed root version.
    pub tag_views: Vec<Option<(SlotId, ValueId, Vec<Step>)>>,
    /// The slot a loan is on, directly or through a view.
    pub loan_slot: Vec<Option<SlotId>>,
    /// Each compiler-local region's mark value.
    pub marks: Vec<Option<ValueId>>,
    /// The unit's field paths, interned as the analyses meet them.
    paths: RefCell<Paths>,
    /// Per address, its place's interned path once asked for.
    place_paths: RefCell<Vec<Option<PathId>>>,
}

impl Facts {
    pub fn new<S>(unit: &LirUnit<S>, table: &LayoutTable) -> Self {
        let lay = Lay(table);
        let places: Vec<_> = (0..unit.addresses.len())
            .map(|a| place_of(unit, lay, AddressId(a as u32)))
            .collect();
        let abi = &table.fn_abis[unit.header.abi.0 as usize];
        let parameters: Vec<_> = abi
            .parameters
            .iter()
            .map(|p| {
                let slot = unit
                    .slots
                    .iter()
                    .position(|s| s.kind == SlotKind::Parameter(p.source))
                    .map(|s| SlotId(s as u32));
                (p.convention, slot)
            })
            .collect();
        let receipt_parameters = unit
            .tables
            .receipt_entry
            .as_ref()
            .into_iter()
            .flat_map(|entry| &entry.parameters)
            .filter_map(|binding| {
                let parameter = abi
                    .receipts
                    .as_ref()?
                    .parameters
                    .iter()
                    .find(|p| p.source == binding.source)?;
                let passing = match parameter.mode {
                    hirsel_lir::sealed_payload_source::ReceiptMode::Borrow => Passing::Borrow,
                    hirsel_lir::sealed_payload_source::ReceiptMode::Take => Passing::Take,
                    hirsel_lir::sealed_payload_source::ReceiptMode::MutSlot => Passing::Mut,
                };
                Some((binding.slot, passing))
            })
            .collect();
        let n = unit.values.len();
        let mut version_of = vec![None; n];
        let mut statics = vec![false; n];
        let mut backing = vec![None; n];
        let mut tag_views = vec![None; n];
        let mut marks = vec![None; unit.tables.regions.len()];
        let slot_root = |a: AddressId| match places[a.index()].as_ref()?.root {
            Root::Slot(s) => Some(s),
            Root::Value(_) => None,
        };
        // Views in definition order; a view through another view inherits
        // its backing, so iterate to a fixpoint over the whole unit.
        let mut changed = true;
        while changed {
            changed = false;
            for inst in &unit.instructions {
                match *inst {
                    Inst::Store {
                        address, version, ..
                    }
                    | Inst::Replace {
                        address, version, ..
                    } => {
                        if let Some(s) = slot_root(address) {
                            version_of[version.index()] = Some(s);
                        }
                        if let Inst::Replace {
                            displaced,
                            displaced_version,
                            ..
                        } = *inst
                        {
                            version_of[displaced_version.index()] = Some(displaced);
                        }
                    }
                    // A sequence write names a new version of its place's root.
                    Inst::SeqReserve {
                        address, version, ..
                    }
                    | Inst::ArenaPush {
                        address, version, ..
                    }
                    | Inst::SeqSetLen {
                        address, version, ..
                    }
                    | Inst::SeqAppend {
                        address, version, ..
                    }
                    | Inst::FlatAppend {
                        address, version, ..
                    }
                    | Inst::FlatField {
                        address, version, ..
                    }
                    | Inst::FlatClose {
                        address, version, ..
                    } => {
                        if let Some(s) = slot_root(address) {
                            version_of[version.index()] = Some(s);
                        }
                    }
                    Inst::Const { result, .. }
                    | Inst::ReadStatic { result, .. }
                    | Inst::FunctionRef { result, .. } => {
                        statics[result.index()] =
                            unit.values[result.index()].holding == Holding::Plain;
                    }
                    Inst::Read {
                        result,
                        address,
                        mode: ReadMode::Borrow,
                        reaching: version,
                        ..
                    } => {
                        if let Some(Place {
                            root: Root::Slot(slot),
                            path,
                            ..
                        }) = &places[address.index()]
                        {
                            tag_views[result.index()] = Some((*slot, version, path.clone()));
                        }
                        let b = match places[address.index()].as_ref().map(|p| p.root) {
                            Some(Root::Slot(s)) => Some(s),
                            Some(Root::Value(v)) => backing[v.index()],
                            None => None,
                        };
                        if b.is_some() && backing[result.index()] != b {
                            backing[result.index()] = b;
                            changed = true;
                        }
                    }
                    Inst::SliceView { result, value, .. } => {
                        let b = backing[value.index()];
                        if b.is_some() && backing[result.index()] != b {
                            backing[result.index()] = b;
                            changed = true;
                        }
                    }
                    Inst::RegionEnter { result, region } => {
                        marks[region.index()] = Some(result);
                    }
                    _ => {}
                }
            }
        }
        for term in &unit.terminators {
            if let Terminator::Call { call, .. } = *term {
                for (address, version) in unit.tables.calls[call.index()].writebacks(&unit.pools) {
                    if let Some(s) = slot_root(address) {
                        version_of[version.index()] = Some(s);
                    }
                }
            }
        }
        for (i, &(_, slot)) in parameters.iter().enumerate() {
            if let (Some(slot), Some(&v)) = (slot, unit.parameters.get(i)) {
                version_of[v.index()] = Some(slot);
            }
        }
        for (s, slot) in unit.slots.iter().enumerate() {
            if let SlotKind::Capture(ordinal) = slot.kind
                && let Some(&v) = unit.captures.get(ordinal as usize)
            {
                version_of[v.index()] = Some(SlotId(s as u32));
            }
        }
        if let Some(entry) = &unit.tables.receipt_entry {
            for binding in &entry.parameters {
                version_of[binding.version.index()] = Some(binding.slot);
            }
        }
        // Version joins and views flow through block parameters.
        let mut changed = true;
        while changed {
            changed = false;
            for edge in &unit.tables.edges {
                let args = edge.arguments.get(&unit.pools.values).unwrap_or(&[]);
                let params = unit.blocks[edge.target.index()]
                    .parameters
                    .get(&unit.pools.values)
                    .unwrap_or(&[]);
                for (&a, &p) in args.iter().zip(params) {
                    if version_of[p.index()].is_none() && version_of[a.index()].is_some() {
                        version_of[p.index()] = version_of[a.index()];
                        changed = true;
                    }
                    if backing[p.index()].is_none() && backing[a.index()].is_some() {
                        backing[p.index()] = backing[a.index()];
                        changed = true;
                    }
                }
            }
        }
        let phis = unit
            .blocks
            .iter()
            .map(|b| {
                b.parameters
                    .get(&unit.pools.values)
                    .unwrap_or(&[])
                    .iter()
                    .enumerate()
                    .filter_map(|(j, p)| version_of[p.index()].map(|s| (j, s)))
                    .collect()
            })
            .collect();
        let loan_slot = unit
            .tables
            .loans
            .iter()
            .map(|l| match places[l.address.index()].as_ref()?.root {
                Root::Slot(s) => Some(s),
                Root::Value(v) => backing[v.index()],
            })
            .collect();
        Self {
            places,
            parameters,
            version_of,
            phis,
            statics,
            backing,
            tag_views,
            receipt_parameters,
            loan_slot,
            marks,
            paths: RefCell::default(),
            place_paths: RefCell::default(),
        }
    }

    /// A block's version joins; blocks a pass added later have none.
    pub fn phis_of(&self, b: BlockId) -> &[(usize, SlotId)] {
        self.phis.get(b.index()).map_or(&[], Vec::as_slice)
    }

    pub fn passing_of(&self, slot: SlotId) -> Option<Passing> {
        self.parameters
            .iter()
            .find(|&&(_, s)| s == Some(slot))
            .map(|&(p, _)| p)
            .or_else(|| self.receipt_parameters.get(&slot).copied())
    }

    pub fn slot_of(&self, address: AddressId) -> Option<(SlotId, &[Step])> {
        let p = self.places.get(address.index())?.as_ref()?;
        match p.root {
            Root::Slot(s) => Some((s, &p.path)),
            Root::Value(_) => None,
        }
    }

    /// `slot_of` with the path interned.
    pub(crate) fn slot_path(&self, address: AddressId) -> Option<(SlotId, PathId)> {
        let (slot, path) = self.slot_of(address)?;
        let mut cache = self.place_paths.borrow_mut();
        if cache.len() <= address.index() {
            cache.resize(address.index() + 1, None);
        }
        let id = *cache[address.index()].get_or_insert_with(|| self.intern(path));
        Some((slot, id))
    }

    pub(crate) fn intern(&self, path: &[Step]) -> PathId {
        self.paths.borrow_mut().intern(path)
    }

    /// The unit's interned paths. Interning while this is held panics.
    pub(crate) fn paths(&self) -> Ref<'_, Paths> {
        self.paths.borrow()
    }
}

pub(crate) fn call_arguments<S>(
    unit: &LirUnit<S>,
    call: hirsel_lir::ir::CallId,
) -> &[hirsel_lir::ir::CallArgument] {
    unit.tables.calls[call.index()]
        .arguments
        .get(&unit.pools.call_arguments)
        .unwrap_or(&[])
}

/// Owned values an operation consumes (fields, token, taken captures,
/// replacement value, stored value, released value). A flat construction
/// only reads the values it encodes into variable fields (LIR schema
/// revision 4).
fn consumed<S>(unit: &LirUnit<S>, lay: Lay<'_>, inst: &Inst) -> Vec<ValueId> {
    let fields = |c: hirsel_lir::ir::ConstructId| {
        let site = &unit.tables.constructs[c.index()];
        let flat = lay.get(site.layout).filter(|r| r.kind == LayoutKind::Flat);
        site.fields
            .get(&unit.pools.fields)
            .unwrap_or(&[])
            .iter()
            .filter(|f| {
                let borrowed = lay.get(site.layout).is_some_and(|r| {
                    r.fields.iter().any(|d| {
                        d.ordinal == f.field && hirsel_lir::layout::field_is_borrowed(lay.0, r, d)
                    })
                });
                !borrowed
                    && !flat.is_some_and(|r| {
                        r.fields.iter().any(|d| {
                            d.ordinal == f.field
                                && lay
                                    .get(d.layout)
                                    .is_some_and(|l| matches!(l.size, SizeSpec::Dynamic(_)))
                        })
                    })
            })
            .map(|f| f.value)
            .collect::<Vec<_>>()
    };
    match *inst {
        Inst::Construct { construct, .. }
        | Inst::FlatAppend { construct, .. }
        | Inst::FlatClose { construct, .. } => fields(construct),
        Inst::Reuse {
            token, construct, ..
        } => {
            let mut out = vec![token];
            out.extend(fields(construct));
            out
        }
        Inst::Closure { closure, .. } => unit.tables.closures[closure.index()]
            .captures
            .get(&unit.pools.captures)
            .unwrap_or(&[])
            .iter()
            .filter(|c| c.usage == CaptureUse::Take)
            .map(|c| c.value)
            .collect(),
        Inst::Replace { value, .. } | Inst::Store { value, .. } => vec![value],
        Inst::Release { value, .. } | Inst::Reset { value, .. } | Inst::Forget { value } => {
            vec![value]
        }
        Inst::Weaken { result, value } if unit.values[result.index()].holding == Holding::Owned => {
            vec![value]
        }
        Inst::Recast { result, value } if unit.values[result.index()].holding == Holding::Owned => {
            vec![value]
        }
        Inst::SealArena { root, .. } => vec![root],
        Inst::Freeze { value, .. } | Inst::Thaw { value, .. } => vec![value],
        Inst::Dispose { token } => vec![token],
        Inst::RegionExit { mark, .. } => vec![mark],
        Inst::HoleFill { hole, value } => vec![hole, value],
        Inst::HoleAbandon { hole, root } => [hole].into_iter().chain(root).collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Unit editing

/// A unit under rewrite: each block's instructions as its own vector, so a
/// pass inserts and deletes freely; `finish` flattens them again and drops
/// every block, call, edge and failure edge the rewrite left unreachable or unnamed.
pub struct Editor<S> {
    pub unit: LirUnit<S>,
    pub code: Vec<Vec<Inst>>,
}

impl<S: Stage> Editor<S> {
    pub fn new(mut unit: LirUnit<S>) -> Self {
        let code = unit
            .blocks
            .iter()
            .map(|b| {
                b.instructions
                    .get(&unit.instructions)
                    .unwrap_or(&[])
                    .to_vec()
            })
            .collect();
        unit.instructions.clear();
        Self { unit, code }
    }

    pub fn value(&mut self, ty: LirTypeId, holding: Holding) -> ValueId {
        let id = ValueId(self.unit.values.len() as u32);
        self.unit.values.push(ValueInfo { ty, holding });
        id
    }

    pub fn block(
        &mut self,
        kind: BlockKind,
        parameters: &[ValueId],
        anchor: SourceAnchorId,
    ) -> BlockId {
        let id = BlockId(self.unit.blocks.len() as u32);
        let parameters = PoolRange::push(&mut self.unit.pools.values, parameters.iter().copied());
        self.unit.blocks.push(Block {
            kind,
            parameters,
            instructions: PoolRange::EMPTY,
            anchor,
        });
        self.unit.terminators.push(Terminator::Unreachable);
        self.code.push(Vec::new());
        id
    }

    pub fn edge(&mut self, target: BlockId, arguments: &[ValueId]) -> EdgeId {
        let id = EdgeId(self.unit.tables.edges.len() as u32);
        let arguments = PoolRange::push(&mut self.unit.pools.values, arguments.iter().copied());
        self.unit.tables.edges.push(Edge { target, arguments });
        id
    }

    pub fn failure(&mut self, cleanup: EdgeId, anchor: SourceAnchorId) -> FailureId {
        let id = FailureId(self.unit.tables.failures.len() as u32);
        self.unit.tables.failures.push(FailureEdge {
            cleanup,
            rule: None,
            anchor,
            dispose: PoolRange::EMPTY,
        });
        id
    }

    pub fn cleanup_anchor(&mut self, anchor: CleanupAnchor) -> CleanupAnchorId {
        if let Some(i) = self
            .unit
            .tables
            .cleanup_anchors
            .iter()
            .position(|a| *a == anchor)
        {
            return CleanupAnchorId(i as u32);
        }
        self.unit.tables.cleanup_anchors.push(anchor);
        CleanupAnchorId(self.unit.tables.cleanup_anchors.len() as u32 - 1)
    }

    /// An address of the whole content of `slot`, prepared at entry. Slot
    /// roots with no projection check nothing, so an entry preparation
    /// dominates every use.
    pub fn whole(&mut self, facts: &mut Facts, slot: SlotId) -> AddressId {
        self.whole_reporting(facts, slot, |_, _| {})
    }

    fn whole_reporting(
        &mut self,
        facts: &mut Facts,
        slot: SlotId,
        inserted: impl FnOnce(usize, usize),
    ) -> AddressId {
        for (a, info) in self.unit.addresses.iter().enumerate() {
            let id = AddressId(a as u32);
            if info.base == AddressBase::Slot(slot)
                && info.projection.len == 0
                && self.prepared_at_entry(id)
            {
                return id;
            }
        }
        let ty = self.unit.slots[slot.index()].ty;
        let id = AddressId(self.unit.addresses.len() as u32);
        self.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Slot(slot),
            projection: PoolRange::EMPTY,
            ty,
        });
        facts.places.push(Some(Place {
            root: Root::Slot(slot),
            path: Vec::new(),
            crossings: Vec::new(),
        }));
        // An entry preparation fails nowhere; its failure edge still needs a
        // cleanup, which the entry's own preparations already name.
        let entry = self.unit.entry.index();
        let failure = self.code[entry]
            .iter()
            .find_map(|i| match *i {
                Inst::Address { failure, .. } => Some(failure),
                _ => None,
            })
            .map(|f| {
                let cleanup = self.unit.tables.failures[f.index()].cleanup;
                let edge = &self.unit.tables.edges[cleanup.index()];
                let (target, args) = (
                    edge.target,
                    edge.arguments
                        .get(&self.unit.pools.values)
                        .unwrap_or(&[])
                        .to_vec(),
                );
                let anchor = self.unit.tables.failures[f.index()].anchor;
                let e = self.edge(target, &args);
                self.failure(e, anchor)
            });
        let failure = match failure {
            Some(f) => f,
            None => {
                let anchor = self.unit.blocks[entry].anchor;
                let b = self.block(BlockKind::FailureCleanup, &[], anchor);
                self.unit.terminators[b.index()] = Terminator::Propagate;
                let e = self.edge(b, &[]);
                self.failure(e, anchor)
            }
        };
        // Beside the entry's own preparations, so it fails in their state
        // (after a region the unit enters first, say).
        let at = self.code[entry]
            .iter()
            .position(|i| matches!(i, Inst::Address { .. }))
            .unwrap_or(0);
        self.code[entry].insert(
            at,
            Inst::Address {
                address: id,
                failure,
            },
        );
        inserted(entry, at);
        id
    }

    fn prepared_at_entry(&self, address: AddressId) -> bool {
        self.code[self.unit.entry.index()]
            .iter()
            .any(|i| matches!(*i, Inst::Address { address: a, .. } if a == address))
    }

    /// Flatten the blocks and drop what the rewrite left unreachable or unnamed.
    pub fn finish(self) -> LirUnit<S> {
        self.finish_reporting(|_, _, _| ()).0
    }

    /// Diagnostic callers retain the exact block/edge compaction performed by
    /// this editor. Ordinary callers instantiate a zero-sized report and do
    /// not clone either map. Instruction origins belong to the rewriting pass.
    pub(crate) fn finish_reporting<R>(
        mut self,
        report: impl FnOnce(&[Option<BlockId>], &[Option<u32>], &[Option<CleanupAnchorId>]) -> R,
    ) -> (LirUnit<S>, R) {
        let n = self.unit.blocks.len();
        let succ = |unit: &LirUnit<S>, code: &[Vec<Inst>], b: usize| -> Vec<BlockId> {
            let mut out = Vec::new();
            for f in code[b].iter().filter_map(Inst::failure) {
                let e = unit.tables.failures[f.index()].cleanup;
                out.push(unit.tables.edges[e.index()].target);
            }
            for e in terminator_edges(unit, &unit.terminators[b]) {
                out.push(unit.tables.edges[e.index()].target);
            }
            out
        };
        let mut reach = vec![false; n];
        let mut queue = VecDeque::from([self.unit.entry]);
        reach[self.unit.entry.index()] = true;
        while let Some(b) = queue.pop_front() {
            for t in succ(&self.unit, &self.code, b.index()) {
                if !std::mem::replace(&mut reach[t.index()], true) {
                    queue.push_back(t);
                }
            }
        }
        let mut block_map = vec![None; n];
        let mut kept = 0u32;
        for b in 0..n {
            if reach[b] {
                block_map[b] = Some(BlockId(kept));
                kept += 1;
            }
        }
        let mut failure_used = vec![false; self.unit.tables.failures.len()];
        let mut edge_used = vec![false; self.unit.tables.edges.len()];
        let mut call_used = vec![false; self.unit.tables.calls.len()];
        for b in (0..n).filter(|&b| reach[b]) {
            for f in self.code[b].iter().filter_map(Inst::failure) {
                failure_used[f.index()] = true;
            }
            let term = &self.unit.terminators[b];
            if let Terminator::Call { call, .. } = term {
                call_used[call.index()] = true;
            }
            for f in term.failure_edges(&self.unit.tables) {
                failure_used[f.index()] = true;
            }
            for e in Terminator::normal_edges(term, &self.unit.pools) {
                edge_used[e.index()] = true;
            }
        }
        for (f, used) in failure_used.iter().enumerate() {
            if *used {
                edge_used[self.unit.tables.failures[f].cleanup.index()] = true;
            }
        }
        let renumber = |used: &[bool]| {
            let mut next = 0u32;
            used.iter()
                .map(|&u| {
                    u.then(|| {
                        next += 1;
                        next - 1
                    })
                })
                .collect::<Vec<_>>()
        };
        let failure_map = renumber(&failure_used);
        let edge_map = renumber(&edge_used);
        let call_map = renumber(&call_used);
        let fm = |f: FailureId| FailureId(failure_map[f.index()].expect("named failure"));
        let em = |e: EdgeId| EdgeId(edge_map[e.index()].expect("named edge"));
        let mut blocks = Vec::new();
        let mut terminators = Vec::new();
        let mut instructions = Vec::new();
        let old_blocks = std::mem::take(&mut self.unit.blocks);
        let old_terms = std::mem::take(&mut self.unit.terminators);
        let old_arms = std::mem::take(&mut self.unit.pools.switch_arms);
        let code = std::mem::take(&mut self.code);
        for (b, ((mut block, term), insts)) in
            old_blocks.into_iter().zip(old_terms).zip(code).enumerate()
        {
            if !reach[b] {
                continue;
            }
            let start = instructions.len() as u32;
            for mut inst in insts {
                if let Some(f) = Inst::failure_mut(&mut inst) {
                    *f = fm(*f);
                }
                instructions.push(inst);
            }
            block.instructions = PoolRange::new(start, instructions.len() as u32 - start);
            blocks.push(block);
            terminators.push(match term {
                Terminator::Jump(e) => Terminator::Jump(em(e)),
                Terminator::Branch { condition, yes, no } => Terminator::Branch {
                    condition,
                    yes: em(yes),
                    no: em(no),
                },
                Terminator::Recover {
                    arena,
                    caught,
                    failed,
                } => Terminator::Recover {
                    arena,
                    caught: em(caught),
                    failed: em(failed),
                },
                Terminator::Isolate { start, caught } => Terminator::Isolate {
                    start,
                    caught: em(caught),
                },
                Terminator::Switch {
                    value,
                    arms,
                    otherwise,
                } => Terminator::Switch {
                    value,
                    arms: PoolRange::push(
                        &mut self.unit.pools.switch_arms,
                        arms.get(&old_arms).unwrap_or(&[]).iter().map(|a| {
                            hirsel_lir::ir::SwitchArm {
                                value: a.value,
                                edge: em(a.edge),
                            }
                        }),
                    ),
                    otherwise: em(otherwise),
                },
                Terminator::Call {
                    call,
                    normal,
                    failure,
                } => Terminator::Call {
                    call: CallId(call_map[call.index()].expect("named call")),
                    normal: normal.map(em),
                    failure: fm(failure),
                },
                Terminator::Raise(f) => Terminator::Raise(fm(f)),
                other => other,
            });
        }
        let failures = std::mem::take(&mut self.unit.tables.failures);
        self.unit.tables.failures = failures
            .into_iter()
            .enumerate()
            .filter(|(f, _)| failure_used[*f])
            .map(|(_, mut f)| {
                f.cleanup = em(f.cleanup);
                f
            })
            .collect();
        let edges = std::mem::take(&mut self.unit.tables.edges);
        self.unit.tables.edges = edges
            .into_iter()
            .enumerate()
            .filter(|(e, _)| edge_used[*e])
            .map(|(_, mut e)| {
                e.target = block_map[e.target.index()].expect("edge into a kept block");
                e
            })
            .collect();
        let calls = std::mem::take(&mut self.unit.tables.calls);
        let arguments = std::mem::take(&mut self.unit.pools.call_arguments);
        self.unit.tables.calls = calls
            .into_iter()
            .enumerate()
            .filter(|(call, _)| call_used[*call])
            .map(|(_, mut call)| {
                call.pretransfer_failure = call.pretransfer_failure.map(fm);
                call.arguments = PoolRange::push(
                    &mut self.unit.pools.call_arguments,
                    call.arguments
                        .get(&arguments)
                        .unwrap_or(&[])
                        .iter()
                        .copied(),
                );
                call
            })
            .collect();
        self.unit.entry = block_map[self.unit.entry.index()].expect("entry is reachable");
        self.unit.blocks = blocks;
        self.unit.terminators = terminators;
        self.unit.instructions = instructions;
        compact_values(&mut self.unit);
        let anchors = prune_anchors(&mut self.unit);
        let report = report(&block_map, &edge_map, &anchors);
        (self.unit, report)
    }
}

/// Every edge a terminator names, normal ones first, then its failure cleanup.
pub(crate) fn terminator_edges<S: Stage>(unit: &LirUnit<S>, term: &Terminator) -> Vec<EdgeId> {
    let mut out = Terminator::normal_edges(term, &unit.pools);
    for f in term.failure_edges(&unit.tables) {
        out.push(unit.tables.failures[f.index()].cleanup);
    }
    out
}

// ---------------------------------------------------------------------------
// Forward state: initialization, loans, regions and credits

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
    pub fn may(self) -> bool {
        self != Tri::No
    }
}

/// `Element` flags. Dead storage, an inactive loan, a region not entered:
/// what most of a state is.
pub(crate) const DEAD: u8 = 1;
/// Joined with a `DEAD` element, this one stays as it is.
const ABSORBS: u8 = 2;
/// Storage holding nothing: uninitialized on every path.
pub(crate) const EMPTY: u8 = 4;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SlotState {
    pub live: Tri,
    pub init: Tri,
    /// Absent owners: transferred parts and inactive variant fields.
    pub moved: Tris,
    /// Actual transfers, separate from fields absent on inactive variant edges.
    pub taken: Tris,
    /// Resource shells whose parent finalizer is already suppressed/completed.
    /// Defuse regions empty these shells before any join.
    pub fields_pending: Tris,
    /// Versions that may name the content here.
    pub versions: Versions,
    /// Proved to own nothing on every path since its last write, as C's
    /// `owned_empty`: a join keeps the proof where path correlation is lost,
    /// so no flag claims content there. Unlike C, a join still merges both
    /// sides' moved parts, since drops are planned from them.
    pub owned_empty: bool,
}

/// The versions that may name a slot's content at a point. Only one names
/// it usably, so every set of two or more is one state, `Many`: a join stays
/// the same size however many paths meet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Versions {
    None,
    One(ValueId),
    Many,
}

impl Versions {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Versions::None, x) | (x, Versions::None) => x,
            (Versions::One(a), Versions::One(b)) if a == b => self,
            _ => Versions::Many,
        }
    }
    /// The one version, if exactly one reaches.
    pub fn one(self) -> Option<ValueId> {
        match self {
            Versions::One(v) => Some(v),
            _ => None,
        }
    }
}

impl From<Option<ValueId>> for Versions {
    fn from(v: Option<ValueId>) -> Self {
        v.map_or(Versions::None, Versions::One)
    }
}

impl SlotState {
    const DEAD: Self = Self {
        live: Tri::No,
        init: Tri::No,
        moved: Tris::EMPTY,
        taken: Tris::EMPTY,
        fields_pending: Tris::EMPTY,
        versions: Versions::None,
        owned_empty: false,
    };
    /// Whole and definitely initialized: no part moved out on any path.
    pub fn whole(&self) -> bool {
        self.init == Tri::Yes && self.taken.is_empty()
    }

    /// A mutable loan or an address argument may write any part overlapping
    /// `path`: of those, only the parts actually taken stay absent.
    fn expose(&mut self, path: PathId, paths: &Paths) {
        let at = paths.steps(path);
        self.moved.retain(|q| !overlap(paths.steps(q), at));
        for (q, t) in self.taken.iter() {
            if overlap(paths.steps(q), at) {
                self.moved.set(q, t);
            }
        }
    }

    /// The part at `path` and everything under it are gone, as a take or a
    /// drop of it leaves them: whole storage is uninitialized, a part
    /// becomes taken.
    fn remove(&mut self, path: PathId, paths: &Paths) {
        let at = paths.steps(path);
        if at.is_empty() {
            self.init = Tri::No;
            self.moved.clear();
            self.taken.clear();
            self.fields_pending.clear();
            self.versions = Versions::None;
        } else if !through_slot(at) {
            let below = |q: PathId| {
                let q = paths.steps(q);
                q.len() > at.len() && overlap(q, at)
            };
            self.taken.retain(|q| !below(q));
            self.moved.retain(|q| !below(q));
            self.taken.set(path, Tri::Yes);
            self.moved.set(path, Tri::Yes);
        }
        if self.init == Tri::No {
            self.owned_empty = false;
        }
    }

    /// A store of new content at `path` (`whole` unless it reaches into an
    /// element named without a count check).
    fn write(&mut self, path: PathId, paths: &Paths, whole: bool) {
        let at = paths.steps(path);
        if at.is_empty() {
            self.init = Tri::Yes;
            self.moved.clear();
            self.taken.clear();
        } else if whole {
            self.taken.retain(|q| {
                let q = paths.steps(q);
                !(q.len() > at.len() && overlap(q, at))
            });
            self.moved.retain(|q| !overlap(paths.steps(q), at));
        }
        self.taken.retain(|q| {
            let q = paths.steps(q);
            !(q.len() >= at.len() && overlap(q, at))
        });
        self.fields_pending
            .retain(|q| !paths.steps(q).starts_with(at));
        self.owned_empty = false;
    }
}

impl SlotState {
    fn join(&mut self, other: &SlotState) {
        self.live = self.live.join(other.live);
        if self.init == Tri::No {
            self.moved = other.moved.clone();
            self.taken = other.taken.clone();
            self.fields_pending = other.fields_pending.clone();
            self.owned_empty = other.owned_empty;
        } else if other.init != Tri::No {
            self.moved.join(&other.moved);
            self.taken.join(&other.taken);
            self.fields_pending.join(&other.fields_pending);
            self.owned_empty &= other.owned_empty;
        }
        self.init = self.init.join(other.init);
        self.versions = self.versions.join(other.versions);
    }
}

impl Element for SlotState {
    fn flags(&self) -> u8 {
        let dead = self.live == Tri::No
            && self.init == Tri::No
            && self.taken.is_empty()
            && self.moved.is_empty()
            && self.fields_pending.is_empty()
            && self.versions == Versions::None
            && !self.owned_empty;
        // Joined with dead storage, a `Yes` becomes `Maybe` and nothing else
        // changes.
        let absorbs = self.live != Tri::Yes
            && self.init != Tri::Yes
            && (self.init != Tri::No
                || self.moved.is_empty()
                    && self.taken.is_empty()
                    && self.fields_pending.is_empty());
        u8::from(dead) * DEAD + u8::from(absorbs) * ABSORBS + u8::from(self.init == Tri::No) * EMPTY
    }
    fn absorbs(own: u8, other: u8) -> bool {
        own & ABSORBS != 0 && other & DEAD != 0
    }
}

/// The ownership state at one program point. Slots, loans and regions share
/// structure with the states they were cloned from (`PVec`). Credits are
/// exact: a valid unit agrees on them at every join, so a join keeps the
/// first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Invocation {
    Inactive,
    Active(SourceAnchorId),
    Ambiguous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct State {
    pub slots: PVec<SlotState>,
    pub loans: PVec<Tri>,
    pub regions: PVec<Tri>,
    pub credits: BTreeMap<ValueId, u32>,
    /// The call admission to retire before failure cleanup releases caller storage.
    pub invocation: Invocation,
}

impl State {
    fn join(&mut self, other: &State) {
        if self.invocation != other.invocation {
            self.invocation = Invocation::Ambiguous;
        }
        self.slots.join(&other.slots, SlotState::join);
        self.loans.join(&other.loans, |x, y| *x = x.join(*y));
        self.regions.join(&other.regions, |x, y| *x = x.join(*y));
    }
}

/// The join of `states`, neighbours first, level by level. The states
/// reaching a block with many predecessors (a cleanup tail shared by many
/// failure sites) are successive points of one function and share structure
/// with their neighbours, so each level costs what differs within its pairs:
/// joining them one by one into an accumulator would instead cost, for each,
/// all that the accumulator has gathered. The first state's credits stay.
pub(crate) fn join_all(mut states: Vec<State>) -> Option<State> {
    while states.len() > 1 {
        let mut pairs = states.into_iter();
        let mut next = Vec::with_capacity(pairs.len().div_ceil(2));
        while let Some(mut a) = pairs.next() {
            if let Some(b) = pairs.next() {
                a.join(&b);
            }
            next.push(a);
        }
        states = next;
    }
    states.pop()
}

/// A failure id the forward analysis does not follow: the placeholder of an
/// operation whose cleanup is synthesized afterwards.
pub(crate) const PENDING_FAILURE: FailureId = FailureId(u32::MAX);

pub(crate) struct Flow<'a, S> {
    pub unit: &'a LirUnit<S>,
    pub code: &'a [Vec<Inst>],
    pub lay: Lay<'a>,
    pub facts: &'a Facts,
}

impl<S: Stage> Flow<'_, S> {
    pub fn entry_state(&self) -> State {
        let unit = self.unit;
        let slots = unit
            .slots
            .iter()
            .enumerate()
            .map(|(s, slot)| {
                let id = SlotId(s as u32);
                let entered = matches!(
                    slot.kind,
                    SlotKind::Parameter(_) | SlotKind::Capture(_) | SlotKind::ReceiptParameter(_)
                );
                let versions = match slot.kind {
                    SlotKind::Capture(ordinal) => unit.captures.get(ordinal as usize).copied(),
                    SlotKind::ReceiptParameter(ordinal) => unit
                        .tables
                        .receipt_entry
                        .as_ref()
                        .and_then(|entry| entry.parameters.iter().find(|p| p.source == ordinal))
                        .map(|p| p.version),
                    _ => self
                        .facts
                        .parameters
                        .iter()
                        .position(|&(_, p)| p == Some(id))
                        .and_then(|i| unit.parameters.get(i).copied()),
                }
                .into();
                SlotState {
                    live: if entered
                        || matches!(slot.kind, SlotKind::ReturnSlot | SlotKind::ReceiptResult)
                    {
                        Tri::Yes
                    } else {
                        Tri::No
                    },
                    init: if entered { Tri::Yes } else { Tri::No },
                    moved: Tris::EMPTY,
                    taken: Tris::EMPTY,
                    fields_pending: Tris::EMPTY,
                    versions,
                    owned_empty: false,
                }
            })
            .collect();
        State {
            invocation: Invocation::Inactive,
            slots,
            loans: unit.tables.loans.iter().map(|_| Tri::No).collect(),
            regions: unit.tables.regions.iter().map(|_| Tri::No).collect(),
            credits: unit
                .parameters
                .iter()
                .filter(|&&p| {
                    unit.values[p.index()].holding == Holding::Owned && owns(unit, self.lay, p)
                })
                .map(|&p| (p, 1))
                .collect(),
        }
    }

    /// Every block's successors over normal and failure edges, in code order.
    pub fn successors(&self) -> Vec<Vec<BlockId>> {
        let unit = self.unit;
        let target = |f: FailureId| {
            unit.tables.edges[unit.tables.failures[f.index()].cleanup.index()].target
        };
        (0..unit.blocks.len())
            .map(|b| {
                let mut out: Vec<BlockId> = self.code[b]
                    .iter()
                    .filter_map(Inst::failure)
                    .filter(|&f| f != PENDING_FAILURE)
                    .map(target)
                    .collect();
                let term = &unit.terminators[b];
                out.extend(
                    Terminator::normal_edges(term, &unit.pools)
                        .into_iter()
                        .map(|e| unit.tables.edges[e.index()].target),
                );
                out.extend(
                    term.failure_edges(&unit.tables)
                        .into_iter()
                        .filter(|&f| f != PENDING_FAILURE)
                        .map(target),
                );
                out
            })
            .collect()
    }

    /// Every block's entry state, to a fixpoint over normal and failure
    /// edges. A block's state is the join of the states its predecessors
    /// last sent along their edges, computed when the block is visited, and
    /// blocks are visited in reverse postorder, earliest first: a block is
    /// walked again only when a feedback edge changed what reaches it.
    pub fn solve(&self) -> Vec<Option<State>> {
        self.solve_counted(|_| {})
    }

    /// The same fixed point, reporting the instruction and terminator rows of
    /// each actual block transfer for deterministic work measurements.
    pub(crate) fn solve_counted(&self, mut transferred: impl FnMut(usize)) -> Vec<Option<State>> {
        let n = self.unit.blocks.len();
        let entry = self.unit.entry;
        let succ = self.successors();
        let mut rank = vec![usize::MAX; n];
        let adjacent: Vec<Vec<usize>> = succ
            .iter()
            .map(|ts| ts.iter().map(|b| b.index()).collect())
            .collect();
        let post: Vec<_> = hirsel_graph::reverse_postorder(&adjacent, entry.index())
            .into_iter()
            .map(|b| BlockId(b as u32))
            .collect();
        for (i, b) in post.iter().enumerate() {
            rank[b.index()] = i;
        }
        let mut ins: Vec<Option<State>> = vec![None; n];
        // Per block, the state it last sent along each of its edges, and per
        // block, where its incoming states are: (source, index in its list).
        let mut outs: Vec<Vec<(EdgeId, State)>> = vec![Vec::new(); n];
        let mut incoming: Vec<Vec<(BlockId, usize)>> = vec![Vec::new(); n];
        let mut work = BTreeSet::from([0usize]);
        while let Some(next) = work.pop_first() {
            let b = post[next];
            let arriving = (b == entry).then(|| self.entry_state()).into_iter().chain(
                incoming[b.index()]
                    .iter()
                    .map(|&(p, i)| outs[p.index()][i].1.clone()),
            );
            let joined = join_all(arriving.collect());
            if joined == ins[b.index()] {
                continue;
            }
            ins[b.index()] = joined;
            let Some(state) = ins[b.index()].clone() else {
                continue;
            };
            transferred(self.code[b.index()].len() + 1);
            let new = self.block_after(b, state, |_, _| {});
            let first = outs[b.index()].is_empty();
            for (i, (edge, out)) in new.iter().enumerate() {
                let target = self.unit.tables.edges[edge.index()].target;
                if first {
                    incoming[target.index()].push((b, i));
                } else if outs[b.index()][i].1 == *out {
                    continue;
                }
                work.insert(rank[target.index()]);
            }
            outs[b.index()] = new;
        }
        ins
    }

    /// Walk one block from its entry state, calling `visit(index, before,
    /// after)` around every instruction (index = instruction count for the
    /// terminator), and return the state along every outgoing edge.
    pub fn block(
        &self,
        b: BlockId,
        st: State,
        mut visit: impl FnMut(usize, &State, &State),
    ) -> Vec<(EdgeId, State)> {
        self.walk(b, st, true, |i, pre, post| {
            visit(i, pre.expect("kept before every instruction"), post)
        })
    }

    /// `block` for a visitor of the state after each instruction only. It
    /// keeps no state from before an instruction that its failure edge does
    /// not start from, so the walk writes slots it owns instead of copying
    /// them out of a state it shares.
    pub fn block_after(
        &self,
        b: BlockId,
        st: State,
        mut visit: impl FnMut(usize, &State),
    ) -> Vec<(EdgeId, State)> {
        self.walk(b, st, false, |i, _, post| visit(i, post))
    }

    fn walk(
        &self,
        b: BlockId,
        mut st: State,
        keep: bool,
        mut visit: impl FnMut(usize, Option<&State>, &State),
    ) -> Vec<(EdgeId, State)> {
        let unit = self.unit;
        let mut outs = Vec::new();
        let insts = &self.code[b.index()];
        for (i, inst) in insts.iter().enumerate() {
            // A failure edge leaves from after a release, a reset, a drop or
            // a region exit, and from before anything else.
            let failure = Inst::failure(inst).filter(|&f| f != PENDING_FAILURE);
            let after = matches!(
                inst,
                Inst::Release { .. }
                    | Inst::Reset { .. }
                    | Inst::Drop { .. }
                    | Inst::RegionExit { .. }
            );
            let pre = (keep || failure.is_some() && !after).then(|| st.clone());
            self.inst(inst, &mut st);
            visit(i, pre.as_ref(), &st);
            if let Some(failure) = failure {
                let fst = match pre {
                    Some(mut f) if !after => {
                        for v in consumed(unit, self.lay, inst)
                            .into_iter()
                            .filter(|_| !matches!(inst, Inst::SealArena { .. }))
                        {
                            if owns(unit, self.lay, v) {
                                take_credit(&mut f.credits, v);
                            }
                        }
                        f
                    }
                    _ => st.clone(),
                };
                outs.push(self.failure(failure, fst));
            }
        }
        let term = &unit.terminators[b.index()];
        let pre = keep.then(|| st.clone());
        match *term {
            Terminator::Call {
                call,
                normal,
                failure,
            } => {
                if let Some(rejected) = unit.tables.calls[call.index()].pretransfer_failure {
                    outs.push(self.failure(rejected, st.clone()));
                }
                for a in call_arguments(unit, call) {
                    match a.value {
                        ArgumentValue::Value(v) if a.passing == Passing::Take => {
                            if owns(unit, self.lay, v) {
                                take_credit(&mut st.credits, v);
                            }
                        }
                        _ => {}
                    }
                }
                for (address, version) in unit.tables.calls[call.index()].writebacks(&unit.pools) {
                    if let Some((slot, path)) = self.facts.slot_path(address) {
                        let slot = &mut st.slots[slot.index()];
                        slot.expose(path, &self.facts.paths());
                        slot.owned_empty = false;
                        slot.versions = Versions::One(version);
                    }
                }
                if failure != PENDING_FAILURE {
                    outs.push(self.failure(failure, st.clone()));
                }
                for result in unit.tables.calls[call.index()].normal_results() {
                    self.create(&mut st, result);
                }
                visit(insts.len(), pre.as_ref(), &st);
                if let Some(normal) = normal {
                    outs.push(self.leave(normal, st));
                }
                return outs;
            }
            Terminator::Return(v) => {
                if let Some(v) = v {
                    take_credit(&mut st.credits, v);
                }
            }
            Terminator::Raise(failure) => {
                visit(insts.len(), pre.as_ref(), &st);
                outs.push(self.failure(failure, st));
                return outs;
            }
            _ => {}
        }
        visit(insts.len(), pre.as_ref(), &st);
        for e in Terminator::normal_edges(term, &unit.pools) {
            let mut state = st.clone();
            self.switch_parts(term, e, &mut state);
            if let Terminator::Branch { condition, yes, .. } = *term
                && let Some(flag) = unit
                    .tables
                    .init_flags
                    .iter()
                    .find(|f| f.environment.is_none() && f.value == condition)
            {
                let path = flag_steps(unit, self.code, flag.path).expect("validated flag path");
                let id = self.facts.intern(&path);
                let paths = self.facts.paths();
                let slot = &mut state.slots[flag.slot.index()];
                if e == yes {
                    slot.owned_empty = false;
                }
                if path.is_empty() {
                    if e == yes {
                        slot.live = Tri::Yes;
                        slot.init = Tri::Yes;
                    } else {
                        slot.init = Tri::No;
                    }
                } else if e == yes {
                    slot.live = Tri::Yes;
                    slot.init = Tri::Yes;
                    slot.moved.retain(|p| !path.starts_with(paths.steps(p)));
                    slot.taken.retain(|p| !path.starts_with(paths.steps(p)));
                } else {
                    slot.taken.set(id, Tri::Yes);
                    slot.moved.set(id, Tri::Yes);
                }
                drop(paths);
                self.remember_empty(&mut state, flag.slot);
            }
            outs.push(self.leave(e, state));
        }
        outs
    }

    /// A current tag observation proves every field of the other variants absent.
    /// This is ordinary field presence, not a special empty-owner judgment.
    fn switch_parts(&self, term: &Terminator, edge: EdgeId, state: &mut State) {
        let Terminator::Switch {
            value,
            arms,
            otherwise,
        } = term
        else {
            return;
        };
        let Some((slot, version, prefix)) = self
            .facts
            .tag_views
            .get(value.index())
            .and_then(Option::as_ref)
        else {
            return;
        };
        if state.slots[slot.index()].versions != Versions::One(*version) {
            return;
        }
        let layout = value_layout(self.unit, *value);
        let Some(tag) = self.lay.get(layout).and_then(|r| r.tag.as_ref()) else {
            return;
        };
        let arms = arms.get(&self.unit.pools.switch_arms).unwrap_or(&[]);
        for variant in &tag.variants {
            if arms
                .iter()
                .find(|a| a.value == variant.ordinal)
                .map_or(*otherwise, |a| a.edge)
                != edge
            {
                for field in &variant.fields {
                    let mut path = prefix.clone();
                    path.push(Step::Variant(variant.ordinal, field.ordinal));
                    let path = self.facts.intern(&path);
                    state.slots[slot.index()].moved.set(path, Tri::Yes);
                }
            } else {
                for field in &variant.fields {
                    let mut path = prefix.clone();
                    path.push(Step::Variant(variant.ordinal, field.ordinal));
                    let path = self.facts.intern(&path);
                    let s = &mut state.slots[slot.index()];
                    s.moved.set(path, s.taken.get(path));
                }
            }
        }
        self.remember_empty(state, *slot);
    }

    /// Record that `slot` owns nothing once every owner is absent, as C does
    /// after a take, a drop, a finalization or a variant observation.
    fn remember_empty(&self, st: &mut State, slot: SlotId) {
        let layout = self.unit.types[self.unit.slots[slot.index()].ty.index()].layout;
        if self.lay.trivial(layout) {
            return;
        }
        let s = &st.slots[slot.index()];
        let paths = self.facts.paths();
        if s.init != Tri::No
            && !s.owned_empty
            && (paths
                .get(&[])
                .is_some_and(|whole| s.moved.get(whole) == Tri::Yes)
                || crate::cleanup::absent(s, &paths, &mut Vec::new(), layout, self.lay))
        {
            st.slots[slot.index()].owned_empty = true;
        }
    }

    fn failure(&self, failure: FailureId, mut st: State) -> (EdgeId, State) {
        let f = &self.unit.tables.failures[failure.index()];
        for &t in f.dispose.get(&self.unit.pools.values).unwrap_or(&[]) {
            take_credit(&mut st.credits, t);
        }
        (f.cleanup, self.leave(f.cleanup, st).1)
    }

    fn leave(&self, edge: EdgeId, mut st: State) -> (EdgeId, State) {
        let unit = self.unit;
        let e = &unit.tables.edges[edge.index()];
        let args = e.arguments.get(&unit.pools.values).unwrap_or(&[]);
        let params = unit.blocks[e.target.index()]
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[]);
        for &(j, slot) in self.facts.phis_of(e.target) {
            st.slots[slot.index()].versions = Versions::One(params[j]);
        }
        let mut incoming = Vec::new();
        for (&a, &p) in args.iter().zip(params) {
            if owns(unit, self.lay, p) && unit.values[p.index()].holding == Holding::Owned {
                take_credit(&mut st.credits, a);
                incoming.push(p);
            }
        }
        for p in incoming {
            *st.credits.entry(p).or_insert(0) += 1;
        }
        (edge, st)
    }

    fn create(&self, st: &mut State, value: ValueId) {
        if owns(self.unit, self.lay, value)
            && self.unit.values[value.index()].holding == Holding::Owned
        {
            *st.credits.entry(value).or_insert(0) += 1;
        }
    }

    fn place(&self, address: AddressId) -> Option<(SlotId, PathId)> {
        self.facts.slot_path(address)
    }

    /// Shared effect for Defuse and completed explicit finalization. It keeps
    /// storage/version and all remaining field owners intact.
    pub(crate) fn fields_pending(&self, st: &mut State, address: AddressId) {
        if let Some((slot, path)) = self.place(address) {
            st.slots[slot.index()].fields_pending.set(path, Tri::Yes);
        }
    }

    pub fn inst(&self, inst: &Inst, st: &mut State) {
        let unit = self.unit;
        match *inst {
            Inst::Const { result, .. }
            | Inst::FunctionRef { result, .. }
            | Inst::ReadStatic { result, .. }
            | Inst::TokenNone { result }
            | Inst::Unary { result, .. }
            | Inst::Binary { result, .. }
            | Inst::Compare { result, .. }
            | Inst::IsUnique { result, .. }
            | Inst::IsWritable { result, .. }
            | Inst::UpdateMask { result, .. }
            | Inst::FlatCopy { result, .. }
            | Inst::FlatView { result, .. }
            | Inst::SliceView { result, .. }
            | Inst::FlatLength { result, .. }
            | Inst::FlatNext { result, .. }
            | Inst::SeqLen { result, .. }
            | Inst::SeqCapacity { result, .. }
            | Inst::Hash { result, .. }
            | Inst::Encode { result, .. }
            | Inst::Decode { result, .. }
            | Inst::UniqueCopy { result, .. }
            | Inst::MulAdd { result, .. }
            | Inst::ScalarUnary { result, .. }
            | Inst::CopyValue { result, .. }
            | Inst::Copy { result, .. } => self.create(st, result),
            Inst::Convert { result, flag, .. } | Inst::IntArith { result, flag, .. } => {
                self.create(st, result);
                if let Some(flag) = flag {
                    self.create(st, flag);
                }
            }
            Inst::Check { .. }
            | Inst::EndScope { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::Unavailable { .. }
            | Inst::ArenaInventory { .. }
            | Inst::RegionPublish { .. }
            | Inst::FinalizeBegin { .. } => {}
            Inst::FrameEnter { failure } =>
                st.invocation = Invocation::Active(unit.tables.failures[failure.index()].anchor),
            Inst::FrameLeave => st.invocation = Invocation::Inactive,
            Inst::FinalizeEnd { address, .. } | Inst::Defuse { address } => {
                self.fields_pending(st, address);
                if let Some((slot, _)) = self.place(address) {
                    self.remember_empty(st, slot);
                }
            }
            Inst::Address { address, .. } => {
                if let Some((slot, path)) = self.facts.slot_of(address) {
                    let mut layout =
                        self.unit.types[self.unit.slots[slot.index()].ty.index()].layout;
                    for (n, step) in path.iter().enumerate() {
                        let Some(record) = self.lay.get(layout) else {
                            break;
                        };
                        layout = match *step {
                            Step::Payload => match record.kind {
                                LayoutKind::Box { payload } => payload,
                                _ => break,
                            },
                            Step::Field(field) => {
                                match record.fields.iter().find(|f| f.ordinal == field) {
                                    Some(f) => f.layout,
                                    None => break,
                                }
                            }
                            Step::Variant(selected, field) => {
                                let Some(tag) = record.tag.as_ref() else {
                                    break;
                                };
                                for variant in &tag.variants {
                                    if variant.ordinal != selected {
                                        for field in &variant.fields {
                                            let mut absent = path[..n].to_vec();
                                            absent.push(Step::Variant(
                                                variant.ordinal,
                                                field.ordinal,
                                            ));
                                            let absent = self.facts.intern(&absent);
                                            st.slots[slot.index()].moved.set(absent, Tri::Yes);
                                        }
                                    } else {
                                        for field in &variant.fields {
                                            let mut present = path[..n].to_vec();
                                            present.push(Step::Variant(selected, field.ordinal));
                                            let present = self.facts.intern(&present);
                                            let s = &mut st.slots[slot.index()];
                                            s.moved.set(present, s.taken.get(present));
                                        }
                                    }
                                }
                                match tag
                                    .variants
                                    .iter()
                                    .find(|v| v.ordinal == selected)
                                    .and_then(|v| v.fields.iter().find(|f| f.ordinal == field))
                                {
                                    Some(f) => f.layout,
                                    None => break,
                                }
                            }
                            Step::Inline(_) => match record.kind {
                                LayoutKind::Inline { element, .. } => element,
                                _ => break,
                            },
                            _ => break,
                        };
                    }
                    self.remember_empty(st, slot);
                }
            }
            Inst::StorageLive { slot } => {
                st.slots[slot.index()] = SlotState {
                    live: Tri::Yes,
                    ..SlotState::DEAD
                };
            }
            Inst::StorageDead { slot, .. } => st.slots[slot.index()] = SlotState::DEAD,
            Inst::Read {
                result,
                address,
                mode,
                ..
            } => {
                if mode == ReadMode::Take
                    && let Some((slot, path)) = self.place(address)
                {
                    st.slots[slot.index()].remove(path, &self.facts.paths());
                    self.remember_empty(st, slot);
                }
                self.create(st, result);
            }
            Inst::Store {
                address,
                value,
                version,
            } => {
                take_credit(&mut st.credits, value);
                if let Some((slot, path)) = self.place(address) {
                    let paths = self.facts.paths();
                    let s = &mut st.slots[slot.index()];
                    s.write(path, &paths, !through_slot(paths.steps(path)));
                    s.versions = Versions::One(version);
                }
            }
            // A sequence write keeps the place's content and names a new
            // version of its root (LIR schema revision 7).
            Inst::SeqReserve {
                address, version, ..
            }
            | Inst::ArenaPush {
                address, version, ..
            }
            | Inst::SeqSetLen {
                address, version, ..
            }
            // So does a write of a builder's open element, whose operand is
            // plain (LIR schema revision 24).
            | Inst::FlatField {
                address, version, ..
            } => {
                if let Some((slot, _)) = self.place(address) {
                    st.slots[slot.index()].owned_empty = false;
                    st.slots[slot.index()].versions = Versions::One(version);
                }
            }
            // An appended element moves into the block (LIR schema revision 19).
            Inst::SeqAppend {
                address,
                value,
                version,
            } => {
                take_credit(&mut st.credits, value);
                if let Some((slot, _)) = self.place(address) {
                    st.slots[slot.index()].owned_empty = false;
                    st.slots[slot.index()].versions = Versions::One(version);
                }
            }
            // An element built in place consumes its fixed fields (LIR schema
            // revision 22).
            Inst::FlatAppend {
                address, version, ..
            }
            | Inst::FlatClose {
                address, version, ..
            } => {
                for v in consumed(unit, self.lay, inst) {
                    take_credit(&mut st.credits, v);
                }
                if let Some((slot, _)) = self.place(address) {
                    st.slots[slot.index()].owned_empty = false;
                    st.slots[slot.index()].versions = Versions::One(version);
                }
            }
            Inst::Replace {
                address,
                value,
                displaced,
                version,
                displaced_version,
                ..
            } => {
                take_credit(&mut st.credits, value);
                if let Some((slot, path)) = self.place(address) {
                    let before = st.slots[slot.index()].init;
                    // The displaced content keeps its pending shells, at
                    // their paths relative to the replaced part.
                    let suffixes: Vec<(Vec<Step>, Tri)> = {
                        let paths = self.facts.paths();
                        let at = paths.steps(path);
                        st.slots[slot.index()]
                            .fields_pending
                            .iter()
                            .filter_map(|(p, t)| {
                                paths.steps(p).strip_prefix(at).map(|suffix| (suffix.to_vec(), t))
                            })
                            .collect()
                    };
                    let mut phases = Tris::EMPTY;
                    for (suffix, t) in suffixes {
                        phases.set(self.facts.intern(&suffix), t);
                    }
                    let paths = self.facts.paths();
                    let whole = paths.steps(path).is_empty();
                    let s = &mut st.slots[slot.index()];
                    s.write(path, &paths, true);
                    s.versions = Versions::One(version);
                    let d = &mut st.slots[displaced.index()];
                    d.init = if whole { before } else { Tri::Yes };
                    d.moved.clear();
                    d.taken.clear();

                    d.fields_pending = phases;
                    d.owned_empty = false;
                    d.versions = Versions::One(displaced_version);
                }
            }
            Inst::Construct { result, .. }
            | Inst::Reuse { result, .. }
            | Inst::Weaken { result, .. }
            | Inst::Recast { result, .. }
            | Inst::SealArena { result, .. }
            | Inst::PayloadReceipt { result, .. }
            | Inst::BorrowSealedRoot { result, .. }
            | Inst::Freeze { result, .. }
            | Inst::Thaw { result, .. }
            | Inst::Closure { result, .. } => {
                for v in consumed(unit, self.lay, inst) {
                    take_credit(&mut st.credits, v);
                }
                self.create(st, result);
            }
            Inst::BeginLoan {
                loan,
                address,
                mutable,
                ..
            } => {
                st.loans[loan.index()] = Tri::Yes;
                if mutable && let Some((slot, path)) = self.place(address) {
                    let state = &mut st.slots[slot.index()];
                    state.expose(path, &self.facts.paths());
                    state.owned_empty = false;
                }
            }
            Inst::EndLoan { loan, .. } => st.loans[loan.index()] = Tri::No,
            Inst::Drop { address, .. } => {
                if let Some((slot, path)) = self.place(address) {
                    st.slots[slot.index()].remove(path, &self.facts.paths());
                    self.remember_empty(st, slot);
                }
            }
            // A constructor-tail hole and its block are both owned; a fill
            // consumes the hole and its value, an abandonment the hole.
            Inst::HoleOpen { result, hole, .. } => {
                self.create(st, result);
                self.create(st, hole);
            }
            Inst::HoleFill { .. } | Inst::HoleAbandon { .. } => {
                for v in consumed(unit, self.lay, inst) {
                    take_credit(&mut st.credits, v);
                }
            }
            Inst::Dup { value } => *st.credits.entry(value).or_insert(0) += 1,
            // A semantic copy's mark reads its value (MEM-6).
            Inst::Share { .. } => {}
            Inst::Release { value, .. }
            | Inst::Forget { value }
            | Inst::Dispose { token: value } => take_credit(&mut st.credits, value),
            Inst::Reset { result, value, .. } => {
                take_credit(&mut st.credits, value);
                self.create(st, result);
            }
            Inst::RegionEnter { result, region } => {
                st.regions[region.index()] = Tri::Yes;
                self.create(st, result);
            }
            Inst::RegionReserve { result, .. }
            | Inst::ArenaOpen { result, .. }
            | Inst::ArenaToken { result, .. } => self.create(st, result),
            Inst::RegionExit { mark, region, .. } => {
                take_credit(&mut st.credits, mark);
                st.regions[region.index()] = Tri::No;
            }
        }
    }
}

fn take_credit(credits: &mut BTreeMap<ValueId, u32>, v: ValueId) {
    if let Some(n) = credits.get_mut(&v) {
        *n -= 1;
        if *n == 0 {
            credits.remove(&v);
        }
    }
}

// ---------------------------------------------------------------------------
// Slot-content liveness

/// A dense set of small indices.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SlotSet(Vec<u64>);

impl SlotSet {
    fn insert(&mut self, i: usize) {
        let (w, b) = (i / 64, i % 64);
        if w >= self.0.len() {
            self.0.resize(w + 1, 0);
        }
        self.0[w] |= 1 << b;
    }
    fn remove(&mut self, i: usize) {
        if let Some(w) = self.0.get_mut(i / 64) {
            *w &= !(1 << (i % 64));
        }
    }
    fn contains(&self, i: usize) -> bool {
        self.0.get(i / 64).is_some_and(|w| w & (1 << (i % 64)) != 0)
    }
    fn union(&mut self, other: &SlotSet) -> bool {
        if self.0.len() < other.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            let next = *a | *b;
            changed |= next != *a;
            *a = next;
        }
        changed
    }
    fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &bits)| {
            (0..64)
                .filter(move |b| bits & (1 << b) != 0)
                .map(move |b| w * 64 + b)
        })
    }
}

/// Which slots' current content some path from a point still needs. A use
/// reads or writes the content, prepares a projection into it, ends or
/// begins a loan on it, or uses a view of it; a whole store, a whole take
/// and a whole drop end the content. Failure successors count, so content a
/// cleanup still needs stays live up to the failing operation.
///
/// Only the tracked slots are in the sets, by their ordinal among them: a
/// set is as wide as the slots someone asks about, not as the unit.
pub(crate) struct SlotLiveness {
    /// The tracked slots, ascending.
    pub tracked: Vec<SlotId>,
    /// Per slot, its ordinal in `tracked`.
    ordinal: Vec<Option<u32>>,
    pub live_in: Vec<SlotSet>,
    pub live_out: Vec<SlotSet>,
}

impl SlotLiveness {
    fn insert(&self, set: &mut SlotSet, slot: SlotId) {
        if let Some(i) = self.ordinal[slot.index()] {
            set.insert(i as usize);
        }
    }
    fn remove(&self, set: &mut SlotSet, slot: SlotId) {
        if let Some(i) = self.ordinal[slot.index()] {
            set.remove(i as usize);
        }
    }
    pub fn contains(&self, set: &SlotSet, slot: SlotId) -> bool {
        self.ordinal[slot.index()].is_some_and(|i| set.contains(i as usize))
    }
    /// The slots of a set, ascending.
    pub fn slots<'a>(&'a self, set: &'a SlotSet) -> impl Iterator<Item = SlotId> + 'a {
        set.iter().map(|i| self.tracked[i])
    }
}

pub(crate) struct Uses {
    pub used: Vec<SlotId>,
    pub killed: Vec<SlotId>,
}

impl<S: Stage> Flow<'_, S> {
    fn view_slot(&self, v: ValueId) -> Option<SlotId> {
        self.facts.backing.get(v.index()).copied().flatten()
    }

    /// The slots an instruction uses and ends.
    pub fn inst_uses(&self, inst: &Inst) -> Uses {
        let unit = self.unit;
        let mut used = Vec::new();
        let mut killed = Vec::new();
        let slot = |a: AddressId| self.facts.slot_of(a);
        let addr_use = |a: AddressId, used: &mut Vec<SlotId>| match self
            .facts
            .places
            .get(a.index())
            .and_then(|p| p.as_ref())
            .map(|p| p.root)
        {
            Some(Root::Slot(s)) => used.push(s),
            Some(Root::Value(v)) => used.extend(self.view_slot(v)),
            None => {}
        };
        match *inst {
            Inst::Address { address, .. } => {
                // A preparation with a projection reads the content (bounds
                // and tags); a whole-slot preparation checks nothing.
                if self.facts.places[address.index()]
                    .as_ref()
                    .is_some_and(|p| !p.path.is_empty() || matches!(p.root, Root::Value(_)))
                {
                    addr_use(address, &mut used);
                }
            }
            Inst::Read { address, mode, .. } => {
                addr_use(address, &mut used);
                if mode == ReadMode::Take
                    && let Some((s, path)) = slot(address)
                    && path.is_empty()
                {
                    killed.push(s);
                }
            }
            Inst::Copy { address, .. }
            | Inst::FlatCopy { address, .. }
            | Inst::FlatView { address, .. }
            | Inst::FlatLength { address, .. }
            | Inst::FlatNext { address, .. }
            | Inst::FinalizeBegin { address, .. }
            | Inst::Defuse { address }
            | Inst::FinalizeEnd { address, .. }
            | Inst::Replace { address, .. }
            | Inst::BeginLoan { address, .. }
            // A sequence write writes the content its place holds.
            | Inst::SeqReserve { address, .. }
            | Inst::ArenaPush { address, .. }
            | Inst::SeqSetLen { address, .. }
            | Inst::SeqAppend { address, .. } => addr_use(address, &mut used),
            Inst::Store { address, .. } => match slot(address) {
                Some((s, path)) if path.is_empty() => killed.push(s),
                _ => addr_use(address, &mut used),
            },
            Inst::Drop { address, .. } => match slot(address) {
                Some((s, path)) if path.is_empty() => killed.push(s),
                _ => addr_use(address, &mut used),
            },
            Inst::EndLoan { loan, .. } => used.extend(self.facts.loan_slot[loan.index()]),
            Inst::StorageDead { slot, .. } => killed.push(slot),
            Inst::BindOrigin { .. } | Inst::BindFields { .. } | Inst::EndOrigin { .. } => {}
            _ => {}
        }
        // Any use of a view uses the storage it reads.
        for v in crate::liveness::inst_uses(unit, inst) {
            used.extend(self.view_slot(v));
        }
        Uses { used, killed }
    }

    pub fn terminator_uses(&self, term: &Terminator) -> Vec<SlotId> {
        let unit = self.unit;
        let mut used = Vec::new();
        if let Terminator::Call { call, .. } = *term {
            for (address, _) in unit.tables.calls[call.index()].writebacks(&unit.pools) {
                match self.facts.places[address.index()].as_ref().map(|p| p.root) {
                    Some(Root::Slot(s)) => used.push(s),
                    Some(Root::Value(v)) => used.extend(self.view_slot(v)),
                    None => {}
                }
            }
        }
        for v in crate::liveness::terminator_uses(unit, term) {
            used.extend(self.view_slot(v));
        }
        for e in Terminator::normal_edges(term, &unit.pools) {
            for &a in unit.tables.edges[e.index()]
                .arguments
                .get(&unit.pools.values)
                .unwrap_or(&[])
            {
                used.extend(self.view_slot(a));
            }
        }
        used
    }

    fn failure_needs(&self, live: &SlotLiveness, f: FailureId) -> SlotSet {
        if f == PENDING_FAILURE {
            return SlotSet::default();
        }
        let e = self.unit.tables.failures[f.index()].cleanup;
        self.edge_needs(live, e)
    }

    fn edge_needs(&self, live: &SlotLiveness, e: EdgeId) -> SlotSet {
        let edge = &self.unit.tables.edges[e.index()];
        let mut out = live.live_in[edge.target.index()].clone();
        for &a in edge.arguments.get(&self.unit.pools.values).unwrap_or(&[]) {
            if let Some(s) = self.view_slot(a) {
                live.insert(&mut out, s);
            }
        }
        out
    }

    /// Live just before the terminator, its failure successor included.
    fn exit_live(&self, live: &SlotLiveness, b: BlockId) -> SlotSet {
        let mut set = live.live_out[b.index()].clone();
        let term = &self.unit.terminators[b.index()];
        for f in term.failure_edges(&self.unit.tables) {
            set.union(&self.failure_needs(live, f));
        }
        for s in self.terminator_uses(term) {
            live.insert(&mut set, s);
        }
        set
    }

    fn step_back(&self, live: &SlotLiveness, inst: &Inst, set: &mut SlotSet) {
        let uses = self.inst_uses(inst);
        for s in uses.killed {
            live.remove(set, s);
        }
        for s in uses.used {
            live.insert(set, s);
        }
        if let Some(f) = Inst::failure(inst) {
            set.union(&self.failure_needs(live, f));
        }
    }

    /// The liveness of the slots `tracked` admits.
    pub fn slot_liveness(&self, tracked: impl Fn(SlotId) -> bool) -> SlotLiveness {
        let n = self.unit.blocks.len();
        let tracked: Vec<SlotId> = (0..self.unit.slots.len() as u32)
            .map(SlotId)
            .filter(|&s| tracked(s))
            .collect();
        let mut ordinal = vec![None; self.unit.slots.len()];
        for (i, s) in tracked.iter().enumerate() {
            ordinal[s.index()] = Some(i as u32);
        }
        let mut live = SlotLiveness {
            tracked,
            ordinal,
            live_in: vec![SlotSet::default(); n],
            live_out: vec![SlotSet::default(); n],
        };
        let mut changed = true;
        while changed {
            changed = false;
            for b in (0..n).rev() {
                let block = BlockId(b as u32);
                let mut out = SlotSet::default();
                for e in Terminator::normal_edges(&self.unit.terminators[b], &self.unit.pools) {
                    out.union(&self.edge_needs(&live, e));
                }
                changed |= live.live_out[b].union(&out);
                let mut set = self.exit_live(&live, block);
                for inst in self.code[b].iter().rev() {
                    self.step_back(&live, inst, &mut set);
                }
                changed |= live.live_in[b].union(&set);
            }
        }
        live
    }
}

// ---------------------------------------------------------------------------
// Failure cleanup synthesis

/// The scope nesting depth of `scope` in the unit's scope forest.
fn depth<S>(unit: &LirUnit<S>, scope: ScopeId) -> usize {
    let mut d = 0;
    let mut at = unit.scopes.get(scope.index()).and_then(|s| s.parent);
    while let Some(p) = at {
        d += 1;
        if d > unit.scopes.len() {
            break;
        }
        at = unit.scopes.get(p.index()).and_then(|s| s.parent);
    }
    d
}

fn ancestor_or_self<S>(unit: &LirUnit<S>, outer: ScopeId, inner: ScopeId) -> bool {
    let mut at = Some(inner);
    for _ in 0..=unit.scopes.len() {
        match at {
            Some(s) if s == outer => return true,
            Some(s) => at = unit.scopes.get(s.index()).and_then(|x| x.parent),
            None => return false,
        }
    }
    false
}

/// The body ordinal and site that synthesized cleanup anchors name.
fn anchor_site<S>(unit: &LirUnit<S>, at: SourceAnchorId) -> (u32, hirsel_lir::ir::SiteRange) {
    let a = &unit.tables.source_anchors[at.index()];
    (a.body, a.site)
}

/// A failure successor whose cleanup releases everything `state` owns:
/// active loans end, pending owned values are released, initialized slots
/// drop in TIR's nested order (inner scopes first, temporaries before names,
/// each in descending slot order), entered regions exit, then the failure
/// propagates. A live call admission retires before caller cleanup, whose
/// source finalizers admit their own invocations. None when the state holds
/// something such a cleanup cannot express (a pending value with user-visible
/// destruction, a region whose mark is not pending, or slots of unrelated scopes).
pub(crate) fn synthesize_failure<S: Stage>(
    ed: &mut Editor<S>,
    lay: Lay<'_>,
    facts: &mut Facts,
    state: &State,
    at: SourceAnchorId,
    shared: Option<&mut SharedChains>,
) -> Option<FailureId> {
    synthesize_failure_reporting::<S, false>(ed, lay, facts, state, at, shared, |_, _, _, _| {})
}

fn synthesize_failure_reporting<S: Stage, const TRACE: bool>(
    ed: &mut Editor<S>,
    lay: Lay<'_>,
    facts: &mut Facts,
    state: &State,
    at: SourceAnchorId,
    mut shared: Option<&mut SharedChains>,
    mut observed: impl FnMut(BlockId, AddressId, CleanupAnchorId, bool),
) -> Option<FailureId> {
    let unit = &ed.unit;
    let mut pending = Vec::new();
    let mut marks = Vec::new();
    for (&v, &n) in &state.credits {
        if n == 0 {
            continue;
        }
        let ty = unit.values[v.index()].ty;
        match unit.types[ty.index()].repr {
            ValueRepr::RegionMark => marks.push(v),
            ValueRepr::Token => return None,
            ValueRepr::Block | ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle
                if !destructible_type(unit, lay, ty) && n == 1 =>
            {
                pending.push(v)
            }
            _ => return None,
        }
    }
    let mut regions = Vec::new();
    for (r, t) in state.regions.lacking(DEAD) {
        match t {
            Tri::No => {}
            Tri::Maybe => return None,
            Tri::Yes => {
                let mark = facts.marks[r]?;
                if !marks.contains(&mark) {
                    return None;
                }
                regions.push((RegionInstanceId(r as u32), mark));
            }
        }
    }
    if marks.len() != regions.len() {
        return None;
    }
    let mut drops: Vec<(SlotId, Tri)> = Vec::new();
    for (s, st) in state.slots.lacking(EMPTY) {
        let id = SlotId(s as u32);
        let slot = &unit.slots[s];
        if !st.init.may() || !owns_type(unit, lay, slot.ty) {
            continue;
        }
        match (facts.passing_of(id), slot.kind) {
            (Some(Passing::Mut | Passing::Borrow), _)
            | (_, SlotKind::Capture(_) | SlotKind::ReturnSlot | SlotKind::ReceiptResult) => {
                continue;
            }
            _ => {}
        }
        if st.live != Tri::Yes && st.live != Tri::Maybe {
            continue;
        }
        drops.push((id, st.init));
    }
    drops.sort_by_key(|&(s, _)| {
        let info = &unit.slots[s.index()];
        (
            std::cmp::Reverse(depth(unit, info.scope)),
            !info.kind.is_temporary(),
            std::cmp::Reverse(s.0),
        )
    });
    for w in drops.windows(2) {
        let (a, b) = (
            unit.slots[w[0].0.index()].scope,
            unit.slots[w[1].0.index()].scope,
        );
        if !ancestor_or_self(unit, b, a) {
            return None;
        }
    }
    let loans: Vec<(LoanId, Tri)> = state
        .loans
        .lacking(DEAD)
        .into_iter()
        .map(|(l, t)| (LoanId(l as u32), *t))
        .collect();
    let (body, site) = anchor_site(unit, at);
    // Build the chain back to front: each link's failure continues with the
    // rest of the chain, as a cleanup operation finishes before failing.
    let active = match state.invocation {
        Invocation::Inactive => None,
        Invocation::Active(anchor) => Some(anchor),
        Invocation::Ambiguous => return None,
    };
    let tail = match shared
        .as_deref()
        .and_then(|s| s.tails.get(&active).copied())
    {
        Some(tail) => tail,
        None => {
            let tail = ed.block(BlockKind::FailureCleanup, &[], at);
            ed.unit.terminators[tail.index()] = Terminator::Propagate;
            if let Some(s) = shared.as_deref_mut() {
                s.tails.insert(active, tail);
            }
            tail
        }
    };
    let mut next = tail;
    let link = |ed: &mut Editor<S>, inst: Inst, next: BlockId| -> BlockId {
        let b = ed.block(BlockKind::FailureCleanup, &[], at);
        let e = ed.edge(next, &[]);
        ed.unit.terminators[b.index()] = Terminator::Jump(e);
        let fe = ed.edge(next, &[]);
        let f = ed.failure(fe, at);
        let mut inst = inst;
        if let Some(slot) = Inst::failure_mut(&mut inst) {
            *slot = f;
        }
        ed.code[b.index()].push(inst);
        b
    };
    // A link already built for the same step from the same state continues
    // at the same block; otherwise `make` builds it. Its anchor is made only
    // with the block, so every anchor stays discharged.
    let mut step = |ed: &mut Editor<S>,
                    key: SharedLink,
                    next: BlockId,
                    make: &mut dyn FnMut(&mut Editor<S>, BlockId) -> BlockId|
     -> BlockId {
        let Some(s) = shared.as_deref_mut() else {
            return make(ed, next);
        };
        if let Some(&b) = s.links.get(&(key.clone(), next)) {
            return b;
        }
        let b = make(ed, next);
        s.links.insert((key, next), b);
        b
    };
    for &(region, mark) in regions.iter().rev() {
        next = step(
            ed,
            SharedLink::Region(region, mark),
            next,
            &mut |ed, next| {
                link(
                    ed,
                    Inst::RegionExit {
                        mark,
                        region,
                        failure: PENDING_FAILURE,
                    },
                    next,
                )
            },
        );
    }
    for &(slot, init) in drops.iter().rev() {
        let address = ed.whole(facts, slot);
        let key = SharedLink::Drop(address, SharedState::of(&state.slots[slot.index()]));
        next = step(ed, key, next, &mut |ed, next| {
            let anchor = ed.cleanup_anchor(CleanupAnchor {
                body,
                cleanup: STATE_RELEASE,
                action: slot.0,
                site,
            });
            link(
                ed,
                Inst::Drop {
                    address,
                    if_live: init != Tri::Yes,
                    anchor,
                    failure: PENDING_FAILURE,
                },
                next,
            )
        });
        if TRACE {
            let Inst::Drop {
                anchor, if_live, ..
            } = ed.code[next.index()][0]
            else {
                unreachable!("shared cleanup drop link")
            };
            observed(next, address, anchor, if_live);
        }
    }
    for &v in pending.iter().rev() {
        next = step(ed, SharedLink::Release(v), next, &mut |ed, next| {
            let anchor = ed.cleanup_anchor(CleanupAnchor {
                body,
                cleanup: STATE_RELEASE,
                action: u32::MAX - v.0,
                site,
            });
            link(
                ed,
                Inst::Release {
                    value: v,
                    layout: lay
                        .payload(value_layout(&ed.unit, v))
                        .unwrap_or_else(|| value_layout(&ed.unit, v)),
                    live: MaskRef::All,
                    anchor,
                    failure: PENDING_FAILURE,
                },
                next,
            )
        });
    }
    if !loans.is_empty() {
        next = step(
            ed,
            SharedLink::Ends(loans.clone()),
            next,
            &mut |ed, next| {
                let b = ed.block(BlockKind::FailureCleanup, &[], at);
                for &(loan, t) in &loans {
                    ed.code[b.index()].push(Inst::EndLoan {
                        loan,
                        if_active: t != Tri::Yes,
                    });
                }
                let e = ed.edge(next, &[]);
                ed.unit.terminators[b.index()] = Terminator::Jump(e);
                b
            },
        );
    }
    if active.is_some() {
        // The failed preparation belongs to the admitted call. Retire it
        // before releasing caller storage: a resource drop can enter a source
        // finalizer, which requires its own independent admission.
        let retired = ed.block(BlockKind::FailureCleanup, &[], at);
        ed.code[retired.index()].push(Inst::FrameLeave);
        let edge = ed.edge(next, &[]);
        ed.unit.terminators[retired.index()] = Terminator::Jump(edge);
        next = retired;
    }
    let head = ed.edge(next, &[]);
    Some(ed.failure(head, at))
}

/// Synthesized failure-cleanup blocks shared between failure edges that can
/// never be taken: those of early releases, whose content has no
/// user-visible destruction and so cannot fail to release. Their chains,
/// built per release, would otherwise each repeat a drop of everything still
/// live, the frame's long-lived resources included. A link is keyed by what
/// it runs, the state of the slot it drops and the block it continues at, so
/// chains meet at their common tail as the import's do; a shared link keeps
/// the anchor site of the release that first needed it.
#[derive(Default)]
pub(crate) struct SharedChains {
    tails: BTreeMap<Option<SourceAnchorId>, BlockId>,
    links: HashMap<(SharedLink, BlockId), BlockId>,
}

#[derive(Clone, Eq, PartialEq, Hash)]
enum SharedLink {
    Region(RegionInstanceId, ValueId),
    Drop(AddressId, SharedState),
    Release(ValueId),
    Ends(Vec<(LoanId, Tri)>),
}

/// The part of a dropped slot's state its drop depends on; versions are
/// joined where links meet, as at the import's shared links.
#[derive(Clone, Eq, PartialEq, Hash)]
struct SharedState {
    live: Tri,
    init: Tri,
    moved: Tris,
    taken: Tris,
    fields_pending: Tris,
    owned_empty: bool,
}

impl SharedState {
    fn of(s: &SlotState) -> Self {
        Self {
            live: s.live,
            init: s.init,
            moved: s.moved.clone(),
            taken: s.taken.clone(),
            fields_pending: s.fields_pending.clone(),
            owned_empty: s.owned_empty,
        }
    }
}

// ---------------------------------------------------------------------------
// Release at last use

/// Failure sites share the tails of their cleanup chains (the import keys a
/// chain block by what it runs and the state of the slot it drops). An early
/// release changes that state on the paths after it, so a tail shared by
/// sites before and after a release would see its slot conditionally full.
/// Each cleanup block is copied once per state its incoming paths agree on:
/// the initialization of every eligible slot it or any block after it drops.
/// The settling below then keeps a drop in one copy and deletes it in
/// another, as each site's own chain would, and copies meet again where
/// what is left to drop agrees. Blocks are visited in topological order of
/// the cleanup DAG, each from the exact states of its incoming edges.
fn split_cleanups(
    ed: &mut Editor<Rc>,
    lay: Lay<'_>,
    facts: &Facts,
    eligible: impl Fn(SlotId) -> bool,
    mut cloned: impl FnMut(BlockId),
) {
    let n = ed.unit.blocks.len();
    let cleanup =
        |unit: &LirUnit<Rc>, b: BlockId| unit.blocks[b.index()].kind == BlockKind::FailureCleanup;
    let mut incoming: Vec<Vec<(EdgeId, State)>> = vec![Vec::new(); n];
    let succ = {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts,
        };
        let ins = flow.solve();
        for (b, state) in ins.into_iter().enumerate() {
            let block = BlockId(b as u32);
            let Some(state) = state.filter(|_| !cleanup(&ed.unit, block)) else {
                continue;
            };
            for (e, out) in flow.block_after(block, state, |_, _| {}) {
                let target = ed.unit.tables.edges[e.index()].target;
                if cleanup(&ed.unit, target) {
                    incoming[target.index()].push((e, out));
                }
            }
        }
        flow.successors()
    };
    // Kahn's order over the cleanup blocks; a cycle among them (never
    // produced) leaves the unit as it is.
    let mut preds = vec![0usize; n];
    for b in (0..n)
        .map(|b| BlockId(b as u32))
        .filter(|&b| cleanup(&ed.unit, b))
    {
        for &t in &succ[b.index()] {
            preds[t.index()] += 1;
        }
    }
    let mut ready: Vec<BlockId> = (0..n)
        .map(|b| BlockId(b as u32))
        .filter(|&b| cleanup(&ed.unit, b) && preds[b.index()] == 0)
        .collect();
    let mut order = Vec::new();
    while let Some(b) = ready.pop() {
        order.push(b);
        for &t in &succ[b.index()] {
            preds[t.index()] -= 1;
            if preds[t.index()] == 0 && cleanup(&ed.unit, t) {
                ready.push(t);
            }
        }
    }
    if order.len()
        != (0..n)
            .filter(|&b| cleanup(&ed.unit, BlockId(b as u32)))
            .count()
    {
        return;
    }
    // Per cleanup block, the eligible slots it and the blocks after it drop.
    let mut downstream: Vec<Vec<SlotId>> = vec![Vec::new(); n];
    for &b in order.iter().rev() {
        let mut slots: Vec<SlotId> = ed.code[b.index()]
            .iter()
            .filter_map(|inst| match *inst {
                Inst::Drop { address, .. } => facts
                    .slot_of(address)
                    .filter(|(slot, path)| path.is_empty() && eligible(*slot))
                    .map(|(slot, _)| slot),
                _ => None,
            })
            .collect();
        for &t in &succ[b.index()] {
            slots.extend_from_slice(&downstream[t.index()]);
        }
        slots.sort();
        slots.dedup();
        downstream[b.index()] = slots;
    }
    for b in order {
        let arrivals = std::mem::take(&mut incoming[b.index()]);
        if arrivals.is_empty() {
            continue;
        }
        let plain = ed.unit.blocks[b.index()].parameters.len == 0
            && matches!(
                ed.unit.terminators[b.index()],
                Terminator::Jump(_) | Terminator::Propagate | Terminator::Unreachable
            )
            && ed.code[b.index()]
                .iter()
                .all(|inst| Inst::defs(inst).is_empty());
        // The incoming edges by the initialization of each slot still to
        // drop, in the order the groups are first seen.
        let mut groups: Vec<(Vec<Tri>, Vec<(EdgeId, State)>)> = Vec::new();
        for (e, state) in arrivals {
            let key: Vec<Tri> = if plain {
                downstream[b.index()]
                    .iter()
                    .map(|s| state.slots[s.index()].init)
                    .collect()
            } else {
                Vec::new()
            };
            match groups
                .iter_mut()
                .find(|(k, members)| *k == key && members[0].1.invocation == state.invocation)
            {
                Some((_, members)) => members.push((e, state)),
                None => groups.push((key, vec![(e, state)])),
            }
        }
        for (k, (_, members)) in groups.into_iter().enumerate() {
            let target = if k == 0 {
                b
            } else {
                cloned(b);
                clone_cleanup(ed, b)
            };
            let mut states = Vec::with_capacity(members.len());
            for (e, state) in members {
                ed.unit.tables.edges[e.index()].target = target;
                states.push(state);
            }
            let joined = join_all(states);
            let flow = Flow {
                unit: &ed.unit,
                code: &ed.code,
                lay,
                facts,
            };
            let outs = flow.block_after(target, joined.expect("a group has a member"), |_, _| {});
            for (e, out) in outs {
                let t = ed.unit.tables.edges[e.index()].target;
                if cleanup(&ed.unit, t) {
                    incoming[t.index()].push((e, out));
                }
            }
        }
    }
}

/// A copy of a parameterless cleanup block with its own edges and failure
/// edges to the same successors.
fn clone_cleanup(ed: &mut Editor<Rc>, b: BlockId) -> BlockId {
    fn copy_edge(ed: &mut Editor<Rc>, e: EdgeId) -> EdgeId {
        let edge = ed.unit.tables.edges[e.index()].clone();
        let args = edge
            .arguments
            .get(&ed.unit.pools.values)
            .unwrap_or(&[])
            .to_vec();
        ed.edge(edge.target, &args)
    }
    let (kind, anchor) = (
        ed.unit.blocks[b.index()].kind,
        ed.unit.blocks[b.index()].anchor,
    );
    let c = ed.block(kind, &[], anchor);
    let mut code = ed.code[b.index()].clone();
    for inst in &mut code {
        if let Some(f) = Inst::failure_mut(inst)
            && *f != PENDING_FAILURE
        {
            let old = ed.unit.tables.failures[f.index()].clone();
            let cleanup = copy_edge(ed, old.cleanup);
            *f = FailureId(ed.unit.tables.failures.len() as u32);
            ed.unit.tables.failures.push(FailureEdge { cleanup, ..old });
        }
    }
    ed.code[c.index()] = code;
    ed.unit.terminators[c.index()] = match ed.unit.terminators[b.index()].clone() {
        Terminator::Jump(e) => Terminator::Jump(copy_edge(ed, e)),
        other => other,
    };
    c
}

/// Slots whose content insertion may release early: owned, not anchored
/// (no user-visible destruction and no `Shared` root, S-14), and owned by
/// this frame.
fn eligible<S>(unit: &LirUnit<S>, lay: Lay<'_>, facts: &Facts) -> Vec<bool> {
    unit.slots
        .iter()
        .enumerate()
        .map(|(s, slot)| {
            let id = SlotId(s as u32);
            owns_type(unit, lay, slot.ty)
                && !anchored_type(unit, lay, slot.ty)
                && matches!(
                    slot.kind,
                    SlotKind::Local
                        | SlotKind::Temporary
                        | SlotKind::Parameter(_)
                        | SlotKind::LoopCarried(_)
                        | SlotKind::Displaced
                )
                && !matches!(facts.passing_of(id), Some(Passing::Mut | Passing::Borrow))
        })
        .collect()
}

/// One early release, placed before instruction `index` of `block`.
struct Point {
    block: BlockId,
    index: usize,
    slot: SlotId,
}

struct ReleaseOrigins<const TRACK: bool> {
    rows: Vec<Vec<Option<u32>>>,
    moves: crate::observation::Moves,
    cloned: Vec<BlockId>,
    retirements: Vec<(
        BlockId,
        u32,
        AddressId,
        CleanupAnchorId,
        Option<hirsel_lir::ownership_trace::Moment>,
    )>,
    generated:
        BTreeMap<(BlockId, AddressId), (hirsel_lir::ownership_trace::Point, CleanupAnchorId, bool)>,
}
impl<const TRACK: bool> ReleaseOrigins<TRACK> {
    fn new(unit: &LirUnit<Rc>) -> Self {
        Self {
            rows: if TRACK {
                unit.blocks
                    .iter()
                    .map(|block| (0..block.instructions.len).map(Some).collect())
                    .collect()
            } else {
                Vec::new()
            },
            moves: crate::observation::Moves {
                sources: if TRACK {
                    (0..unit.blocks.len())
                        .map(|at| BlockId(at as u32))
                        .collect()
                } else {
                    Vec::new()
                },
                ends: if TRACK {
                    (0..unit.blocks.len())
                        .map(|at| BlockId(at as u32))
                        .collect()
                } else {
                    Vec::new()
                },
                empty: Default::default(),
            },
            cloned: Vec::new(),
            retirements: Vec::new(),
            generated: BTreeMap::new(),
        }
    }
    fn insert(&mut self, block: usize, at: usize, count: usize) {
        if TRACK {
            self.rows[block].splice(at..at, std::iter::repeat_n(None, count));
        }
    }
    fn remove(&mut self, block: usize, at: usize) {
        if TRACK {
            self.rows[block].remove(at);
        }
    }
    fn complete(&mut self, ed: &Editor<Rc>) {
        if TRACK {
            self.rows.resize_with(ed.code.len(), Vec::new);
            for at in self.moves.sources.len()..ed.code.len() {
                self.moves.sources.push(BlockId(at as u32));
            }
            for (origins, code) in self.rows.iter_mut().zip(&ed.code) {
                origins.resize(code.len(), None);
            }
        }
    }
}

fn release_at_last_use(unit: LirUnit<Rc>, table: &LayoutTable) -> Result<LirUnit<Rc>, PassError> {
    let (editor, _) = release_at_last_use_with::<false>(unit, table, None)?;
    Ok(editor.finish())
}

fn release_at_last_use_with<const TRACK: bool>(
    unit: LirUnit<Rc>,
    table: &LayoutTable,
    observed: Option<&hirsel_lir::ownership_trace::Plan>,
) -> Result<(Editor<Rc>, ReleaseOrigins<TRACK>), PassError> {
    let mut excluded = BTreeSet::new();
    loop {
        let lost = match release_pass::<TRACK>(unit.clone(), table, &excluded, observed)? {
            Ok(result) => return Ok(result),
            Err(lost) => lost,
        };
        // A release could not stand for every anchor it superseded: keep
        // those slots' semantic drops.
        let before = excluded.len();
        excluded.extend(lost);
        if excluded.len() == before {
            return Err(PassError::Malformed(
                "a cleanup anchor lost its discharge".into(),
            ));
        }
    }
}

/// One attempt: the unit, or the slots whose early release could not stand
/// (no expressible failure cleanup, or an anchor left undischarged).
fn release_pass<const TRACK: bool>(
    unit: LirUnit<Rc>,
    table: &LayoutTable,
    excluded: &BTreeSet<SlotId>,
    observed: Option<&hirsel_lir::ownership_trace::Plan>,
) -> Result<Result<(Editor<Rc>, ReleaseOrigins<TRACK>), BTreeSet<SlotId>>, PassError> {
    let mut origins = ReleaseOrigins::<TRACK>::new(&unit);
    let lay = Lay(table);
    let mut facts = Facts::new(&unit, table);
    let mut ed = Editor::new(unit);
    // Whole-slot addresses first: creating one prepares it in the entry
    // block, which must happen before any position is recorded.
    for s in 0..ed.unit.slots.len() {
        if owns_type(&ed.unit, lay, ed.unit.slots[s].ty) {
            ed.whole_reporting(&mut facts, SlotId(s as u32), |b, at| {
                origins.insert(b, at, 1)
            });
        }
    }
    let ok = eligible(&ed.unit, lay, &facts);
    let eligible = |s: SlotId| ok[s.index()] && !excluded.contains(&s);

    // 1. Death points from slot liveness over the input.
    let mut points = Vec::new();
    {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts: &facts,
        };
        let live = flow.slot_liveness(eligible);
        let mut exit_live = Vec::new();
        for b in 0..ed.unit.blocks.len() {
            exit_live.push(flow.exit_live(&live, BlockId(b as u32)));
        }
        let mut preds: Vec<Vec<BlockId>> = vec![Vec::new(); ed.unit.blocks.len()];
        for b in 0..ed.unit.blocks.len() {
            for e in Terminator::normal_edges(&ed.unit.terminators[b], &ed.unit.pools) {
                preds[ed.unit.tables.edges[e.index()].target.index()].push(BlockId(b as u32));
            }
        }
        for b in 0..ed.unit.blocks.len() {
            let block = BlockId(b as u32);
            if ed.unit.blocks[b].kind != BlockKind::Body
                || matches!(ed.unit.terminators[b], Terminator::Unreachable)
            {
                // A sealed Unreachable is an exhaustive switch's impossible
                // default, not an executable exit owing cleanup. An early
                // conditional release there would later split the block and
                // sever that switch's proof of the terminator.
                continue;
            }
            // Dies on entry: a predecessor still needed it.
            let dying: BTreeSet<SlotId> = preds[b]
                .iter()
                .flat_map(|p| live.slots(&exit_live[p.index()]))
                .filter(|&slot| !live.contains(&live.live_in[b], slot))
                .collect();
            points.extend(dying.into_iter().map(|slot| Point {
                block,
                index: 0,
                slot,
            }));
            // Dies after its last use inside the block: walking back from
            // the terminator, `after` is what is live after each instruction.
            let mut after = flow.exit_live(&live, block);
            for (i, inst) in ed.code[b].iter().enumerate().rev() {
                let uses = flow.inst_uses(inst);
                // A use after which nothing needs the content, or a whole
                // definition nothing reads.
                let defined = match *inst {
                    Inst::Store { address, .. } | Inst::Replace { address, .. } => facts
                        .slot_of(address)
                        .filter(|(_, p)| p.is_empty())
                        .map(|(s, _)| s),
                    _ => None,
                };
                let ended: BTreeSet<SlotId> = uses
                    .used
                    .iter()
                    .copied()
                    .filter(|s| !uses.killed.contains(s))
                    .chain(defined)
                    .filter(|&slot| eligible(slot) && !live.contains(&after, slot))
                    .collect();
                points.extend(ended.into_iter().map(|slot| Point {
                    block,
                    index: i + 1,
                    slot,
                }));
                flow.step_back(&live, inst, &mut after);
            }
        }
    }
    // Insert placeholder drops back to front so indices stay valid.
    points.sort_by_key(|p| (p.block, std::cmp::Reverse(p.index), p.slot));
    let mut anchors_of: BTreeMap<SlotId, Vec<CleanupAnchorId>> = BTreeMap::new();
    for code in &ed.code {
        for inst in code {
            if let Inst::Drop {
                address, anchor, ..
            } = *inst
                && let Some((s, path)) = facts.slot_of(address)
                && path.is_empty()
            {
                anchors_of.entry(s).or_default().push(anchor);
            }
        }
    }
    for p in &points {
        let Some(&anchor) = anchors_of.get(&p.slot).and_then(|a| a.first()) else {
            continue;
        };
        let address = ed.whole_reporting(&mut facts, p.slot, |b, at| origins.insert(b, at, 1));
        // A silent owner dying at a join is discharged on its incoming
        // edges, before their different moved sets lose their correlation.
        // Keep the transport after cleanup so no joined value is read early.
        if p.index == 0 {
            let edges: BTreeSet<_> = ed
                .unit
                .terminators
                .iter()
                .flat_map(|t| Terminator::normal_edges(t, &ed.unit.pools))
                .filter(|e| ed.unit.tables.edges[e.index()].target == p.block)
                .collect();
            for edge in edges {
                let old = ed.unit.tables.edges[edge.index()].clone();
                let at = ed.unit.blocks[p.block.index()].anchor;
                let split = ed.block(BlockKind::Body, &[], at);
                ed.unit.tables.edges[edge.index()].target = split;
                ed.unit.tables.edges[edge.index()].arguments = PoolRange::EMPTY;
                let args = old
                    .arguments
                    .get(&ed.unit.pools.values)
                    .unwrap_or(&[])
                    .to_vec();
                let next = ed.edge(old.target, &args);
                ed.unit.terminators[split.index()] = Terminator::Jump(next);
                ed.code[split.index()].push(Inst::Drop {
                    address,
                    if_live: true,
                    anchor,
                    failure: PENDING_FAILURE,
                });
            }
            continue;
        }
        origins.insert(p.block.index(), p.index, 1);
        ed.code[p.block.index()].insert(
            p.index,
            Inst::Drop {
                address,
                if_live: true,
                anchor,
                failure: PENDING_FAILURE,
            },
        );
    }

    // A version join of content an early release ended on some incoming
    // path names nothing any more: the join goes.
    prune_phis(&mut ed, lay, &mut facts);
    split_cleanups(&mut ed, lay, &facts, eligible, |block| {
        if TRACK {
            origins.cloned.push(block);
        }
    });
    origins.complete(&ed);

    // 2. Settle every drop of an eligible slot by the state it now sees.
    let states = {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts: &facts,
        };
        flow.solve()
    };
    let mut edits: Vec<(BlockId, usize, Settle)> = Vec::new();
    {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts: &facts,
        };
        for (b, state) in states.iter().enumerate() {
            let Some(state) = state.clone() else {
                continue;
            };
            let block = BlockId(b as u32);
            let code = &ed.code[b];
            flow.block(block, state, |i, pre, _| {
                let Some(&Inst::Drop {
                    address, failure, ..
                }) = code.get(i)
                else {
                    return;
                };
                let Some((slot, path)) = facts.slot_of(address) else {
                    return;
                };
                if !path.is_empty() || !eligible(slot) {
                    return;
                }
                let s = &pre.slots[slot.index()];
                let settle = if !s.init.may() {
                    Settle::Delete
                } else if failure == PENDING_FAILURE
                    && s.whole()
                    && s.versions.one().is_some()
                    && flow.unit.types[flow.unit.slots[slot.index()].ty.index()].repr
                        == ValueRepr::Block
                {
                    Settle::Release(s.versions.one())
                } else if s.init == Tri::Yes {
                    Settle::Keep(false)
                } else {
                    Settle::Keep(true)
                };
                edits.push((block, i, settle));
            });
        }
    }
    edits.sort_by_key(|&(b, i, _)| (b, std::cmp::Reverse(i)));
    let mut erased: Vec<(
        BlockId,
        usize,
        u32,
        hirsel_lir::ownership_trace::ErasedCleanup,
    )> = Vec::new();
    let mut releases: Vec<(BlockId, usize)> = Vec::new();
    let mut retirements: Vec<(
        BlockId,
        usize,
        AddressId,
        CleanupAnchorId,
        Option<hirsel_lir::ownership_trace::Moment>,
    )> = Vec::new();
    for (b, i, settle) in edits {
        let code = &mut ed.code[b.index()];
        let Inst::Drop {
            address,
            anchor,
            failure,
            ..
        } = code[i]
        else {
            unreachable!("settled a drop");
        };
        match settle {
            Settle::Delete => {
                let original = TRACK.then(|| origins.rows[b.index()][i]).flatten();
                let observed_drop = original.is_some_and(|old| {
                    observed.is_some_and(|plan| {
                        plan.observations.iter().any(|row| {
                            row.marker.kind == hirsel_lir::ownership_trace::Kind::Drop
                                && matches!(row.point,
                                hirsel_lir::ownership_trace::Point::Instruction { block, index, .. }
                                if block == b && index == old)
                        })
                    })
                });
                for r in erased.iter_mut().filter(|r| r.0 == b && r.1 > i) {
                    r.1 -= 1;
                }
                if observed_drop {
                    erased.push((
                        b,
                        i,
                        original.unwrap(),
                        (&ed.unit.tables.cleanup_anchors[anchor.index()]).into(),
                    ));
                }
                code.remove(i);
                origins.remove(b.index(), i);
                for r in releases.iter_mut().filter(|r| r.0 == b && r.1 > i) {
                    r.1 -= 1;
                }
                for r in retirements.iter_mut().filter(|r| r.0 == b && r.1 > i) {
                    r.1 -= 1;
                }
            }
            Settle::Keep(if_live) => {
                code[i] = Inst::Drop {
                    address,
                    if_live,
                    anchor,
                    failure,
                };
                if failure == PENDING_FAILURE {
                    if TRACK && origins.rows[b.index()][i].is_none() {
                        let moment = if if_live {
                            hirsel_lir::ownership_trace::Moment::IfLive
                        } else {
                            hirsel_lir::ownership_trace::Moment::Before
                        };
                        retirements.push((b, i, address, anchor, Some(moment)));
                    }
                    releases.push((b, i));
                }
            }
            Settle::Release(version) => {
                // The release retains the semantic drop's origin; its preceding
                // take is the physical transport of the same slot content.
                origins.insert(b.index(), i, 1);
                let (slot, _) = facts.slot_of(address).expect("slot drop");
                let Some(reaching) = version else {
                    return Err(PassError::Malformed(format!(
                        "no version reaches {slot:?} at an early release"
                    )));
                };
                let ty = ed.unit.slots[slot.index()].ty;
                let layout = lay
                    .payload(ed.unit.types[ty.index()].layout)
                    .expect("a block slot");
                let v = ed.value(ty, Holding::Owned);
                let code = &mut ed.code[b.index()];
                code[i] = Inst::Read {
                    result: v,
                    address,
                    reaching,
                    mode: ReadMode::Take,
                };
                code.insert(
                    i + 1,
                    Inst::Release {
                        value: v,
                        layout,
                        live: MaskRef::All,
                        anchor,
                        failure: PENDING_FAILURE,
                    },
                );
                for r in releases.iter_mut().filter(|r| r.0 == b && r.1 > i) {
                    r.1 += 1;
                }
                for r in retirements.iter_mut().filter(|r| r.0 == b && r.1 > i) {
                    r.1 += 1;
                }
                if TRACK && origins.rows[b.index()][i + 1].is_none() {
                    retirements.push((b, i + 1, address, anchor, None));
                }
                for r in erased.iter_mut().filter(|r| r.0 == b && r.1 > i) {
                    r.1 += 1;
                }
                releases.push((b, i + 1));
            }
        }
    }

    origins.retirements = retirements
        .into_iter()
        .map(|(block, index, address, anchor, moment)| {
            (block, index as u32, address, anchor, moment)
        })
        .collect();

    // 3. Each early release's failure successor: a cleanup of what the
    // state after it still owns.
    let states = {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts: &facts,
        };
        flow.solve()
    };
    let mut after_states: BTreeMap<(BlockId, usize), State> = BTreeMap::new();
    {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts: &facts,
        };
        let wanted: BTreeSet<_> = releases.iter().copied().collect();
        for (b, state) in states.iter().enumerate() {
            let Some(state) = state.clone() else {
                continue;
            };
            let block = BlockId(b as u32);
            flow.block_after(block, state, |i, post| {
                if wanted.contains(&(block, i)) {
                    after_states.insert((block, i), post.clone());
                }
            });
        }
    }
    let mut lost = BTreeSet::new();
    let mut shared = SharedChains::default();
    for (b, i) in releases {
        let Some(state) = after_states.get(&(b, i)) else {
            continue;
        };
        let at = ed.unit.blocks[b.index()].anchor;
        match synthesize_failure_reporting::<Rc, TRACK>(
            &mut ed,
            lay,
            &mut facts,
            state,
            at,
            Some(&mut shared),
            |block, address, anchor, if_live| {
                if TRACK {
                    origins.generated.entry((block, address)).or_insert((
                        hirsel_lir::ownership_trace::Point::Instruction {
                            block: b,
                            index: i as u32,
                            moment: hirsel_lir::ownership_trace::Moment::Before,
                        },
                        anchor,
                        if_live,
                    ));
                }
            },
        ) {
            Some(f) => {
                if let Some(slot) = Inst::failure_mut(&mut ed.code[b.index()][i]) {
                    *slot = f;
                }
            }
            None => {
                // The state here cannot be cleaned by a synthesized chain:
                // keep this slot's semantic drops.
                if let Some(s) = released_slot(&ed, &facts, b, i) {
                    lost.insert(s);
                }
            }
        }
    }

    // 4. Every anchor still discharged.
    let mut discharged = BTreeSet::new();
    for code in &ed.code {
        for inst in code {
            match *inst {
                Inst::Drop { anchor, .. }
                | Inst::Release { anchor, .. }
                | Inst::Reset { anchor, .. }
                | Inst::FinalizeBegin { anchor, .. }
                | Inst::FinalizeEnd { anchor, .. } => {
                    discharged.insert(anchor);
                }
                _ => {}
            }
        }
    }
    // An anchor of a drop that an earlier release superseded on every path
    // anchored nothing observable (the content has no user-visible
    // destruction), so it leaves the table; any other undischarged anchor
    // means the release could not stand in for it.
    let superseded: BTreeSet<CleanupAnchorId> = anchors_of
        .iter()
        .filter(|(s, _)| eligible(**s) && !lost.contains(*s))
        .flat_map(|(_, a)| a.iter().copied())
        .filter(|a| !discharged.contains(a))
        .collect();
    for (slot, anchors) in &anchors_of {
        if anchors
            .iter()
            .any(|a| !discharged.contains(a) && !superseded.contains(a))
        {
            lost.insert(*slot);
        }
    }
    if !lost.is_empty() {
        return Ok(Err(lost));
    }
    let _ = superseded;
    // Only diagnostics add these pure forwarding blocks. The original source
    // drop remains an exact empty-custody assertion after its physical release.
    erased.sort_by_key(|&(block, at, _, _)| (block, std::cmp::Reverse(at)));
    for (block, at, original, anchor) in erased {
        let (kind, source) = (
            ed.unit.blocks[block.index()].kind,
            ed.unit.blocks[block.index()].anchor,
        );
        let empty = ed.block(kind, &[], source);
        let tail = ed.block(kind, &[], source);
        ed.code[tail.index()] = ed.code[block.index()].split_off(at);
        ed.unit.terminators[tail.index()] = ed.unit.terminators[block.index()].clone();
        let enter = ed.edge(empty, &[]);
        let leave = ed.edge(tail, &[]);
        ed.unit.terminators[block.index()] = Terminator::Jump(enter);
        ed.unit.terminators[empty.index()] = Terminator::Jump(leave);
        let rows = origins.rows[block.index()].split_off(at);
        origins.complete(&ed);
        origins.rows[tail.index()] = rows;
        let source_block = origins.moves.sources[block.index()];
        origins.moves.sources[tail.index()] = source_block;
        if origins.moves.ends[source_block.index()] == block {
            origins.moves.ends[source_block.index()] = tail;
        }
        origins.moves.empty.insert(
            (block, original),
            hirsel_lir::ownership_trace::Point::EmptyDrop {
                anchor,
                block: empty,
            },
        );
        for (at_block, index, _, _, _) in &mut origins.retirements {
            if *at_block == block && *index >= at as u32 {
                *at_block = tail;
                *index -= at as u32;
            }
        }
        for (trigger, _, _) in origins.generated.values_mut() {
            if let hirsel_lir::ownership_trace::Point::Instruction {
                block: at_block,
                index,
                ..
            } = trigger
            {
                if *at_block == block && *index >= at as u32 {
                    *at_block = tail;
                    *index -= at as u32;
                }
            }
        }
    }
    origins.complete(&ed);
    Ok(Ok((ed, origins)))
}

#[derive(Clone, Copy, Debug)]
enum Settle {
    Delete,
    Keep(bool),
    Release(Option<ValueId>),
}

fn released_slot(ed: &Editor<Rc>, facts: &Facts, b: BlockId, i: usize) -> Option<SlotId> {
    match ed.code[b.index()][i] {
        Inst::Drop { address, .. } => facts.slot_of(address).map(|(s, _)| s),
        Inst::Release { value, .. } => {
            let read = ed.code[b.index()][..i]
                .iter()
                .rev()
                .find_map(|x| match *x {
                    Inst::Read {
                        result, address, ..
                    } if result == value => Some(address),
                    _ => None,
                })?;
            facts.slot_of(read).map(|(s, _)| s)
        }
        _ => None,
    }
}

/// Delete these cleanup anchors from the table, renumbering the rest.
fn remove_anchors<S>(
    unit: &mut LirUnit<S>,
    gone: &BTreeSet<CleanupAnchorId>,
) -> Vec<Option<CleanupAnchorId>> {
    if gone.is_empty() {
        return Vec::new();
    }
    let mut map = Vec::new();
    let mut next = 0u32;
    for a in 0..unit.tables.cleanup_anchors.len() as u32 {
        map.push(if gone.contains(&CleanupAnchorId(a)) {
            None
        } else {
            next += 1;
            Some(CleanupAnchorId(next - 1))
        });
    }
    let anchors = std::mem::take(&mut unit.tables.cleanup_anchors);
    unit.tables.cleanup_anchors = anchors
        .into_iter()
        .enumerate()
        .filter(|(a, _)| map[*a].is_some())
        .map(|(_, a)| a)
        .collect();
    for inst in &mut unit.instructions {
        if let Some(anchor) = anchor_mut(inst) {
            *anchor = map[anchor.index()].expect("a named anchor stays");
        }
    }
    map
}

fn anchor_mut(inst: &mut Inst) -> Option<&mut CleanupAnchorId> {
    match inst {
        Inst::Drop { anchor, .. }
        | Inst::Release { anchor, .. }
        | Inst::Reset { anchor, .. }
        | Inst::FinalizeBegin { anchor, .. }
        | Inst::FinalizeEnd { anchor, .. } => Some(anchor),
        _ => None,
    }
}

/// Anchors no kept instruction discharges any more: their only cleanup sat
/// on a path the rewrite removed, so nothing can skip them.
fn prune_anchors<S>(unit: &mut LirUnit<S>) -> Vec<Option<CleanupAnchorId>> {
    let mut named = BTreeSet::new();
    for inst in &mut unit.instructions {
        if let Some(a) = anchor_mut(inst) {
            named.insert(*a);
        }
    }
    let gone = (0..unit.tables.cleanup_anchors.len() as u32)
        .map(CleanupAnchorId)
        .filter(|a| !named.contains(a))
        .collect();
    remove_anchors(unit, &gone)
}

/// Rename every value the unit names through `map`. Exhaustive over the
/// rows that hold values, so a new value-holding field fails to compile here.
pub(crate) fn remap_values<S>(unit: &mut LirUnit<S>, map: &impl Fn(ValueId) -> ValueId) {
    for input in &mut unit.pools.receipt_inputs {
        input.original = map(input.original);
        input.receipt = input.receipt.map(map);
    }
    for payloads in &mut unit.tables.seal_payloads {
        for binding in &mut payloads.bindings {
            for value in &mut binding.values {
                *value = map(*value);
            }
        }
    }
    let mask = |m: &mut MaskRef| {
        if let MaskRef::Dynamic(v) = m {
            *v = map(*v);
        }
    };
    for inst in &mut unit.instructions {
        match inst {
            Inst::Const { result, .. }
            | Inst::FunctionRef { result, .. }
            | Inst::ReadStatic { result, .. }
            | Inst::TokenNone { result }
            | Inst::FlatCopy { result, .. }
            | Inst::FlatView { result, .. }
            | Inst::FlatLength { result, .. }
            | Inst::Construct { result, .. }
            | Inst::Closure { result, .. }
            | Inst::RegionEnter { result, .. } => *result = map(*result),
            Inst::ArenaOpen { result, budget } => {
                *result = map(*result);
                *budget = budget.map(map);
            }
            Inst::HoleOpen { result, hole, .. } => {
                *result = map(*result);
                *hole = map(*hole);
            }
            Inst::HoleFill { hole, value } => {
                *hole = map(*hole);
                *value = map(*value);
            }
            Inst::HoleAbandon { hole, root } => {
                *hole = map(*hole);
                *root = root.map(map);
            }
            Inst::ArenaToken {
                result,
                mark,
                budget,
            } => {
                *result = map(*result);
                *mark = map(*mark);
                *budget = budget.map(map);
            }
            Inst::SliceView {
                result,
                value,
                start,
                end,
                ..
            } => {
                *result = map(*result);
                *value = map(*value);
                *start = map(*start);
                *end = map(*end);
            }
            Inst::Unary {
                result, operand, ..
            } => {
                *result = map(*result);
                *operand = map(*operand);
            }
            Inst::Binary {
                result,
                left,
                right,
                ..
            }
            | Inst::Compare {
                result,
                left,
                right,
                ..
            } => {
                *result = map(*result);
                *left = map(*left);
                *right = map(*right);
            }
            Inst::Convert {
                result,
                flag,
                operand,
                ..
            } => {
                *result = map(*result);
                if let Some(flag) = flag {
                    *flag = map(*flag);
                }
                *operand = map(*operand);
            }
            Inst::IntArith {
                result,
                flag,
                left,
                right,
                ..
            } => {
                *result = map(*result);
                if let Some(flag) = flag {
                    *flag = map(*flag);
                }
                *left = map(*left);
                *right = map(*right);
            }
            Inst::MulAdd {
                result,
                left,
                right,
                addend,
            } => {
                *result = map(*result);
                *left = map(*left);
                *right = map(*right);
                *addend = map(*addend);
            }
            Inst::ScalarUnary {
                result, operand, ..
            } => {
                *result = map(*result);
                *operand = map(*operand);
            }
            Inst::SeqLen { result, value }
            | Inst::SeqCapacity { result, value }
            | Inst::Hash { result, value, .. }
            | Inst::Encode { result, value, .. }
            | Inst::Decode { result, value, .. }
            | Inst::CopyValue { result, value }
            | Inst::Recast { result, value }
            | Inst::Freeze { result, value }
            | Inst::Thaw { result, value } => {
                *result = map(*result);
                *value = map(*value);
            }
            Inst::SeqReserve {
                capacity: value,
                version,
                ..
            }
            | Inst::SeqSetLen {
                len: value,
                version,
                ..
            }
            | Inst::SeqAppend { value, version, .. } => {
                *value = map(*value);
                *version = map(*version);
            }
            Inst::FlatAppend { version, .. } | Inst::FlatClose { version, .. } => {
                *version = map(*version)
            }
            Inst::FlatField {
                op: FlatFieldOp::Open,
                version,
                ..
            } => *version = map(*version),
            Inst::FlatField {
                op: FlatFieldOp::Reserve { capacity: value } | FlatFieldOp::Push { value, .. },
                version,
                ..
            } => {
                *value = map(*value);
                *version = map(*version);
            }
            Inst::FlatNext { result, .. } => *result = map(*result),
            Inst::Check { condition, .. } => *condition = map(*condition),
            Inst::Read {
                result, reaching, ..
            }
            | Inst::Copy {
                result, reaching, ..
            } => {
                *result = map(*result);
                *reaching = map(*reaching);
            }
            Inst::SealArena {
                result,
                grant,
                root,
                ..
            } => {
                *result = map(*result);
                *grant = map(*grant);
                *root = map(*root);
            }
            Inst::PayloadReceipt {
                arena,
                result,
                value,
                ..
            } => {
                *arena = arena.get().map(map).into();
                *result = map(*result);
                *value = map(*value);
            }
            Inst::BorrowSealedRoot { result, owner, .. } => {
                *result = map(*result);
                *owner = map(*owner);
            }
            Inst::ArenaInventory {
                receipt,
                arena,
                value,
                previous,
                ..
            } => {
                *arena = map(*arena);
                *receipt = receipt.get().map(map).into();
                *value = map(*value);
                if let Some(previous) = previous {
                    *previous = map(*previous);
                }
            }
            Inst::UniqueCopy { result, value, .. }
            | Inst::IsUnique { result, value }
            | Inst::IsWritable { result, value }
            | Inst::Weaken { result, value } => {
                *result = map(*result);
                *value = map(*value);
            }
            Inst::ArenaPush { count, version, .. } => {
                if let Some(count) = count {
                    *count = map(*count);
                }
                *version = map(*version);
            }
            Inst::Store { value, version, .. } => {
                *value = map(*value);
                *version = map(*version);
            }
            Inst::Replace {
                value,
                version,
                displaced_version,
                ..
            } => {
                *value = map(*value);
                *version = map(*version);
                *displaced_version = map(*displaced_version);
            }
            Inst::UpdateMask {
                result, mask: m, ..
            } => {
                *result = map(*result);
                *m = map(*m);
            }
            Inst::Dup { value }
            | Inst::Share { value }
            | Inst::Forget { value }
            | Inst::RegionPublish { value }
            | Inst::Dispose { token: value } => *value = map(*value),
            Inst::Release { value, live, .. } => {
                *value = map(*value);
                mask(live);
            }
            Inst::Reset {
                result,
                value,
                live,
                ..
            } => {
                *result = map(*result);
                *value = map(*value);
                mask(live);
            }
            Inst::Reuse { result, token, .. } => {
                *result = map(*result);
                *token = map(*token);
            }
            Inst::RegionReserve {
                result,
                mark,
                extent,
                ..
            } => {
                *result = map(*result);
                *mark = map(*mark);
                *extent = map(*extent);
            }
            Inst::RegionExit { mark, .. } => *mark = map(*mark),
            Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::Drop { .. }
            | Inst::Defuse { .. }
            | Inst::FinalizeBegin { .. }
            | Inst::FinalizeEnd { .. }
            | Inst::FrameEnter { .. }
            | Inst::Address { .. }
            | Inst::FrameLeave
            | Inst::Unavailable { .. } => {}
        }
    }
    for term in &mut unit.terminators {
        match term {
            Terminator::Branch { condition, .. } => *condition = map(*condition),
            Terminator::Switch { value, .. } => *value = map(*value),
            Terminator::Recover { arena, .. } => *arena = map(*arena),
            Terminator::Isolate { start, .. } => *start = map(*start),
            Terminator::Return(Some(v)) => *v = map(*v),
            Terminator::Jump(_)
            | Terminator::Call { .. }
            | Terminator::Return(None)
            | Terminator::Raise(_)
            | Terminator::Propagate
            | Terminator::RejectTransfer
            | Terminator::Unreachable
            | Terminator::InvariantFault { .. } => {}
        }
    }
    for v in &mut unit.pools.values {
        *v = map(*v);
    }
    for f in &mut unit.pools.fields {
        f.value = map(f.value);
    }
    for step in &mut unit.pools.projections {
        match step {
            ProjectionStep::Payload
            | ProjectionStep::Field(_)
            | ProjectionStep::VariantField { .. } => {}
            ProjectionStep::Index(v) | ProjectionStep::Slot(v) => *v = map(*v),
            ProjectionStep::FlatAt { index, offset } => {
                *index = map(*index);
                *offset = map(*offset);
            }
        }
    }
    for c in &mut unit.pools.captures {
        c.value = map(c.value);
    }
    for a in &mut unit.pools.call_arguments {
        match &mut a.value {
            ArgumentValue::Value(v) => *v = map(*v),
            ArgumentValue::Address { version, .. } => *version = map(*version),
        }
    }
    if let Some(entry) = &mut unit.tables.receipt_entry {
        entry.remap_values(map);
    }
    for closure in &mut unit.tables.closures {
        if let Some(receipts) = &mut closure.receipts {
            receipts.remap_values(map);
        }
    }
    for call in &mut unit.tables.calls {
        if let Some(receipts) = &mut call.receipts {
            receipts.remap_values(map);
        }
        if let Some(r) = &mut call.result {
            *r = map(*r);
        }
        if let Some(v) = call.callee.value_mut() {
            *v = map(*v);
        }
    }
    for a in &mut unit.addresses {
        if let AddressBase::Value(v) = &mut a.base {
            *v = map(*v);
        }
    }
    for v in unit.parameters.iter_mut().chain(unit.captures.iter_mut()) {
        *v = map(*v);
    }
    for flag in &mut unit.tables.init_flags {
        flag.value = map(flag.value);
    }
}

/// Drop every value the unit no longer defines, renumbering the rest.
pub(crate) fn compact_values<S: Stage>(unit: &mut LirUnit<S>) {
    let mut defined = vec![false; unit.values.len()];
    let mut mark = |v: ValueId| defined[v.index()] = true;
    for &v in unit.parameters.iter().chain(&unit.captures) {
        mark(v);
    }
    if let Some(entry) = &unit.tables.receipt_entry {
        entry.versions().for_each(&mut mark);
    }
    for b in &unit.blocks {
        for &p in b.parameters.get(&unit.pools.values).unwrap_or(&[]) {
            mark(p);
        }
    }
    for inst in &unit.instructions {
        for v in Inst::defs(inst) {
            mark(v);
        }
    }
    for term in &unit.terminators {
        for v in crate::liveness::terminator_defs(unit, term) {
            mark(v);
        }
    }
    // A drop flag of a removed block goes with it.
    unit.tables
        .init_flags
        .retain(|flag| defined[flag.value.index()]);
    if defined.iter().all(|d| *d) {
        return;
    }
    // Pool rows the rewrite left unused (such as a dead block's parameter
    // range) may still name a removed value; they name the first
    // kept value instead, as nothing reads them.
    let mut map = Vec::with_capacity(defined.len());
    let mut next = 0u32;
    for &d in &defined {
        map.push(d.then_some(ValueId(next)));
        if d {
            next += 1;
        }
    }
    let map: Vec<ValueId> = map.into_iter().map(|m| m.unwrap_or(ValueId(0))).collect();
    let values = std::mem::take(&mut unit.values);
    unit.values = values
        .into_iter()
        .zip(&defined)
        .filter(|(_, d)| **d)
        .map(|(v, _)| v)
        .collect();
    remap_values(unit, &|v: ValueId| map[v.index()]);
}

/// Remove every version-join parameter whose slot, on some incoming edge, no
/// longer holds the content the argument names.
fn prune_phis<S: Stage>(ed: &mut Editor<S>, lay: Lay<'_>, facts: &mut Facts) {
    let mut gone: BTreeSet<(BlockId, usize)> = BTreeSet::new();
    {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts,
        };
        let states = flow.solve();
        for (b, state) in states.iter().enumerate() {
            let Some(state) = state.clone() else {
                continue;
            };
            let block = BlockId(b as u32);
            let end = ed.code[b].len();
            let mut post = None;
            flow.block_after(block, state, |i, after| {
                if i == end {
                    post = Some(after.clone());
                }
            });
            let Some(post) = post else {
                continue;
            };
            for e in Terminator::normal_edges(&ed.unit.terminators[b], &ed.unit.pools) {
                let edge = &ed.unit.tables.edges[e.index()];
                let args = edge.arguments.get(&ed.unit.pools.values).unwrap_or(&[]);
                for &(j, slot) in facts.phis_of(edge.target) {
                    if post.slots[slot.index()].versions != Versions::One(args[j]) {
                        gone.insert((edge.target, j));
                    }
                }
            }
        }
    }
    if gone.is_empty() {
        return;
    }
    // A removed join's parameter may itself be carried on into a later join.
    loop {
        let removed: BTreeSet<ValueId> = gone
            .iter()
            .map(|&(t, j)| {
                ed.unit.blocks[t.index()]
                    .parameters
                    .get(&ed.unit.pools.values)
                    .unwrap_or(&[])[j]
            })
            .collect();
        let before = gone.len();
        for edge in &ed.unit.tables.edges {
            let args = edge.arguments.get(&ed.unit.pools.values).unwrap_or(&[]);
            for &(j, _) in facts.phis_of(edge.target) {
                if removed.contains(&args[j]) {
                    gone.insert((edge.target, j));
                }
            }
        }
        if gone.len() == before {
            break;
        }
    }
    let keep = |target: BlockId, j: usize| !gone.contains(&(target, j));
    for e in 0..ed.unit.tables.edges.len() {
        let target = ed.unit.tables.edges[e].target;
        if !gone.iter().any(|&(t, _)| t == target) {
            continue;
        }
        let args: Vec<ValueId> = ed.unit.tables.edges[e]
            .arguments
            .get(&ed.unit.pools.values)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .filter(|&(j, _)| keep(target, j))
            .map(|(_, &v)| v)
            .collect();
        ed.unit.tables.edges[e].arguments = PoolRange::push(&mut ed.unit.pools.values, args);
    }
    let targets: BTreeSet<BlockId> = gone.iter().map(|&(t, _)| t).collect();
    for t in targets {
        let params: Vec<ValueId> = ed.unit.blocks[t.index()]
            .parameters
            .get(&ed.unit.pools.values)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .filter(|&(j, _)| keep(t, j))
            .map(|(_, &v)| v)
            .collect();
        ed.unit.blocks[t.index()].parameters = PoolRange::push(&mut ed.unit.pools.values, params);
        let removed: Vec<usize> = gone
            .iter()
            .filter(|&&(b, _)| b == t)
            .map(|&(_, j)| j)
            .collect();
        facts.phis[t.index()] = facts.phis[t.index()]
            .iter()
            .filter(|(j, _)| !removed.contains(j))
            .map(|&(j, s)| (j - removed.iter().filter(|&&r| r < j).count(), s))
            .collect();
    }
}
// ---------------------------------------------------------------------------
// Copy on write

/// The layout one projection step reaches from `layout` (a block reference
/// is dereferenced first).
fn child_layout(lay: Lay<'_>, layout: LayoutId, step: &ProjectionStep) -> Option<LayoutId> {
    if *step == ProjectionStep::Payload {
        return match lay.get(layout)?.kind {
            LayoutKind::Box { payload } => Some(payload),
            _ => None,
        };
    }
    let layout = lay.payload(layout).unwrap_or(layout);
    let record = lay.get(layout)?;
    Some(match *step {
        ProjectionStep::Payload => unreachable!("handled before implicit dereference"),
        ProjectionStep::Field(f) => record.fields.iter().find(|x| x.ordinal == f)?.layout,
        ProjectionStep::VariantField { variant, field } => {
            record
                .tag
                .iter()
                .flat_map(|t| &t.variants)
                .find(|v| v.ordinal == variant)?
                .fields
                .iter()
                .find(|x| x.ordinal == field)?
                .layout
        }
        ProjectionStep::Index(_) => match record.kind {
            LayoutKind::Inline { element, .. }
            | LayoutKind::Simd { element, .. }
            | LayoutKind::Slice { element }
            | LayoutKind::SoaColumns { element }
            | LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element } => element,
            _ => return None,
        },
        ProjectionStep::Slot(_) => match record.kind {
            LayoutKind::Sequence { element } | LayoutKind::SoaColumns { element } => element,
            _ => return None,
        },
        ProjectionStep::FlatAt { .. } => return None,
    })
}

/// A slot-rooted address's flattened projection.
fn projection_of<S>(
    unit: &LirUnit<S>,
    address: AddressId,
) -> Option<(SlotId, Vec<ProjectionStep>)> {
    let mut chain = vec![address];
    loop {
        match unit.addresses[chain.last()?.index()].base {
            AddressBase::Address(base) if !chain.contains(&base) => chain.push(base),
            AddressBase::Slot(slot) => {
                let mut steps = Vec::new();
                for id in chain.iter().rev() {
                    steps.extend_from_slice(
                        unit.addresses[id.index()]
                            .projection
                            .get(&unit.pools.projections)?,
                    );
                }
                return Some((slot, steps));
            }
            _ => return None,
        }
    }
}

/// A type of this representation and layout, added when the unit has none.
pub(crate) fn type_of<S>(unit: &mut LirUnit<S>, repr: ValueRepr, layout: LayoutId) -> LirTypeId {
    let wanted = hirsel_lir::ir::LirType { repr, layout };
    match unit.types.iter().position(|t| *t == wanted) {
        Some(i) => LirTypeId(i as u32),
        None => {
            unit.types.push(wanted);
            LirTypeId(unit.types.len() as u32 - 1)
        }
    }
}

/// One write through counted storage: the preparation of its address.
struct Write {
    block: BlockId,
    index: usize,
    address: AddressId,
    /// The write changes the block the place holds (`SeqReserve`,
    /// `SeqSetLen`, `SeqAppend`), which is detached after the blocks the path crosses.
    holding: bool,
    /// A `mut` argument: when its path crosses a block, the List it lends is
    /// shared (MEM-6), since that block's holders are not tracked.
    lend: bool,
}

/// Whether the place `steps` of `slot` holds a List, Array, Map or Set block:
/// the blocks MEM-6 tracks in an arena.
fn holds_sequence<S>(
    unit: &LirUnit<S>,
    lay: Lay<'_>,
    slot: SlotId,
    steps: &[ProjectionStep],
) -> bool {
    let mut layout = unit.types[unit.slots[slot.index()].ty.index()].layout;
    for step in steps {
        match child_layout(lay, layout, step) {
            Some(child) => layout = child,
            None => return false,
        }
    }
    lay.payload(layout).is_some_and(|p| {
        hirsel_lir::layout::keyed(lay.0, p)
            || lay
                .get(p)
                .is_some_and(|r| matches!(r.kind, LayoutKind::Sequence { .. }))
    })
}

/// Whether the place `steps` of `slot` is reached through another block
/// (MEM-6): one whose holders are not tracked. A Map's or Set's own entries
/// and index are its storage, so crossing its block is not.
fn through_block<S>(
    unit: &LirUnit<S>,
    lay: Lay<'_>,
    slot: SlotId,
    steps: &[ProjectionStep],
) -> bool {
    let mut layout = unit.types[unit.slots[slot.index()].ty.index()].layout;
    for step in steps {
        if lay
            .payload(layout)
            .is_some_and(|p| !hirsel_lir::layout::keyed(lay.0, p))
        {
            return true;
        }
        match child_layout(lay, layout, step) {
            Some(child) => layout = child,
            None => return false,
        }
    }
    false
}

/// Before an address that a write, a mutable loan or a `mut` argument uses
/// is prepared, make every counted block its path dereferences unique,
/// outermost first (MEM-1, OWN-7): test the block; when it is shared, copy
/// it uniquely, put the copy in its place and release the displaced
/// reference. The slot's version joins after the test. A `mut` argument's
/// preparation precedes its loan, which follows the last argument (#66), so
/// the copy happens after every argument is evaluated.
pub(crate) fn copy_on_write(
    unit: LirUnit<Rc>,
    table: &LayoutTable,
    proofs: &Proofs,
) -> Result<LirUnit<Rc>, PassError> {
    if proofs.noalloc {
        return Ok(unit);
    }
    let lay = Lay(table);
    let mut facts = Facts::new(&unit, table);
    let mut written = BTreeSet::new();
    // `SeqReserve` and `SeqSetLen` write the block their place holds, not
    // only the blocks its path crosses (LIR schema revision 7). A block
    // whose only write is a reserve is not detached: `SeqReserve` makes a
    // shared block unique itself, copying it once at the requested
    // capacity (`hirsel_rt_v2_seq_grow`), so the static empty sequence of
    // `[]` and a shared list are each copied once, not copied then grown.
    let mut holding = BTreeSet::new();
    // `ArenaPush` copies or relocates a tracked arena List itself, and
    // leaves any other block to the push's append (MEM-6, LIR schema
    // revision 32).
    let mut relocated: BTreeSet<AddressId> = unit
        .instructions
        .iter()
        .filter_map(|inst| match *inst {
            Inst::SeqReserve { address, .. } | Inst::ArenaPush { address, .. } => Some(address),
            _ => None,
        })
        .collect();
    for inst in &unit.instructions {
        match *inst {
            Inst::Store { address, .. }
            | Inst::Replace { address, .. }
            | Inst::BeginLoan {
                address,
                mutable: true,
                ..
            }
            | Inst::Read {
                address,
                mode: ReadMode::Take,
                ..
            }
            | Inst::SeqSetLen { address, .. } => {
                relocated.remove(&address);
            }
            _ => {}
        }
    }
    for term in &unit.terminators {
        if let Terminator::Call { call, .. } = *term {
            for a in call_arguments(&unit, call) {
                if let ArgumentValue::Address { address, .. } = a.value {
                    relocated.remove(&address);
                }
            }
        }
    }
    for inst in &unit.instructions {
        match *inst {
            Inst::Store { address, .. }
            | Inst::Replace { address, .. }
            | Inst::BeginLoan {
                address,
                mutable: true,
                ..
            } => {
                written.insert(address);
            }
            // A take through a `Slot` step empties an element slot: a write.
            Inst::Read {
                address,
                mode: ReadMode::Take,
                ..
            } if facts.places[address.index()]
                .as_ref()
                .is_some_and(|p| through_slot(&p.path) || !p.crossings.is_empty()) =>
            {
                written.insert(address);
            }
            Inst::SeqReserve { address, .. }
            | Inst::ArenaPush { address, .. }
            | Inst::SeqSetLen { address, .. }
            | Inst::SeqAppend { address, .. }
            | Inst::FlatAppend { address, .. }
            | Inst::FlatField { address, .. }
            | Inst::FlatClose { address, .. } => {
                written.insert(address);
                if !relocated.contains(&address) {
                    holding.insert(address);
                }
            }
            _ => {}
        }
    }
    let mut lent = BTreeSet::new();
    // Lists lent `mut` to a storage primitive that writes them in place.
    let mut in_place = BTreeSet::new();
    for (b, term) in unit.terminators.iter().enumerate() {
        if let Terminator::Call { call, .. } = *term {
            let site = &unit.tables.calls[call.index()];
            for a in call_arguments(&unit, call) {
                if let ArgumentValue::Address { address, .. } = a.value {
                    written.insert(address);
                    if a.passing == Passing::Mut {
                        lent.insert(address);
                        // The checked replacement wrapper reaches this leaf only
                        // after validation and its empty return. Unlike ordinary
                        // mut calls, the leaf writes the list held in this slot.
                        if let hirsel_lir::ir::Callee::Operation(op) = site.callee
                            && unit.tables.operations[op.index()]
                                .definition
                                .is_core("bytes.replace_range")
                            && a.parameter == 0
                        {
                            holding.insert(address);
                        }
                        // `pop`, `remove` and `swap` allocate nothing, so they
                        // cannot detach: they write the List they are lent in
                        // place. An arena block is never written in place, and a
                        // `unique` parameter may hold one, so the caller makes the
                        // List writable first whatever its root proves.
                        let callee = match site.callee {
                            hirsel_lir::ir::Callee::Direct(symbol) => {
                                Some(&unit.tables.symbols[symbol.index()].definition)
                            }
                            hirsel_lir::ir::Callee::Operation(op) => {
                                Some(&unit.tables.operations[op.index()].definition)
                            }
                            _ => None,
                        };
                        if a.parameter == 0
                            && callee.is_some_and(|d| {
                                ["list.pop", "list.remove", "list.swap"]
                                    .iter()
                                    .any(|name| d.is_core(name))
                            })
                        {
                            in_place.insert((BlockId(b as u32), address));
                        }
                    }
                }
            }
        }
    }
    let kinds: Vec<SlotKind> = unit.slots.iter().map(|s| s.kind).collect();
    let proven = |s: SlotId| matches!(kinds[s.index()], SlotKind::Parameter(i) if proofs.unique.contains(&i));
    let crossing = |a: AddressId| {
        facts.places[a.index()].as_ref().is_some_and(|p| {
            matches!(p.root, Root::Slot(s) if !proven(s))
                && (!p.crossings.is_empty() || holding.contains(&a))
        })
    };
    if in_place.is_empty() && !written.iter().any(|&a| crossing(a)) {
        return Ok(unit);
    }
    let Some(bool_layout) = table
        .layouts
        .iter()
        .position(|l| l.kind == LayoutKind::Scalar(hirsel_lir::layout_types::ScalarKind::Bool))
    else {
        return Err(PassError::Malformed(
            "no bool layout for a uniqueness test".into(),
        ));
    };
    let mut ed = Editor::new(unit);
    let mut writes = Vec::new();
    for (b, code) in ed.code.iter().enumerate() {
        if ed.unit.blocks[b].kind != BlockKind::Body {
            continue;
        }
        for (i, inst) in code.iter().enumerate() {
            if let Inst::Address { address, .. } = *inst
                && written.contains(&address)
                && crossing(address)
            {
                writes.push(Write {
                    block: BlockId(b as u32),
                    index: i,
                    address,
                    holding: holding.contains(&address),
                    lend: lent.contains(&address),
                });
            }
        }
    }
    writes.extend(viewed(&ed, lay, &facts, &writes));
    let bool_ty = type_of(
        &mut ed.unit,
        ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::Bool),
        LayoutId(bool_layout as u32),
    );
    let mut targets = detach_targets(&ed, lay, &facts, &writes, &in_place);
    // Back to front, so earlier positions in a block stay put.
    let mut order: Vec<usize> = (0..targets.len()).collect();
    order.sort_by_key(|&k| (targets[k].block, std::cmp::Reverse(targets[k].index)));
    let mut ruled = Vec::new();
    for k in order {
        let t = targets[k].clone();
        let mut at = (t.block, t.index);
        let mut version = t.version;
        for (prefix, place) in &t.prefixes {
            let (joined, next) = detach(
                &mut ed,
                lay,
                &mut facts,
                t.slot,
                prefix,
                *place,
                version,
                at,
                t.failure.clone(),
                bool_ty,
                &mut ruled,
                &mut targets,
            )?;
            version = joined;
            at = (next, 0);
        }
        if let Some(steps) = &t.lend {
            share_held(
                &mut ed,
                lay,
                &mut facts,
                t.slot,
                steps,
                version,
                at,
                t.failure.clone(),
                &mut ruled,
            )?;
        }
    }
    fill_pending(&mut ed, lay, &mut facts)?;
    for (place, rule) in ruled {
        for inst in ed.code.iter().flatten() {
            if let Inst::Address { address, failure } = *inst
                && address == place
            {
                ed.unit.tables.failures[failure.index()].rule = rule.clone();
            }
        }
    }
    Ok(ed.finish())
}

/// The preparations of shared loans a write detaches under: a view of a
/// block that a write while the view is active detaches would read the
/// abandoned block (OWN-3: a sibling field written while an argument views
/// its neighbour), so the block is detached where the view is prepared and
/// the write then finds it unique.
fn viewed(ed: &Editor<Rc>, lay: Lay<'_>, facts: &Facts, writes: &[Write]) -> Vec<Write> {
    let flow = Flow {
        unit: &ed.unit,
        code: &ed.code,
        lay,
        facts,
    };
    let states = flow.solve();
    // A place's block prefixes: the steps before each block it crosses.
    let blocks = |a: AddressId| -> Option<(SlotId, Vec<Vec<ProjectionStep>>)> {
        let (slot, steps) = projection_of(&ed.unit, a)?;
        let mut layout = ed.unit.types[ed.unit.slots[slot.index()].ty.index()].layout;
        let mut prefixes = Vec::new();
        for (i, step) in steps.iter().enumerate() {
            if lay.payload(layout).is_some() {
                prefixes.push(steps[..i].to_vec());
            }
            layout = child_layout(lay, layout, step)?;
        }
        Some((slot, prefixes))
    };
    let mut views = BTreeSet::new();
    for w in writes {
        let Some(state) = states[w.block.index()].clone() else {
            continue;
        };
        let mut pre = None;
        flow.block(w.block, state, |i, at, _| {
            if i == w.index {
                pre = Some(at.clone());
            }
        });
        let (Some(pre), Some((slot, written))) = (pre, blocks(w.address)) else {
            continue;
        };
        for (l, loan) in ed.unit.tables.loans.iter().enumerate() {
            if loan.mutable || pre.loans[l] == Tri::No {
                continue;
            }
            if let Some((s, viewed)) = blocks(loan.address)
                && s == slot
                && viewed.iter().any(|p| written.contains(p))
            {
                views.insert(loan.address);
            }
        }
    }
    let mut out = Vec::new();
    for (b, code) in ed.code.iter().enumerate() {
        if ed.unit.blocks[b].kind != BlockKind::Body {
            continue;
        }
        for (i, inst) in code.iter().enumerate() {
            if let Inst::Address { address, .. } = *inst
                && views.contains(&address)
            {
                out.push(Write {
                    block: BlockId(b as u32),
                    index: i,
                    address,
                    holding: false,
                    lend: false,
                });
            }
        }
    }
    out
}

/// Where to detach, which blocks, and the root slot's version there.
#[derive(Clone)]
struct Target {
    block: BlockId,
    index: usize,
    slot: SlotId,
    /// Each block's place, outermost first, and its prepared address when
    /// one already exists.
    prefixes: Vec<(Vec<ProjectionStep>, Option<AddressId>)>,
    version: ValueId,
    /// The write's preparation failure when the detach sits right before
    /// it, else that failure's rule.
    failure: Result<FailureId, Option<String>>,
    /// The place of a List a `mut` argument lends through another block,
    /// shared after the detaches (MEM-6).
    lend: Option<Vec<ProjectionStep>>,
}

/// The detaches the writes need. A block whose place an active mutable loan
/// lends was detached where that loan began (an exclusive loan keeps it
/// unique), so a write under it, a loop's say, tests nothing. A write's
/// detach moves back to the latest point of its block where nothing but a
/// region mark is pending, so its failure cleanup only drops slots, and no
/// later operation it passes defines an index the places use or touches the
/// root slot. Each loan active at an `in_place` call detaches its block where
/// it begins.
fn detach_targets(
    ed: &Editor<Rc>,
    lay: Lay<'_>,
    facts: &Facts,
    writes: &[Write],
    in_place: &BTreeSet<(BlockId, AddressId)>,
) -> Vec<Target> {
    let flow = Flow {
        unit: &ed.unit,
        code: &ed.code,
        lay,
        facts,
    };
    let states = flow.solve();
    let pre_states = |b: BlockId| -> Vec<State> {
        let Some(state) = states[b.index()].clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        flow.block(b, state, |_, pre, _| out.push(pre.clone()));
        out
    };
    let loan_place = |l: LoanId| projection_of(&ed.unit, ed.unit.tables.loans[l.index()].address);
    let mut targets = Vec::new();
    let mut lent = BTreeSet::new();
    let mut loans_by_address: BTreeMap<AddressId, Vec<LoanId>> = in_place
        .iter()
        .map(|&(_, address)| (address, Vec::new()))
        .collect();
    if !loans_by_address.is_empty() {
        for (l, loan) in ed.unit.tables.loans.iter().enumerate() {
            if loan.mutable
                && let Some(loans) = loans_by_address.get_mut(&loan.address)
            {
                loans.push(LoanId(l as u32));
            }
        }
    }
    // An address can be lent by several distinct calls. Only the loan active
    // at the in-place primitive needs a detach: earlier reserve and push loans
    // make their own storage writable and must not copy the empty list first.
    for &(block, address) in in_place {
        let Some(state) = &states[block.index()] else {
            continue;
        };
        // Only BeginLoan and EndLoan change this component within a block.
        // Retain its terminal facts without cloning every instruction's slots
        // and credits just to inspect the state at the primitive call.
        let mut loans: BTreeSet<_> = loans_by_address
            .get(&address)
            .into_iter()
            .flatten()
            .copied()
            .filter(|l| state.loans[l.index()] != Tri::No)
            .collect();
        for inst in &ed.code[block.index()] {
            match *inst {
                Inst::BeginLoan {
                    loan,
                    address: borrowed,
                    mutable: true,
                    ..
                } if borrowed == address => {
                    loans.insert(loan);
                }
                Inst::EndLoan { loan, .. } => {
                    loans.remove(&loan);
                }
                _ => {}
            }
        }
        lent.extend(loans);
    }
    for w in writes {
        let pre = pre_states(w.block);
        let Some(state) = pre.get(w.index) else {
            continue;
        };
        let Some((slot, steps)) = projection_of(&ed.unit, w.address) else {
            continue;
        };
        let mut crossings = facts.places[w.address.index()]
            .as_ref()
            .map(|p| p.crossings.clone())
            .unwrap_or_default();
        if w.holding {
            crossings.push(steps.len());
        }
        let mut prefixes = Vec::new();
        for &c in &crossings {
            let prefix = steps[..c].to_vec();
            let covering = (0..ed.unit.tables.loans.len())
                .map(|l| LoanId(l as u32))
                .find(|&l| {
                    state.loans[l.index()] == Tri::Yes
                        && ed.unit.tables.loans[l.index()].mutable
                        && loan_place(l) == Some((slot, prefix.clone()))
                });
            match covering {
                Some(l) => {
                    lent.insert(l);
                }
                None => prefixes.push((prefix, None)),
            }
        }
        let crosses = through_block(&ed.unit, lay, slot, &steps);
        let lend = (w.lend && !w.holding && crosses && holds_sequence(&ed.unit, lay, slot, &steps))
            .then(|| steps.clone());
        if prefixes.is_empty() && lend.is_none() {
            continue;
        }
        let used: BTreeSet<ValueId> = prefixes
            .iter()
            .map(|(p, _)| p)
            .chain(&lend)
            .flat_map(|p| p.iter())
            .flat_map(|s| match *s {
                ProjectionStep::Index(v) | ProjectionStep::Slot(v) => vec![v],
                ProjectionStep::FlatAt { index, offset } => vec![index, offset],
                ProjectionStep::Payload
                | ProjectionStep::Field(_)
                | ProjectionStep::VariantField { .. } => Vec::new(),
            })
            .collect();
        let quiet = |s: &State| {
            s.credits.keys().all(|&v| {
                ed.unit.types[ed.unit.values[v.index()].ty.index()].repr == ValueRepr::RegionMark
            })
        };
        let touches = |inst: &Inst| {
            Inst::defs(inst).iter().any(|v| used.contains(v))
                || match *inst {
                    Inst::Store { address, .. }
                    | Inst::Replace { address, .. }
                    | Inst::Drop { address, .. }
                    | Inst::BeginLoan { address, .. }
                    | Inst::SeqReserve { address, .. }
                    | Inst::ArenaPush { address, .. }
                    | Inst::SeqSetLen { address, .. }
                    | Inst::SeqAppend { address, .. }
                    | Inst::FlatAppend { address, .. }
                    | Inst::FlatField { address, .. }
                    | Inst::FlatClose { address, .. }
                    | Inst::Read {
                        address,
                        mode: ReadMode::Take,
                        ..
                    } => facts.slot_of(address).is_some_and(|(s, _)| s == slot),
                    Inst::StorageLive { slot: s } | Inst::StorageDead { slot: s, .. } => s == slot,
                    _ => false,
                }
        };
        let code = &ed.code[w.block.index()];
        let mut index = w.index;
        while !quiet(&pre[index]) && index > 0 && !touches(&code[index - 1]) {
            index -= 1;
        }
        if !quiet(&pre[index]) {
            index = w.index;
        }
        // A detach moves every place located through the blocks it copies,
        // so it also goes above the preparations just before it of such
        // places: they hold no credit, and each is then located after it
        // (C's `StaleAddress`).
        while index > 0
            && let Inst::Address { address, .. } = code[index - 1]
            && projection_of(&ed.unit, address).is_some_and(|(s, place)| {
                s == slot
                    && prefixes
                        .iter()
                        .any(|(q, _)| place.len() > q.len() && place.starts_with(q))
            })
            && quiet(&pre[index - 1])
        {
            index -= 1;
        }
        // A place prepared at entry (a parameter's whole place a library
        // body writes, say) is detached after the entry's preparations, so
        // the test reads it prepared and before any use.
        if w.block == ed.unit.entry {
            index = code
                .iter()
                .rposition(|i| matches!(i, Inst::Address { .. }))
                .map_or(index, |last| last + 1);
        }
        let Some(version) = pre[index.min(pre.len() - 1)].slots[slot.index()]
            .versions
            .one()
        else {
            continue;
        };
        let failure = match code[w.index] {
            Inst::Address { failure, .. } if index == w.index => Ok(failure),
            Inst::Address { failure, .. } => {
                Err(ed.unit.tables.failures[failure.index()].rule.clone())
            }
            _ => Err(None),
        };
        targets.push(Target {
            block: w.block,
            index,
            slot,
            prefixes,
            version,
            failure,
            lend,
        });
    }
    // Each lending loan detaches its own block where it begins.
    for l in lent {
        let Some((slot, prefix)) = loan_place(l) else {
            continue;
        };
        let address = ed.unit.tables.loans[l.index()].address;
        for (b, code) in ed.code.iter().enumerate() {
            for (i, inst) in code.iter().enumerate() {
                if !matches!(*inst, Inst::BeginLoan { loan, .. } if loan == l) {
                    continue;
                }
                let pre = pre_states(BlockId(b as u32));
                let Some(state) = pre.get(i) else {
                    continue;
                };
                let Some(version) = state.slots[slot.index()].versions.one() else {
                    continue;
                };
                targets.push(Target {
                    block: BlockId(b as u32),
                    index: i,
                    slot,
                    prefixes: vec![(prefix.clone(), Some(address))],
                    version,
                    failure: Err(None),
                    lend: None,
                });
            }
        }
    }
    targets
}

/// Make the block at `slot`'s place `prefix` unique before position `at`.
/// Returns the slot's joined version and the block the rest of the code
/// continues in.
#[allow(clippy::too_many_arguments)]
fn detach(
    ed: &mut Editor<Rc>,
    lay: Lay<'_>,
    facts: &mut Facts,
    slot: SlotId,
    prefix: &[ProjectionStep],
    prepared: Option<AddressId>,
    version: ValueId,
    at: (BlockId, usize),
    prepare_failure: Result<FailureId, Option<String>>,
    bool_ty: LirTypeId,
    ruled: &mut Vec<(AddressId, Option<String>)>,
    targets: &mut [Target],
) -> Result<(ValueId, BlockId), PassError> {
    let slot_ty = ed.unit.slots[slot.index()].ty;
    let mut layout = ed.unit.types[slot_ty.index()].layout;
    for step in prefix {
        layout = child_layout(lay, layout, step)
            .ok_or_else(|| PassError::Malformed("a projection outside its layout".into()))?;
    }
    // Whether the path reaches this block through another one.
    let nested = through_block(&ed.unit, lay, slot, prefix);
    let payload = lay
        .payload(layout)
        .ok_or_else(|| PassError::Malformed("a crossing without a block".into()))?;
    // MEM-6: a List written through another block is shared, since that
    // block's holders are not tracked, so the write copies it.
    let share = nested && holds_sequence(&ed.unit, lay, slot, prefix);
    let block_ty = type_of(&mut ed.unit, ValueRepr::Block, layout);
    let (b, i) = at;
    let anchor = ed.unit.blocks[b.index()].anchor;
    // The place holding the reference.
    let place = if let Some(place) = prepared {
        place
    } else if prefix.is_empty() {
        ed.whole(facts, slot)
    } else {
        let id = AddressId(ed.unit.addresses.len() as u32);
        let projection = PoolRange::push(&mut ed.unit.pools.projections, prefix.iter().copied());
        ed.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Slot(slot),
            projection,
            ty: block_ty,
        });
        facts.places.push(place_of(&ed.unit, lay, id));
        id
    };
    // Split: the rest of the block continues in `rest`, after the join.
    let joined = ed.value(ed.unit.values[version.index()].ty, Holding::Plain);
    let joined_holding = ed.unit.values[version.index()].holding;
    ed.unit.values[joined.index()].holding = joined_holding;
    let rest = ed.block(ed.unit.blocks[b.index()].kind, &[joined], anchor);
    let tail: Vec<Inst> = ed.code[b.index()].split_off(i);
    ed.code[rest.index()] = tail;
    ed.unit.terminators.swap(b.index(), rest.index());
    if facts.phis.len() <= rest.index() {
        facts.phis.resize(rest.index() + 1, Vec::new());
    }
    facts.phis[rest.index()] = vec![(0, slot)];
    // Test.
    let mut code = Vec::new();
    if prepared.is_none() && !prefix.is_empty() {
        // The write's own preparation fails the same way here, unless the
        // detach moved earlier: then its cleanup is synthesized, keeping the
        // rule.
        let f = match prepare_failure {
            Ok(f) => copy_failure(ed, f),
            Err(rule) => {
                ruled.push((place, rule));
                PENDING_FAILURE
            }
        };
        code.push(Inst::Address {
            address: place,
            failure: f,
        });
    }
    let test_loan = new_loan(ed, facts, place, slot);
    let view = ed.value(block_ty, Holding::Borrowed(test_loan));
    let unique = ed.value(bool_ty, Holding::Plain);
    code.push(Inst::BeginLoan {
        loan: test_loan,
        address: place,
        mutable: false,
        if_inactive: false,
    });
    code.push(Inst::Read {
        result: view,
        address: place,
        reaching: version,
        mode: ReadMode::Borrow,
    });
    if share {
        code.push(Inst::Share { value: view });
    }
    // An unshared arena List is writable in place though never unique.
    code.push(Inst::IsWritable {
        result: unique,
        value: view,
    });
    code.push(Inst::EndLoan {
        loan: test_loan,
        if_active: false,
    });
    ed.code[b.index()].extend(code);
    let shared = ed.block(BlockKind::Body, &[], anchor);
    let yes = ed.edge(rest, &[version]);
    let no = ed.edge(shared, &[]);
    ed.unit.terminators[b.index()] = Terminator::Branch {
        condition: unique,
        yes,
        no,
    };
    // Shared: copy, replace, release the displaced reference.
    let copy_loan = new_loan(ed, facts, place, slot);
    let source = ed.value(block_ty, Holding::Borrowed(copy_loan));
    let fresh = ed.value(block_ty, Holding::Owned);
    let construct = hirsel_lir::ir::ConstructId(ed.unit.tables.constructs.len() as u32);
    ed.unit
        .tables
        .constructs
        .push(hirsel_lir::ir::ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor: hirsel_lir::ir::ConstructorKind::Tuple,
            layout: payload,
            fields: PoolRange::EMPTY,
            destination: hirsel_lir::ir::DestinationClass::Inferred,
            placement: Some(hirsel_lir::ir::Placement::Fixed(
                crate::placement::TASK_HEAP,
            )),
            anchor,
        });
    let scope = ed.unit.slots[slot.index()].scope;
    let displaced = SlotId(ed.unit.slots.len() as u32);
    ed.unit.slots.push(hirsel_lir::ir::SlotInfo {
        ty: block_ty,
        kind: SlotKind::Displaced,
        scope,
    });
    let dropped = ed.whole(facts, displaced);
    let after = ed.value(ed.unit.values[version.index()].ty, joined_holding);
    let displaced_version = ed.value(block_ty, Holding::Plain);
    let (body, site) = anchor_site(&ed.unit, anchor);
    let release = ed.cleanup_anchor(CleanupAnchor {
        body,
        // The detach installs a new holder and drops the displaced reference
        // at this instruction, on the normal path. STATE_RELEASE is reserved
        // for synthesized failure cleanup, not replacement-local destruction.
        cleanup: INSTRUCTION_DROP,
        action: displaced.0,
        site,
    });
    ed.code[shared.index()] = vec![
        Inst::BeginLoan {
            loan: copy_loan,
            address: place,
            mutable: false,
            if_inactive: false,
        },
        Inst::Read {
            result: source,
            address: place,
            reaching: version,
            mode: ReadMode::Borrow,
        },
        // A detach copies the written block alone; its children gain a
        // holder and are detached in turn only if a write reaches them
        // (OWN-4). A deep copy would be `unique x`.
        Inst::UniqueCopy {
            result: fresh,
            value: source,
            construct,
            deep: false,
            failure: PENDING_FAILURE,
        },
        Inst::EndLoan {
            loan: copy_loan,
            if_active: false,
        },
        Inst::StorageLive { slot: displaced },
        Inst::Replace {
            address: place,
            value: fresh,
            displaced,
            version: after,
            displaced_version,
            if_live: false,
            failure: PENDING_FAILURE,
        },
        Inst::Drop {
            address: dropped,
            if_live: false,
            anchor: release,
            failure: PENDING_FAILURE,
        },
        Inst::StorageDead {
            slot: displaced,
            if_live: false,
        },
    ];
    let join = ed.edge(rest, &[after]);
    ed.unit.terminators[shared.index()] = Terminator::Jump(join);
    if facts.version_of.len() < ed.unit.values.len() {
        facts.version_of.resize(ed.unit.values.len(), None);
        facts.backing.resize(ed.unit.values.len(), None);
        facts.statics.resize(ed.unit.values.len(), false);
    }
    for v in [joined, after] {
        facts.version_of[v.index()] = Some(slot);
    }
    for v in [view, source] {
        facts.backing[v.index()] = Some(slot);
    }
    // A later detach was discovered against the original CFG too. Rename its
    // reaching version together with the reads and transports it will replace.
    rename_version(ed, rest, version, joined, targets);
    Ok((joined, rest))
}

/// Share the List at `slot`'s place `steps`, which a `mut` argument lends
/// through another block (MEM-6), at position `at`: prepare the place, view
/// it and mark it, so the callee's first write copies it.
#[allow(clippy::too_many_arguments)]
fn share_held(
    ed: &mut Editor<Rc>,
    lay: Lay<'_>,
    facts: &mut Facts,
    slot: SlotId,
    steps: &[ProjectionStep],
    version: ValueId,
    at: (BlockId, usize),
    prepare_failure: Result<FailureId, Option<String>>,
    ruled: &mut Vec<(AddressId, Option<String>)>,
) -> Result<(), PassError> {
    let slot_ty = ed.unit.slots[slot.index()].ty;
    let mut layout = ed.unit.types[slot_ty.index()].layout;
    for step in steps {
        layout = child_layout(lay, layout, step)
            .ok_or_else(|| PassError::Malformed("a projection outside its layout".into()))?;
    }
    let block_ty = type_of(&mut ed.unit, ValueRepr::Block, layout);
    let place = AddressId(ed.unit.addresses.len() as u32);
    let projection = PoolRange::push(&mut ed.unit.pools.projections, steps.iter().copied());
    ed.unit.addresses.push(AddressInfo {
        source_place: None,
        unchecked: Vec::new(),
        base: AddressBase::Slot(slot),
        projection,
        ty: block_ty,
    });
    facts.places.push(place_of(&ed.unit, lay, place));
    let failure = match prepare_failure {
        Ok(f) => copy_failure(ed, f),
        Err(rule) => {
            ruled.push((place, rule));
            PENDING_FAILURE
        }
    };
    let loan = new_loan(ed, facts, place, slot);
    let view = ed.value(block_ty, Holding::Borrowed(loan));
    if facts.backing.len() < ed.unit.values.len() {
        facts.version_of.resize(ed.unit.values.len(), None);
        facts.backing.resize(ed.unit.values.len(), None);
        facts.statics.resize(ed.unit.values.len(), false);
    }
    facts.backing[view.index()] = Some(slot);
    let code = [
        Inst::Address {
            address: place,
            failure,
        },
        Inst::BeginLoan {
            loan,
            address: place,
            mutable: false,
            if_inactive: false,
        },
        Inst::Read {
            result: view,
            address: place,
            reaching: version,
            mode: ReadMode::Borrow,
        },
        Inst::Share { value: view },
        Inst::EndLoan {
            loan,
            if_active: false,
        },
    ];
    let (b, i) = at;
    ed.code[b.index()].splice(i..i, code);
    Ok(())
}

fn new_loan(ed: &mut Editor<Rc>, facts: &mut Facts, address: AddressId, slot: SlotId) -> LoanId {
    let id = LoanId(ed.unit.tables.loans.len() as u32);
    ed.unit.tables.loans.push(hirsel_lir::ir::LoanInfo {
        address,
        mutable: false,
    });
    facts.loan_slot.push(Some(slot));
    id
}

/// A second failure edge into the same cleanup, for a new site.
fn copy_failure<S: Stage>(ed: &mut Editor<S>, f: FailureId) -> FailureId {
    let old = ed.unit.tables.failures[f.index()].clone();
    let edge = ed.unit.tables.edges[old.cleanup.index()].clone();
    let args = edge
        .arguments
        .get(&ed.unit.pools.values)
        .unwrap_or(&[])
        .to_vec();
    let e = ed.edge(edge.target, &args);
    let id = ed.failure(e, old.anchor);
    ed.unit.tables.failures[id.index()].rule = old.rule;
    id
}

/// Every use of version `from` that `block` dominates now names `to`: reads'
/// and copies' reaching operands and edge arguments. `block` must be the only
/// way past the split it continues, as `detach` makes it.
fn rename_version<S: Stage>(
    ed: &mut Editor<S>,
    block: BlockId,
    from: ValueId,
    to: ValueId,
    targets: &mut [Target],
) {
    let n = ed.unit.blocks.len();
    let successors = |ed: &Editor<S>, b: usize| -> Vec<BlockId> {
        let mut out: Vec<BlockId> = ed.code[b]
            .iter()
            .filter_map(Inst::failure)
            .filter(|&f| f != PENDING_FAILURE)
            .map(|f| {
                ed.unit.tables.edges[ed.unit.tables.failures[f.index()].cleanup.index()].target
            })
            .collect();
        for e in terminator_edges(&ed.unit, &ed.unit.terminators[b]) {
            out.push(ed.unit.tables.edges[e.index()].target);
        }
        out
    };
    // Reachable from the entry without passing `block`: not dominated.
    let mut free = vec![false; n];
    let mut stack = vec![ed.unit.entry];
    while let Some(b) = stack.pop() {
        if b == block || std::mem::replace(&mut free[b.index()], true) {
            continue;
        }
        stack.extend(successors(ed, b.index()));
    }
    for target in targets {
        if !free[target.block.index()] && target.version == from {
            target.version = to;
        }
    }
    for b in (0..n).filter(|&b| !free[b]) {
        for inst in &mut ed.code[b] {
            match inst {
                Inst::Read { reaching, .. } | Inst::Copy { reaching, .. } if *reaching == from => {
                    *reaching = to;
                }
                _ => {}
            }
        }
        let term = ed.unit.terminators[b].clone();
        let edges = ed.code[b]
            .iter()
            .filter_map(Inst::failure)
            .filter(|&f| f != PENDING_FAILURE)
            .map(|f| ed.unit.tables.failures[f.index()].cleanup)
            .chain(terminator_edges(&ed.unit, &term))
            .collect::<Vec<_>>();
        for e in edges {
            let range = ed.unit.tables.edges[e.index()].arguments;
            let start = range.start as usize;
            for v in &mut ed.unit.pools.values[start..start + range.len as usize] {
                if *v == from {
                    *v = to;
                }
            }
        }
    }
}

/// Give every operation still carrying `PENDING_FAILURE` a synthesized
/// cleanup of what its failure leaves owned: the state after a cleanup
/// operation (it finishes before failing), else the state before it less
/// what it consumes.
pub(crate) fn fill_pending(
    ed: &mut Editor<Rc>,
    lay: Lay<'_>,
    facts: &mut Facts,
) -> Result<(), PassError> {
    let mut wanted: BTreeMap<(BlockId, usize), State> = BTreeMap::new();
    {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts,
        };
        let states = flow.solve();
        for (b, state) in states.iter().enumerate() {
            let Some(state) = state.clone() else {
                continue;
            };
            let block = BlockId(b as u32);
            let code = &ed.code[b];
            flow.block(block, state, |i, pre, post| {
                let Some(inst) = code.get(i) else {
                    return;
                };
                if Inst::failure(inst) != Some(PENDING_FAILURE) {
                    return;
                }
                let s = match inst {
                    Inst::Release { .. }
                    | Inst::Reset { .. }
                    | Inst::Drop { .. }
                    | Inst::RegionExit { .. } => post.clone(),
                    _ => {
                        let mut f = pre.clone();
                        for v in consumed(&ed.unit, lay, inst)
                            .into_iter()
                            .filter(|_| !matches!(inst, Inst::SealArena { .. }))
                        {
                            if owns(&ed.unit, lay, v) {
                                take_credit(&mut f.credits, v);
                            }
                        }
                        f
                    }
                };
                wanted.insert((block, i), s);
            });
        }
    }
    for ((b, i), state) in wanted {
        let at = ed.unit.blocks[b.index()].anchor;
        let f = synthesize_failure(ed, lay, facts, &state, at, None).ok_or_else(|| {
            PassError::Malformed("a failure whose cleanup cannot be synthesized".into())
        })?;
        if let Some(slot) = Inst::failure_mut(&mut ed.code[b.index()][i]) {
            *slot = f;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finishing_prunes_unreferenced_calls_and_remaps_kept_calls_and_arguments() {
        use hirsel_heapcheck::build::{Layouts, fixture_unit, path};
        use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
        use hirsel_lir::testing::UnitBuilder;
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let scalar = layouts.scalar(ScalarKind::U64);
        let (_, table) = layouts.finish();
        let mut builder = UnitBuilder::<Rc>::new(fixture_unit(path("calls")));
        builder.unit.header.stage = Rc::KIND;
        let value = builder.plain(ValueRepr::Scalar(ScalarKind::U64), scalar);
        builder.parameter(value);
        let entry = builder.block(BlockKind::Body, &[]);
        let dead = builder.block(BlockKind::Body, &[]);
        let done = builder.block(BlockKind::Body, &[]);
        let failed = builder.block(BlockKind::FailureCleanup, &[]);
        let arguments = [(Passing::Borrow, ArgumentValue::Value(value))];
        let dead_callee = builder.direct(path("dead"));
        let dead_call = builder.call(dead_callee, &arguments, None);
        let kept_callee = builder.direct(path("kept"));
        let kept_call = builder.call(kept_callee, &arguments, None);
        // A detached row need not belong to even an unreachable block.
        builder.call(dead_callee, &arguments, None);
        let normal = builder.edge(done, &[]);
        let cleanup = builder.edge(failed, &[]);
        let failure = builder.failure(cleanup, None, &[]);
        for (block, call) in [(entry, kept_call), (dead, dead_call)] {
            builder.term(
                block,
                Terminator::Call {
                    call,
                    normal: Some(normal),
                    failure,
                },
            );
        }
        builder.term(done, Terminator::Return(None));
        builder.term(failed, Terminator::Propagate);
        let unit = Editor::new(builder.finish_with_layouts(&table)).finish();
        assert_eq!(unit.check_identity(&table), Ok(()));
        assert_eq!(unit.tables.calls.len(), 1);
        assert_eq!(unit.tables.calls[0].callee, kept_callee);
        assert_eq!(unit.pools.call_arguments.len(), 1);
        assert!(matches!(
            unit.terminators[0],
            Terminator::Call {
                call: CallId(0),
                ..
            }
        ));
    }

    /// `PVec::join` skips dead storage meeting a state that absorbs it: the
    /// predicate must say exactly when the join would change nothing.
    #[test]
    fn a_slot_absorbs_dead_storage_exactly_when_the_join_changes_nothing() {
        let tris = [Tri::No, Tri::Yes, Tri::Maybe];
        let mut paths = Paths::default();
        let path = paths.intern(&[Step::Field(0)]);
        let row = |t: Option<Tri>| {
            let mut row = Tris::EMPTY;
            if let Some(t) = t {
                row.set(path, t);
            }
            row
        };
        for live in tris {
            for init in tris {
                for moved in [None, Some(Tri::No), Some(Tri::Yes), Some(Tri::Maybe)] {
                    for taken in [None, Some(Tri::Yes), Some(Tri::Maybe)] {
                        for pending in [None, Some(Tri::No), Some(Tri::Yes), Some(Tri::Maybe)] {
                            for version in [None, Some(ValueId(3))] {
                                let state = SlotState {
                                    live,
                                    init,
                                    moved: row(moved),
                                    taken: row(taken),
                                    fields_pending: row(pending),
                                    versions: version.into(),
                                    owned_empty: false,
                                };
                                let mut joined = state.clone();
                                joined.join(&SlotState::DEAD);
                                let absorbs =
                                    SlotState::absorbs(state.flags(), SlotState::DEAD.flags());
                                assert_eq!(absorbs, joined == state, "{state:?}");
                                assert_eq!(state.flags() & DEAD != 0, state == SlotState::DEAD);
                                assert_eq!(state.flags() & EMPTY != 0, state.init == Tri::No);
                                let mut same = state.clone();
                                same.join(&state);
                                assert_eq!(same, state, "a join is idempotent");
                            }
                        }
                    }
                }
            }
        }
        for t in tris {
            assert_eq!(
                Tri::absorbs(t.flags(), Tri::No.flags()),
                t.join(Tri::No) == t
            );
            assert_eq!(t.flags() & DEAD != 0, t == Tri::No);
        }
    }
    #[test]
    fn version_renaming_reaches_planned_detaches_and_failure_transports() {
        use hirsel_heapcheck::build::{Layouts, fixture_unit, path};
        use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
        use hirsel_lir::testing::UnitBuilder;
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let scalar = layouts.scalar(ScalarKind::U64);
        let (_, table) = layouts.finish();
        let repr = ValueRepr::Scalar(ScalarKind::U64);
        let mut builder = UnitBuilder::<Rc>::new(fixture_unit(path("rename")));
        let from = builder.plain(repr, scalar);
        let to = builder.plain(repr, scalar);
        let result = builder.plain(repr, scalar);
        let slot = builder.slot(repr, scalar, SlotKind::Local);
        let address = builder.address(AddressBase::Slot(slot), &[], repr, scalar);
        let entry = builder.block(BlockKind::Body, &[]);
        let rest = builder.block(BlockKind::Body, &[to]);
        let later = builder.block(BlockKind::Body, &[]);
        let failed_value = builder.plain(repr, scalar);
        let failed = builder.block(BlockKind::FailureCleanup, &[failed_value]);
        let into = builder.edge(rest, &[from]);
        builder.term(entry, Terminator::Jump(into));
        let cleanup = builder.edge(failed, &[from]);
        let failure = builder.failure(cleanup, None, &[]);
        builder.push(rest, Inst::Address { address, failure });
        builder.push(
            rest,
            Inst::Read {
                result,
                address,
                reaching: from,
                mode: ReadMode::Bits,
            },
        );
        let next = builder.edge(later, &[]);
        builder.term(rest, Terminator::Jump(next));
        // Feedback transports the joined version rather than reviving `from`.
        let back = builder.edge(rest, &[from]);
        builder.term(later, Terminator::Jump(back));
        builder.term(failed, Terminator::Propagate);
        let mut ed = Editor::new(builder.finish_with_layouts(&table));
        let mut targets = [Target {
            block: later,
            index: 0,
            slot,
            prefixes: Vec::new(),
            version: from,
            failure: Err(None),
            lend: None,
        }];
        rename_version(&mut ed, rest, from, to, &mut targets);
        assert_eq!(targets[0].version, to);
        assert!(matches!(ed.code[rest.index()][1], Inst::Read { reaching, .. } if reaching == to));
        for edge in [cleanup, back] {
            assert_eq!(
                ed.unit.tables.edges[edge.index()]
                    .arguments
                    .get(&ed.unit.pools.values),
                Some([to].as_slice())
            );
        }
        assert_eq!(
            ed.unit.tables.edges[into.index()]
                .arguments
                .get(&ed.unit.pools.values),
            Some([from].as_slice())
        );
    }

    /// `finish` renumbers edges, so it rebuilds the switch-arm pool from the
    /// switches it keeps: an arm range left by a removed block (or replaced by
    /// a rewrite) would keep edge ids past the compacted table, and a range two
    /// kept switches share would be renumbered twice.
    #[test]
    fn finishing_keeps_only_the_arms_of_kept_switches_renumbered_once() {
        use hirsel_heapcheck::build::{Layouts, fixture_unit, path};
        use hirsel_lir::ir::SwitchArm;
        use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
        use hirsel_lir::testing::UnitBuilder;
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let scalar = layouts.scalar(ScalarKind::U64);
        let (_, table) = layouts.finish();
        let repr = ValueRepr::Scalar(ScalarKind::U64);
        let mut builder = UnitBuilder::<Rc>::new(fixture_unit(path("arms")));
        builder.unit.header.stage = Rc::KIND;
        let tag = builder.plain(repr, scalar);
        builder.parameter(tag);
        let entry = builder.block(BlockKind::Body, &[]);
        let dead = builder.block(BlockKind::Body, &[]);
        let left = builder.block(BlockKind::Body, &[]);
        let right = builder.block(BlockKind::Body, &[]);
        let done = builder.block(BlockKind::Body, &[]);
        // The removed block's edges come first and last, so kept edges move
        // down and the removed arm names the highest edge.
        let dead_else = builder.edge(done, &[]);
        let to_left = builder.edge(left, &[]);
        let to_right = builder.edge(right, &[]);
        let to_done = builder.edge(done, &[]);
        let dead_arm = builder.edge(done, &[]);
        let shared = PoolRange::push(
            &mut builder.unit.pools.switch_arms,
            [SwitchArm {
                value: 0,
                edge: to_left,
            }],
        );
        let removed = PoolRange::push(
            &mut builder.unit.pools.switch_arms,
            [SwitchArm {
                value: 0,
                edge: dead_arm,
            }],
        );
        builder.term(
            entry,
            Terminator::Switch {
                value: tag,
                arms: shared,
                otherwise: to_right,
            },
        );
        builder.term(
            dead,
            Terminator::Switch {
                value: tag,
                arms: removed,
                otherwise: dead_else,
            },
        );
        builder.term(
            left,
            Terminator::Switch {
                value: tag,
                arms: shared,
                otherwise: to_done,
            },
        );
        builder.term(right, Terminator::Jump(to_done));
        builder.term(done, Terminator::Return(None));
        let unit = Editor::new(builder.finish_with_layouts(&table)).finish();
        assert_eq!(unit.check_identity(&table), Ok(()));
        assert_eq!(unit.tables.edges.len(), 3);
        assert_eq!(unit.pools.switch_arms.len(), 2);
        let left = BlockId(1);
        for b in [0, 1] {
            let Terminator::Switch { arms, .. } = unit.terminators[b] else {
                panic!("block {b} keeps its switch");
            };
            let [arm] = arms.get(&unit.pools.switch_arms).unwrap() else {
                panic!("one arm");
            };
            assert_eq!(unit.tables.edges[arm.edge.index()].target, left);
        }
    }
}
