//! Placement: Value LIR to Placed LIR.
//!
//! Each construction's TIR `Destination` is an obligation class: `Heap` and
//! `Arena` are fixed, `Caller` is the hidden ABI destination of the unit's
//! own `FnAbi`, and `Inferred` is placement's own choice. Every call whose
//! callee takes a hidden destination is given one explicitly: the caller's
//! task heap.
//!
//! An `Inferred` block allocation whose value never leaves the frame goes to
//! the unit's internal region (MEM-10): the region is entered at the unit's
//! entry and exited on every return and propagation, and every block it holds
//! is published (immutable) as soon as it is built. "Never leaves" is a flow
//! class over SSA values and the slots holding them, joined through whole-slot
//! stores, reads, copies, replacements and block parameters. A class stays in
//! the region only when every member is defined by a region construction or
//! by that flow and no member is returned, passed to a call, captured,
//! stored into a foreign container, written
//! through or disposed. Region values are uncounted: their slots lose their
//! drops, which the region's exit discharges, and a non-flat copy is a plain
//! read. A flat copy still owns distinct bytes.
//!
//! With regions off, every compiler-local allocation is a counted heap block,
//! except in a `noalloc` function, where region placement is how the promise
//! holds (MEM-5) and stays on.
use crate::insert::{Editor, Facts, Lay, PENDING_FAILURE, synthesize_failure};
use crate::instance::{InstanceId, Instances};
use hirsel_lir::ir::{
    ArenaInstanceId, ArgumentValue, BlockId, BlockKind, ConstructorKind, DestinationClass,
    ExternalInstance, Holding, Inst, LirType, LirTypeId, LirUnit, Placement, ReadMode,
    RegionInstance, RegionInstanceId, ScopeId, ScopeInfo, SlotId, SlotInfo, SlotKind, StorageOwner,
    TaskDomainId, Terminator, ValueId, ValueRepr,
};
use hirsel_lir::layout_types::{CopyRecipe, LayoutKind, LayoutTable, StorageKind};
use hirsel_lir::stage::{Placed, Value};
use hirsel_types::Allocation;
use std::collections::{BTreeMap, BTreeSet};

/// The single executing task's heap domain (M1b is single-task).
pub const TASK_HEAP: StorageOwner = StorageOwner::Heap(TaskDomainId(0));

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Place compiler-local allocations in internal regions. Off, every
    /// compiler-local value is a counted heap block (the regions
    /// measurement's baseline).
    pub regions: bool,
    /// The unit's declared `noalloc` promise: regions stay on, and a
    /// heap allocation placement cannot avoid is an error.
    pub noalloc: bool,
}

impl Options {
    /// The options for one instance of a lowered program.
    pub fn for_instance(regions: bool, instances: &Instances, id: InstanceId) -> Self {
        Self {
            regions,
            noalloc: noalloc(instances, id),
        }
    }
}

/// Whether an instance's callable type declares `noalloc`.
pub fn noalloc(instances: &Instances, id: InstanceId) -> bool {
    instances.callable(id).allocation == Allocation::NoAlloc
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaceError {
    /// A destination class placement cannot honour, named.
    Unsupported(String),
    /// A `noalloc` unit whose allocation can only be a heap block.
    HeapInNoalloc(String),
}

impl std::fmt::Display for PlaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "Unsupported({what:?})"),
            Self::HeapInNoalloc(what) => write!(f, "HeapInNoalloc({what:?})"),
        }
    }
}

/// Give every allocation one storage owner and every call destination a
/// class. The result still has to pass the Placed validator.
pub fn place(
    unit: LirUnit<Value>,
    table: &LayoutTable,
    options: &Options,
) -> Result<LirUnit<Placed>, PlaceError> {
    let mut unit = unit.into_stage::<Placed>();
    let own_destination = table.fn_abis[unit.header.abi.0 as usize].destination;
    unit.tables.task_domains = unit.tables.task_domains.max(1);
    // One arena instance per slot an arena destination names.
    let mut arenas = std::mem::take(&mut unit.tables.arenas);
    let mut fixed = |d: &DestinationClass| -> Result<Placement, PlaceError> {
        Ok(match d {
            DestinationClass::Heap | DestinationClass::Inferred => Placement::Fixed(TASK_HEAP),
            DestinationClass::Caller(_) if own_destination => Placement::CallerDestination,
            DestinationClass::Caller(_) => {
                return Err(PlaceError::Unsupported(
                    "a caller destination in a unit whose ABI has none".into(),
                ));
            }
            DestinationClass::Arena(slot) => {
                let index = match arenas.iter().position(|a| a.slot == *slot) {
                    Some(index) => index,
                    None => {
                        arenas.push(ExternalInstance { slot: *slot });
                        arenas.len() - 1
                    }
                };
                Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(index as u32)))
            }
            DestinationClass::Region => {
                return Err(PlaceError::Unsupported(
                    "a TIR region destination (lowering never emits one)".into(),
                ));
            }
        })
    };
    for site in &mut unit.tables.constructs {
        site.placement = Some(fixed(&site.destination)?);
    }
    for site in &mut unit.tables.closures {
        site.placement = Some(fixed(&site.destination)?);
    }
    unit.tables.arenas = arenas;
    for call in &mut unit.tables.calls {
        if let Some(d @ DestinationClass::Inferred) = &mut call.destination {
            *d = DestinationClass::Heap;
        }
    }
    // Arenas first: a compiler-local region entered after them is outer,
    // so its exits follow theirs.
    unit = open_arenas(unit, table)?;
    if options.regions || options.noalloc {
        unit = localize(unit, table)?;
    }
    if options.noalloc {
        let heap =
            |p: &Option<Placement>| matches!(p, Some(Placement::Fixed(StorageOwner::Heap(_))));
        let blocks = |layout| {
            matches!(
                table.layout(layout).map(|l| &l.kind),
                Some(LayoutKind::Block { .. } | LayoutKind::Box { .. })
            )
        };
        for (i, site) in unit.tables.constructs.iter().enumerate() {
            if heap(&site.placement) && constructs_block(&unit, i) {
                return Err(PlaceError::HeapInNoalloc(format!(
                    "construction {i} of {:?} escapes the frame",
                    unit.header.definition
                )));
            }
        }
        for (i, site) in unit.tables.closures.iter().enumerate() {
            if heap(&site.placement) && blocks(site.environment) {
                return Err(PlaceError::HeapInNoalloc(format!(
                    "closure {i} of {:?} has a heap environment",
                    unit.header.definition
                )));
            }
        }
    }
    crate::arena_credit::elide(&mut unit, table);
    Ok(unit)
}

/// Placement with an explicit identity recipe for observations whose exact
/// coordinates and operands survive. Regional/arena rewrites that move a
/// tracked point refuse until their own relocation transport is implemented.
pub fn place_observed(
    unit: LirUnit<Value>,
    table: &LayoutTable,
    options: &Options,
    input: &hirsel_lir::ownership_trace::Plan,
) -> Result<
    (
        LirUnit<Placed>,
        hirsel_lir::ownership_trace::Plan,
        hirsel_lir::ownership_trace::Recipe,
    ),
    String,
> {
    hirsel_lir::ownership_trace::check(&unit, input).map_err(|error| error.to_string())?;
    let before = unit.clone();
    let after = place(unit, table, options).map_err(|error| error.to_string())?;
    crate::observation::identity(&before, input, after)
}

/// Whether construction `i` builds a counted block (its result is one).
fn constructs_block<S>(unit: &LirUnit<S>, i: usize) -> bool {
    unit.instructions.iter().any(|inst| match *inst {
        Inst::Construct {
            result, construct, ..
        }
        | Inst::UniqueCopy {
            result, construct, ..
        } if construct.index() == i => {
            unit.types[unit.values[result.index()].ty.index()].repr == ValueRepr::Block
        }
        _ => false,
    })
}

// ---------------------------------------------------------------------------
// Region classes

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Node {
    Value(ValueId),
    Slot(SlotId),
}

struct Classes {
    parent: Vec<usize>,
    values: usize,
}

impl Classes {
    fn new(values: usize, slots: usize) -> Self {
        Self {
            parent: (0..values + slots).collect(),
            values,
        }
    }
    fn index(&self, n: Node) -> usize {
        match n {
            Node::Value(v) => v.index(),
            Node::Slot(s) => self.values + s.index(),
        }
    }
    fn find(&mut self, i: usize) -> usize {
        let mut root = i;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut at = i;
        while self.parent[at] != root {
            let next = self.parent[at];
            self.parent[at] = root;
            at = next;
        }
        root
    }
    fn union(&mut self, a: Node, b: Node) {
        let (a, b) = (self.index(a), self.index(b));
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[a] = b;
        }
    }
    fn of(&mut self, n: Node) -> usize {
        let i = self.index(n);
        self.find(i)
    }
}

/// Instructions whose operands are only observed during this instruction:
/// they return fresh scalar bits or check an address, never a retained view.
/// Reads through the checked address join their non-plain result to the root
/// below; writes and mutable loans escape the root in the written-address scan.
/// These operations accept uncounted payloads. Header queries (IsUnique and
/// IsWritable), codecs producing owned results, and unknown calls do not qualify.
fn observes_payload(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::Address { .. }
            | Inst::SeqLen { .. }
            | Inst::SeqCapacity { .. }
            | Inst::Compare { .. }
            | Inst::Hash { .. }
    )
}

/// Move every frame-local block allocation into the unit's region.
fn localize(unit: LirUnit<Placed>, table: &LayoutTable) -> Result<LirUnit<Placed>, PlaceError> {
    let lay = Lay(table);
    let facts = Facts::new(&unit, table);
    let n_values = unit.values.len();
    let mut classes = Classes::new(n_values, unit.slots.len());
    let carries = |ty: LirTypeId| {
        let ty = &unit.types[ty.index()];
        ty.repr == ValueRepr::Block || ty.repr == ValueRepr::Aggregate && !lay.trivial(ty.layout)
    };

    // Members defined some other way than a region construction or the
    // class's own flow, and members that leave the frame.
    let mut foreign: BTreeSet<Node> = BTreeSet::new();
    let mut escaped: BTreeSet<Node> = BTreeSet::new();
    let mut sources: BTreeSet<ValueId> = BTreeSet::new();
    let whole = |a: hirsel_lir::ir::AddressId| {
        facts
            .slot_of(a)
            .filter(|(_, p)| p.is_empty())
            .map(|(s, _)| s)
    };
    let root_of = |a: hirsel_lir::ir::AddressId| -> Option<Node> {
        let p = facts.places.get(a.index())?.as_ref()?;
        Some(match p.root {
            crate::insert::Root::Slot(s) => Node::Slot(s),
            crate::insert::Root::Value(v) => Node::Value(v),
        })
    };
    for (s, slot) in unit.slots.iter().enumerate() {
        if matches!(
            slot.kind,
            SlotKind::Parameter(_) | SlotKind::Capture(_) | SlotKind::ReturnSlot
        ) {
            foreign.insert(Node::Slot(SlotId(s as u32)));
        }
    }
    for &v in unit.parameters.iter().chain(&unit.captures) {
        foreign.insert(Node::Value(v));
    }
    // A published region block is read through projections but never written
    // or moved out of: a projected address any operation writes, takes an
    // owned part through or lends mutably escapes its root.
    let projected = |a: hirsel_lir::ir::AddressId| {
        facts.places[a.index()]
            .as_ref()
            .is_some_and(|p| !p.path.is_empty() || matches!(p.root, crate::insert::Root::Value(_)))
    };
    let mut written: BTreeSet<hirsel_lir::ir::AddressId> = BTreeSet::new();
    for inst in &unit.instructions {
        match *inst {
            Inst::Store { address, .. }
            | Inst::Replace { address, .. }
            | Inst::Drop { address, .. }
            | Inst::FinalizeBegin { address, .. }
            | Inst::FinalizeEnd { address, .. }
            | Inst::BeginLoan {
                address,
                mutable: true,
                ..
            } => {
                written.insert(address);
            }
            _ => {}
        }
    }
    for term in &unit.terminators {
        if let Terminator::Call { call, .. } = *term {
            for a in crate::insert::call_arguments(&unit, call) {
                if let ArgumentValue::Address { address, .. } = a.value {
                    written.insert(address);
                }
            }
        }
    }
    for &a in &written {
        if projected(a)
            && let Some(root) = root_of(a)
        {
            escaped.insert(root);
        }
    }
    for b in 0..unit.blocks.len() {
        let block = &unit.blocks[b];
        for inst in block.instructions.get(&unit.instructions).unwrap_or(&[]) {
            let mut define_foreign = |v: ValueId| {
                foreign.insert(Node::Value(v));
            };
            match *inst {
                Inst::Construct {
                    result, construct, ..
                } => {
                    let site = &unit.tables.constructs[construct.index()];
                    let inline = unit.types[unit.values[result.index()].ty.index()].repr
                        == ValueRepr::Aggregate
                        && !lay.anchored(site.layout);
                    if region_candidate(&unit, table, site, result) {
                        sources.insert(result);
                    } else if inline
                        && site
                            .fields
                            .get(&unit.pools.fields)
                            .unwrap_or(&[])
                            .iter()
                            .any(|f| carries(unit.values[f.value.index()].ty))
                    {
                        // An inline container derives regional provenance from
                        // its children. A fieldless variant (for example None
                        // alongside Some(List)) has no such source: keep its
                        // whole flow class counted, even when only observed.
                    } else {
                        define_foreign(result);
                    }
                    let encoded = site.constructor != ConstructorKind::Box
                        && table
                            .layout(site.layout)
                            .is_some_and(|l| l.kind == LayoutKind::Flat);
                    for f in site.fields.get(&unit.pools.fields).unwrap_or(&[]) {
                        if encoded {
                            escaped.insert(Node::Value(f.value));
                        } else if carries(unit.values[f.value.index()].ty) {
                            classes.union(Node::Value(result), Node::Value(f.value));
                        }
                    }
                }
                // An element built in place copies its fields' bytes; its
                // sources are treated as escaping, conservatively.
                Inst::FlatAppend { construct, .. } | Inst::FlatClose { construct, .. } => {
                    let site = &unit.tables.constructs[construct.index()];
                    for f in site.fields.get(&unit.pools.fields).unwrap_or(&[]) {
                        escaped.insert(Node::Value(f.value));
                    }
                }
                Inst::Store {
                    address,
                    value,
                    version,
                } => match whole(address) {
                    Some(s) => {
                        classes.union(Node::Value(value), Node::Slot(s));
                        classes.union(Node::Value(version), Node::Slot(s));
                    }
                    None => {
                        escaped.insert(Node::Value(value));
                    }
                },
                Inst::Replace {
                    address,
                    value,
                    displaced,
                    version,
                    displaced_version,
                    ..
                } => match whole(address) {
                    Some(s) => {
                        classes.union(Node::Value(value), Node::Slot(s));
                        classes.union(Node::Value(version), Node::Slot(s));
                        classes.union(Node::Value(displaced_version), Node::Slot(displaced));
                        classes.union(Node::Slot(displaced), Node::Slot(s));
                    }
                    None => {
                        escaped.insert(Node::Value(value));
                        foreign.insert(Node::Slot(displaced));
                    }
                },
                // A flat copy owns distinct bytes even when the source lives
                // in a region (MEM-12). Do not join its result to the source:
                // rewrite would otherwise replace the copy with a borrowed read.
                Inst::Copy { result, .. }
                    if table
                        .layout(unit.types[unit.values[result.index()].ty.index()].layout)
                        .is_some_and(|r| r.copy == CopyRecipe::FlatBytes) =>
                {
                    define_foreign(result);
                }
                Inst::Read {
                    result,
                    address,
                    reaching,
                    ..
                }
                | Inst::Copy {
                    result,
                    address,
                    reaching,
                    ..
                } => match whole(address) {
                    Some(s) => {
                        classes.union(Node::Value(result), Node::Slot(s));
                        classes.union(Node::Value(reaching), Node::Slot(s));
                    }
                    None if carries(unit.values[result.index()].ty) => {
                        if let Some(root) = root_of(address) {
                            classes.union(Node::Value(result), root);
                        } else {
                            define_foreign(result);
                        }
                    }
                    None => define_foreign(result),
                },
                Inst::Closure {
                    result, closure, ..
                } => {
                    define_foreign(result);
                    for c in unit.tables.closures[closure.index()]
                        .captures
                        .get(&unit.pools.captures)
                        .unwrap_or(&[])
                    {
                        escaped.insert(Node::Value(c.value));
                    }
                }
                Inst::UniqueCopy { result, value, .. } => {
                    define_foreign(result);
                    escaped.insert(Node::Value(value));
                }
                Inst::Reuse { result, token, .. } => {
                    define_foreign(result);
                    escaped.insert(Node::Value(token));
                }
                _ => {
                    for v in Inst::defs(inst) {
                        define_foreign(v);
                    }
                    for v in crate::liveness::inst_uses(&unit, inst) {
                        if !observes_payload(inst)
                            && !matches!(
                                inst,
                                Inst::Drop { .. } | Inst::RegionPublish { .. } | Inst::Share { .. }
                            )
                        {
                            escaped.insert(Node::Value(v));
                        }
                    }
                }
            }
            if let Some(f) = Inst::failure(inst) {
                for &d in unit.tables.failures[f.index()]
                    .dispose
                    .get(&unit.pools.values)
                    .unwrap_or(&[])
                {
                    escaped.insert(Node::Value(d));
                }
            }
        }
        // Values pending on a failure edge flow into its cleanup's
        // parameters, as along normal edges.
        let failures = block
            .instructions
            .get(&unit.instructions)
            .unwrap_or(&[])
            .iter()
            .filter_map(Inst::failure)
            .chain(unit.terminators[b].failure_edges(&unit.tables));
        for f in failures {
            let edge = &unit.tables.edges[unit.tables.failures[f.index()].cleanup.index()];
            let args = edge.arguments.get(&unit.pools.values).unwrap_or(&[]);
            let params = unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[]);
            for (&a, &p) in args.iter().zip(params) {
                classes.union(Node::Value(a), Node::Value(p));
            }
        }
        let term = &unit.terminators[b];
        match *term {
            Terminator::Call { call, failure, .. } => {
                let site = &unit.tables.calls[call.index()];
                for a in crate::insert::call_arguments(&unit, call) {
                    match a.value {
                        ArgumentValue::Value(v) => {
                            escaped.insert(Node::Value(v));
                        }
                        ArgumentValue::Address { address, version } => {
                            if let Some(root) = root_of(address) {
                                escaped.insert(root);
                            }
                            foreign.insert(Node::Value(version));
                        }
                    }
                }
                if let Some(v) = site.callee.value() {
                    escaped.insert(Node::Value(v));
                }
                if let Some(r) = site.result {
                    foreign.insert(Node::Value(r));
                }
                for &d in unit.tables.failures[failure.index()]
                    .dispose
                    .get(&unit.pools.values)
                    .unwrap_or(&[])
                {
                    escaped.insert(Node::Value(d));
                }
            }
            Terminator::Return(Some(v)) => {
                escaped.insert(Node::Value(v));
            }
            _ => {
                for v in crate::liveness::terminator_uses(&unit, term) {
                    escaped.insert(Node::Value(v));
                }
            }
        }
        for e in Terminator::normal_edges(term, &unit.pools) {
            let edge = &unit.tables.edges[e.index()];
            let args = edge.arguments.get(&unit.pools.values).unwrap_or(&[]);
            let params = unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[]);
            for (&a, &p) in args.iter().zip(params) {
                classes.union(Node::Value(a), Node::Value(p));
            }
        }
    }
    // A class is local when it holds a region construction, only block
    // references, nothing foreign and nothing escaping.
    let mut bad: BTreeSet<usize> = BTreeSet::new();
    let mut has_source: BTreeSet<usize> = BTreeSet::new();
    for n in foreign.iter().chain(&escaped) {
        let c = classes.of(*n);
        bad.insert(c);
    }
    for v in 0..n_values {
        let node = Node::Value(ValueId(v as u32));
        if !carries(unit.values[v].ty) {
            let c = classes.of(node);
            bad.insert(c);
        }
    }
    for s in 0..unit.slots.len() {
        if !carries(unit.slots[s].ty) {
            let c = classes.of(Node::Slot(SlotId(s as u32)));
            bad.insert(c);
        }
    }
    for &v in &sources {
        has_source.insert(classes.of(Node::Value(v)));
    }
    let local: BTreeSet<usize> = has_source.difference(&bad).copied().collect();
    if local.is_empty() {
        return Ok(unit);
    }
    let in_class = |classes: &mut Classes, n: Node| local.contains(&classes.of(n));
    let local_values: Vec<bool> = (0..n_values)
        .map(|v| in_class(&mut classes, Node::Value(ValueId(v as u32))))
        .collect();
    let local_slots: Vec<bool> = (0..unit.slots.len())
        .map(|s| in_class(&mut classes, Node::Slot(SlotId(s as u32))))
        .collect();
    rewrite(unit, table, lay, &local_values, &local_slots)
}

/// A construction placement may move into the region: an inferred block of
/// a payload that is not anchored (no user-visible destruction and no `Shared`
/// root: a region frees at its own end, not at the value's cleanup anchor) and
/// that the layout lets a region hold.
fn region_candidate(
    unit: &LirUnit<Placed>,
    table: &LayoutTable,
    site: &hirsel_lir::ir::ConstructSite,
    result: ValueId,
) -> bool {
    let lay = Lay(table);
    let ty = &unit.types[unit.values[result.index()].ty.index()];
    site.destination == DestinationClass::Inferred
        && ty.repr == ValueRepr::Block
        && matches!(
            site.constructor,
            ConstructorKind::Box
                | ConstructorKind::Sequence
                | ConstructorKind::Record(_)
                | ConstructorKind::Tuple
                | ConstructorKind::Variant { .. }
        )
        && !lay.anchored(site.layout)
        && table
            .layout(site.layout)
            .is_some_and(|l| l.storage.contains(&StorageKind::LocalRegion))
}

fn rewrite(
    unit: LirUnit<Placed>,
    table: &LayoutTable,
    lay: Lay<'_>,
    local_values: &[bool],
    local_slots: &[bool],
) -> Result<LirUnit<Placed>, PlaceError> {
    let mut unit = unit;
    // Uncounted twins of the block types the region holds.
    let mut twins: BTreeMap<LirTypeId, LirTypeId> = BTreeMap::new();
    let mut twin = |unit: &mut LirUnit<Placed>, ty: LirTypeId| -> LirTypeId {
        *twins.entry(ty).or_insert_with(|| {
            let layout = unit.types[ty.index()].layout;
            let wanted = LirType {
                repr: ValueRepr::Uncounted,
                layout,
            };
            match unit.types.iter().position(|t| *t == wanted) {
                Some(i) => LirTypeId(i as u32),
                None => {
                    unit.types.push(wanted);
                    LirTypeId(unit.types.len() as u32 - 1)
                }
            }
        })
    };
    for v in 0..unit.values.len() {
        if local_values[v] {
            let old = unit.values[v].ty;
            let ty = twin(&mut unit, old);
            unit.values[v].ty = ty;
            unit.values[v].holding = Holding::Plain;
        }
    }
    for s in 0..unit.slots.len() {
        if local_slots[s] {
            let old = unit.slots[s].ty;
            let ty = twin(&mut unit, old);
            unit.slots[s].ty = ty;
        }
    }
    let facts = Facts::new(&unit, table);
    for a in 0..unit.addresses.len() {
        let local_root = facts.places[a].as_ref().is_some_and(|p| match p.root {
            crate::insert::Root::Slot(s) => local_slots[s.index()],
            crate::insert::Root::Value(v) => local_values[v.index()],
        });
        let ty = &unit.types[unit.addresses[a].ty.index()];
        if local_root
            && (ty.repr == ValueRepr::Block
                || ty.repr == ValueRepr::Aggregate && !lay.trivial(ty.layout))
        {
            let old = unit.addresses[a].ty;
            let ty = twin(&mut unit, old);
            unit.addresses[a].ty = ty;
        }
    }
    let mark_ty = mark_type(&mut unit, table)?;
    let region = unit_region(&mut unit);

    let mut facts = Facts::new(&unit, table);
    let mut ed = Editor::new(unit);
    let mark = ed.value(mark_ty, Holding::Owned);
    facts.marks[region.index()] = Some(mark);
    let whole_local = |facts: &Facts, a| {
        facts
            .slot_of(a)
            .is_some_and(|(s, p)| p.is_empty() && local_slots[s.index()])
    };
    for b in 0..ed.code.len() {
        let mut out = Vec::with_capacity(ed.code[b].len());
        for inst in std::mem::take(&mut ed.code[b]) {
            match inst {
                Inst::Construct {
                    result, construct, ..
                } if local_values[result.index()] => {
                    if lay
                        .payload(ed.unit.types[ed.unit.values[result.index()].ty.index()].layout)
                        .is_some()
                    {
                        ed.unit.tables.constructs[construct.index()].arena_no_drop = true;
                        ed.unit.tables.constructs[construct.index()].placement =
                            Some(Placement::Fixed(StorageOwner::LocalRegion(region)));
                        out.push(inst);
                        out.push(Inst::RegionPublish { value: result });
                    } else {
                        out.push(inst);
                    }
                }
                // Region contents end with the region.
                Inst::Drop { address, .. } if whole_local(&facts, address) => {}
                // A region block is never an arena List's (MEM-6).
                Inst::Share { value } if local_values[value.index()] => {}
                // A copy of an uncounted reference is a plain read.
                Inst::Copy {
                    result,
                    address,
                    reaching,
                    ..
                } if local_values[result.index()] => out.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Borrow,
                }),
                Inst::Read {
                    result,
                    address,
                    reaching,
                    ..
                } if local_values[result.index()] => out.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Borrow,
                }),
                other => out.push(other),
            }
        }
        ed.code[b] = out;
    }
    let entry = ed.unit.entry.index();
    ed.code[entry].insert(
        0,
        Inst::RegionEnter {
            result: mark,
            region,
        },
    );
    exit_everywhere(&mut ed, lay, &mut facts, region, mark)?;
    Ok(ed.finish())
}

/// The type of a region mark.
fn mark_type(unit: &mut LirUnit<Placed>, table: &LayoutTable) -> Result<LirTypeId, PlaceError> {
    let scalar = table
        .layouts
        .iter()
        .position(|l| matches!(l.kind, LayoutKind::Scalar(_)))
        .ok_or_else(|| PlaceError::Unsupported("no scalar layout for a region mark".into()))?;
    unit.types.push(LirType {
        repr: ValueRepr::RegionMark,
        layout: hirsel_lir::layout_types::LayoutId(scalar as u32),
    });
    Ok(LirTypeId(unit.types.len() as u32 - 1))
}

/// A new region instance spanning the whole unit.
fn unit_region(unit: &mut LirUnit<Placed>) -> RegionInstanceId {
    let root_scope = (0..unit.scopes.len())
        .find(|&s| unit.scopes[s].parent.is_none())
        .map_or(ScopeId(0), |s| ScopeId(s as u32));
    let region = RegionInstanceId(unit.tables.regions.len() as u32);
    let entry_anchor = unit.blocks[unit.entry.index()].anchor;
    unit.tables.regions.push(RegionInstance {
        scope: root_scope,
        anchor: entry_anchor,
        seal_demand: false,
    });
    region
}

/// Lower each `ArenaOpen` (only in an entry block, by the validator) to a
/// region spanning the unit: its entry at the unit's start, its token in
/// place of the opening (after the budget it reads), and its exit on every
/// return and propagation.
fn open_arenas(unit: LirUnit<Placed>, table: &LayoutTable) -> Result<LirUnit<Placed>, PlaceError> {
    let mut unit = unit;
    let entry = unit.entry.index();
    let openings: Vec<(usize, ValueId, Option<ValueId>)> = unit
        .blocks
        .iter()
        .enumerate()
        .flat_map(|(b, block)| {
            let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
            insts.iter().filter_map(move |inst| match *inst {
                Inst::ArenaOpen { result, budget } => Some((b, result, budget)),
                _ => None,
            })
        })
        .collect();
    if openings.is_empty() {
        return Ok(unit);
    }
    if openings.iter().any(|&(b, _, _)| b != entry) {
        return Err(PlaceError::Unsupported(
            "an arena opened outside the entry block".into(),
        ));
    }
    let lay = Lay(table);
    let mark_ty = mark_type(&mut unit, table)?;
    let regions: Vec<RegionInstanceId> = openings.iter().map(|_| unit_region(&mut unit)).collect();
    let mut facts = Facts::new(&unit, table);
    let mut ed = Editor::new(unit);
    let mut marks = Vec::new();
    for (&(_, token, budget), &region) in openings.iter().zip(&regions) {
        let mark = ed.value(mark_ty, Holding::Owned);
        facts.marks[region.index()] = Some(mark);
        let at = ed.code[entry]
            .iter()
            .position(|inst| matches!(*inst, Inst::ArenaOpen { result, .. } if result == token))
            .expect("the opening is in the entry block");
        ed.code[entry][at] = Inst::ArenaToken {
            result: token,
            mark,
            budget,
        };
        let enter = marks.len();
        ed.code[entry].insert(
            enter,
            Inst::RegionEnter {
                result: mark,
                region,
            },
        );
        marks.push(mark);
    }
    // Innermost first: the last opened closes first.
    for (&region, &mark) in regions.iter().zip(&marks).rev() {
        exit_everywhere(&mut ed, lay, &mut facts, region, mark)?;
    }
    Ok(ed.finish())
}

/// Exit the region after return preparation on each returning path. An owned
/// result stays in storage until the exit succeeds, so an exit failure can
/// drop it without moving the exit ahead of another failure-capable operation.
fn exit_everywhere(
    ed: &mut Editor<Placed>,
    lay: Lay<'_>,
    facts: &mut Facts,
    region: RegionInstanceId,
    mark: ValueId,
) -> Result<(), PlaceError> {
    let n = ed.unit.blocks.len();
    // Propagations: the exit's own failure propagates too.
    for b in 0..n {
        if ed.unit.terminators[b] == Terminator::Propagate {
            let anchor = ed.unit.blocks[b].anchor;
            let tail = ed.block(BlockKind::FailureCleanup, &[], anchor);
            ed.unit.terminators[tail.index()] = Terminator::Propagate;
            let e = ed.edge(tail, &[]);
            let failure = ed.failure(e, anchor);
            ed.code[b].push(Inst::RegionExit {
                mark,
                region,
                failure,
            });
        }
    }
    // A result already in whole-slot storage needs only its final, infallible
    // take after the exit. A pending owned result gets equivalent staging in
    // an outer scope, released after frame cleanup if the region exit fails.
    let mut points: Vec<(BlockId, usize)> = Vec::new();
    let mut result_scope = None;
    for b in 0..n {
        let Terminator::Return(result) = ed.unit.terminators[b] else {
            continue;
        };
        let block = BlockId(b as u32);
        let mut at = ed.code[b].len();
        if let Some(value) = result {
            let stored = ed.code[b].last().is_some_and(|inst| {
                matches!(*inst, Inst::Read { result, address, mode: ReadMode::Take, .. }
                    if result == value && facts.slot_of(address).is_some_and(|(_, path)| path.is_empty()))
            });
            if stored {
                at -= 1;
            } else if crate::insert::owns(&ed.unit, lay, value) {
                let scope = *result_scope.get_or_insert_with(|| {
                    let scope = ScopeId(ed.unit.scopes.len() as u32);
                    for info in &mut ed.unit.scopes {
                        if info.parent.is_none() {
                            info.parent = Some(scope);
                        }
                    }
                    ed.unit.scopes.push(ScopeInfo { parent: None });
                    scope
                });
                let ty = ed.unit.values[value.index()].ty;
                let slot = SlotId(ed.unit.slots.len() as u32);
                ed.unit.slots.push(SlotInfo {
                    ty,
                    kind: SlotKind::Temporary,
                    scope,
                });
                let preparations = ed.code[ed.unit.entry.index()].len();
                let address = ed.whole(facts, slot);
                let added = ed.code[ed.unit.entry.index()].len() - preparations;
                for (block, point) in &mut points {
                    if *block == ed.unit.entry {
                        *point += added;
                    }
                }
                // `whole` may add an entry preparation, including in this block.
                at = ed.code[b].len();
                let version = ed.value(ty, Holding::Plain);
                let returned = ed.value(ty, ed.unit.values[value.index()].holding);
                facts.version_of.resize(ed.unit.values.len(), None);
                facts.backing.resize(ed.unit.values.len(), None);
                facts.statics.resize(ed.unit.values.len(), false);
                facts.version_of[version.index()] = Some(slot);
                ed.code[b].extend([
                    Inst::StorageLive { slot },
                    Inst::Store {
                        address,
                        value,
                        version,
                    },
                    Inst::Read {
                        result: returned,
                        address,
                        reaching: version,
                        mode: ReadMode::Take,
                    },
                ]);
                at += 2;
                ed.unit.terminators[b] = Terminator::Return(Some(returned));
            }
        }
        points.push((block, at));
    }
    // Each exit's failure cleans what remains owned, including staged results.
    for &(b, i) in &points {
        ed.code[b.index()].insert(
            i,
            Inst::RegionExit {
                mark,
                region,
                failure: PENDING_FAILURE,
            },
        );
    }
    let states = {
        let flow = crate::insert::Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts,
        };
        let states = flow.solve();
        let mut after = Vec::new();
        for &(b, i) in &points {
            let Some(state) = states[b.index()].clone() else {
                continue;
            };
            let mut post = None;
            flow.block_after(b, state, |j, s| {
                if j == i {
                    post = Some(s.clone());
                }
            });
            after.push(((b, i), post));
        }
        after
    };
    for ((b, i), post) in states {
        let Some(post) = post else {
            continue;
        };
        let at = ed.unit.blocks[b.index()].anchor;
        let failure = synthesize_failure(ed, lay, facts, &post, at, None).ok_or_else(|| {
            PlaceError::Unsupported("a region exit whose failure cannot be cleaned".into())
        })?;
        if let Inst::RegionExit { failure: f, .. } = &mut ed.code[b.index()][i] {
            *f = failure;
        }
    }
    Ok(())
}
