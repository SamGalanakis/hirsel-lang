//! Drop specialization and reuse: Rc LIR to Reuse LIR.
//!
//! Three rewrites, in order, each a discover-then-commit pass over the unit:
//!
//! - `specialize_drop`: a block whose counted children are copied right
//!   before its release tests its count; when unique the children are moved
//!   out (no `Dup`) and the block released under a mask of what stays, when
//!   shared the copies and the plain release stay.
//! - this module, token pairing: a `Release` of an owned counted heap block
//!   becomes `Reset` when a construction of the same exact `(usable bytes,
//!   alignment, task heap)` bucket may follow on a path without a feedback
//!   edge, and that construction becomes a `Reuse` of the token. Tokens are
//!   linear: each split edge carries its own continuation, a join receives
//!   one through a token block parameter (explicit `TokenNone` on edges that
//!   have none), every failure edge disposes of the tokens live there, and
//!   tokens no later construction can use are disposed of where that becomes
//!   known, at the latest before an exit or a feedback edge of the loop
//!   nesting forest (irreducible cycles included).
//! - `reuse_stores`: a reuse whose field operand was taken out of the same
//!   field of its donor skips that store (`Reuse::kept`).
//!
//! Donors are only owned counted heap blocks of a known task heap: never a
//! static, region, arena, frozen, caller-destination or borrowed value, and
//! never a block the specialized drop just found shared. A release keeps its
//! place, so every operand that reads the donor was evaluated before the
//! donor's storage is given up. `Options::reuse = false` is the reuse-off
//! switch: the Rc unit, retagged, with no token at all.
//!
//! The stage validator replays tokens, masks and kept fields independently.
use crate::insert::Lay;
use crate::insert::{Editor, Facts, owns};
use hirsel_lir::ir::{
    BlockId, BlockKind, EdgeId, FailureId, Holding, Inst, LirTypeId, LirUnit, Placement, PoolRange,
    StorageOwner, TaskDomainId, Terminator, ValueId, ValueRepr,
};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, LayoutTable};
use hirsel_lir::runtime_abi::{SizeClass, size_class};
use hirsel_lir::stage::{Rc, Reuse, Stage};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReuseError {
    /// LIR that contradicts the stage invariants it was validated against.
    Malformed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Options {
    /// Off: the unit is retagged unchanged (an explicit empty reuse plan),
    /// the baseline every reuse-on run must observe identically.
    pub reuse: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { reuse: true }
    }
}

/// Lower an Rc unit to Reuse. The result still has to pass the Reuse
/// validator.
pub fn reuse(
    unit: LirUnit<Rc>,
    table: &LayoutTable,
    options: &Options,
) -> Result<LirUnit<Reuse>, ReuseError> {
    let unit = unit.into_stage::<Reuse>();
    if !options.reuse {
        return Ok(unit);
    }
    let unit = crate::specialize_drop::specialize(unit, table)?;
    let unit = pair_tokens(unit, table)?;
    Ok(crate::reuse_stores::keep(unit, table))
}

/// A token bucket: exact usable bytes, alignment and task heap.
pub(crate) type Bucket = (u64, u32, TaskDomainId);

/// The bucket of a fixed-size payload in `domain`: its size class's
/// capacity, or the request itself on the large path.
pub(crate) fn bucket(
    table: &LayoutTable,
    payload: LayoutId,
    domain: TaskDomainId,
) -> Option<Bucket> {
    let record = table.layout(payload)?;
    if record.kind == LayoutKind::Flat {
        // A flat payload's extent is its encoding's, not the layout's.
        return None;
    }
    let hirsel_lir::layout_types::SizeSpec::Fixed(size) = record.size else {
        return None;
    };
    let capacity = match size_class(size, u64::from(record.align)) {
        SizeClass::Small { capacity, .. } | SizeClass::Large { capacity } => capacity,
    };
    Some((capacity, record.align.max(8), domain))
}

// ---------------------------------------------------------------------------
// Control flow

/// Normal and failure successors, and the loop nesting forest's feedback
/// edges: in each cyclic strongly connected component every edge into one of
/// its entries is a feedback edge, and the rest is decomposed again, so every
/// cycle (reducible or not) contains one whatever the search order.
pub(crate) struct Graph {
    /// Per block, `(edge, target)` of its normal edges.
    pub normal: Vec<Vec<(EdgeId, BlockId)>>,
    /// Per block, every successor (normal and failure cleanup).
    pub all: Vec<Vec<(EdgeId, BlockId)>>,
    /// Per block, incoming normal edges with their source.
    pub preds: Vec<Vec<(BlockId, EdgeId)>>,
    pub feedback: BTreeSet<EdgeId>,
    pub entry: BlockId,
    reach: Vec<bool>,
}

impl Graph {
    pub fn new<S: Stage>(ed: &Editor<S>) -> Self {
        let unit = &ed.unit;
        let n = unit.blocks.len();
        let mut normal = vec![Vec::new(); n];
        let mut all = vec![Vec::new(); n];
        let mut preds = vec![Vec::new(); n];
        for b in 0..n {
            for f in ed.code[b].iter().filter_map(Inst::failure) {
                let e = unit.tables.failures[f.index()].cleanup;
                all[b].push((e, unit.tables.edges[e.index()].target));
            }
            let term = &unit.terminators[b];
            for e in Terminator::normal_edges(term, &unit.pools) {
                let t = unit.tables.edges[e.index()].target;
                normal[b].push((e, t));
                all[b].push((e, t));
                preds[t.index()].push((BlockId(b as u32), e));
            }
            if let Some(f) = Terminator::failure(term) {
                let e = unit.tables.failures[f.index()].cleanup;
                all[b].push((e, unit.tables.edges[e.index()].target));
            }
        }
        let mut adjacent: Vec<Vec<usize>> = all
            .iter()
            .map(|ts| ts.iter().map(|&(_, b)| b.index()).collect())
            .collect();
        let reach = hirsel_graph::reachable(&adjacent, unit.entry.index());
        // Reuse's forest considers only predecessors that can execute.
        for (b, targets) in adjacent.iter_mut().enumerate() {
            if !reach[b] {
                targets.clear();
            }
        }
        let feedback_pairs = hirsel_graph::feedback_edges(&adjacent, unit.entry.index());
        let pairs = &feedback_pairs;
        let feedback = all
            .iter()
            .enumerate()
            .flat_map(|(b, targets)| {
                targets.iter().filter_map(move |&(edge, target)| {
                    pairs.contains(&(b, target.index())).then_some(edge)
                })
            })
            .collect();
        Self {
            normal,
            all,
            preds,
            feedback,
            reach,
            entry: unit.entry,
        }
    }

    /// Blocks reachable from the entry along normal edges that are not
    /// feedback edges, sources before targets (the graph is acyclic).
    pub fn forward_order(&self) -> Vec<BlockId> {
        let n = self.normal.len();
        let mut indegree = vec![0usize; n];
        let reach = &self.reach;
        for b in (0..n).filter(|&b| reach[b]) {
            for &(e, t) in &self.normal[b] {
                if !self.feedback.contains(&e) {
                    indegree[t.index()] += 1;
                }
            }
        }
        let mut ready: Vec<BlockId> = (0..n)
            .filter(|&b| reach[b] && indegree[b] == 0)
            .map(|b| BlockId(b as u32))
            .collect();
        ready.reverse();
        let mut out = Vec::new();
        while let Some(b) = ready.pop() {
            out.push(b);
            for &(e, t) in self.normal[b.index()].iter().rev() {
                if self.feedback.contains(&e) {
                    continue;
                }
                indegree[t.index()] -= 1;
                if indegree[t.index()] == 0 {
                    ready.push(t);
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Donors and constructions

/// Which values are owned counted blocks of a known task heap: everything a
/// counted block can be except a construction or closure placed elsewhere,
/// a static, and block parameters joining any of those.
fn heap_values<S: Stage>(ed: &Editor<S>, facts: &Facts) -> Vec<bool> {
    let unit = &ed.unit;
    let n = unit.values.len();
    let mut ok: Vec<Option<bool>> = vec![None; n];
    if unit.tables.task_domains == 0 {
        return vec![false; n];
    }
    let fixed = |p: Option<Placement>| {
        matches!(
            p,
            Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))))
        )
    };
    for code in &ed.code {
        for inst in code {
            match *inst {
                Inst::Construct {
                    result, construct, ..
                }
                | Inst::Reuse {
                    result, construct, ..
                }
                | Inst::UniqueCopy {
                    result, construct, ..
                } => {
                    ok[result.index()] =
                        Some(fixed(unit.tables.constructs[construct.index()].placement))
                }
                Inst::Closure {
                    result, closure, ..
                } => {
                    ok[result.index()] =
                        Some(fixed(unit.tables.closures[closure.index()].placement))
                }
                Inst::Const { result, .. }
                | Inst::ReadStatic { result, .. }
                | Inst::FunctionRef { result, .. } => ok[result.index()] = Some(false),
                _ => {}
            }
        }
    }
    for (v, s) in facts.statics.iter().enumerate() {
        if *s {
            ok[v] = Some(false);
        }
    }
    // Block parameters: known only if every argument is.
    let mut params: Vec<(ValueId, BlockId, usize)> = Vec::new();
    for (b, block) in unit.blocks.iter().enumerate() {
        for (i, &p) in block
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            params.push((p, BlockId(b as u32), i));
            ok[p.index()] = None;
        }
    }
    let incoming = |b: BlockId, i: usize| -> Vec<ValueId> {
        let mut out = Vec::new();
        for edge in &unit.tables.edges {
            if edge.target == b
                && let Some(&a) = edge
                    .arguments
                    .get(&unit.pools.values)
                    .and_then(|a| a.get(i))
            {
                out.push(a);
            }
        }
        out
    };
    let mut answer: Vec<bool> = ok.iter().map(|o| o.unwrap_or(true)).collect();
    loop {
        let mut changed = false;
        for &(p, b, i) in &params {
            let v = incoming(b, i).iter().all(|a| answer[a.index()]);
            if answer[p.index()] && !v {
                answer[p.index()] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    answer
}

/// Whole-slot drops of a closure that can donate its environment block, by
/// `(block, instruction)`: the slot's one reaching version, the layout of the
/// environment the closure was built with (its static callable type may be
/// erased) and the callable's type. The slot's content is definitely whole,
/// came from an owned heap closure, and its environment is fixed-size
/// without user-visible destruction.
fn closure_drops<S: Stage>(
    ed: &Editor<S>,
    facts: &Facts,
    lay: Lay<'_>,
    heap: &[bool],
) -> BTreeMap<(usize, usize), (ValueId, LayoutId, LirTypeId)> {
    let unit = &ed.unit;
    let mut defs: BTreeMap<ValueId, Inst> = BTreeMap::new();
    for code in &ed.code {
        for inst in code {
            for v in Inst::defs(inst) {
                defs.insert(v, *inst);
            }
        }
    }
    let flow = crate::insert::Flow {
        unit,
        code: &ed.code,
        lay,
        facts,
    };
    let ins = flow.solve();
    let mut out = BTreeMap::new();
    for (b, state) in ins.into_iter().enumerate() {
        let Some(state) = state else { continue };
        if unit.blocks[b].kind == BlockKind::FailureCleanup {
            continue;
        }
        flow.block(BlockId(b as u32), state, |i, before, _| {
            let Some(Inst::Drop {
                address,
                if_live: false,
                ..
            }) = ed.code[b].get(i)
            else {
                return;
            };
            let Some((slot, path)) = facts.slot_of(*address) else {
                return;
            };
            let ty = unit.slots[slot.index()].ty;
            let s = &before.slots[slot.index()];
            if unit.types[ty.index()].repr != ValueRepr::Callable || !path.is_empty() || !s.whole()
            {
                return;
            }
            let Some(version) = s.versions.one() else {
                return;
            };
            let Some(environment) = environment(ed, &defs, version, heap, &mut BTreeSet::new())
            else {
                return;
            };
            // Empty environments are static, even at a heap closure site.
            // They own no allocation that can donate storage.
            if !lay.destructible(environment)
                && lay.0.layout(environment).is_some_and(|record| {
                    record.size != hirsel_lir::layout_types::SizeSpec::Fixed(0)
                })
            {
                out.insert((b, i), (version, environment, ty));
            }
        });
    }
    out
}

/// The environment layout of the heap closure a callable value (or a slot
/// version holding one) was built as, through `Weaken`, whole-slot stores
/// and takes and agreeing block parameters; the validator replays the same
/// provenance.
fn environment<S: Stage>(
    ed: &Editor<S>,
    defs: &BTreeMap<ValueId, Inst>,
    value: ValueId,
    heap: &[bool],
    seen: &mut BTreeSet<ValueId>,
) -> Option<LayoutId> {
    let unit = &ed.unit;
    if !seen.insert(value) {
        return None;
    }
    match defs.get(&value) {
        Some(&Inst::Closure {
            result, closure, ..
        }) => heap[result.index()].then(|| unit.tables.closures[closure.index()].environment),
        Some(&Inst::Weaken { value, .. }) => environment(ed, defs, value, heap, seen),
        Some(&Inst::Store {
            value: stored,
            version,
            ..
        }) if version == value => environment(ed, defs, stored, heap, seen),
        Some(&Inst::Read {
            address,
            reaching,
            mode: hirsel_lir::ir::ReadMode::Take,
            ..
        }) => {
            let info = &unit.addresses[address.index()];
            let whole = matches!(info.base, hirsel_lir::ir::AddressBase::Slot(_))
                && info.projection.len == 0;
            if whole {
                environment(ed, defs, reaching, heap, seen)
            } else {
                None
            }
        }
        Some(_) => None,
        None => {
            // A block parameter: every incoming argument agrees.
            let (b, i) = unit.blocks.iter().enumerate().find_map(|(b, block)| {
                block
                    .parameters
                    .get(&unit.pools.values)
                    .unwrap_or(&[])
                    .iter()
                    .position(|&p| p == value)
                    .map(|i| (b, i))
            })?;
            let mut found = None;
            let mut any = false;
            for (e, edge) in unit.tables.edges.iter().enumerate() {
                if edge.target.index() != b || !named(ed, EdgeId(e as u32)) {
                    continue;
                }
                any = true;
                let arg = *edge.arguments.get(&unit.pools.values)?.get(i)?;
                let env = environment(ed, defs, arg, heap, &mut seen.clone())?;
                if found.is_some_and(|f| f != env) {
                    return None;
                }
                found = Some(env);
            }
            found.filter(|_| any)
        }
    }
}

/// Whether a terminator names `edge` (an editor keeps unnamed edges until
/// it finishes).
fn named<S: Stage>(ed: &Editor<S>, edge: EdgeId) -> bool {
    ed.unit
        .terminators
        .iter()
        .any(|t| Terminator::normal_edges(t, &ed.unit.pools).contains(&edge))
}

fn repr<S>(unit: &LirUnit<S>, v: ValueId) -> ValueRepr {
    unit.types[unit.values[v.index()].ty.index()].repr
}

/// The bucket of a heap block construction a token may install.
fn construct_bucket<S: Stage>(ed: &Editor<S>, table: &LayoutTable, inst: &Inst) -> Option<Bucket> {
    let Inst::Construct {
        result, construct, ..
    } = *inst
    else {
        return None;
    };
    let unit = &ed.unit;
    let site = &unit.tables.constructs[construct.index()];
    let Some(Placement::Fixed(StorageOwner::Heap(domain))) = site.placement else {
        return None;
    };
    if repr(unit, result) != ValueRepr::Block {
        return None;
    }
    bucket(table, site.layout, domain)
}

// ---------------------------------------------------------------------------
// Pairing

type Live = Vec<(ValueId, Bucket)>;

fn pair_tokens(unit: LirUnit<Reuse>, table: &LayoutTable) -> Result<LirUnit<Reuse>, ReuseError> {
    let Some(header) = table
        .layouts
        .iter()
        .position(|l| l.kind == LayoutKind::Header)
        .map(|i| LayoutId(i as u32))
    else {
        // No token type in this table: nothing can be reused.
        return Ok(unit);
    };
    let facts = Facts::new(&unit, table);
    let lay = Lay(table);
    let mut ed = Editor::new(unit);
    let token_ty = type_of(&mut ed.unit, ValueRepr::Token, header);
    let graph = Graph::new(&ed);
    let heap = heap_values(&ed, &facts);
    let closures = closure_drops(&ed, &facts, lay, &heap);
    let n = ed.unit.blocks.len();

    // Anticipated buckets: a construction of the bucket lies ahead on some
    // path of normal edges that crosses no feedback edge.
    let own: Vec<BTreeSet<Bucket>> = (0..n)
        .map(|b| {
            ed.code[b]
                .iter()
                .filter_map(|i| construct_bucket(&ed, table, i))
                .collect()
        })
        .collect();
    let mut ant_in: Vec<BTreeSet<Bucket>> = own.clone();
    let mut ant_out: Vec<BTreeSet<Bucket>> = vec![BTreeSet::new(); n];
    loop {
        let mut changed = false;
        for b in (0..n).rev() {
            let mut out = BTreeSet::new();
            for &(e, t) in &graph.normal[b] {
                if !graph.feedback.contains(&e) {
                    out.extend(ant_in[t.index()].iter().copied());
                }
            }
            let mut inn = own[b].clone();
            inn.extend(out.iter().copied());
            if out != ant_out[b] || inn != ant_in[b] {
                ant_out[b] = out;
                ant_in[b] = inn;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // A block entered only by the shared edge of `IsUnique(d)` releases a
    // block it knows is shared: that release never yields storage.
    let mut shared: Vec<Option<ValueId>> = vec![None; n];
    for b in 0..n {
        if let Terminator::Branch { condition, no, .. } = ed.unit.terminators[b] {
            let target = ed.unit.tables.edges[no.index()].target;
            let tested = ed.code[b].iter().find_map(|i| match *i {
                Inst::IsUnique { result, value } if result == condition => Some(value),
                _ => None,
            });
            if graph.preds[target.index()].len() == 1 {
                shared[target.index()] = tested;
            }
        }
    }

    let header_block = |b: BlockId| {
        graph.preds[b.index()]
            .iter()
            .any(|(_, e)| graph.feedback.contains(e))
    };
    let mut params: Vec<Vec<(ValueId, Bucket)>> = vec![Vec::new(); n];
    let mut exit: Vec<Option<Live>> = vec![None; n];
    let mut new_params: Vec<Vec<ValueId>> = vec![Vec::new(); n];
    for b in graph.forward_order() {
        let bi = b.index();
        if ed.unit.blocks[bi].kind == BlockKind::FailureCleanup {
            exit[bi] = Some(Vec::new());
            continue;
        }
        let preds = &graph.preds[bi];
        let mut live: Live = if b == graph.entry || header_block(b) || preds.is_empty() {
            Vec::new()
        } else if preds.len() == 1 {
            exit[preds[0].0.index()].clone().unwrap_or_default()
        } else {
            // A join: one token parameter per anticipated bucket some
            // predecessor brings.
            let mut buckets = BTreeSet::new();
            for &(p, _) in preds {
                for &(_, k) in exit[p.index()].iter().flatten() {
                    if ant_in[bi].contains(&k) {
                        buckets.insert(k);
                    }
                }
            }
            let mut live = Vec::new();
            for k in buckets {
                let p = ed.value(token_ty, Holding::Owned);
                params[bi].push((p, k));
                new_params[bi].push(p);
                live.push((p, k));
            }
            live
        };
        let mut code = std::mem::take(&mut ed.code[bi]);
        // Buckets still anticipated after each position.
        let mut after: Vec<BTreeSet<Bucket>> = vec![ant_out[bi].clone(); code.len() + 1];
        for i in (0..code.len()).rev() {
            let mut s = after[i + 1].clone();
            if let Some(k) = construct_bucket(&ed, table, &code[i]) {
                s.insert(k);
            }
            after[i] = s;
        }
        let mut out = Vec::with_capacity(code.len());
        dispose_unwanted(&mut out, &mut live, &after[0]);
        for (i, mut inst) in code.drain(..).enumerate() {
            let wanted = &after[i + 1];
            match inst {
                Inst::Release {
                    value,
                    layout,
                    live: mask,
                    anchor,
                    failure,
                } if shared[bi] != Some(value)
                    && heap[value.index()]
                    && repr(&ed.unit, value) == ValueRepr::Block
                    && ed.unit.values[value.index()].holding == Holding::Owned
                    && owns(&ed.unit, lay, value) =>
                {
                    match bucket(table, layout, TaskDomainId(0)).filter(|k| wanted.contains(k)) {
                        Some(k) => {
                            set_dispose(&mut ed, failure, &live);
                            let result = ed.value(token_ty, Holding::Owned);
                            out.push(Inst::Reset {
                                result,
                                value,
                                layout,
                                live: mask,
                                anchor,
                                failure,
                            });
                            live.push((result, k));
                        }
                        None => {
                            set_dispose(&mut ed, failure, &live);
                            out.push(inst);
                        }
                    }
                }
                Inst::Drop {
                    address,
                    anchor,
                    failure,
                    ..
                } if closures.contains_key(&(bi, i)) => {
                    let (reaching, environment, ty) = closures[&(bi, i)];
                    match bucket(table, environment, TaskDomainId(0)).filter(|k| wanted.contains(k))
                    {
                        Some(k) => {
                            // The closure's environment block donates.
                            let value = ed.value(ty, Holding::Owned);
                            out.push(Inst::Read {
                                result: value,
                                address,
                                reaching,
                                mode: hirsel_lir::ir::ReadMode::Take,
                            });
                            set_dispose(&mut ed, failure, &live);
                            let result = ed.value(token_ty, Holding::Owned);
                            out.push(Inst::Reset {
                                result,
                                value,
                                layout: environment,
                                live: hirsel_lir::ir::MaskRef::All,
                                anchor,
                                failure,
                            });
                            live.push((result, k));
                        }
                        None => {
                            set_dispose(&mut ed, failure, &live);
                            out.push(inst);
                        }
                    }
                }
                Inst::Construct {
                    result,
                    construct,
                    failure,
                } => {
                    let k = construct_bucket(&ed, table, &inst);
                    let found = k.and_then(|k| live.iter().rposition(|&(_, l)| l == k));
                    match found {
                        Some(at) => {
                            let (token, _) = live.remove(at);
                            set_dispose(&mut ed, failure, &live);
                            out.push(Inst::Reuse {
                                result,
                                token,
                                construct,
                                kept: None,
                                failure,
                            });
                        }
                        None => {
                            set_dispose(&mut ed, failure, &live);
                            out.push(inst);
                        }
                    }
                }
                _ => {
                    if let Some(f) = Inst::failure_mut(&mut inst) {
                        let f = *f;
                        set_dispose(&mut ed, f, &live);
                    }
                    out.push(inst);
                }
            }
            dispose_unwanted(&mut out, &mut live, wanted);
        }
        match ed.unit.terminators[bi] {
            Terminator::Return(_)
            | Terminator::Propagate
            | Terminator::Unreachable
            | Terminator::InvariantFault { .. } => {
                for (t, _) in live.drain(..) {
                    out.push(Inst::Dispose { token: t });
                }
            }
            Terminator::Raise(f) => {
                set_dispose(&mut ed, f, &live);
                live.clear();
            }
            // A call's failure edge disposes of what is still live at the
            // call, which the edge phase below decides.
            _ => {}
        }
        ed.code[bi] = out;
        exit[bi] = Some(live);
    }

    // Join parameters, appended after each block's own.
    for (b, extra) in new_params.iter().enumerate() {
        if extra.is_empty() {
            continue;
        }
        let block = &ed.unit.blocks[b];
        let mut all = block
            .parameters
            .get(&ed.unit.pools.values)
            .unwrap_or(&[])
            .to_vec();
        all.extend(extra.iter().copied());
        ed.unit.blocks[b].parameters = PoolRange::push(&mut ed.unit.pools.values, all);
    }

    // Edges: what each carries, passes into a join or leaves behind.
    for b in 0..n {
        let Some(live) = exit[b].clone() else {
            continue;
        };
        if live.is_empty()
            && graph.normal[b]
                .iter()
                .all(|&(_, t)| params[t.index()].is_empty())
        {
            continue;
        }
        let plans: Vec<(EdgeId, BlockId, Vec<ValueId>, Vec<Bucket>, Vec<ValueId>)> = graph.normal
            [b]
            .iter()
            .map(|&(e, t)| {
                let ti = t.index();
                if graph.feedback.contains(&e) || header_block(t) {
                    return (
                        e,
                        t,
                        Vec::new(),
                        Vec::new(),
                        live.iter().map(|x| x.0).collect(),
                    );
                }
                if params[ti].is_empty() && graph.preds[ti].len() == 1 {
                    // Carried whole; the successor disposes of what it cannot use.
                    return (e, t, Vec::new(), Vec::new(), Vec::new());
                }
                let mut left: Live = live.clone();
                let mut args = Vec::new();
                for &(_, k) in &params[ti] {
                    match left.iter().position(|&(_, l)| l == k) {
                        Some(at) => args.push(Some(left.remove(at).0)),
                        None => args.push(None),
                    }
                }
                let dispose = left.iter().map(|x| x.0).collect();
                let args: Vec<ValueId> = args
                    .into_iter()
                    .map(|a| a.unwrap_or(ValueId(u32::MAX)))
                    .collect();
                (e, t, args, Vec::new(), dispose)
            })
            .collect();
        // Tokens no edge keeps go before the terminator.
        let kept: BTreeSet<ValueId> = plans
            .iter()
            .flat_map(|(_, t, args, _, dispose)| {
                let carried = params[t.index()].is_empty()
                    && graph.preds[t.index()].len() == 1
                    && dispose.is_empty()
                    && args.is_empty();
                let mut k: Vec<ValueId> =
                    args.iter().copied().filter(|a| a.0 != u32::MAX).collect();
                if carried {
                    k.extend(live.iter().map(|x| x.0));
                }
                k
            })
            .collect();
        let everywhere: Vec<ValueId> = live
            .iter()
            .map(|x| x.0)
            .filter(|t| !kept.contains(t))
            .collect();
        for &t in &everywhere {
            ed.code[b].push(Inst::Dispose { token: t });
        }
        if let Terminator::Call { failure, .. } = ed.unit.terminators[b] {
            let at_call: Live = live
                .iter()
                .copied()
                .filter(|(t, _)| !everywhere.contains(t))
                .collect();
            set_dispose(&mut ed, failure, &at_call);
        }
        let jump = matches!(ed.unit.terminators[b], Terminator::Jump(_));
        for (e, t, args, _, dispose) in plans {
            let dispose: Vec<ValueId> = dispose
                .into_iter()
                .filter(|d| !everywhere.contains(d))
                .collect();
            if args.is_empty() && dispose.is_empty() {
                continue;
            }
            let mut actions = Vec::new();
            for d in dispose {
                actions.push(Inst::Dispose { token: d });
            }
            let mut real = Vec::new();
            for a in args {
                if a.0 == u32::MAX {
                    let none = ed.value(token_ty, Holding::Owned);
                    actions.push(Inst::TokenNone { result: none });
                    real.push(none);
                } else {
                    real.push(a);
                }
            }
            if jump {
                ed.code[b].extend(actions);
                append_arguments(&mut ed, e, &real);
            } else {
                split(&mut ed, BlockId(b as u32), e, t, actions, &real);
            }
        }
    }
    Ok(ed.finish())
}

/// Dispose of live tokens whose bucket no construction ahead wants.
fn dispose_unwanted(out: &mut Vec<Inst>, live: &mut Live, wanted: &BTreeSet<Bucket>) {
    live.retain(|&(t, k)| {
        if wanted.contains(&k) {
            true
        } else {
            out.push(Inst::Dispose { token: t });
            false
        }
    });
}

/// A failure successor disposes of the tokens live at its site, beside its
/// original cleanup. Each failure edge has exactly one site.
fn set_dispose<S: Stage>(ed: &mut Editor<S>, failure: FailureId, live: &Live) {
    let tokens: Vec<ValueId> = live.iter().map(|x| x.0).collect();
    ed.unit.tables.failures[failure.index()].dispose = if tokens.is_empty() {
        PoolRange::EMPTY
    } else {
        PoolRange::push(&mut ed.unit.pools.values, tokens)
    };
}

fn append_arguments<S: Stage>(ed: &mut Editor<S>, edge: EdgeId, extra: &[ValueId]) {
    if extra.is_empty() {
        return;
    }
    let e = &ed.unit.tables.edges[edge.index()];
    let mut all = e
        .arguments
        .get(&ed.unit.pools.values)
        .unwrap_or(&[])
        .to_vec();
    all.extend_from_slice(extra);
    ed.unit.tables.edges[edge.index()].arguments = PoolRange::push(&mut ed.unit.pools.values, all);
}

/// Put `actions` on edge `e` from `from` to `to`: a new block between them
/// runs them and jumps on with the edge's arguments plus `extra`.
fn split<S: Stage>(
    ed: &mut Editor<S>,
    from: BlockId,
    e: EdgeId,
    to: BlockId,
    actions: Vec<Inst>,
    extra: &[ValueId],
) {
    let kinds = (
        ed.unit.blocks[from.index()].kind,
        ed.unit.blocks[to.index()].kind,
    );
    let kind = if kinds.0 == kinds.1 {
        kinds.0
    } else {
        BlockKind::Body
    };
    let anchor = ed.unit.blocks[to.index()].anchor;
    let mid = ed.block(kind, &[], anchor);
    let mut args = ed.unit.tables.edges[e.index()]
        .arguments
        .get(&ed.unit.pools.values)
        .unwrap_or(&[])
        .to_vec();
    args.extend_from_slice(extra);
    let onward = ed.edge(to, &args);
    ed.unit.tables.edges[e.index()].target = mid;
    ed.unit.tables.edges[e.index()].arguments = PoolRange::EMPTY;
    ed.code[mid.index()] = actions;
    ed.unit.terminators[mid.index()] = Terminator::Jump(onward);
}

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
