//! Strength reduction of loops over variable-element FlatFormatV0 and sequential
//! Packed sequences (MEM-13).
//! Discovery and proof are read-only; only a complete plan may edit a unit.
use crate::insert::{Editor, Facts};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutKind, LayoutTable, ScalarKind, SizeSpec};
use hirsel_lir::stage::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The driver's matrix may disable this Value-stage rewrite.
    pub enabled: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// A discovered loop and sequence. No proof or mutation is implied.
#[derive(Clone, Debug)]
pub struct Candidate {
    header: usize,
    latch: usize,
    members: BTreeSet<usize>,
    root: SlotId,
    prefix: Vec<ProjectionStep>,
    element: LayoutId,
    index: ValueId,
    reads: Vec<(usize, usize, AddressId, usize)>,
}
/// A complete proof, tied to the exact unit it was discovered in.
#[derive(Clone, Debug)]
pub struct Plan {
    candidate: Candidate,
    digest: hirsel_syntax::ContentHash,
    entry_edges: Vec<(usize, EdgeId)>,
    first: (usize, usize, AddressId, usize),
    index_slot: SlotId,
    feedback: EdgeId,
    rewrites: Vec<AddressId>,
}

pub(crate) struct Graph {
    pub(crate) succ: Vec<Vec<(usize, EdgeId)>>,
    pub(crate) pred: Vec<Vec<(usize, EdgeId)>>,
    dom: hirsel_graph::Dominators,
}
impl Graph {
    pub(crate) fn new(u: &LirUnit<Value>) -> Self {
        let mut succ = vec![vec![]; u.blocks.len()];
        let mut pred = succ.clone();
        for (b, t) in u.terminators.iter().enumerate() {
            for e in Terminator::normal_edges(t, &u.pools) {
                let to = u.tables.edges[e.index()].target.index();
                succ[b].push((to, e));
                pred[to].push((b, e));
            }
        }
        let adjacent: Vec<Vec<usize>> = succ
            .iter()
            .map(|ts| ts.iter().map(|&(b, _)| b).collect())
            .collect();
        let dom = hirsel_graph::Dominators::new(&adjacent, u.entry.index());
        Self { succ, pred, dom }
    }
    pub(crate) fn dominates(&self, a: usize, b: usize) -> bool {
        self.dom.dominates(a, b)
    }
}

pub(crate) fn code(u: &LirUnit<Value>, b: usize) -> &[Inst] {
    u.blocks[b].instructions.get(&u.instructions).unwrap_or(&[])
}
pub(crate) struct Values {
    pub(crate) inst: Vec<Option<Inst>>,
    alias: Vec<Option<ValueId>>,
}
fn aliases(u: &LirUnit<Value>) -> Vec<Option<ValueId>> {
    let mut incoming = vec![Vec::new(); u.values.len()];
    for e in &u.tables.edges {
        for (&p, &a) in u.blocks[e.target.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(e.arguments.get(&u.pools.values).unwrap_or(&[]))
        {
            incoming[p.index()].push(a);
        }
    }
    incoming
        .into_iter()
        .map(|args| {
            args.first()
                .copied()
                .filter(|a| args.iter().all(|x| x == a))
        })
        .collect()
}
pub(crate) fn scalar_source(u: &LirUnit<Value>, defs: &Values, mut v: ValueId) -> ValueId {
    let mut seen = BTreeSet::new();
    while seen.insert(v) {
        if let Some(a) = defs.alias[v.index()] {
            v = a;
            continue;
        }
        match defs.inst[v.index()] {
            Some(Inst::Read {
                address, reaching, ..
            }) if u.addresses[address.index()].projection.len == 0 => v = reaching,
            Some(Inst::Store { value, .. }) => v = value,
            Some(Inst::Replace { value, version, .. }) if version == v => v = value,
            _ => break,
        }
    }
    v
}
pub(crate) fn definitions(u: &LirUnit<Value>) -> Values {
    let mut defs = vec![None; u.values.len()];
    for i in &u.instructions {
        for v in Inst::defs(i) {
            defs[v.index()] = Some(*i);
        }
    }
    Values {
        inst: defs,
        alias: aliases(u),
    }
}
pub(crate) fn integer(u: &LirUnit<Value>, defs: &Values, v: ValueId) -> Option<u128> {
    match defs.inst[scalar_source(u, defs, v).index()]? {
        Inst::Const { constant, .. } => match u.tables.constants[constant.index()].value {
            ConstantValue::Integer(n) => Some(n),
            _ => None,
        },
        _ => None,
    }
}
fn variable_sequence(
    table: &LayoutTable,
    mut layout: LayoutId,
    path: &[ProjectionStep],
) -> Option<(usize, LayoutId)> {
    for (p, step) in path.iter().enumerate() {
        if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
            table.layouts.get(layout.0 as usize)?.kind
        {
            layout = payload;
        }
        let r = table.layouts.get(layout.0 as usize)?;
        match *step {
            ProjectionStep::Index(_)
                if r.kind == LayoutKind::Flat
                    && matches!(r.size, SizeSpec::Dynamic(_))
                    && r.fields.len() == 1
                    && r.fields[0].offset == 16
                    && !matches!(
                        table.layouts.get(r.fields[0].layout.0 as usize)?.size,
                        SizeSpec::Fixed(_)
                    ) =>
            {
                return Some((p, r.fields[0].layout));
            }
            // A sequential Packed sequence (MEM-13): offsets count from its
            // first element.
            ProjectionStep::Index(_) if matches!(r.kind, LayoutKind::Sequential { .. }) => {
                let LayoutKind::Sequential { element } = r.kind else {
                    unreachable!("matched above")
                };
                return Some((p, element));
            }
            ProjectionStep::Field(f) => layout = r.fields.iter().find(|x| x.ordinal == f)?.layout,
            _ => return None,
        }
    }
    None
}

/// Find natural loops and indexed variable flat sequences. Irreducible loops
/// and multiple feedback edges are conservatively left alone.
pub fn discover(u: &LirUnit<Value>, table: &LayoutTable) -> Vec<Candidate> {
    discover_with(u, table, &Graph::new(u), &definitions(u))
}
fn discover_with(
    u: &LirUnit<Value>,
    table: &LayoutTable,
    g: &Graph,
    defs: &Values,
) -> Vec<Candidate> {
    let mut loops: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (b, outs) in g.succ.iter().enumerate() {
        for &(h, _) in outs {
            if g.dominates(h, b) {
                loops.entry(h).or_default().push(b);
            }
        }
    }
    let mut out = vec![];
    for (header, latches) in loops {
        if latches.len() != 1 {
            continue;
        }
        let latch = latches[0];
        let mut members = BTreeSet::from([header]);
        let mut q = vec![latch];
        while let Some(b) = q.pop() {
            if members.insert(b) {
                q.extend(g.pred[b].iter().map(|p| p.0));
            }
        }
        let mut groups: Vec<Candidate> = vec![];
        for &b in &members {
            for (i, inst) in code(u, b).iter().enumerate() {
                let Inst::Address { address, .. } = *inst else {
                    continue;
                };
                let a = &u.addresses[address.index()];
                let AddressBase::Slot(root) = a.base else {
                    continue;
                };
                let path = a.projection.get(&u.pools.projections).unwrap_or(&[]);
                let Some((p, element)) = variable_sequence(
                    table,
                    u.types[u.slots[root.index()].ty.index()].layout,
                    path,
                ) else {
                    continue;
                };
                let ProjectionStep::Index(index) = path[p] else {
                    continue;
                };
                let index = scalar_source(u, &defs, index);
                if let Some(c) = groups
                    .iter_mut()
                    .find(|c| c.root == root && c.prefix == path[..p] && c.index == index)
                {
                    c.reads.push((b, i, address, p));
                } else {
                    groups.push(Candidate {
                        header,
                        latch,
                        members: members.clone(),
                        root,
                        prefix: path[..p].to_vec(),
                        element,
                        index,
                        reads: vec![(b, i, address, p)],
                    });
                }
            }
        }
        out.extend(groups);
    }
    out
}

/// Prove initialization, unit stride, unchanged reaching content and a checked
/// read on every feedback path. Breaks are safe; a skipped read declines.
pub fn prove(u: &LirUnit<Value>, table: &LayoutTable, c: Candidate) -> Option<Plan> {
    prove_with(
        u,
        c,
        &Graph::new(u),
        &definitions(u),
        &Facts::new(u, table),
        u.digest(),
    )
}
fn prove_with(
    u: &LirUnit<Value>,
    mut c: Candidate,
    g: &Graph,
    defs: &Values,
    facts: &Facts,
    digest: hirsel_syntax::ContentHash,
) -> Option<Plan> {
    let params = u.blocks[c.header].parameters.get(&u.pools.values)?;
    let ordinal = params.iter().position(|&p| p == c.index)?;
    let index_slot = facts.version_of[c.index.index()]?;
    let ty = &u.types[u.values[c.index.index()].ty.index()];
    if ty.repr != ValueRepr::Scalar(ScalarKind::U64) {
        return None;
    }
    let mut entry_edges = vec![];
    for &(b, e) in &g.pred[c.header] {
        let arg = *u.tables.edges[e.index()]
            .arguments
            .get(&u.pools.values)?
            .get(ordinal)?;
        if !c.members.contains(&b) {
            if integer(u, &defs, arg) != Some(0) {
                return None;
            }
            entry_edges.push((b, e));
        } else {
            if b != c.latch {
                return None;
            }
            let value = scalar_source(u, &defs, arg);
            let Some(Inst::Binary {
                op: BinaryOp::Add,
                left,
                right,
                ..
            }) = defs.inst[value.index()]
            else {
                return None;
            };
            if !((scalar_source(u, &defs, left) == c.index && integer(u, &defs, right) == Some(1))
                || (scalar_source(u, &defs, right) == c.index
                    && integer(u, &defs, left) == Some(1)))
            {
                return None;
            }
        }
    }
    if entry_edges.is_empty() {
        return None;
    }
    // A natural loop omits break/return-only paths. Follow the index's
    // lexical lifetime too, stopping before its scope ends, so those reads
    // and writes participate in the proof without including later statements.
    let scope = u.slots[index_slot.index()].scope;
    let ends_index_scope = |mut child: ScopeId, outer: ScopeId| {
        loop {
            if child == outer {
                return true;
            }
            let Some(parent) = u.scopes[child.index()].parent else {
                return false;
            };
            child = parent;
        }
    };
    let mut body = BTreeMap::new();
    let mut work = vec![c.header];
    while let Some(b) = work.pop() {
        if body.contains_key(&b) || !g.dominates(c.header, b) {
            continue;
        }
        let insts = code(u, b);
        let stop = insts
            .iter()
            .position(|inst| match *inst {
                Inst::EndScope { scope: ended } => ends_index_scope(scope, ended),
                Inst::StorageDead { slot, .. } => slot == index_slot,
                _ => false,
            })
            .unwrap_or(insts.len());
        body.insert(b, stop);
        if stop == insts.len() {
            work.extend(g.succ[b].iter().map(|p| p.0));
        }
    }
    if c.members
        .iter()
        .any(|b| body.get(b).is_none_or(|&end| end != code(u, *b).len()))
    {
        return None;
    }
    c.members = body.keys().copied().collect();
    for (&b, &end) in &body {
        for (i, inst) in code(u, b)[..end].iter().enumerate() {
            let Inst::Address { address, .. } = *inst else {
                continue;
            };
            let a = &u.addresses[address.index()];
            let path = a.projection.get(&u.pools.projections)?;
            if a.base == AddressBase::Slot(c.root)
                && path.starts_with(&c.prefix)
                && matches!(path.get(c.prefix.len()),Some(ProjectionStep::Index(v)) if scalar_source(u,defs,*v)==c.index)
                && !c.reads.iter().any(|r| r.2 == address)
            {
                c.reads.push((b, i, address, c.prefix.len()));
            }
        }
    }
    // No alternative entry, write, exclusive call, or nested cycle. The latter
    // also declines nested walks over this sequence, rather than sharing cursors.
    for &b in &c.members {
        if b != c.header && g.pred[b].iter().any(|p| !c.members.contains(&p.0)) {
            return None;
        }
        for &(s, _) in &g.succ[b] {
            if c.members.contains(&s) && s != c.header && g.dominates(s, b) {
                return None;
            }
        }
        for i in &code(u, b)[..body[&b]] {
            let a = match *i {
                Inst::Store { address, .. }
                | Inst::Replace { address, .. }
                | Inst::SeqReserve { address, .. }
                | Inst::SeqSetLen { address, .. }
                | Inst::SeqAppend { address, .. }
                | Inst::FlatAppend { address, .. }
                | Inst::FlatField { address, .. }
                | Inst::FlatClose { address, .. }
                | Inst::BeginLoan {
                    address,
                    mutable: true,
                    ..
                } => Some(address),
                Inst::Read {
                    address,
                    mode: ReadMode::Take,
                    ..
                } if facts.slot_of(address).is_some_and(|(s, _)| s == c.root) => Some(address),
                _ => None,
            };
            if a.is_some_and(|a| facts.slot_of(a).is_none_or(|(s, _)| s == c.root)) {
                return None;
            }
        }
        if body[&b] == code(u, b).len()
            && let Terminator::Call { call, .. } = u.terminators[b]
        {
            for a in u.tables.calls[call.index()]
                .arguments
                .get(&u.pools.call_arguments)?
            {
                if let ArgumentValue::Address { address, .. } = a.value
                    && facts.slot_of(address).is_none_or(|(s, _)| s == c.root)
                {
                    return None;
                }
            }
        }
    }
    let first = *c
        .reads
        .iter()
        .filter(|r| g.dominates(r.0, c.latch))
        .min_by_key(|r| (g.dom.order(r.0).unwrap_or(usize::MAX), r.1))?;
    if !c
        .reads
        .iter()
        .all(|r| g.dominates(first.0, r.0) && (r.0 != first.0 || r.1 >= first.1))
    {
        return None;
    }
    // All prepares of this sequence in the loop must use this induction value;
    // nested loops or unrelated subscripts leave the complete loop unchanged.
    for &b in &c.members {
        for inst in &code(u, b)[..body[&b]] {
            if let Inst::Address { address, .. } = *inst {
                let a = &u.addresses[address.index()];
                if a.base == AddressBase::Slot(c.root) {
                    let p = a.projection.get(&u.pools.projections)?;
                    if p.starts_with(&c.prefix)
                        && p.len() > c.prefix.len()
                        && let ProjectionStep::Index(i) = p[c.prefix.len()]
                        && scalar_source(u, &defs, i) != c.index
                    {
                        return None;
                    }
                }
            }
        }
    }
    let feedback = g.pred[c.header].iter().find(|(b, _)| *b == c.latch)?.1;
    let mut named = BTreeSet::new();
    for &b in &c.members {
        for inst in &code(u, b)[..body[&b]] {
            inst.visit(&mut |r| {
                if let Ref::Address(a) = r {
                    named.insert(a);
                }
            });
        }
    }
    let rewrites=named.into_iter().filter(|a| {
        let info=&u.addresses[a.index()];
        let path=info.projection.get(&u.pools.projections).unwrap_or(&[]);
        info.base==AddressBase::Slot(c.root) && path.starts_with(&c.prefix) && matches!(path.get(c.prefix.len()),Some(ProjectionStep::Index(v)) if scalar_source(u,defs,*v)==c.index)
    }).collect();
    Some(Plan {
        feedback,
        rewrites,
        candidate: c,
        digest,
        entry_edges,
        first,
        index_slot,
    })
}

/// Commit only against the unit whose immutable proof this plan records.
pub fn commit(u: LirUnit<Value>, p: Plan) -> LirUnit<Value> {
    assert_eq!(
        u.digest(),
        p.digest,
        "flat-walk plan belongs to another unit"
    );
    let mut ed = Editor::new(u);
    commit_into(&mut ed, p);
    ed.finish()
}
fn commit_into(ed: &mut Editor<Value>, p: Plan) {
    let c = p.candidate;
    let ty = ed.unit.slots[p.index_slot.index()].ty;
    let slot = SlotId(ed.unit.slots.len() as u32);
    ed.unit.slots.push(SlotInfo {
        ty,
        kind: SlotKind::LoopCarried(slot.0),
        scope: ed.unit.slots[p.index_slot.index()].scope,
    });
    let at = AddressId(ed.unit.addresses.len() as u32);
    ed.unit.addresses.push(AddressInfo {
        source_place: None,
        unchecked: Vec::new(),
        base: AddressBase::Slot(slot),
        projection: PoolRange::EMPTY,
        ty,
    });
    let phi = ed.value(ty, Holding::Plain);
    let mut params = ed.unit.blocks[c.header]
        .parameters
        .get(&ed.unit.pools.values)
        .unwrap()
        .to_vec();
    params.push(phi);
    ed.unit.blocks[c.header].parameters = PoolRange::push(&mut ed.unit.pools.values, params);
    let cleanup_info = ed.unit.tables.edges[ed.unit.tables.failures[0].cleanup.index()].clone();
    let cleanup_args = cleanup_info
        .arguments
        .get(&ed.unit.pools.values)
        .unwrap()
        .to_vec();
    let cleanup = ed.edge(cleanup_info.target, &cleanup_args);
    let failure = ed.failure(cleanup, ed.unit.blocks[ed.unit.entry.index()].anchor);
    ed.code[ed.unit.entry.index()].insert(
        0,
        Inst::Address {
            address: at,
            failure,
        },
    );
    for (mut b, mut e) in p.entry_edges {
        // Another proved sequence in this loop may already have split entry.
        while ed.unit.tables.edges[e.index()].target.index() != c.header {
            b = ed.unit.tables.edges[e.index()].target.index();
            let Terminator::Jump(next) = ed.unit.terminators[b] else {
                unreachable!()
            };
            e = next;
        }
        let zero = ed.value(ty, Holding::Plain);
        let version = ed.value(ty, Holding::Plain);
        let constant = ConstantId(ed.unit.tables.constants.len() as u32);
        ed.unit.tables.constants.push(ConstantData {
            ty,
            value: ConstantValue::Integer(0),
        });
        let target = ed.unit.tables.edges[e.index()].target;
        let mut args = ed.unit.tables.edges[e.index()]
            .arguments
            .get(&ed.unit.pools.values)
            .unwrap()
            .to_vec();
        args.push(version);
        let edge = ed.edge(target, &args);
        let anchor = ed.unit.blocks[b].anchor;
        let init = ed.block(BlockKind::Body, &[], anchor);
        ed.code[init.index()] = vec![
            Inst::StorageLive { slot },
            Inst::Const {
                result: zero,
                constant,
            },
            Inst::Store {
                address: at,
                value: zero,
                version,
            },
        ];
        ed.unit.terminators[init.index()] = Terminator::Jump(edge);
        ed.unit.tables.edges[e.index()].target = init;
        // Preserve any pre-existing header arguments on the new forwarding edge.
        ed.unit.tables.edges[e.index()].arguments = PoolRange::EMPTY;
    }
    let offset = ed.value(ty, Holding::Plain);
    // One immutable offset read dominates all rewritten prepares. A candidate
    // whose first read doesn't dominate another read was declined by prove.
    let (b, _, a, position) = p.first;
    let i = ed.code[b]
        .iter()
        .position(|inst| matches!(inst,Inst::Address{address,..} if *address==a))
        .unwrap();
    ed.code[b].insert(
        i,
        Inst::Read {
            result: offset,
            address: at,
            reaching: phi,
            mode: ReadMode::Take,
        },
    );
    for address in p.rewrites {
        let path = ed.unit.addresses[address.index()]
            .projection
            .get(&ed.unit.pools.projections)
            .unwrap()
            .to_vec();
        let mut path = path;
        let ProjectionStep::Index(index) = path[position] else {
            continue;
        };
        path[position] = ProjectionStep::FlatAt { index, offset };
        ed.unit.addresses[address.index()].projection =
            PoolRange::push(&mut ed.unit.pools.projections, path);
    }
    // Compute the next boundary immediately after the first successful prepare:
    // no extra bounds failure or read occurs on a break/continue before it.
    let mut element_path = ed.unit.addresses[a.index()]
        .projection
        .get(&ed.unit.pools.projections)
        .unwrap()[..=position]
        .to_vec();
    // Use the first prepare's index, already available at this point.
    let element_at = AddressId(ed.unit.addresses.len() as u32);
    let element_ty = crate::insert::type_of(&mut ed.unit, ValueRepr::Aggregate, c.element);
    ed.unit.addresses.push(AddressInfo {
        source_place: ed.unit.addresses[a.index()].source_place,
        unchecked: ed.unit.addresses[a.index()]
            .unchecked
            .iter()
            .copied()
            .filter(|i| *i as usize <= position)
            .collect(),
        base: AddressBase::Slot(c.root),
        projection: PoolRange::push(&mut ed.unit.pools.projections, element_path.drain(..)),
        ty: element_ty,
    });
    let failure = match ed.code[b][i + 1] {
        Inst::Address { failure, .. } => failure,
        _ => unreachable!(),
    };
    // An Address has one failure edge use: copy it for the added preparation.
    let f = ed.unit.tables.failures[failure.index()].clone();
    let e = ed.unit.tables.edges[f.cleanup.index()].clone();
    let args = e.arguments.get(&ed.unit.pools.values).unwrap().to_vec();
    let cleanup = ed.edge(e.target, &args);
    let failure_id = FailureId(ed.unit.tables.failures.len() as u32);
    ed.unit.tables.failures.push(FailureEdge { cleanup, ..f });
    let next = ed.value(ty, Holding::Plain);
    let version = ed.value(ty, Holding::Plain);
    ed.code[b].splice(
        i + 2..i + 2,
        [
            Inst::Address {
                address: element_at,
                failure: failure_id,
            },
            Inst::FlatNext {
                result: next,
                address: element_at,
            },
        ],
    );
    ed.code[c.latch].push(Inst::Store {
        address: at,
        value: next,
        version,
    });
    let e = p.feedback;
    let mut args = ed.unit.tables.edges[e.index()]
        .arguments
        .get(&ed.unit.pools.values)
        .unwrap()
        .to_vec();
    args.push(version);
    ed.unit.tables.edges[e.index()].arguments = PoolRange::push(&mut ed.unit.pools.values, args);
}
/// Run the three phases; disabled means an exact unchanged unit.
pub fn rewrite(u: LirUnit<Value>, table: &LayoutTable, options: &Options) -> LirUnit<Value> {
    if !options.enabled
        || !u.addresses.iter().any(|a| match a.base {
            AddressBase::Slot(s) => variable_sequence(
                table,
                u.types[u.slots[s.index()].ty.index()].layout,
                a.projection.get(&u.pools.projections).unwrap_or(&[]),
            )
            .is_some(),
            _ => false,
        })
    {
        return u;
    }
    let graph = Graph::new(&u);
    let values = definitions(&u);
    let facts = Facts::new(&u, table);
    let digest = u.digest();
    let plans: Vec<_> = discover_with(&u, table, &graph, &values)
        .into_iter()
        .filter_map(|c| prove_with(&u, c, &graph, &values, &facts, digest))
        .collect();
    if plans.is_empty() {
        return u;
    }
    let mut ed = Editor::new(u);
    for plan in plans {
        commit_into(&mut ed, plan);
    }
    ed.finish()
}
