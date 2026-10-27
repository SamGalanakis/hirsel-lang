//! Structure: vocabulary, arity and types, SSA definitions and dominance,
//! block arguments, address typing and preparation, edges and block kinds,
//! placement, and the static facts the flow phases read.
use super::flow::{Cfg, Origin, Out};
use super::{Cx, Report, Site, ViolationKind as K};
use crate::abi_types::{CallMultiplicity, CallableShape, FnAbi};
use crate::effect::Opcode;
use crate::ir::{
    AddressBase, AddressId, ArgumentValue, ArithMode, BinaryOp, BlockId, BlockKind, CallSite,
    Callee, CaptureUse, ConstantValue, ConstructId, ConstructorKind, ConvertMode, DestinationClass,
    EdgeId, FailureId, FlatFieldOp, Holding, Inst, LirType, LoanId, MaskRef, Placement,
    ProjectionStep, ReadMode, RegionInstanceId, ScopeId, SlotId, SlotKind, StorageOwner,
    SymbolKind, TaskDomainId, Terminator, UnitOrigin, ValueId, ValueRepr,
};
use crate::layout_types::{
    CopyRecipe, LayoutId, LayoutKind, LayoutRecord, ScalarKind, SizeSpec, StorageKind,
};
use crate::stage::{Stage, StageKind};
use hirsel_core_contract::Passing;
use std::collections::BTreeSet;

/// Where a value is defined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Def {
    Undefined,
    /// A declared parameter, defined at function entry.
    Entry,
    Param(BlockId, u32),
    Inst(BlockId, u32),
    /// A call result, defined on the call's normal edge.
    Edge(EdgeId),
    /// A `mut` call's write-back version, defined by the call terminator of
    /// this block on both its normal and its failure edge.
    Call(BlockId),
}

/// One field-path step; index steps compare as overlapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Step {
    Payload,
    Field(u32),
    Variant(u32, u32),
    Index,
    /// A statically bounded position in fixed inline storage.
    Inline(u64),
    /// An element slot named without a count check (LIR schema revision 7):
    /// which slots hold an element is dynamic, so no initialization state is
    /// kept below it. It overlaps every element step.
    Slot,
}

impl Step {
    fn element(self) -> bool {
        matches!(self, Step::Index | Step::Inline(_) | Step::Slot)
    }
}

/// Whether the place lies in an element slot named without a count check.
pub(super) fn through_slot(path: &[Step]) -> bool {
    path.contains(&Step::Slot)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Root {
    Slot(SlotId),
    Value(ValueId),
}

/// A flattened address: root, field path, the path lengths after which the
/// walk dereferences a counted block, and the layout reached.
#[derive(Clone, Debug)]
pub(super) struct Addr {
    pub root: Root,
    pub path: Vec<Step>,
    pub crossings: Vec<usize>,
    pub layout: LayoutId,
    /// Inside a dynamic-size part of a flat value: fixed at construction
    /// (MEM-12), since rewriting it would move every later field's cursor.
    pub flat_variable: bool,
    /// One whole element of a sequential Packed sequence (MEM-13): its end
    /// is its start plus its own extent, so `FlatNext` may name it.
    pub sequential_element: bool,
    /// Inside the value a `Shared` root freezes (MEM-11): read only by a
    /// borrow, in a library unit; never written, moved, copied or dropped.
    pub frozen: bool,
}

impl Addr {
    /// The path lengths whose content locates the place: each counted block
    /// dereference, and each tag or bounds check. A write at or above one of
    /// them may move the place; one below all of them cannot.
    pub fn anchors(&self) -> Vec<usize> {
        let mut out = self.crossings.clone();
        out.extend(self.path.iter().enumerate().filter_map(|(i, s)| {
            matches!(s, Step::Variant(..) | Step::Index | Step::Slot).then_some(i)
        }));
        out
    }
}

/// What a borrowed view's validity rests on.
#[derive(Clone, Debug)]
pub(super) enum Backing {
    None,
    /// A part of a slot's content, as of the read.
    Slot {
        slot: SlotId,
        path: Vec<Step>,
        crossings: Vec<usize>,
    },
    /// The payload of another value, valid while that value is.
    Value(ValueId),
}

/// A borrowed block parameter: the views it may be, by its incoming
/// arguments (block parameters flattened), and the loans covering them. With
/// more than one loan it is a joined view, covered by the union of its
/// incoming loans: valid while none of them has ended since the join and no
/// part of a slot under any of its views has been written.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Join {
    pub loans: BTreeSet<LoanId>,
    pub sources: BTreeSet<ValueId>,
}

impl Join {
    pub fn joined(&self) -> bool {
        self.loans.len() > 1
    }
}

pub(super) struct Facts {
    pub borrowed_holders: Vec<BTreeSet<LoanId>>,
    /// Borrowed views retained by a holder, preserving joined-view identity.
    pub borrowed_sources: Vec<BTreeSet<ValueId>>,
    pub cfg: Cfg,
    pub defs: Vec<Def>,
    /// Values naming a slot's content (store, replace and `mut` call
    /// versions, version joins, slot-backed parameters and capture versions).
    pub version: Vec<Option<SlotId>>,
    /// Per block, its version-join parameters as (ordinal, slot).
    pub phis: Vec<Vec<(u32, SlotId)>>,
    /// Per block, its drop-flag parameters as (ordinal, slot) (LIR schema
    /// revision 8).
    pub flags: Vec<Vec<(u32, SlotId, Vec<Step>)>>,
    /// Stores proven to clear a persistent environment live bit.
    pub capture_mask_stores: BTreeSet<Site>,
    pub addresses: Vec<Option<Addr>>,
    pub backing: Vec<Backing>,
    /// Compiler-local regions a mark or uncounted value lives in.
    pub regions: Vec<BTreeSet<RegionInstanceId>>,
    /// Values taken whole out of a slot.
    pub taken_from: Vec<Option<SlotId>>,
    /// Values a `Take` read moved out of a place, whole or a part.
    pub moved: Vec<bool>,
    /// Per declared parameter: its convention and its slot, if any.
    pub parameters: Vec<(Passing, Option<SlotId>)>,
    /// Values of program-lifetime static storage.
    pub statics: Vec<bool>,
    /// Per borrowed block parameter, the views and loans it joins.
    pub joins: Vec<Option<Join>>,
}

/// A use position: instruction ordinal, or the instruction count for the
/// terminator; `edge` is set for an edge's own arguments.
#[derive(Clone, Copy)]
struct At {
    block: BlockId,
    pos: u32,
    edge: Option<EdgeId>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Usage {
    /// An ordinary operand; never a version name.
    Operand,
    /// A `reaching` operand; always a version name.
    Reaching,
    /// A block argument; a version name exactly when its parameter is one.
    Argument,
}

/// The edges a terminator names, normal ones first.
pub(super) fn terminator_edges<S>(cx: &Cx<'_, S>, term: &Terminator) -> Vec<EdgeId> {
    let mut edges = term.normal_edges(&cx.unit.pools);
    for failure in term.failure_edges(&cx.unit.tables) {
        edges.push(cx.unit.tables.failures[failure.index()].cleanup);
    }
    edges
}

pub(super) fn overlap(a: &[Step], b: &[Step]) -> bool {
    a.iter().zip(b).all(|(x, y)| {
        x == y
            || (x.element() && y.element() && !matches!((x, y), (Step::Inline(_), Step::Inline(_))))
    })
}

/// Whether a write to `written` invalidates a view of `viewed`: the paths
/// overlap, or the view reaches its part through a counted block the write
/// also reaches, which a copy-on-write may detach (#77).
pub(super) fn invalidates(written: &[Step], viewed: &[Step], crossings: &[usize]) -> bool {
    if overlap(written, viewed) {
        return true;
    }
    let common = written
        .iter()
        .zip(viewed)
        .take_while(|(x, y)| x == y || (x.element() && y.element()))
        .count();
    crossings
        .iter()
        .any(|&k| k <= common && k < viewed.len() && k < written.len())
}

struct Checker<'c, 'a, S> {
    cx: &'c Cx<'a, S>,
    report: &'c mut Report,
    defs: Vec<Def>,
    operation_abis: std::collections::HashMap<
        (
            crate::ir::OperationRef,
            crate::FnAbiId,
            Vec<crate::operation::OperationArgument>,
        ),
        bool,
    >,
}

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, report: &mut Report) -> Option<Facts> {
    let unit = cx.unit;
    let mut c = Checker {
        cx,
        report,
        defs: vec![Def::Undefined; unit.values.len()],
        operation_abis: Default::default(),
    };
    c.vocabulary();
    let ranges_ok = c.instruction_ranges();
    c.types_and_holdings();
    if !ranges_ok {
        return None;
    }
    c.definitions();
    let cfg = c.cfg()?;
    let addresses = c.addresses();
    let parameters = c.parameters();
    c.captures();
    c.scopes();
    let mut facts = Facts {
        borrowed_holders: vec![BTreeSet::new(); unit.values.len()],
        borrowed_sources: vec![BTreeSet::new(); unit.values.len()],
        cfg,
        defs: std::mem::take(&mut c.defs),
        version: vec![None; unit.values.len()],
        phis: vec![Vec::new(); unit.blocks.len()],
        flags: vec![Vec::new(); unit.blocks.len()],
        capture_mask_stores: BTreeSet::new(),
        addresses,
        backing: vec![Backing::None; unit.values.len()],
        regions: vec![BTreeSet::new(); unit.values.len()],
        taken_from: vec![None; unit.values.len()],
        moved: vec![false; unit.values.len()],
        parameters,
        statics: vec![false; unit.values.len()],
        joins: vec![None; unit.values.len()],
    };
    c.block_kinds(&facts);
    c.instructions(&mut facts);
    c.terminators(&facts);
    c.isolate_protocol();
    c.versions(&mut facts);
    c.init_flags(&mut facts);
    c.capture_mask(&mut facts);
    c.codeless(&facts);
    c.regions(&mut facts);
    c.arena_tokens(&facts);
    c.joins(&mut facts);
    borrowed_holders(cx, &mut facts);
    c.uses(&facts);
    Some(facts)
}

/// Holder provenance is separate from ownership: a Cursor is plain but its
/// uncounted fields must remain under the source loan. Replayed from SSA and
/// storage versions; no producer's lifetime answer is consulted.
fn borrowed_holders<S: Stage>(cx: &Cx<'_, S>, facts: &mut Facts) {
    let u = cx.unit;
    let view_values: Vec<_> = u
        .values
        .iter()
        .map(|v| {
            cx.layouts
                .get(u.types[v.ty.index()].layout)
                .is_some_and(|r| {
                    r.fields
                        .iter()
                        .any(|f| crate::layout::field_is_borrowed(cx.layouts.0, r, f))
                })
        })
        .collect();
    let views = |v: ValueId| view_values[v.index()];
    let mut dependencies = vec![Vec::<ValueId>::new(); u.values.len()];
    let recasts: BTreeSet<_> = u
        .instructions
        .iter()
        .filter_map(|i| {
            if let Inst::Recast { result, .. } = i {
                Some(*result)
            } else {
                None
            }
        })
        .collect();
    for (i, value) in u.values.iter().enumerate() {
        if let Holding::Borrowed(loan) = value.holding {
            facts.borrowed_holders[i].insert(loan);
            if !recasts.contains(&ValueId(i as u32)) {
                facts.borrowed_sources[i].insert(ValueId(i as u32));
            }
        }
    }
    if !view_values.iter().any(|v| *v) {
        return;
    }
    for edge in &u.tables.edges {
        for (&p, &a) in u.blocks[edge.target.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
        {
            if views(p) {
                dependencies[p.index()].push(a);
            }
        }
    }
    for inst in &u.instructions {
        match *inst {
            Inst::Recast { result, value } | Inst::SliceView { result, value, .. }
                if u.values[result.index()].holding != Holding::Owned =>
            {
                dependencies[result.index()].push(value)
            }
            Inst::Read {
                result, reaching, ..
            } if views(result) => dependencies[result.index()].push(reaching),
            Inst::Store { value, version, .. } if views(version) => {
                dependencies[version.index()].push(value)
            }
            Inst::Construct {
                result, construct, ..
            }
            | Inst::Reuse {
                result, construct, ..
            } => {
                let site = &u.tables.constructs[construct.index()];
                if let Some(record) = cx.layouts.get(site.layout) {
                    for f in site.fields.get(&u.pools.fields).unwrap_or(&[]) {
                        if views(f.value)
                            || record.fields.iter().any(|d| {
                                d.ordinal == f.field
                                    && crate::layout::field_is_borrowed(cx.layouts.0, record, d)
                            })
                        {
                            dependencies[result.index()].push(f.value);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // Replay the root version at each partial write. Persistent states share
    // untouched slots; no replacement scans every value of its root slot.
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Version {
        None,
        One(ValueId),
        Many,
    }
    impl crate::persistent::Element for Version {
        fn flags(&self) -> u8 {
            u8::from(*self == Self::Many)
        }
        fn absorbs(own: u8, _: u8) -> bool {
            own == 1
        }
    }
    use crate::persistent::PVec;
    let mut entry: PVec<_> = (0..u.slots.len()).map(|_| Version::None).collect();
    for &v in &u.parameters {
        if let Some(slot) = facts.version[v.index()] {
            entry[slot.index()] = Version::One(v);
        }
    }
    let edge_state = |edge: EdgeId, state: &PVec<Version>| {
        let mut state = state.clone();
        let target = u.tables.edges[edge.index()].target;
        let ps = u.blocks[target.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[]);
        for &(ordinal, slot) in &facts.phis[target.index()] {
            state[slot.index()] = Version::One(ps[ordinal as usize]);
        }
        state
    };
    super::flow::solve(
        &facts.cfg,
        entry,
        |block, initial| {
            let mut state = initial.clone();
            let mut edges = Vec::new();
            for inst in u.blocks[block.index()]
                .instructions
                .get(&u.instructions)
                .unwrap_or(&[])
            {
                if let Inst::Replace {
                    version,
                    value,
                    address,
                    ..
                } = *inst
                {
                    if views(version) {
                        if let Some(slot) = facts.version[version.index()] {
                            if facts.addresses[address.index()]
                                .as_ref()
                                .is_some_and(|a| !a.path.is_empty())
                            {
                                if let Version::One(before) = state[slot.index()] {
                                    dependencies[version.index()].push(before);
                                }
                            }
                            if views(value) {
                                dependencies[version.index()].push(value);
                            }
                        }
                    }
                }
                for v in Inst::defs(inst) {
                    if let Some(slot) = facts.version[v.index()] {
                        state[slot.index()] = Version::One(v);
                    }
                }
                if let Some(failure) = Inst::failure(inst) {
                    let edge = u.tables.failures[failure.index()].cleanup;
                    edges.push((edge, edge_state(edge, &state)));
                }
            }
            if let Terminator::Call { call, .. } = u.terminators[block.index()] {
                for arg in u.tables.calls[call.index()]
                    .arguments
                    .get(&u.pools.call_arguments)
                    .unwrap_or(&[])
                {
                    if let ArgumentValue::Address { version, .. } = arg.value {
                        if let Some(slot) = facts.version[version.index()] {
                            if views(version) {
                                if let Version::One(before) = state[slot.index()] {
                                    dependencies[version.index()].push(before);
                                }
                            }
                            state[slot.index()] = Version::One(version);
                        }
                    }
                }
            }
            for out in &facts.cfg.succ[block.index()] {
                if out.origin == Origin::Terminator {
                    edges.push((out.edge, edge_state(out.edge, &state)));
                }
            }
            edges
        },
        |a, b| {
            a.join(b, |a, b| {
                *a = match (*a, *b) {
                    (Version::None, x) | (x, Version::None) => x,
                    (Version::One(x), Version::One(y)) if x == y => Version::One(x),
                    _ => Version::Many,
                };
            });
        },
    );
    for ds in &mut dependencies {
        ds.sort_unstable();
        ds.dedup();
    }
    for site in &u.tables.calls {
        if let (Some(result), Some(abi)) = (
            site.result,
            cx.layouts.0.fn_abis[site.abi.0 as usize].result.as_ref(),
        ) {
            for a in site.arguments.get(&u.pools.call_arguments).unwrap_or(&[]) {
                if abi.borrowed_parameters.contains(&a.parameter) {
                    if let ArgumentValue::Value(value) = a.value {
                        dependencies[result.index()].push(value);
                    }
                }
            }
        }
    }
    let mut pending: std::collections::VecDeque<_> = (0..u.values.len()).collect();
    let mut users = vec![Vec::new(); u.values.len()];
    for (v, ds) in dependencies.iter().enumerate() {
        for d in ds {
            users[d.index()].push(v);
        }
    }
    while let Some(v) = pending.pop_front() {
        let mut loans = facts.borrowed_holders[v].clone();
        let mut sources = facts.borrowed_sources[v].clone();
        for d in &dependencies[v] {
            loans.extend(&facts.borrowed_holders[d.index()]);
            sources.extend(&facts.borrowed_sources[d.index()]);
        }
        if loans != facts.borrowed_holders[v] || sources != facts.borrowed_sources[v] {
            facts.borrowed_holders[v] = loans;
            facts.borrowed_sources[v] = sources;
            pending.extend(&users[v]);
        }
    }
}

impl<S: Stage> Checker<'_, '_, S> {
    fn add(&mut self, site: Site, kind: K) {
        self.report.add(site, kind);
    }

    fn ty(&self, value: ValueId) -> &LirType {
        &self.cx.unit.types[self.cx.unit.values[value.index()].ty.index()]
    }

    fn same_type(&self, a: ValueId, b: ValueId) -> bool {
        self.ty(a) == self.ty(b)
    }

    fn layout_of(&self, value: ValueId) -> LayoutId {
        self.ty(value).layout
    }

    fn vocabulary(&mut self) {
        let unit = self.cx.unit;
        for (b, block) in unit.blocks.iter().enumerate() {
            let b = BlockId(b as u32);
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                let early_unique = self.cx.stage < StageKind::Rc
                    && unit.header.origin == crate::ir::UnitOrigin::Library
                    && matches!(inst, Inst::IsUnique { .. });
                if !self.cx.stage.admits(inst.opcode()) && !early_unique {
                    let kind =
                        if matches!(inst, Inst::FinalizeBegin { .. } | Inst::FinalizeEnd { .. }) {
                            K::FinalizationStage(inst.opcode())
                        } else {
                            K::Vocabulary(inst.opcode())
                        };
                    self.add(Site::at(b, i as u32), kind);
                }
            }
            let term = &unit.terminators[b.index()];
            if !self.cx.stage.admits(term.opcode()) {
                self.add(
                    Site::at(b, insts.len() as u32),
                    K::Vocabulary(term.opcode()),
                );
            }
        }
    }

    /// Blocks own disjoint instruction ranges.
    fn instruction_ranges(&mut self) -> bool {
        let unit = self.cx.unit;
        let mut owner = vec![false; unit.instructions.len()];
        let mut ok = true;
        for (b, block) in unit.blocks.iter().enumerate() {
            let r = block.instructions;
            for i in r.start..r.start + r.len {
                if std::mem::replace(&mut owner[i as usize], true) {
                    ok = false;
                    self.add(
                        Site::block(BlockId(b as u32)),
                        K::BlockKind("instruction ranges overlap"),
                    );
                    break;
                }
            }
        }
        ok
    }

    /// Representations agree with layout kinds; holdings with representations.
    fn types_and_holdings(&mut self) {
        let unit = self.cx.unit;
        for (i, ty) in unit.types.iter().enumerate() {
            let Some(layout) = self.cx.layouts.get(ty.layout) else {
                continue;
            };
            let ok = match (ty.repr, &layout.kind) {
                (ValueRepr::PayloadReceipt, LayoutKind::PayloadReceipt { .. }) => true,
                (ValueRepr::Unit, LayoutKind::Unit) => true,
                (ValueRepr::Scalar(a), LayoutKind::Scalar(b)) => a == *b,
                (
                    ValueRepr::Block | ValueRepr::Uncounted,
                    LayoutKind::Block { .. } | LayoutKind::Box { .. },
                ) => true,
                // A `Shared` root is always a counted heap block (MEM-11).
                (ValueRepr::Block, LayoutKind::Shared { .. }) => true,
                (ValueRepr::Callable, LayoutKind::Callable { .. }) => true,
                (
                    ValueRepr::Handle,
                    LayoutKind::Handle
                    | LayoutKind::HostCapability { .. }
                    | LayoutKind::SealedRegion { .. },
                ) => true,
                (ValueRepr::Arena, LayoutKind::Arena) => true,
                (ValueRepr::Scalar(ScalarKind::U64), LayoutKind::Path) => true,
                (ValueRepr::Token, LayoutKind::Header) => true,
                (ValueRepr::Hole, LayoutKind::Box { .. }) => true,
                (ValueRepr::RegionMark | ValueRepr::InitMask { .. }, LayoutKind::Scalar(_)) => true,
                (
                    ValueRepr::Aggregate | ValueRepr::Uncounted,
                    LayoutKind::Aggregate
                    | LayoutKind::Environment { .. }
                    | LayoutKind::Variant
                    | LayoutKind::Packed
                    | LayoutKind::Inline { .. }
                    | LayoutKind::Simd { .. }
                    | LayoutKind::Slice { .. }
                    | LayoutKind::Opaque { .. }
                    | LayoutKind::Iterator { .. }
                    | LayoutKind::Resource
                    | LayoutKind::Flat
                    | LayoutKind::SoaColumns { .. }
                    | LayoutKind::Sequence { .. }
                    | LayoutKind::Sequential { .. },
                ) => true,
                _ => false,
            };
            if !ok {
                self.add(
                    Site::UNIT,
                    K::Representation(crate::ir::LirTypeId(i as u32)),
                );
            }
        }
        for (i, slot) in unit.slots.iter().enumerate() {
            if matches!(
                unit.types[slot.ty.index()].repr,
                ValueRepr::Token | ValueRepr::RegionMark
            ) {
                self.add(Site::UNIT, K::Representation(slot.ty));
                let _ = i;
            }
        }
        for (i, info) in unit.values.iter().enumerate() {
            let v = ValueId(i as u32);
            let ok = match (unit.types[info.ty.index()].repr, info.holding) {
                (ValueRepr::Token | ValueRepr::RegionMark, h) => h == Holding::Owned,
                // A hole is owned; a version of a slot holding one is plain.
                (ValueRepr::Hole, h) => matches!(h, Holding::Owned | Holding::Plain),
                (
                    ValueRepr::Unit
                    | ValueRepr::Scalar(_)
                    | ValueRepr::InitMask { .. }
                    | ValueRepr::Uncounted
                    | ValueRepr::Arena,
                    h,
                ) => h == Holding::Plain,
                (_, Holding::Borrowed(_)) => self.cx.owns(v),
                _ => true,
            };
            if !ok {
                self.add(Site::UNIT, K::Holding(v));
            }
        }
    }

    fn define(&mut self, value: ValueId, def: Def, site: Site) {
        let slot = &mut self.defs[value.index()];
        if *slot == Def::Undefined {
            *slot = def;
        } else {
            self.add(site, K::Redefined(value));
        }
    }

    fn definitions(&mut self) {
        let unit = self.cx.unit;
        for &p in unit.parameters.iter().chain(&unit.captures) {
            self.define(p, Def::Entry, Site::UNIT);
        }
        if let Some(entry) = &unit.tables.receipt_entry {
            for version in entry.versions() {
                self.define(version, Def::Entry, Site::UNIT);
            }
        }
        for (b, block) in unit.blocks.iter().enumerate() {
            let b = BlockId(b as u32);
            for (j, &p) in block
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .enumerate()
            {
                self.define(p, Def::Param(b, j as u32), Site::block(b));
            }
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                for v in Inst::defs(inst) {
                    self.define(v, Def::Inst(b, i as u32), Site::at(b, i as u32));
                }
            }
            if let Terminator::Call { call, normal, .. } = unit.terminators[b.index()] {
                let site = Site::at(b, insts.len() as u32);
                let call = &unit.tables.calls[call.index()];
                if let Some(normal) = normal {
                    for result in call.normal_results() {
                        self.define(result, Def::Edge(normal), site);
                    }
                }
                for version in mut_versions(self.cx, call) {
                    self.define(version, Def::Call(b), site);
                }
            }
        }
        for (i, def) in self.defs.clone().iter().enumerate() {
            if *def == Def::Undefined {
                self.add(Site::UNIT, K::Undefined(ValueId(i as u32)));
            }
        }
    }

    /// Every edge and failure edge is named by exactly one site; every block
    /// is reachable; the entry is an ordinary block without parameters.
    fn cfg(&mut self) -> Option<Cfg> {
        let unit = self.cx.unit;
        let t = &unit.tables;
        let mut edge_uses = vec![0u32; t.edges.len()];
        let mut failure_uses = vec![0u32; t.failures.len()];
        let mut outs = Vec::new();
        for (b, block) in unit.blocks.iter().enumerate() {
            let b = BlockId(b as u32);
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                if let Some(f) = Inst::failure(inst) {
                    failure_uses[f.index()] += 1;
                    let edge = t.failures[f.index()].cleanup;
                    edge_uses[edge.index()] += 1;
                    let target = t.edges[edge.index()].target;
                    outs.push((
                        b,
                        Out {
                            edge,
                            target,
                            origin: Origin::Instruction(i as u32),
                        },
                    ));
                }
            }
            let term = &unit.terminators[b.index()];
            for failure in term.failure_edges(&unit.tables) {
                failure_uses[failure.index()] += 1;
            }
            for edge in terminator_edges(self.cx, term) {
                edge_uses[edge.index()] += 1;
                let target = t.edges[edge.index()].target;
                outs.push((
                    b,
                    Out {
                        edge,
                        target,
                        origin: Origin::Terminator,
                    },
                ));
            }
        }
        let mut ok = true;
        for (e, &n) in edge_uses.iter().enumerate() {
            if n != 1 {
                ok = false;
                self.add(Site::UNIT, K::EdgeUse(EdgeId(e as u32)));
            }
        }
        for (f, &n) in failure_uses.iter().enumerate() {
            if n != 1 {
                ok = false;
                self.add(Site::UNIT, K::FailureUse(FailureId(f as u32)));
            }
        }
        if !ok {
            return None;
        }
        let cfg = Cfg::new(unit.blocks.len(), t.edges.len(), unit.entry, &outs);
        for b in 0..unit.blocks.len() {
            let b = BlockId(b as u32);
            if !cfg.reachable(b) {
                self.add(Site::block(b), K::UnreachableBlock(b));
            }
        }
        let entry = &unit.blocks[unit.entry.index()];
        if entry.kind != BlockKind::Body || entry.parameters.len != 0 {
            self.add(Site::block(unit.entry), K::BlockKind("entry block"));
        }
        Some(cfg)
    }

    /// Failure edges enter failure cleanup; failure cleanup never resumes
    /// ordinary code; only failure cleanup propagates and only other blocks
    /// return.
    fn block_kinds(&mut self, facts: &Facts) {
        let unit = self.cx.unit;
        for (b, outs) in facts.cfg.succ.iter().enumerate() {
            let b = BlockId(b as u32);
            let kind = unit.blocks[b.index()].kind;
            let term = &unit.terminators[b.index()];
            for out in outs {
                let target = unit.blocks[out.target.index()].kind;
                let failure_edge = match out.origin {
                    Origin::Instruction(_) => true,
                    Origin::Terminator => term
                        .failure_edges(&unit.tables)
                        .iter()
                        .any(|f| unit.tables.failures[f.index()].cleanup == out.edge),
                };
                // A recovery is the one way from failure cleanup back to ordinary code.
                let recovered = matches!(
                    (out.origin, term),
                    (Origin::Terminator, Terminator::Recover { caught, .. }
                        | Terminator::Isolate { caught, .. }) if *caught == out.edge
                );
                let ok = if recovered {
                    target == BlockKind::Body
                } else if failure_edge || kind == BlockKind::FailureCleanup {
                    target == BlockKind::FailureCleanup
                } else {
                    target != BlockKind::FailureCleanup
                };
                if !ok {
                    self.add(Site::block(b), K::BlockKind("failure path transfer"));
                }
            }
            let len = unit.blocks[b.index()].instructions.len;
            match term {
                Terminator::Return(_) if kind == BlockKind::FailureCleanup => self.add(
                    Site::at(b, len),
                    K::BlockKind("return from failure cleanup"),
                ),
                Terminator::Propagate if kind != BlockKind::FailureCleanup => self.add(
                    Site::at(b, len),
                    K::BlockKind("propagate outside failure cleanup"),
                ),
                Terminator::Recover { .. } if kind != BlockKind::FailureCleanup => self.add(
                    Site::at(b, len),
                    K::BlockKind("recover outside failure cleanup"),
                ),
                Terminator::Isolate { .. } if kind != BlockKind::FailureCleanup => self.add(
                    Site::at(b, len),
                    K::BlockKind("isolate outside failure cleanup"),
                ),
                _ => {}
            }
        }
    }

    /// An inline snapshot must fit frame storage and carry no linear resources.
    fn inline_spill(&self, value: ValueId) -> bool {
        matches!(
            self.ty(value).repr,
            ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Scalar(_) | ValueRepr::Handle
        ) && self.cx.layouts.fixed_size(self.layout_of(value)).is_some()
            && matches!(
                self.cx.layouts.copy(self.layout_of(value)),
                Some(CopyRecipe::Bits | CopyRecipe::Dup | CopyRecipe::Fields)
            )
    }

    /// Flatten every address, checking its projection against the layouts.
    fn addresses(&mut self) -> Vec<Option<Addr>> {
        let unit = self.cx.unit;
        let mut out: Vec<Option<Addr>> = vec![None; unit.addresses.len()];
        for (a, slot) in out.iter_mut().enumerate() {
            let mut chain = vec![AddressId(a as u32)];
            let mut cyclic = false;
            while let AddressBase::Address(base) =
                unit.addresses[chain.last().unwrap().index()].base
            {
                if chain.contains(&base) {
                    cyclic = true;
                    break;
                }
                chain.push(base);
            }
            if cyclic {
                self.add(Site::UNIT, K::AddressCycle(AddressId(a as u32)));
                continue;
            }
            // Walk from the innermost base outwards.
            let mut flat: Option<Addr> = None;
            for &id in chain.iter().rev() {
                let info = &unit.addresses[id.index()];
                let mut addr = match (info.base, flat.take()) {
                    (AddressBase::Slot(slot), _) => Addr {
                        root: Root::Slot(slot),
                        path: Vec::new(),
                        crossings: Vec::new(),
                        layout: unit.types[unit.slots[slot.index()].ty.index()].layout,
                        flat_variable: false,
                        sequential_element: false,
                        frozen: false,
                    },
                    (AddressBase::Value(value), _) => {
                        match self.cx.layouts.payload(self.layout_of(value)) {
                            Some(payload) => Addr {
                                root: Root::Value(value),
                                path: if matches!(
                                    self.cx.layouts.get(self.layout_of(value)).map(|r| &r.kind),
                                    Some(LayoutKind::Box { .. })
                                ) {
                                    vec![Step::Payload]
                                } else {
                                    Vec::new()
                                },
                                crossings: vec![0],
                                layout: payload,
                                flat_variable: false,
                                sequential_element: false,
                                frozen: false,
                            },
                            None if self.inline_spill(value) => Addr {
                                root: Root::Value(value),
                                path: Vec::new(),
                                crossings: Vec::new(),
                                layout: self.layout_of(value),
                                flat_variable: false,
                                sequential_element: false,
                                frozen: false,
                            },
                            None => {
                                self.add(Site::UNIT, K::AddressType(id));
                                flat = None;
                                break;
                            }
                        }
                    }
                    (AddressBase::Address(_), Some(base)) => base,
                    (AddressBase::Address(_), None) => break,
                };
                if let Root::Value(value) = addr.root {
                    addr.frozen |= unit.instructions.iter().any(|inst| {
                        matches!(inst,
                        Inst::BorrowSealedRoot { result, .. } if *result == value)
                    });
                }
                let steps = info.projection.get(&unit.pools.projections).unwrap_or(&[]);
                let mut ok = true;
                for step in steps {
                    match self.project(&mut addr, *step) {
                        true => {}
                        false => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok || addr.layout != unit.types[info.ty.index()].layout {
                    self.add(Site::UNIT, K::AddressType(id));
                    flat = None;
                    break;
                }
                flat = Some(addr);
            }
            *slot = flat;
        }
        out
    }

    /// Apply one projection step, dereferencing a counted block first. A field
    /// of a flat record is its FlatFormatV0 encoding at its cursor: the static
    /// offset plus each earlier variable field's u64 length prefix value; an
    /// element of a flat sequence encoding likewise lies at its cursor.
    fn project(&self, addr: &mut Addr, step: ProjectionStep) -> bool {
        let layouts = self.cx.layouts;
        if step == ProjectionStep::Payload {
            let Some(LayoutKind::Box { payload }) = layouts.get(addr.layout).map(|r| &r.kind)
            else {
                return false;
            };
            addr.crossings.push(addr.path.len());
            addr.path.push(Step::Payload);
            addr.layout = *payload;
            return true;
        }
        // Through a `Shared` root into its `Frozen` payload, only for
        // `shared.with`'s library body (MEM-11).
        if let Some(LayoutKind::Shared { payload }) = layouts.get(addr.layout).map(|r| &r.kind) {
            if self.cx.unit.header.origin != UnitOrigin::Library {
                return false;
            }
            addr.crossings.push(addr.path.len());
            addr.layout = *payload;
            addr.frozen = true;
        } else if let Some(payload) = layouts.payload(addr.layout) {
            addr.crossings.push(addr.path.len());
            if matches!(
                layouts.get(addr.layout).map(|r| &r.kind),
                Some(LayoutKind::Box { .. })
            ) {
                addr.path.push(Step::Payload);
            }
            addr.layout = payload;
        }
        let Some(layout) = layouts.get(addr.layout) else {
            return false;
        };
        let (next, step) = match step {
            ProjectionStep::Payload => unreachable!("handled before implicit dereference"),
            ProjectionStep::Field(f) => (
                layout
                    .fields
                    .iter()
                    .find(|x| x.ordinal == f)
                    .map(|x| x.layout),
                Step::Field(f),
            ),
            ProjectionStep::VariantField { variant, field } => (
                layout
                    .tag
                    .iter()
                    .flat_map(|t| &t.variants)
                    .find(|v| v.ordinal == variant)
                    .and_then(|v| v.fields.iter().find(|x| x.ordinal == field))
                    .map(|x| x.layout),
                Step::Variant(variant, field),
            ),
            ProjectionStep::Index(index) => (
                match layout.kind {
                    // Element i of a flat sequence encoding at its cursor: by
                    // stride, or past each earlier element's prefix (LIR
                    // schema revision 4).
                    LayoutKind::Flat => layouts.flat_sequence(addr.layout),
                    LayoutKind::Inline { element, .. }
                    | LayoutKind::Simd { element, .. }
                    | LayoutKind::Slice { element }
                    | LayoutKind::SoaColumns { element }
                    | LayoutKind::Sequence { element }
                    // Element i of a sequential Packed sequence, walked to (MEM-13).
                    | LayoutKind::Sequential { element } => Some(element),
                    _ => None,
                },
                match layout.kind {
                    LayoutKind::Inline { count, .. } => self
                        .cx
                        .unit
                        .instructions
                        .iter()
                        .find_map(|inst| match *inst {
                            Inst::Const { result, constant } if result == index => {
                                match self.cx.unit.tables.constants[constant.index()].value {
                                    ConstantValue::Integer(n) if n < u128::from(count) => {
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
            // An element slot of a counted sequence block, named without a
            // count check (LIR schema revision 7).
            ProjectionStep::Slot(_) => (
                match layout.kind {
                    LayoutKind::Sequence { element } | LayoutKind::SoaColumns { element }
                        if !addr.crossings.is_empty() =>
                    {
                        Some(element)
                    }
                    _ => None,
                },
                Step::Slot,
            ),
            // An element of a flat sequence of variable elements, at a known
            // content offset (LIR schema revision 7), or of a sequential Packed
            // sequence, at a known offset from its first element (MEM-13).
            ProjectionStep::FlatAt { .. } => (
                match (&layout.kind, layouts.flat_prefixed(addr.layout)) {
                    (LayoutKind::Sequential { element }, _) => Some(*element),
                    (_, Some((16, element))) => Some(element),
                    _ => None,
                },
                Step::Index,
            ),
        };
        match next {
            Some(next) => {
                if layout.kind == LayoutKind::Flat
                    && layouts
                        .get(next)
                        .is_some_and(|r| matches!(r.size, SizeSpec::Dynamic(_)))
                {
                    addr.flat_variable = true;
                }
                addr.sequential_element = matches!(layout.kind, LayoutKind::Sequential { .. })
                    && matches!(step, Step::Index);
                addr.path.push(step);
                addr.layout = next;
                true
            }
            None => false,
        }
    }

    /// The content record of the flat list builder at `address`: a block of a
    /// sequential sequence of prefixed variable flat records (LIR schema
    /// revision 22).
    fn builder_content(&self, facts: &Facts, address: AddressId) -> Option<LayoutId> {
        let layouts = self.cx.layouts;
        let element = self
            .address_layout(facts, address)
            .and_then(|l| layouts.payload(l))
            .and_then(|p| layouts.get(p))
            .and_then(|r| match r.kind {
                LayoutKind::Sequential { element } => Some(element),
                _ => None,
            })?;
        let (offset, content) = layouts.flat_prefixed(element)?;
        (offset == 8 && layouts.fixed_size(content).is_none()).then_some(content)
    }

    /// Whether `address` lies inside the frozen value of a `Shared` root.
    fn frozen(&self, facts: &Facts, address: AddressId) -> bool {
        facts.addresses[address.index()]
            .as_ref()
            .is_some_and(|a| a.frozen)
    }

    /// Whether `address` lies inside a dynamic-size part of a flat value.
    fn flat_variable(&self, facts: &Facts, address: AddressId) -> bool {
        facts.addresses[address.index()]
            .as_ref()
            .is_some_and(|a| a.flat_variable)
    }

    /// Declared parameters against the unit's own `FnAbi`, and their slots.
    fn parameters(&mut self) -> Vec<(Passing, Option<SlotId>)> {
        let unit = self.cx.unit;
        let abi = &self.cx.layouts.0.fn_abis[unit.header.abi.0 as usize];
        if unit.header.track_caller && !abi.caller_site {
            self.add(Site::UNIT, K::Type("caller metadata"));
        }
        if unit.parameters.len() != abi.parameters.len() {
            self.add(Site::UNIT, K::Arity("parameters"));
        }
        let mut out = Vec::new();
        for (i, p) in abi.parameters.iter().enumerate() {
            let slots: Vec<_> = unit
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.kind == SlotKind::Parameter(p.source))
                .map(|(s, _)| SlotId(s as u32))
                .collect();
            if slots.len() > 1 || (p.convention == Passing::Mut && slots.is_empty()) {
                self.add(Site::UNIT, K::Arity("parameter slots"));
            }
            let slot = slots.first().copied();
            if let Some(&value) = unit.parameters.get(i) {
                if self.layout_of(value) != p.layout
                    || slot
                        .is_some_and(|s| unit.slots[s.index()].ty != unit.values[value.index()].ty)
                {
                    self.add(Site::UNIT, K::Type("parameter"));
                }
                let holding = unit.values[value.index()].holding;
                let expected = match (p.convention, slot) {
                    (Passing::Take, None) if self.cx.owns(value) => Holding::Owned,
                    _ => Holding::Plain,
                };
                if holding != expected {
                    self.add(Site::UNIT, K::Holding(value));
                }
            }
            if p.source != i as u32 {
                self.add(Site::UNIT, K::CallShape("parameters out of ordinal order"));
            }
            out.push((p.convention, slot));
        }
        out
    }

    fn fresh_holding(&mut self, site: Site, value: ValueId) {
        let expected = if self.cx.owns(value) {
            Holding::Owned
        } else {
            Holding::Plain
        };
        if self.cx.unit.values[value.index()].holding != expected {
            self.add(site, K::Holding(value));
        }
    }

    fn scalar(&self, value: ValueId) -> Option<ScalarKind> {
        match self.ty(value).repr {
            ValueRepr::Scalar(k) => Some(k),
            _ => None,
        }
    }

    fn instructions(&mut self, facts: &mut Facts) {
        let unit = self.cx.unit;
        let mut prepared = vec![false; unit.addresses.len()];
        for (b, block) in unit.blocks.iter().enumerate() {
            let b = BlockId(b as u32);
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                let site = Site::at(b, i as u32);
                if let Inst::Address { address, .. } = *inst
                    && std::mem::replace(&mut prepared[address.index()], true)
                {
                    self.add(site, K::AddressPreparedTwice(address));
                }
                self.instruction(site, inst, facts);
            }
        }
    }

    fn slot_root(&mut self, site: Site, facts: &Facts, address: AddressId) -> Option<SlotId> {
        match facts.addresses[address.index()].as_ref()?.root {
            Root::Slot(slot) => Some(slot),
            Root::Value(_) => {
                self.add(site, K::WriteThroughValue(address));
                None
            }
        }
    }

    fn address_layout(&self, facts: &Facts, address: AddressId) -> Option<LayoutId> {
        facts.addresses[address.index()].as_ref().map(|a| a.layout)
    }

    fn instruction(&mut self, site: Site, inst: &Inst, facts: &mut Facts) {
        let unit = self.cx.unit;
        let layouts = self.cx.layouts;
        match *inst {
            Inst::Const { result, constant } => {
                let data = &unit.tables.constants[constant.index()];
                let repr = self.ty(result).repr;
                // A String or Bytes literal as a slice over static storage: it
                // holds no credit, its owner being program-lifetime data.
                let static_slice = matches!(
                    data.value,
                    ConstantValue::String(_) | ConstantValue::Bytes(_)
                ) && repr == ValueRepr::Aggregate
                    && matches!(
                        layouts.get(self.layout_of(result)).map(|l| &l.kind),
                        Some(LayoutKind::Slice { .. })
                    );
                let ok = unit.types[data.ty.index()] == *self.ty(result)
                    && match (&data.value, repr) {
                        (ConstantValue::Heap, ValueRepr::Arena) => true,
                        (ConstantValue::Unit, ValueRepr::Unit) => true,
                        (ConstantValue::Bool(_), ValueRepr::Scalar(ScalarKind::Bool)) => true,
                        (ConstantValue::Integer(n), ValueRepr::Scalar(k)) => {
                            integer_bits(k).is_some_and(|bits| bits == 128 || n >> bits == 0)
                        }
                        (ConstantValue::F32(_), ValueRepr::Scalar(ScalarKind::F32)) => true,
                        (ConstantValue::F64(_), ValueRepr::Scalar(ScalarKind::F64)) => true,
                        // A registry index names a layout of this unit's table.
                        (ConstantValue::Descriptor(layout), ValueRepr::Scalar(ScalarKind::U32)) => {
                            layouts.get(*layout).is_some()
                        }
                        (
                            ConstantValue::String(_) | ConstantValue::Bytes(_),
                            ValueRepr::Block | ValueRepr::Uncounted,
                        ) => true,
                        _ => static_slice,
                    };
                if !ok {
                    self.add(site, K::Type("constant"));
                }
                if static_slice {
                    if unit.values[result.index()].holding != Holding::Plain {
                        self.add(site, K::Holding(result));
                    }
                } else {
                    self.fresh_holding(site, result);
                }
                facts.statics[result.index()] = true;
            }
            Inst::FunctionRef { result, symbol } => {
                // A callable, or a definition's C callback address (PROG-4).
                if unit.tables.symbols[symbol.index()].kind != SymbolKind::Function
                    || !matches!(
                        self.ty(result).repr,
                        ValueRepr::Callable | ValueRepr::Scalar(ScalarKind::Pointer)
                    )
                {
                    self.add(site, K::Type("function reference"));
                }
                self.fresh_holding(site, result);
                facts.statics[result.index()] = true;
            }
            Inst::ReadStatic { result, symbol } => {
                if !matches!(
                    unit.tables.symbols[symbol.index()].kind,
                    SymbolKind::Constant | SymbolKind::Shape
                ) {
                    self.add(site, K::Type("static read"));
                }
                self.fresh_holding(site, result);
                facts.statics[result.index()] = true;
            }
            Inst::Unary {
                result,
                op,
                operand,
                ..
            } => {
                use crate::ir::UnaryOp;
                let ok = match (op, self.scalar(operand)) {
                    (UnaryOp::Not, Some(ScalarKind::Bool)) => true,
                    (UnaryOp::Not | UnaryOp::BitNot, Some(k)) => integer_bits(k).is_some(),
                    (UnaryOp::Neg, Some(k)) => {
                        signed(k) || matches!(k, ScalarKind::F32 | ScalarKind::F64)
                    }
                    _ => false,
                };
                if !ok || !self.same_type(result, operand) {
                    self.add(site, K::Type("unary operand"));
                }
            }
            Inst::Binary {
                result,
                op,
                left,
                right,
                ..
            } => {
                use crate::ir::BinaryOp as B;
                let comparison = matches!(op, B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge);
                let kind = self.scalar(left);
                let float = matches!(kind, Some(ScalarKind::F32 | ScalarKind::F64));
                // The named library operations (LIR schema revision 6) hold
                // only in their own domain.
                let domain = match op {
                    B::DivFloor | B::Mod | B::DivExact => {
                        kind.is_some_and(|k| integer_bits(k).is_some())
                    }
                    B::Min | B::Max | B::MinNumber | B::MaxNumber => float,
                    _ => true,
                };
                let ok = kind.is_some()
                    && domain
                    && self.same_type(left, right)
                    && if comparison {
                        self.scalar(result) == Some(ScalarKind::Bool)
                    } else {
                        self.same_type(result, left)
                    };
                if !ok {
                    self.add(site, K::Type("binary operands"));
                }
            }
            Inst::Convert {
                result,
                flag,
                operand,
                mode,
            } => {
                let (from, to) = (self.scalar(operand), self.scalar(result));
                let bits = |k: Option<ScalarKind>| k.and_then(integer_bits);
                let flag_kind = flag.map(|f| self.scalar(f));
                let ok = match mode {
                    // SEM-1: widen requires the source's complete range to fit.
                    ConvertMode::Widen => {
                        matches!((from, to), (Some(a), Some(b)) if widens(a, b))
                            && flag_kind.is_none()
                    }
                    // truncate requires a no-wider destination (docs/spec/core-library.md).
                    ConvertMode::Truncate => {
                        matches!((bits(from), bits(to)), (Some(a), Some(b)) if b <= a)
                            && flag_kind.is_none()
                    }
                    ConvertMode::TryNarrow => {
                        bits(from).is_some()
                            && bits(to).is_some()
                            && flag_kind == Some(Some(ScalarKind::Bool))
                    }
                    ConvertMode::IntToFloat => {
                        bits(from).is_some()
                            && matches!(to, Some(ScalarKind::F32 | ScalarKind::F64))
                            && flag_kind.is_none()
                    }
                    ConvertMode::Reinterpret => {
                        matches!(
                            (from, to),
                            (Some(ScalarKind::F32), Some(ScalarKind::U32))
                                | (Some(ScalarKind::U32), Some(ScalarKind::F32))
                                | (Some(ScalarKind::F64), Some(ScalarKind::U64))
                                | (Some(ScalarKind::U64), Some(ScalarKind::F64))
                        ) && flag_kind.is_none()
                    }
                    ConvertMode::FloatResize => {
                        matches!(
                            (from, to),
                            (Some(ScalarKind::F32), Some(ScalarKind::F64))
                                | (Some(ScalarKind::F64), Some(ScalarKind::F32))
                        ) && flag_kind.is_none()
                    }
                    ConvertMode::FloatToI32 => {
                        matches!(from, Some(ScalarKind::F32 | ScalarKind::F64))
                            && to == Some(ScalarKind::I32)
                            && flag_kind == Some(Some(ScalarKind::I32))
                    }
                };
                if !ok {
                    self.add(site, K::Type("conversion"));
                }
                self.fresh_holding(site, result);
                if let Some(flag) = flag {
                    self.fresh_holding(site, flag);
                }
            }
            Inst::IntArith {
                result,
                flag,
                op,
                mode,
                left,
                right,
            } => {
                // Named checked/saturating arithmetic is Add, Sub and checked
                // Mul (revision 41); proof removal also uses the unchecked
                // integer operator forms.
                let ok = (op == BinaryOp::Add
                    || op == BinaryOp::Sub
                    || op == BinaryOp::Mul && mode == ArithMode::Checked
                    || mode == ArithMode::Wrapping
                        && matches!(
                            op,
                            BinaryOp::Sub
                                | BinaryOp::Mul
                                | BinaryOp::Div
                                | BinaryOp::Rem
                                | BinaryOp::Shl
                                | BinaryOp::Shr
                        ))
                    && self.scalar(left).is_some_and(|k| integer_bits(k).is_some())
                    && self.same_type(left, right)
                    && self.same_type(result, left)
                    && match (mode, flag) {
                        (ArithMode::Checked, Some(f)) => self.scalar(f) == Some(ScalarKind::Bool),
                        (ArithMode::Checked, None) => false,
                        (_, flag) => flag.is_none(),
                    };
                if !ok {
                    self.add(site, K::Type("integer arithmetic"));
                }
                self.fresh_holding(site, result);
                if let Some(flag) = flag {
                    self.fresh_holding(site, flag);
                }
            }
            Inst::ScalarUnary {
                result,
                op,
                operand,
            } => {
                let kind = self.scalar(operand);
                let ok = if op.is_float() {
                    matches!(kind, Some(ScalarKind::F32 | ScalarKind::F64))
                } else {
                    kind.is_some_and(|k| integer_bits(k).is_some())
                } && self.same_type(result, operand);
                if !ok {
                    self.add(site, K::Type("scalar unary operation"));
                }
                self.fresh_holding(site, result);
            }
            Inst::MulAdd {
                result,
                left,
                right,
                addend,
            } => {
                let ok = matches!(self.scalar(left), Some(ScalarKind::F32 | ScalarKind::F64))
                    && self.same_type(left, right)
                    && self.same_type(left, addend)
                    && self.same_type(result, left);
                if !ok {
                    self.add(site, K::Type("fused multiply-add"));
                }
                self.fresh_holding(site, result);
            }
            Inst::Compare {
                result,
                left,
                right,
                layout,
                ..
            } => {
                // One spelling per case: scalars compare with `Binary`.
                if !matches!(
                    layouts.get(layout).map(|l| &l.kind),
                    Some(kind) if !matches!(kind, LayoutKind::Scalar(_))
                ) {
                    self.add(site, K::Type("structural comparison of a scalar layout"));
                } else if !layouts.comparable(layout) {
                    self.add(
                        site,
                        K::Type("structural comparison of an incomparable layout"),
                    );
                }
                if self.layout_of(left) != layout || self.layout_of(right) != layout {
                    self.add(site, K::Type("structural comparison operands"));
                }
                if self.scalar(result) != Some(ScalarKind::Bool) {
                    self.add(site, K::Type("structural comparison result"));
                }
                self.fresh_holding(site, result);
            }
            Inst::IsUnique { result, value } => {
                if self.cx.stage < StageKind::Rc
                    && unit.header.origin == crate::ir::UnitOrigin::Library
                    && !early_unique_cursor(self.cx, value, result)
                    && !owned_root(self.cx, value)
                {
                    self.add(site, K::Type("library uniqueness ownership"));
                }
                if self.ty(value).repr != ValueRepr::Block {
                    self.add(site, K::Type("uniqueness test of an uncounted value"));
                }
                if self.scalar(result) != Some(ScalarKind::Bool) {
                    self.add(site, K::Type("uniqueness test result"));
                }
                self.fresh_holding(site, result);
            }
            // Copy-on-write's test (LIR schema revision 32).
            Inst::IsWritable { result, value } => {
                if self.ty(value).repr != ValueRepr::Block {
                    self.add(site, K::Type("writability test of an uncounted value"));
                }
                if self.scalar(result) != Some(ScalarKind::Bool) {
                    self.add(site, K::Type("writability test result"));
                }
                self.fresh_holding(site, result);
            }
            // A semantic copy marks the arena blocks the value holds at
            // inline depth (MEM-6, LIR schema revision 32).
            Inst::Share { value } => {
                let holds = self.ty(value).repr == ValueRepr::Block
                    || layouts
                        .get(self.layout_of(value))
                        .is_some_and(|r| !r.drop.is_empty());
                if !holds {
                    self.add(site, K::Type("share of a value that holds no block"));
                }
            }
            Inst::Weaken { result, value } => {
                let callable = |v: ValueId| match layouts.get(self.layout_of(v)) {
                    Some(LayoutRecord {
                        kind: LayoutKind::Callable { environment },
                        copy,
                        ..
                    }) if self.ty(v).repr == ValueRepr::Callable => Some((*environment, *copy)),
                    _ => None,
                };
                let ok = matches!(
                    (callable(value), callable(result)),
                    (Some((from, CopyRecipe::Dup)), Some((to, CopyRecipe::NotCopyable)))
                        if from == to
                );
                if !ok {
                    self.add(site, K::Type("weakened callable"));
                }
                // A borrowed view weakens to a view under the same loan.
                let holding = |v: ValueId| self.cx.unit.values[v.index()].holding;
                match holding(value) {
                    Holding::Owned => self.fresh_holding(site, result),
                    view if holding(result) != view => self.add(site, K::Holding(result)),
                    _ => {}
                }
            }
            Inst::Check { condition, .. } => {
                if self.scalar(condition) != Some(ScalarKind::Bool) {
                    self.add(site, K::Type("check condition"));
                }
            }
            Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::EndLoan { .. }
            | Inst::FrameEnter { .. }
            | Inst::FrameLeave
            | Inst::Unavailable { .. } => {}
            Inst::Address { address, failure } => {
                let info = &unit.addresses[address.index()];
                if let AddressBase::Value(v) = info.base
                    && !self.inline_spill(v)
                    && (!matches!(self.ty(v).repr, ValueRepr::Block | ValueRepr::Uncounted)
                        || self.cx.layouts.payload(self.layout_of(v)).is_none())
                {
                    self.add(site, K::AddressType(address));
                }
                let steps = info.projection.get(&unit.pools.projections).unwrap_or(&[]);
                if info.unchecked.windows(2).any(|p| p[0] >= p[1]) {
                    self.add(site, K::Type("unchecked projection evidence"));
                }
                for &ordinal in &info.unchecked {
                    let rule = match steps.get(ordinal as usize) {
                        Some(ProjectionStep::Index(_) | ProjectionStep::FlatAt { .. }) => {
                            Some("MEM-14")
                        }
                        Some(ProjectionStep::VariantField { .. }) => Some("MEM-13"),
                        _ => None,
                    };
                    let accepted =
                        info.source_place
                            .zip(rule)
                            .and_then(|(place, rule)| {
                                let anchor = unit.tables.source_anchors.get(
                                    unit.tables.failures.get(failure.index())?.anchor.index(),
                                )?;
                                let form = crate::discharge::Discharged {
                                    body: anchor.body,
                                    block: anchor.block,
                                    statement: anchor.statement,
                                    form: crate::discharge::Form::Projection {
                                        place,
                                        rule: rule.into(),
                                    },
                                };
                                Some(
                                    self.cx
                                        .discharge?
                                        .sites(&unit.header.definition)?
                                        .contains(&form),
                                )
                            })
                            .unwrap_or(false);
                    if !accepted {
                        self.add(site, K::Type("unchecked projection evidence"));
                    }
                }
                for step in info.projection.get(&unit.pools.projections).unwrap_or(&[]) {
                    match *step {
                        ProjectionStep::Payload
                        | ProjectionStep::Field(_)
                        | ProjectionStep::VariantField { .. }
                        | ProjectionStep::Index(_) => {}
                        // Only a library unit names a slot past the count
                        // (LIR schema revision 7).
                        ProjectionStep::Slot(i) => {
                            self.library_only(site, Opcode::Address);
                            if self.scalar(i) != Some(ScalarKind::U64) {
                                self.add(site, K::Type("slot index"));
                            }
                        }
                        ProjectionStep::FlatAt { index, offset } => {
                            if self.scalar(index) != Some(ScalarKind::U64)
                                || self.scalar(offset) != Some(ScalarKind::U64)
                            {
                                self.add(site, K::Type("flat element offset"));
                            }
                        }
                    }
                }
            }
            Inst::Read {
                result,
                address,
                reaching,
                mode,
            } => {
                let Some(addr) = facts.addresses[address.index()].clone() else {
                    return;
                };
                if self.layout_of(result) != addr.layout {
                    self.add(site, K::Type("read result"));
                }
                self.reaching(site, &addr, reaching);
                let holding = unit.values[result.index()].holding;
                let owns = self.cx.owns(result);
                match mode {
                    ReadMode::Take => {
                        // A field of an owned counted block may be moved out
                        // of its value, one field step deep, so the block's
                        // release mask can say what stays (LIR schema
                        // revision 5); every other take names a slot.
                        match addr.root {
                            Root::Value(base) => {
                                let owned = self.ty(base).repr == ValueRepr::Block
                                    && unit.values[base.index()].holding == Holding::Owned;
                                let part = addr
                                    .path
                                    .strip_prefix(&[Step::Payload])
                                    .unwrap_or(&addr.path);
                                if !owned || part.len() != 1 || addr.crossings != [0] {
                                    self.add(site, K::WriteThroughValue(address));
                                }
                            }
                            Root::Slot(_) => {
                                self.slot_root(site, facts, address);
                            }
                        }
                        self.fresh_holding(site, result);
                        facts.moved[result.index()] = true;
                        if let Root::Slot(slot) = addr.root
                            && addr.path.is_empty()
                        {
                            facts.taken_from[result.index()] = Some(slot);
                        }
                    }
                    ReadMode::Bits => {
                        if layouts.copy(addr.layout) != Some(CopyRecipe::Bits) || owns {
                            self.add(site, K::Type("bits read of a non-plain layout"));
                        }
                        self.fresh_holding(site, result);
                    }
                    ReadMode::Borrow => {
                        if owns {
                            match holding {
                                Holding::Borrowed(loan) => {
                                    if !self.covers(facts, loan, &addr) {
                                        self.add(site, K::LoanCoverage(result));
                                    }
                                }
                                _ => self.add(site, K::Holding(result)),
                            }
                            facts.backing[result.index()] = match addr.root {
                                Root::Slot(slot) => Backing::Slot {
                                    slot,
                                    path: addr.path.clone(),
                                    crossings: addr.crossings.clone(),
                                },
                                Root::Value(v) => Backing::Value(v),
                            };
                        } else if holding != Holding::Plain {
                            self.add(site, K::Holding(result));
                        }
                    }
                }
            }
            Inst::CopyValue { result, value } => {
                if self.ty(result) != self.ty(value)
                    || !matches!(self.ty(value).repr, ValueRepr::Block | ValueRepr::Uncounted)
                    || layouts.copy(self.layout_of(value)) != Some(CopyRecipe::Dup)
                    || layouts.destructible(self.layout_of(value))
                {
                    self.add(site, K::Type("SSA block copy"));
                }
                self.fresh_holding(site, result);
            }
            Inst::Copy {
                result,
                address,
                reaching,
                ..
            } => {
                let Some(addr) = facts.addresses[address.index()].clone() else {
                    return;
                };
                if self.layout_of(result) != addr.layout
                    || layouts.copy(addr.layout) == Some(CopyRecipe::NotCopyable)
                {
                    self.add(site, K::Type("copy"));
                }
                self.reaching(site, &addr, reaching);
                self.fresh_holding(site, result);
            }
            Inst::FlatCopy {
                result,
                address,
                layout,
                ..
            } => {
                let result_layout = self.layout_of(result);
                let same =
                    result_layout == layout && layouts.copy(layout) == Some(CopyRecipe::FlatBytes);
                let decoded = result_layout != layout
                    && layouts.encodes(result_layout, layout)
                    && match layouts.get(result_layout).map(|r| &r.kind) {
                        Some(LayoutKind::Slice { element }) => layouts
                            .0
                            .layouts
                            .iter()
                            .any(|r| r.kind == LayoutKind::Sequence { element: *element }),
                        Some(LayoutKind::Block { payload }) => layouts
                            .get(*payload)
                            .is_some_and(|r| r.kind == LayoutKind::Flat),
                        _ => false,
                    };
                if self.address_layout(facts, address) != Some(layout) || !(same || decoded) {
                    self.add(site, K::Type("flat copy"));
                }
                self.fresh_holding(site, result);
            }
            // A byte interval preserves its source's active loan and backing.
            Inst::SliceView {
                result,
                value,
                start,
                end,
                failure,
            } => {
                let slice = matches!(layouts.get(self.layout_of(value)), Some(LayoutRecord {
                    kind: LayoutKind::Slice { element }, ..
                }) if layouts.get(*element).is_some_and(|e| e.kind == LayoutKind::Scalar(ScalarKind::U8)));
                if !slice
                    || !self.same_type(result, value)
                    || self.scalar(start) != Some(ScalarKind::U64)
                    || self.scalar(end) != Some(ScalarKind::U64)
                    || unit.tables.failures[failure.index()].rule.as_deref() != Some("SEM-3")
                {
                    self.add(site, K::Type("slice view"));
                }
                match (
                    unit.values[value.index()].holding,
                    unit.values[result.index()].holding,
                ) {
                    (Holding::Borrowed(a), Holding::Borrowed(b)) if a == b => {}
                    _ => self.add(site, K::Holding(result)),
                }
                facts.backing[result.index()] = Backing::Value(value);
            }
            // A String or Bytes encoding inside a flat value held in a block,
            // viewed at its source type under a loan covering it.
            Inst::FlatView { result, address } => {
                let Some(addr) = facts.addresses[address.index()].clone() else {
                    return;
                };
                let slice = matches!(layouts.get(self.layout_of(result)), Some(LayoutRecord {
                    kind: LayoutKind::Slice { element }, ..
                }) if layouts.get(*element).is_some_and(|e| e.kind == LayoutKind::Scalar(ScalarKind::U8)));
                if !layouts.flat_bytes(addr.layout)
                    || addr.crossings.is_empty()
                    || !slice
                    || self.ty(result).repr != ValueRepr::Aggregate
                {
                    self.add(site, K::Type("flat view"));
                }
                match unit.values[result.index()].holding {
                    Holding::Borrowed(loan) => {
                        if !self.covers(facts, loan, &addr) {
                            self.add(site, K::LoanCoverage(result));
                        }
                    }
                    _ => self.add(site, K::Holding(result)),
                }
                facts.backing[result.index()] = match addr.root {
                    Root::Slot(slot) => Backing::Slot {
                        slot,
                        path: addr.path.clone(),
                        crossings: addr.crossings.clone(),
                    },
                    Root::Value(v) => Backing::Value(v),
                };
            }
            Inst::FlatLength { result, address } => {
                let sequence = self
                    .address_layout(facts, address)
                    .is_some_and(|l| layouts.flat_sequence(l).is_some());
                if !sequence || self.scalar(result) != Some(ScalarKind::U64) {
                    self.add(site, K::Type("flat length"));
                }
                self.fresh_holding(site, result);
            }
            Inst::FlatNext { result, address } => {
                // The place is one encoding inside a flat value, or one whole
                // element of a sequential Packed sequence (MEM-13): its end is
                // its start plus its own extent.
                let flat = self
                    .address_layout(facts, address)
                    .and_then(|l| layouts.get(l))
                    .is_some_and(|r| r.kind == LayoutKind::Flat)
                    || facts.addresses[address.index()]
                        .as_ref()
                        .is_some_and(|a| a.sequential_element);
                if !flat || self.scalar(result) != Some(ScalarKind::U64) {
                    self.add(site, K::Type("flat element end"));
                }
                self.fresh_holding(site, result);
            }
            Inst::SeqLen { result, value } | Inst::SeqCapacity { result, value } => {
                let capacity = matches!(inst, Inst::SeqCapacity { .. });
                let operand = self.layout_of(value);
                // A sequential Packed sequence counts its elements, but its
                // storage holds bytes, not element slots (MEM-13).
                let sequence = layouts
                    .payload(operand)
                    .and_then(|p| layouts.get(p))
                    .is_some_and(|r| match r.kind {
                        LayoutKind::Sequence { .. } | LayoutKind::SoaColumns { .. } => true,
                        LayoutKind::Sequential { .. } => !capacity,
                        _ => false,
                    });
                let slice = !capacity
                    && layouts
                        .get(operand)
                        .is_some_and(|r| matches!(r.kind, LayoutKind::Slice { .. }));
                if !(sequence || slice) || self.scalar(result) != Some(ScalarKind::U64) {
                    self.add(site, K::Type("sequence length"));
                }
                self.fresh_holding(site, result);
            }
            Inst::SeqReserve {
                address,
                capacity: amount,
                version,
            }
            | Inst::SeqSetLen {
                address,
                len: amount,
                version,
            } => {
                // A sequential block's reservation counts element bytes in all: a
                // flat builder's, which import places in user code (LIR schema
                // revision 22). It has no length to set.
                let reserve = matches!(inst, Inst::SeqReserve { .. });
                let kind = self
                    .address_layout(facts, address)
                    .and_then(|l| layouts.payload(l))
                    .and_then(|p| layouts.get(p))
                    .map(|r| &r.kind);
                let sequential = reserve && matches!(kind, Some(LayoutKind::Sequential { .. }));
                if !sequential {
                    self.library_only(site, inst.opcode());
                }
                let sequence = sequential
                    || matches!(
                        kind,
                        Some(LayoutKind::Sequence { .. } | LayoutKind::SoaColumns { .. })
                    );
                if !sequence || self.scalar(amount) != Some(ScalarKind::U64) {
                    self.add(site, K::Type("sequence write"));
                }
                if let Some(slot) = self.slot_root(site, facts, address) {
                    self.version_type(site, slot, version, "store version");
                }
                if self.flat_variable(facts, address) {
                    self.add(site, K::Type("write into a variable part of a flat value"));
                }
            }
            // One push's preparation of a List block (MEM-6, LIR schema
            // revision 32): `list.push`'s, of rows, of `@soa` columns (revision
            // 35) or of `Packed` values (revision 36). One of `count` elements prepares
            // an Io read's appends, which import places in user code (revision
            // 34).
            Inst::ArenaPush {
                address,
                count,
                version,
                ..
            } => {
                match count {
                    None => self.library_only(site, inst.opcode()),
                    Some(count) if self.scalar(count) != Some(ScalarKind::U64) => {
                        self.add(site, K::Type("arena push count"));
                    }
                    Some(_) => {}
                }
                let sequence = self
                    .address_layout(facts, address)
                    .and_then(|l| layouts.payload(l))
                    .and_then(|p| layouts.get(p))
                    .is_some_and(|r| {
                        matches!(
                            r.kind,
                            LayoutKind::Sequence { .. }
                                | LayoutKind::SoaColumns { .. }
                                | LayoutKind::Sequential { .. }
                        )
                    });
                if !sequence {
                    self.add(
                        site,
                        K::Type("arena push of a place that holds no List block"),
                    );
                }
                if let Some(slot) = self.slot_root(site, facts, address) {
                    self.version_type(site, slot, version, "store version");
                }
            }
            // An owned Packed element appended to a sequential sequence block
            // (MEM-13, LIR schema revision 19).
            Inst::SeqAppend {
                address,
                value,
                version,
            } => {
                self.library_only(site, inst.opcode());
                let element = self
                    .address_layout(facts, address)
                    .and_then(|l| layouts.payload(l))
                    .and_then(|p| layouts.get(p))
                    .and_then(|r| match r.kind {
                        LayoutKind::Sequential { element } => Some(element),
                        _ => None,
                    });
                if element.is_none_or(|e| e != self.layout_of(value)) {
                    self.add(site, K::Type("sequential append"));
                }
                if let Some(slot) = self.slot_root(site, facts, address) {
                    self.version_type(site, slot, version, "store version");
                }
            }
            // One element built in its flat encoding at the byte end of a
            // sequential sequence of variable flat records (LIR schema
            // revision 22): the element is the prefixed encoding of exactly the
            // constructed content, whose fields are checked as a flat
            // construction's.
            Inst::FlatAppend {
                address,
                construct,
                version,
            } => {
                let c = &unit.tables.constructs[construct.index()];
                let encoded = self.builder_content(facts, address) == Some(c.layout);
                let record = matches!(
                    c.constructor,
                    ConstructorKind::Record(_) | ConstructorKind::Tuple
                ) && layouts
                    .get(c.layout)
                    .is_some_and(|r| r.kind == LayoutKind::Flat);
                if !encoded || !record {
                    self.add(site, K::Type("flat append"));
                }
                self.constructed_fields(site, construct, facts, None);
                self.placement(site, (&c.destination, c.placement), None);
                if let Some(slot) = self.slot_root(site, facts, address) {
                    self.version_type(site, slot, version, "store version");
                }
                if self.flat_variable(facts, address) {
                    self.add(site, K::Type("write into a variable part of a flat value"));
                }
            }
            // A write of a builder's open element (LIR schema revision 24):
            // the open field is a byte sequence of the builder's content
            // record after only fixed fields; a reservation counts bytes and
            // a push appends one. Which element is open is `flat_open`'s.
            Inst::FlatField {
                address,
                field,
                op,
                version,
            } => {
                let content = self.builder_content(facts, address);
                let ok = content.and_then(|c| layouts.open_field(c, field)).is_some()
                    && match op {
                        FlatFieldOp::Open => true,
                        FlatFieldOp::Reserve { capacity } => {
                            self.scalar(capacity) == Some(ScalarKind::U64)
                        }
                        FlatFieldOp::Push { value, .. } => {
                            self.scalar(value) == Some(ScalarKind::U8)
                        }
                    };
                if !ok {
                    self.add(site, K::Type("flat field"));
                }
                if let Some(slot) = self.slot_root(site, facts, address) {
                    self.version_type(site, slot, version, "store version");
                }
                if self.flat_variable(facts, address) {
                    self.add(site, K::Type("write into a variable part of a flat value"));
                }
            }
            // The open element completed: a flat construction of exactly the
            // builder's content record, given every field but the open one,
            // whose bytes are in place (LIR schema revision 24).
            Inst::FlatClose {
                address,
                construct,
                field,
                version,
            } => {
                let c = &unit.tables.constructs[construct.index()];
                let record = matches!(
                    c.constructor,
                    ConstructorKind::Record(_) | ConstructorKind::Tuple
                );
                if !record
                    || self.builder_content(facts, address) != Some(c.layout)
                    || layouts.open_field(c.layout, field).is_none()
                {
                    self.add(site, K::Type("flat close"));
                }
                self.constructed_fields(site, construct, facts, Some(field));
                self.placement(site, (&c.destination, c.placement), None);
                if let Some(slot) = self.slot_root(site, facts, address) {
                    self.version_type(site, slot, version, "store version");
                }
                if self.flat_variable(facts, address) {
                    self.add(site, K::Type("write into a variable part of a flat value"));
                }
            }
            Inst::Hash {
                result,
                value,
                layout,
                ..
            } => {
                if !layouts.hashable(layout)
                    || !(self.layout_of(value) == layout
                        || layouts.same_scalar(self.layout_of(value), layout))
                    || self.scalar(result) != Some(ScalarKind::U64)
                {
                    self.add(site, K::Type("hash"));
                }
                self.fresh_holding(site, result);
            }
            // A codec reads its operand whole at the plan's layout and yields a
            // fresh value at the plan's (LIR schema revision 29). The plan
            // itself agrees with the table node by node (`CodecPlan::check`).
            Inst::Encode {
                result,
                value,
                codec,
            }
            | Inst::Decode {
                result,
                value,
                codec,
            } => {
                // An unknown codec is the identity check's violation.
                if let Some(plan) = unit.tables.codecs.get(codec.index()) {
                    let encode = matches!(inst, Inst::Encode { .. });
                    let operand = plan.node(if encode { plan.root } else { plan.bytes });
                    let produced = if encode {
                        plan.node(plan.bytes)
                    } else {
                        plan.result
                            .ok_or_else(|| "a decode plan without a result".to_string())
                            .and_then(|r| plan.node(r))
                    };
                    if plan.check(layouts.0).is_err() || encode == plan.result.is_some() {
                        self.add(site, K::Type("codec plan"));
                    } else if !operand.is_ok_and(|n| n.layout() == self.layout_of(value))
                        || !produced.is_ok_and(|n| n.layout() == self.layout_of(result))
                    {
                        self.add(site, K::Type(if encode { "encode" } else { "decode" }));
                    }
                }
                self.fresh_holding(site, result);
            }
            Inst::Recast { result, value } => {
                let credit_alias = S::KIND >= StageKind::Rc
                    && self.ty(result) == self.ty(value)
                    && matches!(self.ty(value).repr, ValueRepr::Block | ValueRepr::Uncounted)
                    && unit.values[result.index()].holding
                        == if self.cx.owns(result) {
                            Holding::Owned
                        } else {
                            Holding::Plain
                        };
                if !credit_alias {
                    self.library_only(site, Opcode::Recast);
                }
                // The cursor's opaque source: a block over no payload.
                let opaque = |v: ValueId| {
                    layouts
                        .payload(self.layout_of(v))
                        .and_then(|p| layouts.get(p))
                        .map(|r| r.kind == LayoutKind::Unit)
                };
                let block = |v: ValueId| self.ty(v).repr == ValueRepr::Block;
                let loan = |v: ValueId| match unit.values[v.index()].holding {
                    Holding::Borrowed(loan) => Some(loan),
                    _ => None,
                };
                let borrowed = block(value)
                    && block(result)
                    && matches!(
                        (opaque(value), opaque(result)),
                        (Some(true), Some(false)) | (Some(false), Some(true))
                    )
                    && loan(value).is_some()
                    && loan(value) == loan(result);
                let moved = block(value)
                    && block(result)
                    && unit.values[value.index()].holding == Holding::Owned
                    && unit.values[result.index()].holding == Holding::Owned
                    && layouts.payload(self.layout_of(value)).is_some()
                    && layouts.payload(self.layout_of(value))
                        == layouts.payload(self.layout_of(result));
                if !(credit_alias || borrowed || moved) {
                    self.add(site, K::Type("recast"));
                }
                // The result is the same view: it rests on what the operand does.
                facts.backing[result.index()] = if credit_alias && self.cx.owns(result) {
                    Backing::None
                } else {
                    match &facts.backing[value.index()] {
                        Backing::None => Backing::Value(value),
                        backing => backing.clone(),
                    }
                };
            }
            Inst::UniqueCopy {
                result, construct, ..
            } => {
                let site_layout = unit.tables.constructs[construct.index()].layout;
                let layout = self.layout_of(result);
                if layout != site_layout && layouts.payload(layout) != Some(site_layout) {
                    self.add(site, K::Type("unique copy"));
                }
                self.fresh_holding(site, result);
                self.placement(site, construct_placement(self.cx, construct), Some(result));
            }
            // MEM-11 (LIR schema revision 26): a Shared root block reference
            // and the value its Frozen payload holds.
            Inst::Freeze { result, value }
            | Inst::Thaw {
                value: result,
                result: value,
            } => {
                let op = if matches!(*inst, Inst::Freeze { .. }) {
                    Opcode::Freeze
                } else {
                    Opcode::Thaw
                };
                self.library_only(site, op);
                let frozen = match layouts.get(self.layout_of(result)).map(|r| &r.kind) {
                    Some(LayoutKind::Shared { payload }) => {
                        match layouts.get(*payload).map(|r| &r.kind) {
                            Some(LayoutKind::Frozen { value }) => Some(*value),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if self.ty(result).repr != ValueRepr::Block || frozen != Some(self.layout_of(value))
                {
                    self.add(
                        site,
                        K::Type(if op == Opcode::Freeze {
                            "freeze"
                        } else {
                            "thaw"
                        }),
                    );
                }
                let made = if op == Opcode::Freeze { result } else { value };
                self.fresh_holding(site, made);
            }
            Inst::Store {
                address,
                value,
                version,
            } => self.write(site, facts, address, value, version, None),
            Inst::Replace {
                address,
                value,
                displaced,
                version,
                displaced_version,
                ..
            } => {
                self.write(site, facts, address, value, version, Some(displaced));
                self.version_type(site, displaced, displaced_version, "displaced version");
            }
            Inst::UpdateMask {
                result,
                mask,
                field,
                ..
            } => {
                let ok = (matches!(self.ty(mask).repr, ValueRepr::InitMask { fields } if field < fields)
                    || (self.ty(mask).repr == ValueRepr::Scalar(ScalarKind::U64) && field < 64))
                    && self.same_type(result, mask);
                if !ok {
                    self.add(site, K::Type("mask update"));
                }
            }
            Inst::Construct {
                result, construct, ..
            } => {
                self.construct(site, construct, result, facts);
                self.fresh_holding(site, result);
            }
            Inst::Reuse {
                result,
                token,
                construct,
                ..
            } => {
                if self.ty(token).repr != ValueRepr::Token {
                    self.add(site, K::Type("reuse token"));
                }
                self.construct(site, construct, result, facts);
                self.fresh_holding(site, result);
            }
            Inst::Closure {
                result, closure, ..
            } => {
                let c = &unit.tables.closures[closure.index()];
                if c.receipts.is_some() {
                    self.add(site, K::Sealed("receipt closure transport requires independent source and carrier validation".into()));
                }
                let specialized = match layouts.get(self.layout_of(result)) {
                    Some(LayoutRecord {
                        kind:
                            LayoutKind::Callable {
                                environment: Some(e),
                            },
                        ..
                    }) => *e == c.environment,
                    _ => false,
                };
                let ok = match c.code {
                    // A generic nested function's value (LIR schema revision
                    // 9) is only a bound call's environment: the callable
                    // naming it, or the erased callable its generic type
                    // lays out as.
                    None => {
                        self.ty(result).repr == ValueRepr::Callable
                            && (specialized
                                || matches!(
                                    layouts.get(self.layout_of(result)),
                                    Some(LayoutRecord {
                                        kind: LayoutKind::Callable { environment: None },
                                        ..
                                    })
                                ))
                    }
                    Some(code) => {
                        let abi = &layouts.0.fn_abis[code.abi.0 as usize];
                        let function =
                            unit.tables.symbols[code.symbol.index()].kind == SymbolKind::Function;
                        // The result is the specialized callable naming the
                        // site's environment, or the erased callable of the
                        // same multiplicity (the environment stays named by
                        // the site for CLO-5).
                        let erased_copy = match abi.multiplicity {
                            CallMultiplicity::Many => CopyRecipe::Dup,
                            CallMultiplicity::Once => CopyRecipe::NotCopyable,
                        };
                        // An iterator is its step thunk and state (LIR schema
                        // revision 7): the thunk takes only the hidden
                        // environment and returns `Option` of the iterator's
                        // element.
                        let iterator = match layouts.get(self.layout_of(result)) {
                            Some(LayoutRecord {
                                kind: LayoutKind::Iterator { element },
                                ..
                            }) => Some(*element),
                            _ => None,
                        };
                        if let Some(element) = iterator
                            && (self.ty(result).repr != ValueRepr::Aggregate
                                || !function
                                || !self.step_abi(abi, element))
                        {
                            self.add(site, K::Type("iterator closure"));
                        }
                        iterator.is_some()
                            || self.ty(result).repr == ValueRepr::Callable
                                && function
                                && abi.shape == CallableShape::Closure
                                && abi.environment
                                && (specialized
                                    || matches!(
                                        layouts.get(self.layout_of(result)),
                                        Some(LayoutRecord {
                                            kind: LayoutKind::Callable { environment: None },
                                            copy,
                                            ..
                                        }) if *copy == erased_copy
                                    ))
                    }
                };
                if !ok {
                    self.add(site, K::Type("closure"));
                }
                let environment = layouts.get(c.environment);
                if let Some(environment) = environment {
                    let fields = environment
                        .fields
                        .len()
                        .saturating_sub(usize::from(matches!(
                            environment.kind,
                            LayoutKind::Environment { .. }
                        )));
                    if c.captures.len as usize != fields {
                        self.add(site, K::Arity("closure captures"));
                    }
                }

                for (i, capture) in c
                    .captures
                    .get(&unit.pools.captures)
                    .unwrap_or(&[])
                    .iter()
                    .enumerate()
                {
                    // A captured value fills its environment field: a borrowed
                    // capture as the view itself, a mut one as an address
                    // (LIR schema revision 3).
                    let field = environment
                        .and_then(|e| e.fields.iter().find(|f| f.ordinal == i as u32))
                        .map(|f| f.layout);
                    let fits = match (capture.usage, field) {
                        (_, None) => false,
                        (CaptureUse::Mut, Some(f)) => matches!(
                            layouts.get(f).map(|l| &l.kind),
                            Some(LayoutKind::Scalar(ScalarKind::Pointer))
                        ),
                        (_, Some(f)) => f == self.layout_of(capture.value),
                    };
                    if !fits {
                        self.add(site, K::Type("captured value"));
                    }
                    if let Some(environment) = environment
                        && let Some(field) =
                            environment.fields.iter().find(|f| f.ordinal == i as u32)
                        && self.cx.owns(capture.value)
                        && (capture.usage == CaptureUse::Borrow)
                            != crate::layout::field_is_borrowed(layouts.0, environment, field)
                    {
                        self.add(site, K::Type("capture field ownership"));
                    }
                    if capture.usage == CaptureUse::Copy
                        && layouts.copy(self.layout_of(capture.value))
                            == Some(CopyRecipe::NotCopyable)
                    {
                        self.add(site, K::Type("copied capture"));
                    }
                }
                self.fresh_holding(site, result);
                self.placement(site, (&c.destination, c.placement), Some(result));
            }
            Inst::BeginLoan {
                loan,
                address,
                mutable,
                ..
            } => {
                let info = &unit.tables.loans[loan.index()];
                if info.address != address || info.mutable != mutable {
                    self.add(site, K::LoanMismatch(loan));
                }
            }
            Inst::BindOrigin { .. } | Inst::BindFields { .. } | Inst::EndOrigin { .. } => {}
            Inst::Drop { address, .. } => {
                self.slot_root(site, facts, address);
            }
            Inst::Defuse { address }
            | Inst::FinalizeBegin { address, .. }
            | Inst::FinalizeEnd { address, .. } => {
                let Some(layout) = self.address_layout(facts, address) else {
                    return;
                };
                if layouts.get(layout).is_none_or(|l| l.finalizer.is_none()) {
                    self.add(site, K::Type("finalization of a layout without finalizer"));
                }
            }
            Inst::Dup { value } => {
                if self.ty(value).repr != ValueRepr::Block {
                    self.add(site, K::DupUncounted(value));
                }
            }
            Inst::Release {
                value,
                layout,
                live,
                ..
            } => self.release(site, value, layout, live),
            Inst::Reset {
                result,
                value,
                layout,
                live,
                ..
            } => {
                // A closure donates its environment block (LIR schema
                // revision 5); `tokens` proves the environment's layout from
                // the closure that built it.
                if self.ty(value).repr == ValueRepr::Callable {
                    if let MaskRef::Dynamic(mask) = live
                        && !matches!(self.ty(mask).repr, ValueRepr::InitMask { .. })
                    {
                        self.add(site, K::Type("release mask"));
                    }
                } else {
                    self.release(site, value, layout, live);
                }
                if self.ty(result).repr != ValueRepr::Token {
                    self.add(site, K::Type("reset token"));
                }
            }
            Inst::Dispose { token } => {
                if self.ty(token).repr != ValueRepr::Token {
                    self.add(site, K::Type("disposed token"));
                }
            }
            Inst::HoleOpen {
                result,
                hole,
                construct,
                open,
                ..
            } => {
                // A field-less box construction; the result is its block and
                // the hole names the same box layout. The stand-in is a box
                // over plain bytes of the payload's size and alignment.
                let c = &unit.tables.constructs[construct.index()];
                let boxed = self.layout_of(result);
                let fields = c.fields.get(&unit.pools.fields).unwrap_or(&[]);
                let payload = |l| match layouts.get(l).map(|r| &r.kind) {
                    Some(LayoutKind::Box { payload }) => Some(*payload),
                    _ => None,
                };
                let stand_in = payload(open)
                    .and_then(|o| layouts.get(o))
                    .zip(payload(boxed).and_then(|p| layouts.get(p)));
                let ok = c.constructor == ConstructorKind::Box
                    && fields.is_empty()
                    && self.ty(result).repr == ValueRepr::Block
                    && payload(boxed) == Some(c.layout)
                    && self.ty(hole).repr == ValueRepr::Hole
                    && self.layout_of(hole) == boxed
                    && stand_in.is_some_and(|(o, p)| {
                        o.size == p.size
                            && o.align == p.align
                            && o.fields.is_empty()
                            && o.scan.is_empty()
                            && o.drop.is_empty()
                            && o.finalizer.is_none()
                    });
                if !ok {
                    self.add(site, K::Type("hole opening"));
                }
                self.fresh_holding(site, result);
                self.fresh_holding(site, hole);
            }
            Inst::HoleFill { hole, value } => {
                let ok = self.ty(hole).repr == ValueRepr::Hole
                    && unit.values[hole.index()].holding == Holding::Owned
                    && match layouts.get(self.layout_of(hole)).map(|r| &r.kind) {
                        Some(LayoutKind::Box { payload }) => *payload == self.layout_of(value),
                        _ => false,
                    };
                if !ok {
                    self.add(site, K::Type("hole fill"));
                }
            }
            Inst::HoleAbandon { hole, root } => {
                // The root is a block of the hole's box layout, which nothing
                // reachable makes destructible.
                let boxed = self.layout_of(hole);
                let ok = self.ty(hole).repr == ValueRepr::Hole
                    && unit.values[hole.index()].holding == Holding::Owned
                    && root.is_none_or(|root| {
                        self.ty(root).repr == ValueRepr::Block
                            && self.layout_of(root) == boxed
                            && !layouts.destructible(boxed)
                    });
                if !ok {
                    self.add(site, K::Type("abandoned hole"));
                }
            }
            Inst::TokenNone { result } => {
                if self.ty(result).repr != ValueRepr::Token {
                    self.add(site, K::Type("none token"));
                }
            }
            Inst::RegionEnter { result, .. } => {
                if self.ty(result).repr != ValueRepr::RegionMark {
                    self.add(site, K::Type("region mark"));
                }
            }
            Inst::RegionReserve {
                result,
                mark,
                extent,
                ..
            } => {
                if self.ty(mark).repr != ValueRepr::RegionMark
                    || self.scalar(extent) != Some(ScalarKind::U64)
                    || self.ty(result).repr != ValueRepr::Uncounted
                {
                    self.add(site, K::Type("region reservation"));
                }
            }
            Inst::RegionPublish { value } => {
                if self.ty(value).repr != ValueRepr::Uncounted
                    || layouts.payload(self.layout_of(value)).is_none()
                {
                    self.add(site, K::Type("region publication"));
                }
            }
            Inst::RegionExit { mark, .. } => {
                if self.ty(mark).repr != ValueRepr::RegionMark {
                    self.add(site, K::Type("region exit"));
                }
            }
            // Only `arena.with`'s library body opens an arena, at its entry,
            // so that every exit of the unit closes it.
            Inst::SealArena {
                result,
                grant,
                root,
                shape,
                ..
            } => {
                self.library_only(site, Opcode::SealArena);
                let descriptor = &unit.tables.seal_descriptors[shape.index()];
                let valid_owner = matches!(layouts.get(self.layout_of(result)).map(|r| &r.kind),
                    Some(LayoutKind::SealedRegion { root: layout }) if *layout == descriptor.root_layout);
                if self.ty(grant).repr != ValueRepr::Arena
                    || self.layout_of(root) != descriptor.root_layout
                    || !valid_owner
                    || self.ty(result).repr != ValueRepr::Handle
                {
                    self.add(site, K::Type("sealed arena transition"));
                }
                self.fresh_holding(site, result);
            }
            Inst::BorrowSealedRoot {
                result,
                owner,
                loan,
                shape,
            } => {
                self.library_only(site, Opcode::BorrowSealedRoot);
                let descriptor = &unit.tables.seal_descriptors[shape.index()];
                let valid_owner = matches!(layouts.get(self.layout_of(owner)).map(|r| &r.kind),
                    Some(LayoutKind::SealedRegion { root: layout }) if *layout == descriptor.root_layout);
                // Bits carry no storage credit, just as for an ordinary
                // borrowed read. Their explicit owner loan is still checked.
                if !valid_owner
                    || self.layout_of(result) != descriptor.root_layout
                    || unit.values[result.index()].holding
                        != if self.cx.owns(result) {
                            Holding::Borrowed(loan)
                        } else {
                            Holding::Plain
                        }
                {
                    self.add(site, K::Type("read-only sealed root loan"));
                }
                if !unit.tables.loans[loan.index()].mutable {
                    self.add(site, K::Type("sealed owner requires an exclusive loan"));
                }
            }
            Inst::ArenaInventory {
                arena,
                value,
                shape,
                ..
            } => {
                let descriptor = &unit.tables.seal_descriptors[shape.index()];
                if self.ty(arena).repr != ValueRepr::Arena
                    || self.layout_of(value) != descriptor.root_layout
                {
                    self.add(site, K::Type("semantic arena allocation event"));
                }
            }
            Inst::ArenaOpen { result, budget } => {
                self.library_only(site, Opcode::ArenaOpen);
                if site.block != Some(unit.entry) {
                    self.add(
                        site,
                        K::Placement("an arena opened outside the entry block"),
                    );
                }
                if self.ty(result).repr != ValueRepr::Arena {
                    self.add(site, K::Type("arena opening"));
                }
                self.arena_budget(site, budget);
            }
            Inst::ArenaToken {
                result,
                mark,
                budget,
            } => {
                if self.ty(mark).repr != ValueRepr::RegionMark
                    || self.ty(result).repr != ValueRepr::Arena
                {
                    self.add(site, K::Type("arena token"));
                }
                self.arena_budget(site, budget);
            }
            Inst::PayloadReceipt {
                result,
                value,
                shape,
                ..
            } => {
                let descriptor = &unit.tables.seal_descriptors[shape.index()];
                let valid = matches!(layouts.get(self.layout_of(result)).map(|r| &r.kind),
                    Some(LayoutKind::PayloadReceipt { root }) if root == &descriptor.root_type);
                if !valid
                    || self.ty(result).repr != ValueRepr::PayloadReceipt
                    || self.layout_of(value) != descriptor.root_layout
                    || unit.values[result.index()].holding != Holding::Owned
                {
                    self.add(site, K::Type("source payload receipt"));
                }
            }
            Inst::Forget { .. } => {}
        }
    }

    /// A bounded arena's budget is a u64 (MEM-6, LIR schema revision 26).
    fn arena_budget(&mut self, site: Site, budget: Option<ValueId>) {
        if let Some(budget) = budget
            && self.scalar(budget) != Some(ScalarKind::U64)
        {
            self.add(site, K::Type("arena budget"));
        }
    }

    /// Only a unit built by the library may use `op` this way (LIR schema
    /// revision 7): a `Slot` step, `SeqReserve`, `SeqSetLen`, `SeqAppend`, `Recast`
    /// or `ArenaPush`.
    fn library_only(&mut self, site: Site, op: Opcode) {
        if self.cx.unit.header.origin != UnitOrigin::Library {
            self.add(site, K::Vocabulary(op));
        }
    }

    /// Whether `abi` is the step thunk of an iterator over `element`: the
    /// hidden environment, no declared parameter, and a result that is
    /// `Option` of the element (a nullary variant and one holding exactly
    /// the element).
    fn step_abi(&self, abi: &FnAbi, element: LayoutId) -> bool {
        let layouts = self.cx.layouts;
        let option = abi
            .result
            .as_ref()
            .and_then(|r| layouts.get(r.layout))
            .and_then(|r| r.tag.as_ref())
            .is_some_and(|tag| {
                let mut some = tag.variants.iter().filter(|v| !v.fields.is_empty());
                tag.variants.len() == 2
                    && matches!(
                        (some.next(), some.next()),
                        (Some(v), None) if v.fields.len() == 1 && v.fields[0].layout == element
                    )
            });
        matches!(abi.shape, CallableShape::Closure | CallableShape::IterNext)
            && abi.environment
            && abi.parameters.is_empty()
            && option
    }

    /// A slot read names a version of that slot; a payload read names the
    /// block value it reads through.
    fn reaching(&mut self, site: Site, addr: &Addr, reaching: ValueId) {
        match addr.root {
            Root::Slot(slot) => {
                if self.cx.unit.slots[slot.index()].ty != self.cx.unit.values[reaching.index()].ty {
                    self.add(site, K::Type("reaching version"));
                }
            }
            Root::Value(value) => {
                if value != reaching {
                    self.add(site, K::Type("reaching block"));
                }
            }
        }
    }

    fn covers(&self, facts: &Facts, loan: crate::ir::LoanId, addr: &Addr) -> bool {
        // A projection from an already borrowed SSA value inherits its read
        // authority. Its base is independently checked at its definition and
        // on every incoming edge; the label alone does not prove a phi safe.
        if let Root::Value(base) = addr.root
            && self.cx.unit.values[base.index()].holding == Holding::Borrowed(loan)
        {
            return true;
        }
        let loaned = self.cx.unit.tables.loans[loan.index()].address;
        let Some(loaned) = facts.addresses[loaned.index()].as_ref() else {
            return false;
        };
        loaned.root == addr.root
            && loaned.path.len() <= addr.path.len()
            && overlap(&loaned.path, &addr.path)
    }

    fn write(
        &mut self,
        site: Site,
        facts: &Facts,
        address: AddressId,
        value: ValueId,
        version: ValueId,
        displaced: Option<SlotId>,
    ) {
        let unit = self.cx.unit;
        let Some(slot) = self.slot_root(site, facts, address) else {
            return;
        };
        if self.address_layout(facts, address) != Some(self.layout_of(value))
            || displaced.is_some_and(|d| unit.slots[d.index()].ty != unit.values[value.index()].ty)
        {
            self.add(site, K::Type("stored value"));
        }
        if self.flat_variable(facts, address) {
            self.add(site, K::Type("write into a variable part of a flat value"));
        }
        self.version_type(site, slot, version, "store version");
    }

    /// A version name has its slot's type and holds nothing.
    fn version_type(&mut self, site: Site, slot: SlotId, version: ValueId, what: &'static str) {
        let unit = self.cx.unit;
        if unit.slots[slot.index()].ty != unit.values[version.index()].ty
            || unit.values[version.index()].holding != Holding::Plain
        {
            self.add(site, K::Type(what));
        }
    }

    /// Each capture slot's ordinal names exactly one entry of
    /// `LirUnit::captures`, and each entry exactly one capture slot.
    fn captures(&mut self) {
        let unit = self.cx.unit;
        let mut out = vec![None; unit.captures.len()];
        for (s, slot) in unit.slots.iter().enumerate() {
            let SlotKind::Capture(ordinal) = slot.kind else {
                continue;
            };
            match out.get_mut(ordinal as usize) {
                Some(entry @ None) => *entry = Some(SlotId(s as u32)),
                _ => self.add(Site::UNIT, K::Arity("capture slots")),
            }
        }
        for (i, slot) in out.iter().enumerate() {
            match *slot {
                Some(slot) => {
                    self.version_type(Site::UNIT, slot, unit.captures[i], "capture version")
                }
                None => self.add(Site::UNIT, K::Arity("capture slots")),
            }
        }
    }

    /// The scope parent relation is a forest: no scope is its own ancestor.
    fn scopes(&mut self) {
        let scopes = &self.cx.unit.scopes;
        for start in 0..scopes.len() {
            let mut at = scopes[start].parent;
            let mut steps = 0;
            while let Some(p) = at {
                steps += 1;
                if p.index() == start || steps > scopes.len() {
                    self.add(Site::UNIT, K::ScopeCycle(ScopeId(start as u32)));
                    break;
                }
                at = scopes[p.index()].parent;
            }
        }
    }

    /// A block is released at its payload layout; an owning inline value at
    /// its own layout, by its drop program, when no finalizer is reachable
    /// from it without crossing a block (an inline resource is finalized
    /// explicitly).
    fn release(&mut self, site: Site, value: ValueId, layout: LayoutId, live: MaskRef) {
        let own = self.layout_of(value);
        let admitted = match self.ty(value).repr {
            ValueRepr::PayloadReceipt => layout == own && live == MaskRef::All,
            ValueRepr::Block => self.cx.layouts.released(own) == Some(layout),
            ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle => {
                layout == own && self.cx.owns(value) && !self.cx.layouts.inline_finalizer(layout)
            }
            _ => false,
        };
        if !admitted {
            self.add(site, K::Type("released layout"));
        }
        if let MaskRef::Dynamic(mask) = live
            && !matches!(self.ty(mask).repr, ValueRepr::InitMask { .. })
        {
            self.add(site, K::Type("release mask"));
        }
    }

    /// A whole byte-sequence owner can become a Slice owner without changing
    /// storage. The exact start (`sequence_slice_start`, its first element past
    /// the slice data offset) and SeqLen operands also prove the slice interval;
    /// accepting payload type alone would admit forged out-of-bounds lengths.
    fn whole_byte_slice_owner(
        &self,
        facts: &Facts,
        layout: &LayoutRecord,
        fields: &[crate::ir::FieldValue],
        field: u32,
        owner: LayoutId,
    ) -> bool {
        if field != 0 || fields.len() != 3 {
            return false;
        }
        let layouts = self.cx.layouts;
        let LayoutKind::Slice { element } = layout.kind else {
            return false;
        };
        if !matches!(
            layouts.get(element).map(|r| &r.kind),
            Some(LayoutKind::Scalar(ScalarKind::U8))
        ) {
            return false;
        }
        let Some(LayoutRecord {
            kind: LayoutKind::Block { payload },
            ..
        }) = layouts.get(owner)
        else {
            return false;
        };
        if !matches!(layouts.get(*payload).map(|r| &r.kind), Some(LayoutKind::Sequence { element: byte })
            if layouts.same_scalar(element, *byte))
        {
            return false;
        }
        let value = |ordinal| fields.iter().find(|f| f.field == ordinal).map(|f| f.value);
        let (Some(owner), Some(start), Some(length)) = (value(0), value(1), value(2)) else {
            return false;
        };
        let instruction = |value: ValueId| {
            let Def::Inst(b, i) = facts.defs[value.index()] else {
                return None;
            };
            self.cx.unit.blocks[b.index()]
                .instructions
                .get(&self.cx.unit.instructions)?
                .get(i as usize)
        };
        let Some(whole) = layouts
            .get(element)
            .and_then(|e| crate::layout::sequence_slice_start(e).ok())
        else {
            return false;
        };
        matches!(instruction(start), Some(Inst::Const { constant, .. })
            if self.cx.unit.tables.constants[constant.index()].value == ConstantValue::Integer(u128::from(whole)))
            && matches!(instruction(length), Some(Inst::SeqLen { value, .. }) if *value == owner)
    }

    /// Fields against the constructor's layout and the result's representation.
    fn construct(&mut self, site: Site, construct: ConstructId, result: ValueId, facts: &Facts) {
        let unit = self.cx.unit;
        let layouts = self.cx.layouts;
        let c = &unit.tables.constructs[construct.index()];
        if layouts.get(c.layout).is_none() {
            return;
        }
        self.constructed_fields(site, construct, facts, None);
        let result_layout = self.layout_of(result);
        let ok = match self.ty(result).repr {
            ValueRepr::Block | ValueRepr::Uncounted if layouts.payload(result_layout).is_some() => {
                layouts.payload(result_layout) == Some(c.layout)
            }
            _ => result_layout == c.layout && c.constructor != ConstructorKind::Box,
        };
        if !ok {
            self.add(site, K::Type("construction result"));
        }
        self.placement(site, (&c.destination, c.placement), Some(result));
    }

    /// A construction's field values against its constructor's declared
    /// fields; a flat record's variable field takes a source it encodes.
    /// `open` names a flat record's field built in place, which has no value.
    fn constructed_fields(
        &mut self,
        site: Site,
        construct: ConstructId,
        facts: &Facts,
        open: Option<u32>,
    ) {
        let unit = self.cx.unit;
        let layouts = self.cx.layouts;
        let c = &unit.tables.constructs[construct.index()];
        let fields = c.fields.get(&unit.pools.fields).unwrap_or(&[]);
        let Some(layout) = layouts.get(c.layout) else {
            return;
        };
        let declared: Option<Vec<(u32, LayoutId)>> = match c.constructor {
            ConstructorKind::Tuple | ConstructorKind::Record(_) => Some(
                layout
                    .fields
                    .iter()
                    .map(|f| (f.ordinal, f.layout))
                    .collect(),
            ),
            ConstructorKind::Variant { variant, .. } => layout
                .tag
                .iter()
                .flat_map(|t| &t.variants)
                .find(|v| v.ordinal == variant)
                .map(|v| v.fields.iter().map(|f| (f.ordinal, f.layout)).collect())
                .or(Some(Vec::new()))
                .filter(|_| {
                    layout
                        .tag
                        .iter()
                        .flat_map(|t| &t.variants)
                        .any(|v| v.ordinal == variant)
                }),
            ConstructorKind::Box => Some(vec![(0, c.layout)]),
            ConstructorKind::Sequence => match layout.kind {
                LayoutKind::Inline { element, count } => {
                    Some((0..count as u32).map(|i| (i, element)).collect())
                }
                // Simd lanes, as legalization builds a lane-wise result.
                LayoutKind::Simd { element, lanes } => {
                    Some((0..lanes).map(|i| (i, element)).collect())
                }
                // A counted sequence holds as many elements as are given.
                LayoutKind::Sequence { element }
                | LayoutKind::SoaColumns { element }
                | LayoutKind::Sequential { element } => {
                    Some((0..fields.len() as u32).map(|i| (i, element)).collect())
                }
                _ => None,
            },
        };
        match declared {
            Some(declared) => {
                // A flat record's variable field takes its source value, which
                // the construction encodes (LIR schema revision 4).
                let flat = layout.kind == LayoutKind::Flat;
                let mut given: Vec<_> = fields
                    .iter()
                    .map(|f| (f.field, self.layout_of(f.value)))
                    .collect();
                given.sort();
                let mut declared = declared;
                declared.retain(|&(f, _)| Some(f) != open);
                declared.sort();
                let matches = given.len() == declared.len()
                    && given.iter().zip(&declared).all(|(&(gf, g), &(df, d))| {
                        gf == df
                            && (g == d
                                || layouts.same_scalar(g, d)
                                || self.whole_byte_slice_owner(facts, layout, fields, gf, g)
                                || flat && layouts.encodes(g, d))
                    });
                if !matches {
                    self.add(site, K::Type("constructed fields"));
                }
                for f in fields {
                    if layout.fields.iter().any(|d| {
                        d.ordinal == f.field
                            && crate::layout::field_is_borrowed(layouts.0, layout, d)
                    }) && self.cx.owns(f.value)
                        && !matches!(unit.values[f.value.index()].holding, Holding::Borrowed(_))
                    {
                        self.add(site, K::Type("borrowed construction field"));
                    }
                }
            }
            None => self.add(site, K::Type("constructor layout")),
        }
    }

    /// Placement exists exactly from Placed on, honours the destination class
    /// and the layout's storage kinds, and matches the result representation.
    fn placement(
        &mut self,
        site: Site,
        (destination, placement): (&DestinationClass, Option<Placement>),
        result: Option<ValueId>,
    ) {
        let unit = self.cx.unit;
        let placed = self.cx.at_least(StageKind::Placed);
        let Some(placement) = placement else {
            if placed {
                self.add(site, K::Placement("allocation without a storage owner"));
            }
            return;
        };
        if !placed {
            self.add(site, K::Placement("placement before the Placed stage"));
            return;
        }
        let own_abi = &self.cx.layouts.0.fn_abis[unit.header.abi.0 as usize];
        let ok = match (destination, placement) {
            (DestinationClass::Heap, Placement::Fixed(StorageOwner::Heap(_))) => true,
            (DestinationClass::Arena(slot), Placement::Fixed(StorageOwner::Arena(arena))) => {
                unit.tables.arenas[arena.index()].slot == *slot
                    && matches!(
                        unit.slots
                            .get(slot.index())
                            .and_then(|s| self.cx.layouts.get(unit.types[s.ty.index()].layout))
                            .map(|l| &l.kind),
                        Some(LayoutKind::Arena)
                    )
            }
            (DestinationClass::Caller(_), Placement::CallerDestination) => own_abi.destination,
            (DestinationClass::Region, Placement::Fixed(StorageOwner::LocalRegion(_))) => true,
            (DestinationClass::Inferred, Placement::CallerDestination) => own_abi.destination,
            (DestinationClass::Inferred, Placement::Fixed(_)) => true,
            _ => false,
        };
        if !ok {
            self.add(
                site,
                K::Placement("placement outside its destination class"),
            );
        }
        let Some(result) = result else {
            return;
        };
        let repr = self.ty(result).repr;
        let kind = match placement {
            Placement::Fixed(StorageOwner::Heap(TaskDomainId(_))) => Some(StorageKind::Heap),
            Placement::Fixed(StorageOwner::LocalRegion(_)) => Some(StorageKind::LocalRegion),
            Placement::Fixed(StorageOwner::Arena(_)) => Some(StorageKind::Arena),
            Placement::Fixed(StorageOwner::Frozen(_)) => Some(StorageKind::Frozen),
            Placement::Fixed(StorageOwner::Static) => Some(StorageKind::Static),
            Placement::CallerDestination => None,
        };
        // An arena block reference keeps the Block representation: it carries
        // its domain, an immortal arena block or a counted heap block when the
        // token is `heap`.
        let block = self.cx.layouts.payload(self.layout_of(result)).is_some();
        let repr_ok = match (kind, repr) {
            (Some(StorageKind::Heap), r) => !block || r != ValueRepr::Uncounted,
            (Some(StorageKind::Arena), r) => !block || r != ValueRepr::Uncounted,
            (Some(_), r) => r == ValueRepr::Uncounted || !matches!(r, ValueRepr::Block),
            (None, _) => true,
        };
        if !repr_ok {
            self.add(
                site,
                K::Placement("result representation of its storage owner"),
            );
        }
        let layout = self.layout_of(result);
        let layout = self.cx.layouts.payload(layout).unwrap_or(layout);
        if let Some(kind) = kind
            && self
                .cx
                .layouts
                .get(layout)
                .is_some_and(|l| !l.storage.contains(&kind))
        {
            self.add(site, K::Placement("storage kind the layout does not admit"));
        }
    }

    fn terminators(&mut self, facts: &Facts) {
        let unit = self.cx.unit;
        for (b, term) in unit.terminators.iter().enumerate() {
            let b = BlockId(b as u32);
            let site = Site::at(b, unit.blocks[b.index()].instructions.len);
            match *term {
                Terminator::Branch { condition, .. } => {
                    if self.scalar(condition) != Some(ScalarKind::Bool) {
                        self.add(site, K::Type("branch condition"));
                    }
                }
                // MEM-6: only `arena.bounded`'s library body recovers, and only
                // the failure of a bounded arena it opened itself.
                Terminator::Recover { arena, .. } => {
                    self.library_only(site, Opcode::Recover);
                    if self.ty(arena).repr != ValueRepr::Arena {
                        self.add(site, K::Type("recovered arena"));
                    }
                    let opened = unit.instructions.iter().any(|inst| {
                        matches!(*inst,
                            Inst::ArenaOpen { result, budget: Some(_) }
                            | Inst::ArenaToken { result, budget: Some(_), .. } if result == arena)
                    });
                    if !opened {
                        self.add(
                            site,
                            K::Placement("a recovery of an arena the unit did not open bounded"),
                        );
                    }
                }
                // FAIL-2: only `task.isolate`'s library body catches, from the
                // record count (a u64) it read before its callback ran.
                Terminator::Isolate { start, .. } => {
                    self.library_only(site, Opcode::Isolate);
                    if self.scalar(start) != Some(ScalarKind::U64) {
                        self.add(site, K::Type("isolation start"));
                    } else if !self.isolate_begin(facts, start) {
                        self.add(
                            site,
                            K::Placement("isolation mark is not an IsolateBegin result"),
                        );
                    }
                }
                Terminator::Switch { value, arms, .. } => {
                    let arms = arms.get(&unit.pools.switch_arms).unwrap_or(&[]);
                    let distinct: BTreeSet<_> = arms.iter().map(|a| a.value).collect();
                    let ok = distinct.len() == arms.len()
                        && match self.ty(value).repr {
                            ValueRepr::Scalar(k) => integer_bits(k).is_some(),
                            ValueRepr::Aggregate | ValueRepr::Uncounted => {
                                self.tag_variants(value).is_some()
                            }
                            _ => false,
                        };
                    if !ok {
                        self.add(site, K::Type("switch"));
                    }
                }
                Terminator::Call { call, normal, .. } => {
                    let call = &unit.tables.calls[call.index()];
                    self.call(site, facts, call);
                    // A call diverges exactly when its callee's ABI does: no
                    // result and no normal edge (LIR schema revision 19).
                    let abi = &self.cx.layouts.0.fn_abis[call.abi.0 as usize];
                    match (abi.diverges, normal.is_some()) {
                        (true, true) => {
                            self.add(site, K::CallShape("returning call of a diverging callee"))
                        }
                        (false, false) => {
                            self.add(site, K::CallShape("diverging call of a returning callee"))
                        }
                        _ => {}
                    }
                    if normal.is_none() && call.normal_results().next().is_some() {
                        self.add(site, K::Type("diverging call result"));
                    }
                }
                Terminator::Return(value) => {
                    let abi = &self.cx.layouts.0.fn_abis[unit.header.abi.0 as usize];
                    let ok = !abi.diverges
                        && match (value, &abi.result) {
                            (None, None) => true,
                            (Some(v), Some(r)) => self.layout_of(v) == r.layout,
                            _ => false,
                        };
                    if !ok {
                        self.add(site, K::Type("returned value"));
                    }
                }
                Terminator::RejectTransfer => {
                    self.library_only(site, Opcode::RejectTransfer);
                    if unit.blocks[b.index()].kind != BlockKind::FailureCleanup {
                        self.add(site, K::Type("private rejection outside failure cleanup"));
                    }
                }
                Terminator::Unreachable => {
                    if !self.unreachable_proved(facts, b) {
                        self.add(site, K::UnprovedUnreachable);
                    }
                }
                Terminator::Jump(_)
                | Terminator::Raise(_)
                | Terminator::Propagate
                | Terminator::InvariantFault { .. } => {}
            }
            for edge in terminator_edges(self.cx, term) {
                self.edge_types(site, edge);
            }
        }
        for (b, block) in unit.blocks.iter().enumerate() {
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                if let Some(f) = Inst::failure(inst) {
                    self.edge_types(
                        Site::at(BlockId(b as u32), i as u32),
                        unit.tables.failures[f.index()].cleanup,
                    );
                }
            }
        }
    }

    /// A numeric count alone cannot authorize a catch. The immutable SSA mark
    /// must be the normal result of this unit's actual intrinsic begin call;
    /// ordinary SSA availability separately proves that edge dominates the use.
    fn isolate_begin(&self, facts: &Facts, start: ValueId) -> bool {
        let Def::Edge(edge) = facts.defs[start.index()] else {
            return false;
        };
        let Some((block, Origin::Terminator)) = facts.cfg.source[edge.index()] else {
            return false;
        };
        let unit = self.cx.unit;
        let Terminator::Call {
            call,
            normal: Some(normal),
            ..
        } = unit.terminators[block.index()]
        else {
            return false;
        };
        let call = &unit.tables.calls[call.index()];
        let Callee::Operation(operation) = call.callee else {
            return false;
        };
        normal == edge
            && call.result == Some(start)
            && unit.tables.operations[operation.index()]
                .definition
                .is_core("$intrinsic.IsolateBegin")
    }

    /// Check scope transitions independently of the library builder and physical
    /// continuation plan. Failure edges retain the invocation until its End;
    /// a successful begin creates it, and a catch handles its suffix only once.
    fn isolate_protocol(&mut self) {
        let unit = self.cx.unit;
        if !unit
            .terminators
            .iter()
            .any(|term| matches!(term, Terminator::Isolate { .. }))
        {
            return;
        }
        #[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
        struct State {
            scope: Option<(ValueId, bool)>,
            failing: bool,
        }
        let mut pending = vec![(
            unit.entry,
            State {
                scope: None,
                failing: false,
            },
        )];
        let mut seen = BTreeSet::new();
        let target = |edge: EdgeId| unit.tables.edges[edge.index()].target;
        while let Some((block, mut state)) = pending.pop() {
            if !seen.insert((block, state)) {
                continue;
            }
            let site = Site::at(block, unit.blocks[block.index()].instructions.len);
            for inst in unit.blocks[block.index()]
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[])
            {
                // RC gives drops and early releases syntactic cleanup edges,
                // but neither can fail without a reachable finalizer/close.
                // Derive that independently from C's layouts, not RC hints.
                let infallible = match inst {
                    Inst::Release { layout, .. } => !self.cx.layouts.destructible(*layout),
                    Inst::Drop { address, .. } => {
                        let ty = unit.addresses[address.index()].ty;
                        !self.cx.layouts.destructible(unit.types[ty.index()].layout)
                    }
                    _ => false,
                };
                if let Some(failure) = inst.failure()
                    && !infallible
                {
                    pending.push((
                        target(unit.tables.failures[failure.index()].cleanup),
                        State {
                            failing: true,
                            ..state
                        },
                    ));
                }
            }
            match unit.terminators[block.index()] {
                Terminator::Call {
                    call,
                    normal,
                    failure,
                } => {
                    let call = &unit.tables.calls[call.index()];
                    let operation = match call.callee {
                        Callee::Operation(op) => {
                            Some(&unit.tables.operations[op.index()].definition)
                        }
                        _ => None,
                    };
                    let begin =
                        operation.is_some_and(|path| path.is_core("$intrinsic.IsolateBegin"));
                    let end = operation.is_some_and(|path| path.is_core("$intrinsic.IsolateEnd"));
                    if end {
                        if state.scope.take().is_none() {
                            self.add(site, K::Placement("isolation End without its active begin"));
                        }
                    }
                    pending.push((
                        target(unit.tables.failures[failure.index()].cleanup),
                        State {
                            failing: true,
                            ..state
                        },
                    ));
                    if begin {
                        if state.scope.is_some() {
                            self.add(site, K::Placement("isolation begin repeated before End"));
                        }
                        state.scope = call.result.map(|mark| (mark, false));
                    } else if matches!(call.callee, Callee::Indirect(_) | Callee::Bound { .. })
                        && state.scope.is_none()
                    {
                        self.add(site, K::Placement("isolation callback before its begin"));
                    }
                    if let Some(normal) = normal {
                        pending.push((target(normal), state));
                    }
                }
                Terminator::Isolate { start, caught } => {
                    if state.scope != Some((start, false)) || !state.failing {
                        self.add(
                            site,
                            K::Placement(
                                "isolation catch without its unhandled failing invocation",
                            ),
                        );
                    }
                    state.scope = state.scope.map(|(mark, _)| (mark, true));
                    state.failing = false;
                    pending.push((target(caught), state));
                }
                Terminator::Return(_) | Terminator::Propagate => {
                    if state.scope.is_some() {
                        self.add(site, K::Placement("isolation terminal path without End"));
                    }
                }
                ref term => {
                    for edge in term.normal_edges(&unit.pools) {
                        pending.push((target(edge), state));
                    }
                    for failure in term.failure_edges(&unit.tables) {
                        pending.push((
                            target(unit.tables.failures[failure.index()].cleanup),
                            State {
                                failing: true,
                                ..state
                            },
                        ));
                    }
                }
            }
        }
    }

    fn tag_variants(&self, value: ValueId) -> Option<BTreeSet<u32>> {
        let layout = self.cx.layouts.get(self.layout_of(value))?;
        let tag = layout.tag.as_ref()?;
        Some(tag.variants.iter().map(|v| v.ordinal).collect())
    }

    /// `Unreachable` is proved only as the otherwise target of switches on a
    /// variant whose arms cover every variant ordinal.
    fn unreachable_proved(&self, facts: &Facts, block: BlockId) -> bool {
        let unit = self.cx.unit;
        let preds = &facts.cfg.preds[block.index()];
        !preds.is_empty()
            && preds
                .iter()
                .all(|&(p, edge)| match unit.terminators[p.index()] {
                    Terminator::Switch {
                        value,
                        arms,
                        otherwise,
                    } if otherwise == edge => {
                        let covered: BTreeSet<_> = arms
                            .get(&unit.pools.switch_arms)
                            .unwrap_or(&[])
                            .iter()
                            .map(|a| a.value)
                            .collect();
                        self.tag_variants(value)
                            .is_some_and(|all| all.is_subset(&covered))
                    }
                    _ => false,
                })
    }

    fn edge_types(&mut self, site: Site, edge: EdgeId) {
        let unit = self.cx.unit;
        let e = &unit.tables.edges[edge.index()];
        let args = e.arguments.get(&unit.pools.values).unwrap_or(&[]);
        let params = unit.blocks[e.target.index()]
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[]);
        if args.len() != params.len() {
            self.add(site, K::Arity("edge arguments"));
            return;
        }
        for (&a, &p) in args.iter().zip(params) {
            if !self.same_type(a, p) {
                self.add(site, K::Type("edge argument"));
            }
        }
    }

    fn call(&mut self, site: Site, facts: &Facts, call: &CallSite) {
        let unit = self.cx.unit;
        let abi: &FnAbi = &self.cx.layouts.0.fn_abis[call.abi.0 as usize];
        match call.callee {
            Callee::Direct(symbol) => {
                if unit.tables.symbols[symbol.index()].kind != SymbolKind::Function
                    || !matches!(
                        abi.shape,
                        CallableShape::Definition | CallableShape::Foreign
                    )
                {
                    self.add(site, K::CallShape("direct callee"));
                }
            }
            Callee::Indirect(value) => {
                // A call through an iterator is a call of its step thunk
                // (LIR schema revision 7).
                if let Some(LayoutRecord {
                    kind: LayoutKind::Iterator { element },
                    ..
                }) = self.cx.layouts.get(self.layout_of(value))
                {
                    if !self.step_abi(abi, *element) {
                        self.add(site, K::Type("iterator call"));
                    }
                } else if self.ty(value).repr != ValueRepr::Callable || !abi.environment {
                    self.add(site, K::CallShape("indirect callee"));
                }
            }
            Callee::Operation(op) => {
                let id = &unit.tables.operations[op.index()];
                if self.cx.manifest.row(id).is_none() {
                    self.add(site, K::UnknownOperation(op));
                }
                let matches = *self
                    .operation_abis
                    .entry((op, call.abi, call.operation_arguments.clone()))
                    .or_insert_with(|| {
                        self.cx
                            .operations
                            .derive(id, &call.operation_arguments, self.cx.target)
                            .is_ok_and(|(expected, table)| {
                                crate::operation::matches_abi(
                                    &expected,
                                    &table,
                                    abi,
                                    self.cx.layouts.0,
                                )
                            })
                    });
                if !matches {
                    self.add(site, K::CallShape("operation declaration ABI"));
                }
            }
            // A bound call (LIR schema revision 9) runs its code with the
            // environment its closure's callable names.
            Callee::Bound { code, closure } => {
                let callable = matches!(
                    self.cx.layouts.get(self.layout_of(closure)),
                    Some(LayoutRecord {
                        kind: LayoutKind::Callable { .. },
                        ..
                    })
                );
                if unit.tables.symbols[code.index()].kind != SymbolKind::Function
                    || self.ty(closure).repr != ValueRepr::Callable
                    || !callable
                    || abi.shape != CallableShape::Closure
                    || !abi.environment
                {
                    self.add(site, K::CallShape("bound callee"));
                }
            }
        }
        let args = call
            .arguments
            .get(&unit.pools.call_arguments)
            .unwrap_or(&[]);
        if args.len() != abi.parameters.len() {
            self.add(site, K::Arity("call arguments"));
        }
        let ordinals: BTreeSet<_> = args.iter().map(|a| a.parameter).collect();
        if ordinals.len() != args.len() {
            self.add(site, K::Arity("call argument ordinals"));
        }
        for arg in args {
            let Some(param) = abi.parameters.iter().find(|p| p.source == arg.parameter) else {
                self.add(site, K::Arity("call argument ordinals"));
                continue;
            };
            if arg.passing != param.convention {
                self.add(site, K::CallShape("argument passing"));
            }
            let layout = match (arg.passing, arg.value) {
                (Passing::Mut, ArgumentValue::Address { address, version }) => {
                    if let Some(slot) = self.slot_root(site, facts, address) {
                        self.version_type(site, slot, version, "mut call version");
                    }
                    if self.flat_variable(facts, address) {
                        self.add(site, K::Type("write into a variable part of a flat value"));
                    }
                    self.address_layout(facts, address)
                }
                (Passing::Borrow | Passing::Take, ArgumentValue::Value(value)) => {
                    Some(self.layout_of(value))
                }
                _ => {
                    self.add(site, K::CallShape("mut arguments are addresses"));
                    None
                }
            };
            if layout.is_some_and(|l| l != param.layout) {
                self.add(site, K::Type("call argument"));
            }
        }
        if let Some(receipts) = &call.receipts {
            for (address, version) in receipts.writebacks() {
                if let Some(slot) = self.slot_root(site, facts, address) {
                    self.version_type(site, slot, version, "receipt mut call version");
                }
            }
        }
        match (call.result, &abi.result) {
            (None, None) => {}
            (Some(result), Some(r)) => {
                if self.layout_of(result) != r.layout {
                    self.add(site, K::Type("call result"));
                }
                self.fresh_holding(site, result);
            }
            _ => self.add(site, K::Arity("call result")),
        }
        if abi.destination != call.destination.is_some() {
            self.add(site, K::CallShape("caller destination"));
        }
    }

    /// Version names: stores, replaces and slot-backed parameters name their
    /// slot; a block parameter any of whose arguments is a version is a
    /// version join of that slot, and then every argument must be one.
    fn versions(&mut self, facts: &mut Facts) {
        let unit = self.cx.unit;
        for (b, block) in unit.blocks.iter().enumerate() {
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for inst in insts {
                if let Inst::Store {
                    address, version, ..
                }
                | Inst::SeqReserve {
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
                }
                | Inst::Replace {
                    address, version, ..
                } = *inst
                    && let Some(Addr {
                        root: Root::Slot(slot),
                        ..
                    }) = facts.addresses[address.index()]
                {
                    facts.version[version.index()] = Some(slot);
                }
                if let Inst::Replace {
                    displaced,
                    displaced_version,
                    ..
                } = *inst
                {
                    facts.version[displaced_version.index()] = Some(displaced);
                }
            }
            let _ = b;
        }
        for term in &unit.terminators {
            let Terminator::Call { call, .. } = *term else {
                continue;
            };
            for (address, version) in unit.tables.calls[call.index()].writebacks(&unit.pools) {
                if let Some(Addr {
                    root: Root::Slot(slot),
                    ..
                }) = facts.addresses[address.index()]
                {
                    facts.version[version.index()] = Some(slot);
                }
            }
        }
        for (i, &(_, slot)) in facts.parameters.iter().enumerate() {
            if let (Some(slot), Some(&value)) = (slot, unit.parameters.get(i)) {
                facts.version[value.index()] = Some(slot);
            }
        }
        if let Some(entry) = &unit.tables.receipt_entry {
            for binding in entry.parameters.iter().chain(&entry.captures) {
                facts.version[binding.version.index()] = Some(binding.slot);
            }
        }
        for (s, slot) in unit.slots.iter().enumerate() {
            if let SlotKind::Capture(ordinal) = slot.kind
                && let Some(&value) = unit.captures.get(ordinal as usize)
            {
                facts.version[value.index()] = Some(SlotId(s as u32));
            }
        }
        // The first slot reaching a parameter wins; the edge check below
        // reports every argument naming another slot.
        propagate(self.cx, &facts.cfg, &mut facts.version, |param, arg| {
            param.or(*arg)
        });
        for (b, block) in unit.blocks.iter().enumerate() {
            let params = block.parameters.get(&unit.pools.values).unwrap_or(&[]);
            for (j, &p) in params.iter().enumerate() {
                if let Some(slot) = facts.version[p.index()] {
                    facts.phis[b].push((j as u32, slot));
                    if unit.values[p.index()].holding != Holding::Plain {
                        self.add(Site::block(BlockId(b as u32)), K::Holding(p));
                    }
                }
            }
        }
        for (e, edge) in unit.tables.edges.iter().enumerate() {
            let args = edge.arguments.get(&unit.pools.values).unwrap_or(&[]);
            let params = unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[]);
            for (&a, &p) in args.iter().zip(params) {
                if let Some(slot) = facts.version[p.index()]
                    && facts.version[a.index()] != Some(slot)
                {
                    let source = facts.cfg.source[e].map(|(b, _)| b);
                    self.add(
                        source.map_or(Site::UNIT, Site::block),
                        K::VersionJoin { slot, value: a },
                    );
                }
            }
        }
    }

    /// Drop flags (LIR schema revision 8): each a plain bool block parameter,
    /// not a version, named once.
    /// A generic nested function's value has no code (LIR schema revision
    /// 9): it is stored whole into a slot, read whole back, passed along
    /// edges, counted and dropped, and is a bound call's closure. Used as an
    /// indirect callee it would run code it does not have; as a call
    /// argument, a return value, a field, a capture or part of a place it
    /// would reach such a call where this unit cannot see it.
    fn codeless(&mut self, facts: &Facts) {
        let unit = self.cx.unit;
        let mut values = BTreeSet::new();
        for inst in &unit.instructions {
            if let Inst::Closure {
                result, closure, ..
            } = *inst
                && unit.tables.closures[closure.index()].code.is_none()
            {
                values.insert(result);
            }
        }
        if values.is_empty() {
            return;
        }
        let whole = |a: AddressId| match facts.addresses[a.index()].as_ref() {
            Some(Addr {
                root: Root::Slot(s),
                path,
                ..
            }) if path.is_empty() => Some(*s),
            _ => None,
        };
        let mut slots = BTreeSet::new();
        loop {
            let before = (values.len(), slots.len());
            for inst in &unit.instructions {
                match *inst {
                    Inst::Store { address, value, .. } | Inst::Replace { address, value, .. }
                        if values.contains(&value) =>
                    {
                        if let Some(s) = whole(address) {
                            slots.insert(s);
                        }
                    }
                    Inst::Read {
                        result, address, ..
                    } if whole(address).is_some_and(|s| slots.contains(&s)) => {
                        values.insert(result);
                    }
                    _ => {}
                }
            }
            for edge in &unit.tables.edges {
                let args = edge.arguments.get(&unit.pools.values).unwrap_or(&[]);
                let params = unit.blocks[edge.target.index()]
                    .parameters
                    .get(&unit.pools.values)
                    .unwrap_or(&[]);
                for (a, &p) in args.iter().zip(params) {
                    if values.contains(a) {
                        values.insert(p);
                    }
                }
            }
            if (values.len(), slots.len()) == before {
                break;
            }
        }
        let escapes = K::CallShape("a code-less closure outside a bound call");
        for (b, block) in unit.blocks.iter().enumerate() {
            let b = BlockId(b as u32);
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                let escaped = match *inst {
                    Inst::Store { address, value, .. } | Inst::Replace { address, value, .. } => {
                        values.contains(&value) && whole(address).is_none()
                    }
                    Inst::Construct { construct, .. } => unit.tables.constructs[construct.index()]
                        .fields
                        .get(&unit.pools.fields)
                        .unwrap_or(&[])
                        .iter()
                        .any(|f| values.contains(&f.value)),
                    Inst::Closure { closure, .. } => unit.tables.closures[closure.index()]
                        .captures
                        .get(&unit.pools.captures)
                        .unwrap_or(&[])
                        .iter()
                        .any(|c| values.contains(&c.value)),
                    _ => false,
                };
                if escaped {
                    self.add(Site::at(b, i as u32), escapes.clone());
                }
            }
            let site = Site::at(b, block.instructions.len);
            let escaped = match unit.terminators[b.index()] {
                Terminator::Return(Some(v)) => values.contains(&v),
                Terminator::Call { call, .. } => {
                    let call = &unit.tables.calls[call.index()];
                    matches!(call.callee, Callee::Indirect(v) if values.contains(&v))
                        || call
                            .arguments
                            .get(&unit.pools.call_arguments)
                            .unwrap_or(&[])
                            .iter()
                            .any(|a| matches!(a.value, ArgumentValue::Value(v) if values.contains(&v)))
                }
                _ => false,
            };
            if escaped {
                self.add(site, escapes.clone());
            }
        }
    }

    /// Persistent capture takes are committed to caller-owned storage before any
    /// failing operation or edge. Ordinary slot replay proves single takes and
    /// rejects subsequent reads; only this canonical store may edit the live word.
    fn capture_mask(&mut self, facts: &mut Facts) {
        let unit = self.cx.unit;
        let environment = unit
            .tables
            .environment()
            .and_then(|id| self.cx.layouts.get(id));
        let mask = environment.and_then(|r| match &r.kind {
            LayoutKind::Environment { movable } => r.fields.last().map(|word| (r, movable, word)),
            _ => None,
        });
        if unit.tables.environment().is_some()
            && (mask.is_none()
                || self
                    .cx
                    .layouts
                    .0
                    .fn_abis
                    .get(unit.header.abi.0 as usize)
                    .is_none_or(|a| {
                        a.shape != CallableShape::Closure
                            || a.multiplicity != CallMultiplicity::Once
                    }))
        {
            self.add(Site::UNIT, K::Type("capture live mask"));
        }
        let persistent: Vec<_> = unit
            .tables
            .init_flags
            .iter()
            .filter(|flag| flag.environment.is_some())
            .collect();
        if let Some((_, _, word)) = mask {
            if persistent.len() != 1
                || persistent.first().is_none_or(|flag| {
                    unit.slots[flag.slot.index()].kind != SlotKind::Capture(word.ordinal)
                        || unit.captures.get(word.ordinal as usize) != Some(&flag.value)
                })
            {
                self.add(Site::UNIT, K::Type("capture live mask"));
            }
        }
        let mask_slot = mask
            .and_then(|(_, _, word)| {
                unit.slots
                    .iter()
                    .position(|s| s.kind == SlotKind::Capture(word.ordinal))
            })
            .map(|i| SlotId(i as u32));
        if let Some((r, _, _)) = mask {
            if unit.captures.len() != r.fields.len() || mask_slot.is_none() {
                self.add(Site::UNIT, K::Type("capture live mask"));
            }
            for slot in &unit.slots {
                if let SlotKind::Capture(i) = slot.kind
                    && r.fields
                        .get(i as usize)
                        .is_none_or(|f| f.layout != unit.types[slot.ty.index()].layout)
                {
                    self.add(Site::UNIT, K::Type("capture live mask"));
                }
            }
        }
        let root = |address: AddressId| {
            facts
                .addresses
                .get(address.index())
                .and_then(|a| a.as_ref())
                .and_then(|a| match a.root {
                    Root::Slot(s) => Some((s, a.path.is_empty())),
                    _ => None,
                })
        };
        let mut stores = BTreeSet::new();
        for (bi, block) in unit.blocks.iter().enumerate() {
            for (ii, inst) in block
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[])
                .iter()
                .enumerate()
            {
                let Inst::Read {
                    address,
                    mode: ReadMode::Take,
                    ..
                } = inst
                else {
                    continue;
                };
                let Some((slot, whole)) = root(*address) else {
                    continue;
                };
                let SlotKind::Capture(ordinal) = unit.slots[slot.index()].kind else {
                    continue;
                };
                if !self.cx.owns_type(unit.slots[slot.index()].ty)
                    && !mask.is_some_and(|(_, movable, _)| movable.contains(&ordinal))
                {
                    continue;
                }
                let bit =
                    mask.and_then(|(_, movable, _)| movable.iter().position(|&i| i == ordinal));
                let suffix = block
                    .instructions
                    .get(&unit.instructions)
                    .unwrap_or(&[])
                    .get(ii + 1..ii + 4);
                let good = match (whole, bit, mask_slot, suffix) {
                    (
                        true,
                        Some(bit),
                        Some(mask_slot),
                        Some(
                            [
                                Inst::Read {
                                    result: old,
                                    address: read,
                                    mode: ReadMode::Bits,
                                    ..
                                },
                                Inst::UpdateMask {
                                    result: new,
                                    mask: old_mask,
                                    field,
                                    initialized: false,
                                },
                                Inst::Store {
                                    address: store,
                                    value,
                                    ..
                                },
                            ],
                        ),
                    ) => {
                        root(*read) == Some((mask_slot, true))
                            && read == store
                            && old_mask == old
                            && value == new
                            && *field == bit as u32
                    }
                    _ => false,
                };
                if good {
                    stores.insert((bi, ii + 3));
                    facts
                        .capture_mask_stores
                        .insert(Site::at(BlockId(bi as u32), (ii + 3) as u32));
                } else {
                    self.add(
                        Site::at(BlockId(bi as u32), ii as u32),
                        K::Type("capture live mask"),
                    );
                }
            }
        }
        for (bi, block) in unit.blocks.iter().enumerate() {
            for (ii, inst) in block
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[])
                .iter()
                .enumerate()
            {
                if matches!(inst, Inst::Read { address, mode: ReadMode::Take, .. } if root(*address).is_some_and(|(slot, _)| Some(slot) == mask_slot))
                    || matches!(inst, Inst::StorageDead { slot, .. } | Inst::StorageLive { slot } if Some(*slot) == mask_slot)
                {
                    self.add(
                        Site::at(BlockId(bi as u32), ii as u32),
                        K::Type("capture live mask"),
                    );
                }
                let address = match inst {
                    Inst::Store { address, .. } | Inst::Replace { address, .. } => Some(*address),
                    _ => None,
                };
                if let Some((slot, _)) = address.and_then(root) {
                    let guarded_capture = matches!(unit.slots[slot.index()].kind, SlotKind::Capture(i) if mask.is_some_and(|(_, movable, _)| movable.contains(&i)));
                    if (Some(slot) == mask_slot && !stores.contains(&(bi, ii))) || guarded_capture {
                        self.add(
                            Site::at(BlockId(bi as u32), ii as u32),
                            K::Type("capture live mask"),
                        );
                    }
                }
            }
            if let Terminator::Call { call, .. } = unit.terminators[bi] {
                for arg in unit.tables.calls[call.index()]
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .unwrap_or(&[])
                {
                    if let ArgumentValue::Address { address, .. } = arg.value
                        && root(address).is_some_and(|(slot, _)| Some(slot) == mask_slot)
                    {
                        self.add(
                            Site::block(BlockId(bi as u32)),
                            K::Type("capture live mask"),
                        );
                    }
                }
            }
        }
    }

    /// Drop flags name only finite inline places: no counted dereference or
    /// dynamically selected element can carry a frame-owned presence fact.
    fn flag_path(
        &self,
        slot: SlotId,
        range: crate::ir::PoolRange<crate::ir::ProjectionStep>,
    ) -> Option<Vec<Step>> {
        let unit = self.cx.unit;
        let mut layout = unit.types[unit.slots[slot.index()].ty.index()].layout;
        let mut path = Vec::new();
        for step in range.get(&unit.pools.projections)? {
            let record = self.cx.layouts.get(layout)?;
            let (next, part) = match *step {
                crate::ir::ProjectionStep::Field(f) => (
                    record.fields.iter().find(|x| x.ordinal == f)?.layout,
                    Step::Field(f),
                ),
                crate::ir::ProjectionStep::VariantField { variant, field } => (
                    record
                        .tag
                        .as_ref()?
                        .variants
                        .iter()
                        .find(|v| v.ordinal == variant)?
                        .fields
                        .iter()
                        .find(|f| f.ordinal == field)?
                        .layout,
                    Step::Variant(variant, field),
                ),
                crate::ir::ProjectionStep::Index(value) => {
                    if self.ty(value).repr != ValueRepr::Scalar(ScalarKind::U64) {
                        return None;
                    }
                    let LayoutKind::Inline { element, count } = record.kind else {
                        return None;
                    };
                    let index = unit.instructions.iter().find_map(|i| match *i {
                        Inst::Const { result, constant } if result == value => {
                            match unit.tables.constants[constant.index()].value {
                                crate::ir::ConstantValue::Integer(n) if n < u128::from(count) => {
                                    Some(n as u64)
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    })?;
                    (element, Step::Inline(index))
                }
                _ => return None,
            };
            path.push(part);
            layout = next;
        }
        Some(path)
    }

    fn init_flags(&mut self, facts: &mut Facts) {
        let unit = self.cx.unit;
        let mut seen = BTreeSet::new();
        for flag in &unit.tables.init_flags {
            if flag.environment.is_some() {
                if flag.path.len != 0 {
                    self.add(
                        Site::UNIT,
                        K::InitFlag {
                            slot: flag.slot,
                            value: flag.value,
                        },
                    );
                }
                continue;
            }
            let (value, slot) = (flag.value, flag.slot);
            let bad = |k| K::InitFlag { slot, value: k };
            let Def::Param(b, j) = facts.defs[value.index()] else {
                self.add(Site::UNIT, bad(value));
                continue;
            };
            let boolean = self.ty(value).repr == ValueRepr::Scalar(ScalarKind::Bool);
            if !boolean
                || unit.values[value.index()].holding != Holding::Plain
                || facts.version[value.index()].is_some()
                || !seen.insert(value)
            {
                self.add(Site::block(b), bad(value));
                continue;
            }
            let Some(path) = self.flag_path(slot, flag.path) else {
                self.add(Site::block(b), bad(value));
                continue;
            };
            if facts.flags[b.index()]
                .iter()
                .any(|(_, s, p)| *s == slot && *p == path)
            {
                self.add(Site::block(b), bad(value));
                continue;
            }
            facts.flags[b.index()].push((j, slot, path));
        }
    }

    /// Regions a mark or uncounted value belongs to, joined through parameters.
    /// The views and loans each borrowed block parameter joins. A joined
    /// view declares the least of its loans, so its holding is canonical.
    fn joins(&mut self, facts: &mut Facts) {
        let unit = self.cx.unit;
        let mut parameter = vec![false; unit.values.len()];
        for block in &unit.blocks {
            for &p in block.parameters.get(&unit.pools.values).unwrap_or(&[]) {
                parameter[p.index()] = true;
            }
        }
        let mut joins: Vec<Join> = unit
            .values
            .iter()
            .enumerate()
            .map(|(v, info)| match info.holding {
                _ if parameter[v] => Join::default(),
                Holding::Borrowed(loan) => Join {
                    loans: BTreeSet::from([loan]),
                    sources: BTreeSet::from([ValueId(v as u32)]),
                },
                _ => Join {
                    loans: BTreeSet::new(),
                    sources: BTreeSet::from([ValueId(v as u32)]),
                },
            })
            .collect();
        // A read through a borrowed SSA value inherits every possible loan
        // of that value, not just its representative label. Solve projections
        // and block arguments together: a projected child may feed its phi.
        let inherited: Vec<_> = facts
            .backing
            .iter()
            .enumerate()
            .filter_map(|(v, backing)| {
                let Backing::Value(base) = backing else {
                    return None;
                };
                (matches!(unit.values[v].holding, Holding::Borrowed(_))
                    && unit.values[v].holding == unit.values[base.index()].holding)
                    .then_some((v, base.index()))
            })
            .collect();
        loop {
            let before = joins.clone();
            propagate(self.cx, &facts.cfg, &mut joins, |param, arg| Join {
                loans: param.loans.union(&arg.loans).copied().collect(),
                sources: param.sources.union(&arg.sources).copied().collect(),
            });
            for &(value, base) in &inherited {
                let loans = joins[base].loans.clone();
                joins[value].loans.extend(loans);
            }
            if joins == before {
                break;
            }
        }
        for (v, join) in joins.into_iter().enumerate() {
            let Holding::Borrowed(loan) = unit.values[v].holding else {
                continue;
            };
            if !parameter[v] && !inherited.iter().any(|(value, _)| *value == v) {
                continue;
            }
            if join.joined() && join.loans.first() != Some(&loan) {
                self.add(Site::UNIT, K::Holding(ValueId(v as u32)));
            }
            facts.joins[v] = Some(join);
        }
    }

    /// An arena token cannot be forged: only
    /// an arena opening, a region's token, a read or copy of token storage,
    /// a parameter or a block parameter defines one. Every arena instance
    /// names a slot that holds a token. Checked storage-version names only
    /// rename contents and are never runtime token operands.
    fn arena_tokens(&mut self, facts: &Facts) {
        let unit = self.cx.unit;
        for (b, block) in unit.blocks.iter().enumerate() {
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                let producer = matches!(
                    inst,
                    Inst::Const { constant, .. } if unit.tables.constants[constant.index()].value == ConstantValue::Heap
                ) || matches!(
                    inst,
                    Inst::ArenaOpen { .. }
                        | Inst::ArenaToken { .. }
                        | Inst::Read { .. }
                        | Inst::Copy { .. }
                );
                if !producer
                    && Inst::defs(inst).into_iter().any(|v| {
                        self.ty(v).repr == ValueRepr::Arena && facts.version[v.index()].is_none()
                    })
                {
                    self.add(
                        Site::at(BlockId(b as u32), i as u32),
                        K::Type("forged arena token"),
                    );
                }
            }
        }
        for call in &unit.tables.calls {
            if call
                .result
                .is_some_and(|v| self.ty(v).repr == ValueRepr::Arena)
            {
                self.add(Site::UNIT, K::Type("forged arena token"));
            }
        }
        for instance in &unit.tables.arenas {
            let token = unit
                .slots
                .get(instance.slot.index())
                .and_then(|s| self.cx.layouts.get(unit.types[s.ty.index()].layout))
                .is_some_and(|l| l.kind == LayoutKind::Arena);
            if !token {
                self.add(
                    Site::UNIT,
                    K::Placement("an arena instance whose slot holds no token"),
                );
            }
        }
    }

    fn regions(&mut self, facts: &mut Facts) {
        let unit = self.cx.unit;
        for block in &unit.blocks {
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for inst in insts {
                match *inst {
                    Inst::RegionEnter { result, region } => {
                        facts.regions[result.index()].insert(region);
                    }
                    Inst::Construct {
                        result, construct, ..
                    }
                    | Inst::Reuse {
                        result, construct, ..
                    } => {
                        if let Some(Placement::Fixed(StorageOwner::LocalRegion(r))) =
                            unit.tables.constructs[construct.index()].placement
                        {
                            facts.regions[result.index()].insert(r);
                        }
                    }
                    Inst::Closure {
                        result, closure, ..
                    } => {
                        if let Some(Placement::Fixed(StorageOwner::LocalRegion(r))) =
                            unit.tables.closures[closure.index()].placement
                        {
                            facts.regions[result.index()].insert(r);
                        }
                    }
                    _ => {}
                }
            }
        }
        // Reconstruct containment and storage flow independently of placement.
        let mut users = vec![Vec::new(); unit.values.len()];
        let mut flow = |from: ValueId, to: ValueId| {
            users[from.index()].push(to);
        };
        for edge in &unit.tables.edges {
            for (&a, &p) in edge
                .arguments
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .zip(
                    unit.blocks[edge.target.index()]
                        .parameters
                        .get(&unit.pools.values)
                        .unwrap_or(&[]),
                )
            {
                flow(a, p);
            }
        }
        for inst in &unit.instructions {
            match *inst {
                Inst::RegionReserve { result, mark, .. }
                | Inst::ArenaToken { result, mark, .. } => flow(mark, result),
                Inst::Read {
                    result, reaching, ..
                }
                | Inst::Copy {
                    result, reaching, ..
                } if matches!(self.ty(result).repr, ValueRepr::Uncounted) => flow(reaching, result),
                Inst::Store { value, version, .. } | Inst::Replace { value, version, .. } => {
                    flow(value, version)
                }
                Inst::Construct {
                    result, construct, ..
                } => {
                    let c = &unit.tables.constructs[construct.index()];
                    if c.constructor != ConstructorKind::Box
                        && self
                            .cx
                            .layouts
                            .get(c.layout)
                            .is_some_and(|l| l.kind == LayoutKind::Flat)
                    {
                        continue;
                    }
                    for f in c.fields.get(&unit.pools.fields).unwrap_or(&[]) {
                        flow(f.value, result);
                    }
                }
                _ => {}
            }
        }
        let mut queue: std::collections::VecDeque<_> = (0..unit.values.len())
            .filter(|&v| !facts.regions[v].is_empty())
            .collect();
        let mut queued: Vec<_> = facts.regions.iter().map(|r| !r.is_empty()).collect();
        while let Some(v) = queue.pop_front() {
            queued[v] = false;
            for &to in &users[v] {
                let incoming = facts.regions[v].clone();
                let old = facts.regions[to.index()].len();
                facts.regions[to.index()].extend(incoming);
                if facts.regions[to.index()].len() != old && !queued[to.index()] {
                    queue.push_back(to.index());
                    queued[to.index()] = true;
                }
            }
        }
        // Inline uncounted fields must have regional provenance; an ABI or
        // static value with ordinary counted fields cannot acquire this shape.
        for v in 0..unit.values.len() {
            let value = ValueId(v as u32);
            let layout = self.layout_of(value);
            if self.ty(value).repr == ValueRepr::Uncounted
                && self.cx.layouts.payload(layout).is_none()
                && self.cx.layouts.get(layout).is_some_and(|l| {
                    !l.scan.is_empty() || !l.drop.is_empty() || l.finalizer.is_some()
                })
                && facts.regions[v].is_empty()
            {
                self.add(Site::UNIT, K::Type("region construction field"));
            }
        }
        // A headerless regional payload cannot own a heap child. Inline
        // containers use the same rule before being stored into that payload.
        for (b, block) in unit.blocks.iter().enumerate() {
            for (i, inst) in block
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[])
                .iter()
                .enumerate()
            {
                if let Inst::Construct {
                    result, construct, ..
                } = *inst
                {
                    let c = &unit.tables.constructs[construct.index()];
                    let regional = self.ty(result).repr == ValueRepr::Uncounted
                        && (self.cx.layouts.payload(self.layout_of(result)).is_none()
                            || c.arena_no_drop);
                    let encoded = c.constructor != ConstructorKind::Box
                        && self
                            .cx
                            .layouts
                            .get(c.layout)
                            .is_some_and(|l| l.kind == LayoutKind::Flat);
                    for f in c.fields.get(&unit.pools.fields).unwrap_or(&[]) {
                        if encoded {
                            continue;
                        }
                        let child_regions = &facts.regions[f.value.index()];
                        let ok = if regional {
                            self.cx.layouts.trivial(self.layout_of(f.value))
                                || matches!(self.ty(f.value).repr, ValueRepr::Uncounted)
                                    && !child_regions.is_empty()
                                    && *child_regions == facts.regions[result.index()]
                        } else {
                            child_regions.is_empty()
                        };
                        if !ok {
                            self.add(
                                Site::at(BlockId(b as u32), i as u32),
                                K::Type("region construction field"),
                            );
                        }
                    }
                    if regional && self.cx.layouts.destructible(c.layout) {
                        self.add(
                            Site::at(BlockId(b as u32), i as u32),
                            K::Type("region construction finalizer"),
                        );
                    }
                    // RC-ROOT (S-14): a region frees at its own end, not at
                    // the value's cleanup anchor.
                    if regional && self.cx.layouts.root_bearing(c.layout) {
                        self.add(
                            Site::at(BlockId(b as u32), i as u32),
                            K::RootInRegion(result),
                        );
                    }
                }
            }
        }
        for block in &unit.blocks {
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for inst in insts {
                if let Inst::RegionExit { mark, region, .. } = *inst
                    && facts.regions[mark.index()] != BTreeSet::from([region])
                {
                    self.add(Site::UNIT, K::RegionState(region));
                }
            }
        }
    }

    /// Every use is dominated by its definition; addresses by their
    /// preparation; version names appear only as reaching operands and join
    /// arguments.
    fn uses(&mut self, facts: &Facts) {
        let unit = self.cx.unit;
        let mut prepared: Vec<Option<(BlockId, u32)>> = vec![None; unit.addresses.len()];
        for (b, block) in unit.blocks.iter().enumerate() {
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                if let Inst::Address { address, .. } = *inst {
                    prepared[address.index()].get_or_insert((BlockId(b as u32), i as u32));
                }
            }
        }
        for (b, block) in unit.blocks.iter().enumerate() {
            let b = BlockId(b as u32);
            if !facts.cfg.reachable(b) {
                continue;
            }
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            for (i, inst) in insts.iter().enumerate() {
                let at = At {
                    block: b,
                    pos: i as u32,
                    edge: None,
                };
                for (value, usage) in inst_operands(self.cx, inst) {
                    self.use_value(facts, at, value, usage);
                }
                for address in inst_addresses(self.cx, inst) {
                    self.use_address(facts, &prepared, at, address);
                    let read = matches!(
                        inst,
                        Inst::Address { .. }
                            | Inst::Read {
                                mode: ReadMode::Borrow,
                                ..
                            }
                            | Inst::BeginLoan { mutable: false, .. }
                    );
                    if !read && self.frozen(facts, address) {
                        self.add(
                            Site::at(b, i as u32),
                            K::Type("access to a frozen value other than a borrow"),
                        );
                    }
                }
                if let Some(f) = Inst::failure(inst) {
                    self.failure_uses(facts, at, f);
                }
            }
            let pos = insts.len() as u32;
            let term = &unit.terminators[b.index()];
            let at = At {
                block: b,
                pos,
                edge: None,
            };
            for (value, usage) in terminator_operands(self.cx, term) {
                self.use_value(facts, at, value, usage);
            }
            if let Terminator::Call { call, .. } = *term {
                for arg in unit.tables.calls[call.index()]
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .unwrap_or(&[])
                {
                    if let ArgumentValue::Address { address, .. } = arg.value {
                        self.use_address(facts, &prepared, at, address);
                        if self.frozen(facts, address) {
                            self.add(
                                Site::at(b, pos),
                                K::Type("access to a frozen value other than a borrow"),
                            );
                        }
                    }
                }
            }
            for failure in term.failure_edges(&unit.tables) {
                self.failure_uses(facts, at, failure);
            }
            for edge in terminator_edges(self.cx, term) {
                let at = At {
                    block: b,
                    pos,
                    edge: Some(edge),
                };
                for &a in unit.tables.edges[edge.index()]
                    .arguments
                    .get(&unit.pools.values)
                    .unwrap_or(&[])
                {
                    self.use_value(facts, at, a, Usage::Argument);
                }
            }
        }
    }

    fn failure_uses(&mut self, facts: &Facts, at: At, failure: FailureId) {
        let unit = self.cx.unit;
        let f = &unit.tables.failures[failure.index()];
        let edge_at = At {
            edge: Some(f.cleanup),
            ..at
        };
        for &a in unit.tables.edges[f.cleanup.index()]
            .arguments
            .get(&unit.pools.values)
            .unwrap_or(&[])
        {
            self.use_value(facts, edge_at, a, Usage::Argument);
        }
        for &t in f.dispose.get(&unit.pools.values).unwrap_or(&[]) {
            self.use_value(facts, at, t, Usage::Operand);
        }
    }

    fn available(&self, facts: &Facts, def: Def, at: At) -> bool {
        let cfg = &facts.cfg;
        match def {
            Def::Undefined => false,
            Def::Entry => true,
            Def::Param(b, _) => cfg.dominates(b, at.block),
            Def::Inst(b, i) => {
                if b == at.block {
                    i < at.pos
                } else {
                    cfg.dominates(b, at.block)
                }
            }
            Def::Edge(edge) => {
                if at.edge == Some(edge) {
                    return true;
                }
                let target = self.cx.unit.tables.edges[edge.index()].target;
                target != cfg.entry
                    && cfg.preds[target.index()].len() == 1
                    && cfg.dominates(target, at.block)
            }
            Def::Call(b) => {
                if b == at.block {
                    at.edge.is_some()
                } else {
                    cfg.dominates(b, at.block)
                }
            }
        }
    }

    fn use_value(&mut self, facts: &Facts, at: At, value: ValueId, usage: Usage) {
        let site = Site::at(at.block, at.pos);
        // A reaching version names the content a read finds, not a value it
        // uses: one defined in another block need not dominate the read
        // (content a drop flag decides was stored on some paths only), as
        // the ownership check proves it the slot's one reaching version
        // wherever the slot holds content (LIR schema revision 8). One named
        // before its definition in its own block is still refused.
        let elsewhere = match facts.defs[value.index()] {
            Def::Param(b, _) | Def::Inst(b, _) | Def::Call(b) => b != at.block,
            Def::Edge(edge) => self.cx.unit.tables.edges[edge.index()].target != at.block,
            Def::Entry | Def::Undefined => false,
        };
        let named = usage == Usage::Reaching && elsewhere
            || usage == Usage::Argument
                && at.edge.is_some()
                && facts.version[value.index()].is_some();
        if !named && !self.available(facts, facts.defs[value.index()], at) {
            self.add(site, K::NotDominated(value));
        }
        let version = facts.version[value.index()].is_some();
        match usage {
            Usage::Operand if version => self.add(site, K::VersionAsValue(value)),
            Usage::Reaching if !version => {
                self.add(site, K::Type("reaching operand is not a version"))
            }
            _ => {}
        }
    }

    fn use_address(
        &mut self,
        facts: &Facts,
        prepared: &[Option<(BlockId, u32)>],
        at: At,
        address: AddressId,
    ) {
        let ok = prepared[address.index()]
            .is_some_and(|(b, i)| self.available(facts, Def::Inst(b, i), at));
        if !ok {
            self.add(Site::at(at.block, at.pos), K::AddressUnprepared(address));
        }
    }
}

fn integer_bits(k: ScalarKind) -> Option<u32> {
    Some(match k {
        ScalarKind::U8 | ScalarKind::I8 => 8,
        ScalarKind::U16 | ScalarKind::I16 => 16,
        ScalarKind::U32 | ScalarKind::I32 => 32,
        ScalarKind::U64 | ScalarKind::I64 => 64,
        ScalarKind::U128 | ScalarKind::I128 => 128,
        _ => return None,
    })
}

/// Whether every value of integer kind `a` is representable in `b` (SEM-1 widen).
fn widens(a: ScalarKind, b: ScalarKind) -> bool {
    match (integer_bits(a), integer_bits(b)) {
        (Some(x), Some(y)) => match (signed(a), signed(b)) {
            (true, false) => false,
            (false, true) => y > x,
            _ => y >= x,
        },
        _ => false,
    }
}

fn signed(k: ScalarKind) -> bool {
    matches!(
        k,
        ScalarKind::I8 | ScalarKind::I16 | ScalarKind::I32 | ScalarKind::I64 | ScalarKind::I128
    )
}

fn construct_placement<'a, S>(
    cx: &'a Cx<'_, S>,
    construct: ConstructId,
) -> (&'a DestinationClass, Option<Placement>) {
    let c = &cx.unit.tables.constructs[construct.index()];
    (&c.destination, c.placement)
}

/// Whether `value` is available at the start of `block`, i.e. still in scope
/// for a credit entering it.
pub(super) fn in_scope_at<S: Stage>(
    cx: &Cx<'_, S>,
    facts: &Facts,
    value: ValueId,
    block: BlockId,
) -> bool {
    let mut report = Report::default();
    let c = Checker {
        cx,
        report: &mut report,
        defs: Vec::new(),
        operation_abis: Default::default(),
    };
    c.available(
        facts,
        facts.defs[value.index()],
        At {
            block,
            pos: 0,
            edge: None,
        },
    ) && !matches!(facts.defs[value.index()], Def::Inst(b, _) | Def::Param(b, _) if b == block)
}

/// Join a per-value fact from edge arguments into block parameters until
/// nothing changes. `join` must be monotone on a finite lattice.
pub(super) fn propagate<S, T: Clone + PartialEq>(
    cx: &Cx<'_, S>,
    cfg: &Cfg,
    facts: &mut [T],
    mut join: impl FnMut(&T, &T) -> T,
) {
    let unit = cx.unit;
    loop {
        let mut changed = false;
        for (e, edge) in unit.tables.edges.iter().enumerate() {
            if cfg.source[e].is_none() {
                continue;
            }
            let args = edge.arguments.get(&unit.pools.values).unwrap_or(&[]);
            let params = unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[]);
            for (&a, &p) in args.iter().zip(params) {
                let joined = join(&facts[p.index()], &facts[a.index()]);
                if joined != facts[p.index()] {
                    facts[p.index()] = joined;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Value operands of an instruction, excluding the values it defines.
pub(super) fn inst_operands<S>(cx: &Cx<'_, S>, inst: &Inst) -> Vec<(ValueId, Usage)> {
    let unit = cx.unit;
    let op = |v| (v, Usage::Operand);
    let mut out = Vec::new();
    match *inst {
        Inst::PayloadReceipt { value, inputs, .. } => {
            out.push(op(value));
            for input in inputs.get(&unit.pools.receipt_inputs).unwrap_or(&[]) {
                out.extend(input.receipt.map(op));
            }
        }
        Inst::SealArena { grant, root, .. } => out.extend([op(grant), op(root)]),
        Inst::BorrowSealedRoot { owner, .. } => out.push(op(owner)),
        Inst::ArenaInventory {
            arena,
            value,
            previous,
            ..
        } => {
            out.extend([op(arena), op(value)]);
            out.extend(previous.map(op));
        }
        Inst::CopyValue { value: operand, .. }
        | Inst::Unary { operand, .. }
        | Inst::Convert { operand, .. }
        | Inst::ScalarUnary { operand, .. } => out.push(op(operand)),
        Inst::Binary { left, right, .. }
        | Inst::Compare { left, right, .. }
        | Inst::IntArith { left, right, .. } => out.extend([op(left), op(right)]),
        Inst::MulAdd {
            left,
            right,
            addend,
            ..
        } => out.extend([op(left), op(right), op(addend)]),
        Inst::Check { condition, .. } => out.push(op(condition)),
        Inst::SliceView {
            value, start, end, ..
        } => out.extend([op(value), op(start), op(end)]),
        Inst::Address { address, .. } => {
            let info = &unit.addresses[address.index()];
            if let AddressBase::Value(v) = info.base {
                out.push(op(v));
            }
            for step in info.projection.get(&unit.pools.projections).unwrap_or(&[]) {
                match *step {
                    ProjectionStep::Payload
                    | ProjectionStep::Field(_)
                    | ProjectionStep::VariantField { .. } => {}
                    ProjectionStep::Index(v) | ProjectionStep::Slot(v) => out.push(op(v)),
                    ProjectionStep::FlatAt { index, offset } => out.extend([op(index), op(offset)]),
                }
            }
        }
        Inst::Read {
            reaching, address, ..
        }
        | Inst::Copy {
            reaching, address, ..
        } => {
            // A payload read names the block value it reads through.
            let usage = match base_root(unit, address) {
                Some(AddressBase::Value(_)) => Usage::Operand,
                _ => Usage::Reaching,
            };
            out.push((reaching, usage));
        }
        Inst::UniqueCopy { value, .. }
        | Inst::Freeze { value, .. }
        | Inst::Thaw { value, .. }
        | Inst::Store { value, .. }
        | Inst::Replace { value, .. }
        | Inst::Dup { value }
        | Inst::Share { value }
        | Inst::IsUnique { value, .. }
        | Inst::IsWritable { value, .. }
        | Inst::Weaken { value, .. }
        | Inst::SeqLen { value, .. }
        | Inst::SeqCapacity { value, .. }
        | Inst::Hash { value, .. }
        | Inst::Encode { value, .. }
        | Inst::Decode { value, .. }
        | Inst::Recast { value, .. }
        | Inst::SeqReserve {
            capacity: value, ..
        }
        | Inst::SeqSetLen { len: value, .. }
        | Inst::SeqAppend { value, .. }
        | Inst::ArenaPush {
            count: Some(value), ..
        }
        | Inst::RegionPublish { value }
        | Inst::Forget { value } => out.push(op(value)),
        Inst::UpdateMask { mask, .. } => out.push(op(mask)),
        Inst::Construct { construct, .. }
        | Inst::FlatAppend { construct, .. }
        | Inst::FlatClose { construct, .. } => out.extend(construct_values(cx, construct).map(op)),
        Inst::FlatField { op: field, .. } => match field {
            FlatFieldOp::Open => {}
            FlatFieldOp::Reserve { capacity: value } | FlatFieldOp::Push { value, .. } => {
                out.push(op(value))
            }
        },
        Inst::Reuse {
            token, construct, ..
        } => {
            out.push(op(token));
            out.extend(construct_values(cx, construct).map(op));
        }
        Inst::Closure { closure, .. } => {
            let closure = &unit.tables.closures[closure.index()];
            out.extend(
                closure
                    .captures
                    .get(&unit.pools.captures)
                    .unwrap_or(&[])
                    .iter()
                    .map(|c| op(c.value)),
            );
            if let Some(receipts) = &closure.receipts {
                out.extend(receipts.captures.iter().map(|c| op(c.receipt)));
            }
        }
        Inst::Release { value, live, .. } | Inst::Reset { value, live, .. } => {
            out.push(op(value));
            if let MaskRef::Dynamic(mask) = live {
                out.push(op(mask));
            }
        }
        Inst::Dispose { token } => out.push(op(token)),
        Inst::HoleAbandon { hole, root } => {
            out.push(op(hole));
            out.extend(root.map(op));
        }
        Inst::HoleFill { hole, value } => out.extend([op(hole), op(value)]),
        Inst::RegionReserve { mark, extent, .. } => out.extend([op(mark), op(extent)]),
        Inst::RegionExit { mark, .. } => out.push(op(mark)),
        Inst::ArenaToken { mark, budget, .. } => {
            out.push(op(mark));
            out.extend(budget.map(op));
        }
        Inst::ArenaOpen { budget, .. } => out.extend(budget.map(op)),
        _ => {}
    }
    out
}

/// The write-back versions a call's `mut` arguments define.
pub(super) fn mut_versions<'a, S>(
    cx: &'a Cx<'_, S>,
    call: &'a CallSite,
) -> impl Iterator<Item = ValueId> + 'a {
    call.writebacks(&cx.unit.pools).map(|(_, version)| version)
}

/// The innermost base of an address chain, or None for a cyclic chain.
fn base_root<S>(unit: &crate::ir::LirUnit<S>, address: AddressId) -> Option<AddressBase> {
    let mut seen = BTreeSet::new();
    let mut next = address;
    loop {
        if !seen.insert(next) {
            return None;
        }
        match unit.addresses[next.index()].base {
            AddressBase::Address(base) => next = base,
            base => return Some(base),
        }
    }
}

/// Every field value of a construction.
pub(super) fn construct_values<'a, S>(
    cx: &'a Cx<'_, S>,
    construct: ConstructId,
) -> impl Iterator<Item = ValueId> + 'a {
    construct_fields(cx, construct).map(|(v, _)| v)
}

/// The values a construction consumes: every field value but those a flat
/// construction encodes, which it only reads.
pub(super) fn consumed_values<'a, S>(
    cx: &'a Cx<'_, S>,
    construct: ConstructId,
) -> impl Iterator<Item = ValueId> + 'a {
    construct_fields(cx, construct)
        .filter(|&(_, encoded)| !encoded)
        .map(|(v, _)| v)
}

/// The values a flat construction encodes into variable fields: borrowed for
/// the instruction only (LIR schema revision 4).
pub(super) fn encoded_values<'a, S>(
    cx: &'a Cx<'_, S>,
    construct: ConstructId,
) -> impl Iterator<Item = ValueId> + 'a {
    construct_fields(cx, construct)
        .filter(|&(_, encoded)| encoded)
        .map(|(v, _)| v)
}

/// Each field value of a construction and whether the construction encodes
/// it: a value written into a variable field of a flat record.
fn construct_fields<'a, S>(
    cx: &'a Cx<'_, S>,
    construct: ConstructId,
) -> impl Iterator<Item = (ValueId, bool)> + 'a {
    let unit = cx.unit;
    let site = &unit.tables.constructs[construct.index()];
    let record = cx.layouts.get(site.layout);
    site.fields
        .get(&unit.pools.fields)
        .unwrap_or(&[])
        .iter()
        .map(move |f| {
            let encoded = record.is_some_and(|r| {
                r.fields.iter().any(|d| {
                    d.ordinal == f.field
                        && (r.kind == LayoutKind::Flat && cx.layouts.fixed_size(d.layout).is_none()
                            || crate::layout::field_is_borrowed(cx.layouts.0, r, d))
                })
            });
            (f.value, encoded)
        })
}

/// Addresses an instruction accesses, which must be prepared (its own
/// preparation excluded). An origin event (`BindOrigin`, `BindFields`,
/// `EndOrigin`) names places without accessing them, so its addresses need
/// no preparation: a bounds or tag check of a place runs where the place is
/// accessed, never where a name is bound to it.
pub(super) fn inst_addresses<S>(cx: &Cx<'_, S>, inst: &Inst) -> Vec<AddressId> {
    let unit = cx.unit;
    match *inst {
        Inst::Address { address, .. } => match unit.addresses[address.index()].base {
            AddressBase::Address(base) => vec![base],
            _ => Vec::new(),
        },
        Inst::Read { address, .. }
        | Inst::Copy { address, .. }
        | Inst::FlatCopy { address, .. }
        | Inst::FlatView { address, .. }
        | Inst::FlatLength { address, .. }
        | Inst::FlatNext { address, .. }
        | Inst::SeqReserve { address, .. }
        | Inst::ArenaPush { address, .. }
        | Inst::SeqSetLen { address, .. }
        | Inst::SeqAppend { address, .. }
        | Inst::FlatAppend { address, .. }
        | Inst::FlatField { address, .. }
        | Inst::FlatClose { address, .. }
        | Inst::Store { address, .. }
        | Inst::Replace { address, .. }
        | Inst::BeginLoan { address, .. }
        | Inst::Drop { address, .. }
        | Inst::FinalizeBegin { address, .. }
        | Inst::FinalizeEnd { address, .. } => vec![address],
        _ => Vec::new(),
    }
}

pub(super) fn terminator_operands<S>(cx: &Cx<'_, S>, term: &Terminator) -> Vec<(ValueId, Usage)> {
    let unit = cx.unit;
    match *term {
        Terminator::Branch { condition, .. } => vec![(condition, Usage::Operand)],
        Terminator::Switch { value, .. } => vec![(value, Usage::Operand)],
        Terminator::Return(Some(value)) => vec![(value, Usage::Operand)],
        Terminator::Recover { arena, .. } => vec![(arena, Usage::Operand)],
        Terminator::Isolate { start, .. } => vec![(start, Usage::Operand)],
        Terminator::Call { call, .. } => {
            let site = &unit.tables.calls[call.index()];
            let mut out = Vec::new();
            if let Some(v) = site.callee.value() {
                out.push((v, Usage::Operand));
            }
            for arg in site
                .arguments
                .get(&unit.pools.call_arguments)
                .unwrap_or(&[])
            {
                if let ArgumentValue::Value(v) = arg.value {
                    out.push((v, Usage::Operand));
                }
            }
            if let Some(receipts) = &site.receipts {
                out.extend(receipts.inputs().map(|v| (v, Usage::Operand)));
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Before Perceus, only a library cursor may observe a credited source's count.
/// The queried value has exactly one later use: a move (possibly through same-
/// payload recasts) into the cursor whose private flag records this answer.
/// An owned `Shared` root: its count is the number of roots, which only the
/// program's own copies add (MEM-11), so `shared.thaw` may test it before RC.
fn owned_root<S>(cx: &Cx<'_, S>, value: ValueId) -> bool {
    let unit = cx.unit;
    let info = &unit.values[value.index()];
    info.holding == Holding::Owned
        && cx
            .layouts
            .get(unit.types[info.ty.index()].layout)
            .is_some_and(|l| matches!(l.kind, LayoutKind::Shared { .. }))
}

fn early_unique_cursor<S>(cx: &Cx<'_, S>, mut source: ValueId, answer: ValueId) -> bool {
    let unit = cx.unit;
    if early_unique_collection_cursor(cx, source, answer) {
        return true;
    }
    if unit.values[source.index()].holding != Holding::Owned {
        return false;
    }
    let mut seen = std::collections::BTreeSet::new();
    while seen.insert(source) {
        if unit.pools.values.contains(&source) {
            return false;
        }
        if unit.terminators.iter().any(|term| match term {
            Terminator::Return(Some(value)) => *value == source,
            Terminator::Call { call, .. } => {
                let site = &unit.tables.calls[call.index()];
                site.callee.value() == Some(source)
                    || site
                        .arguments
                        .get(&unit.pools.call_arguments)
                        .unwrap_or(&[])
                        .iter()
                        .any(|a| matches!(a.value, ArgumentValue::Value(value) if value == source))
            }
            _ => false,
        }) {
            return false;
        }
        let mut uses = unit.instructions.iter().filter(|inst| {
            !matches!(inst, Inst::IsUnique { result, .. } if *result == answer)
                && inst_operands(cx, inst)
                    .iter()
                    .any(|(value, _)| *value == source)
        });
        let Some(only) = uses.next() else {
            return false;
        };
        if uses.next().is_some() {
            return false;
        }
        match only {
            Inst::Recast { result, value }
                if *value == source && unit.values[result.index()].holding == Holding::Owned =>
            {
                source = *result
            }
            Inst::Construct { construct, .. } => {
                let site = &unit.tables.constructs[construct.index()];
                let ConstructorKind::Record(symbol) = site.constructor else {
                    return false;
                };
                let Some(layout) = cx.layouts.get(site.layout) else {
                    return false;
                };
                if layout.kind != LayoutKind::Resource {
                    return false;
                }
                let Some(finalizer) = &layout.finalizer else {
                    return false;
                };
                let finalizer = &finalizer.definition.0;
                if !finalizer.package.package_is("core", "1")
                    || finalizer.modules != ["$native"]
                    || !matches!(finalizer.name.as_str(), "owned_cursor_drop")
                {
                    return false;
                }
                let path = &unit.tables.symbols[symbol.index()].definition.0;
                let fields = site.fields.get(&unit.pools.fields).unwrap_or(&[]);
                return path.package.package_is("core", "1")
                    && path.modules == ["$intrinsic"]
                    && matches!(
                        (path.name.as_str(), finalizer.name.as_str()),
                        ("OwnedCursor", "owned_cursor_drop")
                    )
                    && fields.iter().filter(|f| f.value == source).count() == 1
                    && fields.iter().any(|f| f.field == 0 && f.value == source)
                    && fields.iter().any(|f| f.field == 2 && f.value == answer);
            }
            _ => return false,
        }
    }
    false
}

/// A collection cursor records root AND entries uniqueness. Shallow detach may
/// leave either block shared; no other use of either answer is admitted.
fn early_unique_collection_cursor<S>(cx: &Cx<'_, S>, queried: ValueId, answer: ValueId) -> bool {
    let unit = cx.unit;
    let identity = |path: &hirsel_syntax::DefPath, module: &str, name: &str| {
        path.package_is("core", "1") && path.0.modules == [module] && path.0.name == name
    };
    let source_path = |mut address: AddressId| {
        let mut fields = 0;
        for _ in 0..unit.addresses.len() {
            let info = &unit.addresses[address.index()];
            for step in info.projection.get(&unit.pools.projections).unwrap_or(&[]) {
                if *step != ProjectionStep::Field(0) {
                    return None;
                }
                fields += 1;
            }
            if fields > 1 {
                return None;
            }
            match info.base {
                AddressBase::Address(parent) => address = parent,
                AddressBase::Slot(slot) => return Some((slot, fields)),
                AddressBase::Value(_) => return None,
            }
        }
        None
    };
    for (construction, inst) in unit.instructions.iter().enumerate() {
        let Inst::Construct { construct, .. } = inst else {
            continue;
        };
        let site = &unit.tables.constructs[construct.index()];
        let ConstructorKind::Record(symbol) = site.constructor else {
            continue;
        };
        let path = &unit.tables.symbols[symbol.index()].definition;
        let (container_owner, finalizer) = if identity(path, "$intrinsic", "OwnedMapCursor") {
            ("Map", "owned_map_cursor_drop")
        } else if identity(path, "$intrinsic", "OwnedSortedMapCursor") {
            ("SortedMap", "owned_sorted_map_cursor_drop")
        } else {
            continue;
        };
        let Some(layout) = cx.layouts.get(site.layout) else {
            continue;
        };
        if layout.kind != LayoutKind::Resource
            || !layout
                .finalizer
                .as_ref()
                .is_some_and(|s| identity(&s.definition, "$native", finalizer))
        {
            continue;
        }
        let fields = site.fields.get(&unit.pools.fields).unwrap_or(&[]);
        let (Some(source), Some(flag)) = (
            fields.iter().find(|f| f.field == 0),
            fields.iter().find(|f| f.field == 2),
        ) else {
            continue;
        };
        let root = source.value;
        if unit.values[root.index()].holding != Holding::Owned
            || fields.iter().filter(|f| f.value == root).count() != 1
        {
            continue;
        }
        let root_layout = unit.types[unit.values[root.index()].ty.index()].layout;
        let Some(payload) = cx
            .layouts
            .payload(root_layout)
            .and_then(|l| cx.layouts.get(l))
        else {
            continue;
        };
        if !matches!(&payload.kind, LayoutKind::Opaque { owner } if owner.package_is("core", "1") && owner.0.modules.is_empty() && owner.0.name == container_owner)
        {
            continue;
        }
        let Some((take, slot, version)) =
            unit.instructions.iter().enumerate().find_map(|(i, inst)| {
                if let Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Take,
                } = inst
                {
                    if *result == root {
                        let (slot, fields) = source_path(*address)?;
                        return (fields == 0).then_some((i, slot, *reaching));
                    }
                }
                None
            })
        else {
            continue;
        };
        let SlotKind::Parameter(parameter) = unit.slots[slot.index()].kind else {
            continue;
        };
        if cx.layouts.0.fn_abis[unit.header.abi.0 as usize].parameters[parameter as usize]
            .convention
            != Passing::Take
        {
            continue;
        }
        let Some((conjunction, left, right)) =
            unit.instructions.iter().enumerate().find_map(|(i, inst)| {
                if let Inst::Binary {
                    result,
                    op: BinaryOp::BitAnd,
                    left,
                    right,
                    ..
                } = inst
                {
                    (*result == flag.value).then_some((i, *left, *right))
                } else {
                    None
                }
            })
        else {
            continue;
        };
        for (root_flag, entries_flag) in [(left, right), (right, left)] {
            let query = |flag| {
                unit.instructions.iter().enumerate().find_map(|(i, inst)| {
                    if let Inst::IsUnique { result, value } = inst {
                        (*result == flag).then_some((i, *value))
                    } else {
                        None
                    }
                })
            };
            let (Some((root_query, view)), Some((entries_query, entries))) =
                (query(root_flag), query(entries_flag))
            else {
                continue;
            };
            if !((queried == view && answer == root_flag)
                || (queried == entries && answer == entries_flag))
            {
                continue;
            }
            let read = |value, fields| {
                unit.instructions.iter().enumerate().find_map(|(i, inst)| {
                    if let Inst::Read {
                        result,
                        address,
                        reaching,
                        mode: ReadMode::Borrow,
                    } = inst
                    {
                        if *result == value
                            && *reaching == version
                            && source_path(*address) == Some((slot, fields))
                        {
                            let Holding::Borrowed(loan) = unit.values[value.index()].holding else {
                                return None;
                            };
                            let loan = &unit.tables.loans[loan.index()];
                            return (loan.address == *address && !loan.mutable).then_some(i);
                        }
                    }
                    None
                })
            };
            let (Some(root_read), Some(entries_read)) = (read(view, 0), read(entries, 1)) else {
                continue;
            };
            let restricted = [root, view, entries, root_flag, entries_flag, flag.value];
            if restricted.iter().any(|v| {
                unit.pools.values.contains(v)
                    || unit
                        .terminators
                        .iter()
                        .any(|t| terminator_operands(cx, t).iter().any(|(used, _)| used == v))
            }) {
                continue;
            }
            let admissible = unit.instructions.iter().enumerate().all(|(i, inst)| {
                inst_operands(cx, inst).iter().all(|(v, _)| {
                    if *v == root || *v == flag.value {
                        i == construction
                    } else if *v == view {
                        i == root_query
                    } else if *v == entries {
                        i == entries_query
                    } else if *v == root_flag || *v == entries_flag {
                        i == conjunction
                    } else {
                        true
                    }
                }) && inst_addresses(cx, inst).iter().all(|address| {
                    if base_root(unit, *address) == Some(AddressBase::Slot(slot)) {
                        if source_path(*address).is_none() {
                            return false;
                        }
                        matches!(
                            inst,
                            Inst::Address { .. } | Inst::BeginLoan { mutable: false, .. }
                        ) || i == root_read
                            || i == entries_read
                            || i == take
                            || matches!(inst, Inst::Drop { .. })
                                && unit.blocks.iter().any(|b| {
                                    b.kind == BlockKind::FailureCleanup
                                        && b.instructions
                                            .get(&unit.instructions)
                                            .unwrap_or(&[])
                                            .iter()
                                            .any(|x| std::ptr::eq(x, inst))
                                })
                    } else {
                        true
                    }
                })
            });
            if admissible {
                return true;
            }
        }
    }
    false
}
