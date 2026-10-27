//! Invocation-bounded nonretention, reconstructed from physical flow after every pass.
//!
//! Uses the finite union/worklist structure of aliasing.rs at 6079c0d2:69,
//! independently of both code-target discovery and credit/loan answers. Ordered
//! capture propagation follows Goose 8511ae34 src/codegen_frames.h:115,210;
//! unlike Goose, erased/selected targets and owning callable copies remain legal.
//! Stage 1 retains heap backing; these facts never license uncounted placement.
use super::structure::{self, Facts, Root, Step};
use super::{Cx, Report, Site, ViolationKind as K};
use crate::abi_types::ParameterPath;
use crate::ir::*;
use crate::stage::Stage;
use hirsel_core_contract::Passing;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Default, PartialEq, Eq)]
struct Origins {
    scoped: BTreeSet<ValueId>,
    environments: BTreeSet<ValueId>,
    referents: BTreeSet<LoanId>,
}
impl Origins {
    fn extend(&mut self, other: &Self) {
        self.scoped.extend(&other.scoped);
        self.environments.extend(&other.environments);
        self.referents.extend(&other.referents);
    }
    fn insert(&mut self, value: ValueId) {
        self.scoped.insert(value);
    }
    fn len(&self) -> usize {
        self.scoped.len() + self.environments.len() + self.referents.len()
    }
}
impl<'a> IntoIterator for &'a Origins {
    type Item = &'a ValueId;
    type IntoIter = std::collections::btree_set::Iter<'a, ValueId>;
    fn into_iter(self) -> Self::IntoIter {
        self.scoped.iter()
    }
}
type Paths = BTreeMap<Vec<Step>, Origins>;

fn union(into: &mut Paths, from: &Paths) -> bool {
    let mut changed = false;
    for (path, roots) in from {
        let row = into.entry(path.clone()).or_default();
        let before = row.len();
        row.extend(roots);
        changed |= before != row.len();
    }
    changed
}
fn project(from: &Paths, path: &[Step]) -> Paths {
    let mut out = Paths::new();
    for (held, roots) in from {
        if held.starts_with(path) {
            out.entry(held[path.len()..].to_vec())
                .or_default()
                .extend(roots);
        } else if path.starts_with(held) || structure::overlap(held, path) {
            // An opaque carrier or a dynamically indexed projection cannot
            // erase its restriction, even when the result claims Owned.
            out.entry(Vec::new()).or_default().extend(roots);
        }
    }
    out
}
fn embed(from: &Paths, path: &[Step]) -> Paths {
    from.iter()
        .map(|(p, roots)| {
            let mut at = path.to_vec();
            at.extend(p);
            (at, roots.clone())
        })
        .collect()
}
fn fields(path: &ParameterPath) -> Vec<Step> {
    path.fields.iter().copied().map(Step::Field).collect()
}

/// All normal and instruction-failure predecessors, including partial writes.
/// Sets name actual reaching versions; a slot name is never itself provenance.
fn reaching<S: Stage>(cx: &Cx<'_, S>, facts: &Facts) -> BTreeMap<Site, Vec<BTreeSet<ValueId>>> {
    let u = cx.unit;
    let mut entry = vec![BTreeSet::new(); u.slots.len()];
    for &v in u.parameters.iter().chain(&u.captures) {
        if let Some(slot) = facts.version[v.index()] {
            entry[slot.index()].insert(v);
        }
    }
    let mut before = BTreeMap::new();
    super::flow::solve(
        &facts.cfg,
        entry,
        |block, initial| {
            let mut state = initial.clone();
            let mut edges = Vec::new();
            for (i, inst) in u.blocks[block.index()]
                .instructions
                .get(&u.instructions)
                .unwrap_or(&[])
                .iter()
                .enumerate()
            {
                before
                    .entry(Site::at(block, i as u32))
                    .and_modify(|old: &mut Vec<BTreeSet<ValueId>>| {
                        for (a, b) in old.iter_mut().zip(&state) {
                            a.extend(b);
                        }
                    })
                    .or_insert_with(|| state.clone());
                // Store cannot fail. Replace's failure retains the incoming storage;
                // its success alone defines the new version and displaced content.
                if let Some(f) = inst.failure() {
                    edges.push((u.tables.failures[f.index()].cleanup, state.clone()));
                }
                for v in inst.defs() {
                    if let Some(slot) = facts.version[v.index()] {
                        state[slot.index()] = BTreeSet::from([v]);
                    }
                }
            }
            before
                .entry(Site::block(block))
                .and_modify(|old| {
                    for (a, b) in old.iter_mut().zip(&state) {
                        a.extend(b);
                    }
                })
                .or_insert_with(|| state.clone());
            let rejected = match u.terminators[block.index()] {
                Terminator::Call { call, .. } => u.tables.calls[call.index()]
                    .pretransfer_failure
                    .map(|f| u.tables.failures[f.index()].cleanup),
                _ => None,
            };
            let pending = rejected.map(|_| state.clone());
            if let Terminator::Call { call, .. } = u.terminators[block.index()] {
                for argument in u.tables.calls[call.index()]
                    .arguments
                    .get(&u.pools.call_arguments)
                    .unwrap_or(&[])
                {
                    if let ArgumentValue::Address { address, version } = argument.value {
                        if let Some(a) = &facts.addresses[address.index()] {
                            if let Root::Slot(slot) = a.root {
                                state[slot.index()] = BTreeSet::from([version]);
                            }
                        }
                    }
                }
            }
            for out in &facts.cfg.succ[block.index()] {
                if out.origin == super::flow::Origin::Terminator {
                    let outgoing = if rejected == Some(out.edge) {
                        pending.as_ref().expect("rejected call state")
                    } else {
                        &state
                    };
                    edges.push((out.edge, outgoing.clone()));
                }
            }
            edges
        },
        |a, b| {
            for (a, b) in a.iter_mut().zip(b) {
                a.extend(b);
            }
        },
    );
    before
}

/// Data entry claims are new storage-safety obligations. Ordinary validation
/// must prove them too; legacy callable qualification remains opt-in.
pub(super) fn has_data_contract<S: Stage>(cx: &Cx<'_, S>) -> bool {
    let abi = &cx.layouts.0.fn_abis[cx.unit.header.abi.0 as usize];
    [
        (&abi.nonretaining, &cx.unit.parameters),
        (&abi.nonretaining_captures, &cx.unit.captures),
    ]
    .into_iter()
    .any(|(paths, values)| {
        paths.iter().any(|p| {
            let selected = values
                .get(p.parameter as usize)
                .map(|v| cx.value_layout(*v))
                .and_then(|layout| {
                    p.fields.iter().try_fold(layout, |layout, field| {
                        cx.layouts
                            .get(layout)?
                            .fields
                            .iter()
                            .find(|f| f.ordinal == *field)
                            .map(|f| f.layout)
                    })
                });
            !selected.is_some_and(|layout| {
                matches!(
                    cx.layouts.get(layout).map(|r| &r.kind),
                    Some(crate::layout_types::LayoutKind::Callable { .. })
                )
            })
        })
    })
}

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, facts: &Facts, report: &mut Report) {
    let u = cx.unit;
    let abi = &cx.layouts.0.fn_abis[u.header.abi.0 as usize];
    let mut n = vec![Paths::new(); u.values.len()];
    let mut callable_origins = BTreeSet::new();
    for (paths, values) in [
        (&abi.nonretaining, &u.parameters),
        (&abi.nonretaining_captures, &u.captures),
    ] {
        for p in paths {
            if let Some(&v) = values.get(p.parameter as usize) {
                let mut layout = u.types[u.values[v.index()].ty.index()].layout;
                let valid = p.fields.iter().all(|field| {
                    let Some(next) = cx
                        .layouts
                        .0
                        .layouts
                        .get(layout.0 as usize)
                        .and_then(|r| r.fields.iter().find(|f| f.ordinal == *field))
                    else {
                        return false;
                    };
                    layout = next.layout;
                    true
                }) && cx.layouts.0.layouts.get(layout.0 as usize).is_some();
                if valid
                    && matches!(
                        cx.layouts.0.layouts[layout.0 as usize].kind,
                        crate::layout_types::LayoutKind::Callable { .. }
                    )
                {
                    callable_origins.insert(v);
                }
                if !valid {
                    report.add(
                        Site::UNIT,
                        K::Type("scoped contract capture/parameter path"),
                    );
                }
                if !(cx.repr(v) == ValueRepr::Unit
                    || matches!(cx.repr(v), ValueRepr::Scalar(k)
                    if k != crate::layout_types::ScalarKind::Pointer))
                {
                    n[v.index()].entry(fields(p)).or_default().insert(v);
                }
            } else {
                report.add(Site::UNIT, K::Type("scoped contract ordinal"));
            }
        }
    }
    for inst in &u.instructions {
        match *inst {
            Inst::Closure {
                result, closure, ..
            } if u.tables.closures[closure.index()].scoped => {
                callable_origins.insert(result);
                n[result.index()]
                    .entry(Vec::new())
                    .or_default()
                    .insert(result);
            }
            Inst::BorrowSealedRoot {
                result,
                owner,
                loan,
                ..
            } if !matches!(cx.repr(result), ValueRepr::Unit | ValueRepr::Scalar(_)) => {
                // Sealed views are scoped even without a callable contract
                // elsewhere in the unit; seed them before the empty fast path.
                // Unit/scalar reads copy bits instead of retaining owner storage;
                // ownership/sealed validation still checks their exclusive loan.
                if cx.repr(result) == ValueRepr::Callable {
                    callable_origins.insert(owner);
                }
                let roots = n[result.index()].entry(Vec::new()).or_default();
                roots.scoped.insert(owner);
                roots.referents.insert(loan);
            }
            _ => {}
        }
    }
    if n.iter().all(|p| p.values().all(|r| r.scoped.is_empty())) {
        return;
    }
    for inst in &u.instructions {
        if let Inst::Closure { result, .. } = *inst {
            n[result.index()]
                .entry(Vec::new())
                .or_default()
                .environments
                .insert(result);
        }
    }
    for (i, loans) in facts
        .borrowed_holders
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
    {
        n[i].entry(Vec::new()).or_default().referents.extend(loans);
    }
    let before = reaching(cx, facts);
    // The finite selector vocabulary consists of paths the physical module
    // can actually observe. All other content collapses to its whole carrier,
    // retaining every root rather than extending paths around a cycle forever.
    let mut observed: Vec<Vec<Step>> = facts
        .addresses
        .iter()
        .flatten()
        .map(|a| a.path.clone())
        .collect();
    observed.extend(
        abi.nonretaining
            .iter()
            .chain(&abi.nonretaining_captures)
            .map(fields),
    );
    loop {
        let old = n.clone();
        for (b, block) in u.blocks.iter().enumerate() {
            for (i, inst) in block
                .instructions
                .get(&u.instructions)
                .unwrap_or(&[])
                .iter()
                .enumerate()
            {
                let at = Site::at(BlockId(b as u32), i as u32);
                match *inst {
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
                    } => {
                        let path = &facts.addresses[address.index()].as_ref().unwrap().path;
                        let mut projected = project(&old[reaching.index()], path);
                        if cx.repr(result) == ValueRepr::Unit
                            || matches!(cx.repr(result), ValueRepr::Scalar(k)
                                if k != crate::layout_types::ScalarKind::Pointer)
                        {
                            for origins in projected.values_mut() {
                                origins.scoped.retain(|v| callable_origins.contains(v));
                            }
                        }
                        union(&mut n[result.index()], &projected);
                    }
                    Inst::Weaken { result, value }
                    | Inst::Recast { result, value }
                    | Inst::UniqueCopy { result, value, .. }
                    | Inst::SliceView { result, value, .. }
                    | Inst::Freeze { result, value }
                    | Inst::Thaw { result, value } => {
                        union(&mut n[result.index()], &old[value.index()]);
                    }
                    Inst::Store {
                        address,
                        value,
                        version,
                    }
                    | Inst::Replace {
                        address,
                        value,
                        version,
                        ..
                    } => {
                        let a = facts.addresses[address.index()].as_ref().unwrap();
                        union(
                            &mut n[version.index()],
                            &embed(&old[value.index()], &a.path),
                        );
                        if let Root::Slot(slot) = a.root {
                            for previous in &before[&at][slot.index()] {
                                if !a.path.is_empty() {
                                    let retained = old[previous.index()]
                                        .iter()
                                        .filter(|(p, _)| !p.starts_with(&a.path))
                                        .map(|(p, r)| (p.clone(), r.clone()))
                                        .collect();
                                    union(&mut n[version.index()], &retained);
                                }
                                if let Inst::Replace {
                                    displaced_version, ..
                                } = *inst
                                {
                                    union(
                                        &mut n[displaced_version.index()],
                                        &project(&old[previous.index()], &a.path),
                                    );
                                }
                            }
                        }
                    }
                    Inst::Construct {
                        result, construct, ..
                    }
                    | Inst::Reuse {
                        result, construct, ..
                    } => {
                        for f in u.tables.constructs[construct.index()]
                            .fields
                            .get(&u.pools.fields)
                            .unwrap_or(&[])
                        {
                            union(
                                &mut n[result.index()],
                                &embed(&old[f.value.index()], &[Step::Field(f.field)]),
                            );
                        }
                    }
                    Inst::Closure {
                        result, closure, ..
                    } => {
                        for c in u.tables.closures[closure.index()]
                            .captures
                            .get(&u.pools.captures)
                            .unwrap_or(&[])
                        {
                            // Reaching into a child environment retains the parent's
                            // restrictions, irrespective of the child's code target.
                            let all = old[c.value.index()].values().fold(
                                Origins::default(),
                                |mut all, r| {
                                    all.extend(r);
                                    all
                                },
                            );
                            union(&mut n[result.index()], &BTreeMap::from([(Vec::new(), all)]));
                        }
                    }
                    // These instructions have no value-preserving result. Their
                    // scoped operands are checked below; no producer is ignored.
                    _ => {}
                }
            }
        }
        // A mut call can leave the original caller-owned value in place.
        // Its write-back SSA name must not erase that incoming restriction,
        // even when forwarding itself obeys a nonretaining contract.
        for (block, terminator) in u.terminators.iter().enumerate() {
            let Terminator::Call { call, .. } = *terminator else {
                continue;
            };
            let at = Site::block(BlockId(block as u32));
            for argument in u.tables.calls[call.index()]
                .arguments
                .get(&u.pools.call_arguments)
                .unwrap_or(&[])
            {
                if let ArgumentValue::Address { address, version } = argument.value {
                    let address = facts.addresses[address.index()].as_ref().unwrap();
                    if let Root::Slot(slot) = address.root {
                        for previous in &before[&at][slot.index()] {
                            union(&mut n[version.index()], &old[previous.index()]);
                        }
                    }
                }
            }
        }
        for edge in &u.tables.edges {
            for (&a, &p) in edge
                .arguments
                .get(&u.pools.values)
                .unwrap_or(&[])
                .iter()
                .zip(
                    u.blocks[edge.target.index()]
                        .parameters
                        .get(&u.pools.values)
                        .unwrap_or(&[]),
                )
            {
                union(&mut n[p.index()], &old[a.index()]);
            }
        }
        for paths in &mut n {
            let unknown: Vec<_> = paths
                .keys()
                .filter(|p| !p.is_empty() && !observed.iter().any(|q| q.starts_with(p)))
                .cloned()
                .collect();
            for p in unknown {
                let roots = paths.remove(&p).unwrap();
                paths.entry(Vec::new()).or_default().extend(&roots);
            }
        }
        if n == old {
            break;
        }
    }
    let reject = |report: &mut Report, at, v: ValueId, operation| {
        for &origin in n[v.index()].values().flatten() {
            report.add(
                at,
                K::ScopedRetention {
                    value: v,
                    origin,
                    operation,
                },
            );
        }
    };
    for (b, block) in u.blocks.iter().enumerate() {
        for (i, inst) in block
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let at = Site::at(BlockId(b as u32), i as u32);
            match *inst {
                Inst::BorrowSealedRoot { .. }
                | Inst::ArenaInventory { .. }
                | Inst::Read { .. }
                | Inst::Weaken { .. }
                | Inst::Recast { .. }
                | Inst::Address { .. }
                | Inst::BeginLoan { .. }
                | Inst::EndLoan { .. }
                | Inst::StorageLive { .. }
                | Inst::StorageDead { .. }
                | Inst::EndScope { .. }
                | Inst::BindOrigin { .. }
                | Inst::BindFields { .. }
                | Inst::EndOrigin { .. }
                | Inst::Drop { .. }
                | Inst::Release { .. } => {}
                Inst::SeqLen { value, .. } => {
                    // The length reads storage without retaining it. Callable
                    // origins cannot be reinterpreted as data observations.
                    if n[value.index()]
                        .values()
                        .flat_map(|r| &r.scoped)
                        .any(|v| callable_origins.contains(v))
                    {
                        reject(report, at, value, "callable used as data");
                    }
                }
                Inst::Store { address, value, .. } | Inst::Replace { address, value, .. } => {
                    let a = facts.addresses[address.index()].as_ref().unwrap();
                    let local = matches!(a.root, Root::Slot(s) if !matches!(u.slots[s.index()].kind, SlotKind::Parameter(_) | SlotKind::Capture(_)))
                        && a.crossings.is_empty();
                    if !local {
                        reject(report, at, value, "persistent or mutable hand-back store");
                    }
                }
                Inst::Construct {
                    result, construct, ..
                }
                | Inst::Reuse {
                    result, construct, ..
                } => {
                    let c = &u.tables.constructs[construct.index()];
                    let inline = u.types[u.values[result.index()].ty.index()].repr
                        == ValueRepr::Aggregate
                        && matches!(
                            cx.layouts.get(c.layout).map(|r| &r.kind),
                            Some(
                                crate::layout_types::LayoutKind::Aggregate
                                    | crate::layout_types::LayoutKind::Environment { .. }
                                    | crate::layout_types::LayoutKind::Variant
                            )
                        );
                    if !inline {
                        for v in structure::construct_values(cx, construct) {
                            reject(report, at, v, "owning carrier construction");
                        }
                    }
                }
                Inst::Closure { closure, .. } => {
                    let c = &u.tables.closures[closure.index()];
                    for (ordinal, capture) in c
                        .captures
                        .get(&u.pools.captures)
                        .unwrap_or(&[])
                        .iter()
                        .enumerate()
                    {
                        if c.scoped && matches!(capture.usage, CaptureUse::Borrow | CaptureUse::Mut)
                        {
                            for (path, roots) in &n[capture.value.index()] {
                                let covered = c.code.is_some_and(|code| {
                                    cx.layouts.0.fn_abis[code.abi.0 as usize]
                                        .nonretaining_captures
                                        .iter()
                                        .any(|p| {
                                            p.parameter == ordinal as u32
                                                && path.starts_with(&fields(p))
                                        })
                                });
                                if !covered && !roots.scoped.is_empty() {
                                    reject(
                                        report,
                                        at,
                                        capture.value,
                                        "nested capture lacks contract",
                                    );
                                }
                            }
                        }
                        if !c.scoped
                            || !matches!(capture.usage, CaptureUse::Borrow | CaptureUse::Mut)
                        {
                            reject(report, at, capture.value, "owning capture");
                        }
                    }
                }
                // Complete operand enumeration is shared with structural C. A
                // new instruction with a scoped user fails closed here until
                // a preservation/sink rule is added above.
                _ => {
                    for (v, _) in structure::inst_operands(cx, inst) {
                        reject(report, at, v, "unqualified scoped operation");
                    }
                    for address in structure::inst_addresses(cx, inst) {
                        let a = facts.addresses[address.index()].as_ref().unwrap();
                        match a.root {
                            Root::Value(v) => {
                                reject(report, at, v, "unqualified scoped address operation")
                            }
                            Root::Slot(slot) => {
                                for &v in &before[&at][slot.index()] {
                                    reject(report, at, v, "unqualified scoped address operation");
                                }
                            }
                        }
                    }
                }
            }
        }
        let at = Site::block(BlockId(b as u32));
        match u.terminators[b] {
            Terminator::Return(Some(v)) => reject(report, at, v, "return or error payload"),
            Terminator::Call { call, .. } => {
                let call = &u.tables.calls[call.index()];
                let callee = &cx.layouts.0.fn_abis[call.abi.0 as usize];
                let environment = call.callee.value();
                if let Some(v) = environment {
                    if n[v.index()]
                        .values()
                        .flat_map(|r| &r.scoped)
                        .any(|origin| !callable_origins.contains(origin))
                    {
                        reject(
                            report,
                            at,
                            v,
                            "data-root callable environment lacks contract",
                        );
                    }
                }
                for a in call.arguments.get(&u.pools.call_arguments).unwrap_or(&[]) {
                    match a.value {
                        ArgumentValue::Value(v) => {
                            for (path, roots) in &n[v.index()] {
                                let covered = a.passing == Passing::Borrow
                                    && !matches!(call.callee, Callee::Operation(_))
                                    && callee.nonretaining.iter().any(|p| {
                                        p.parameter == a.parameter && path.starts_with(&fields(p))
                                    });
                                if !covered {
                                    for &origin in roots {
                                        report.add(
                                            at,
                                            K::ScopedRetention {
                                                value: v,
                                                origin,
                                                operation: "may-retain call argument",
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        ArgumentValue::Address { address, .. } => {
                            let address = facts.addresses[address.index()].as_ref().unwrap();
                            if let Root::Slot(slot) = address.root {
                                for &v in &before[&at][slot.index()] {
                                    for (path, roots) in project(&n[v.index()], &address.path) {
                                        let covered = a.passing == Passing::Mut
                                            && !matches!(call.callee, Callee::Operation(_))
                                            && callee.nonretaining.iter().any(|p| {
                                                p.parameter == a.parameter
                                                    && path.starts_with(&fields(p))
                                            });
                                        if !covered {
                                            for origin in roots.scoped {
                                                report.add(
                                                    at,
                                                    K::ScopedRetention {
                                                        value: v,
                                                        origin,
                                                        operation: "mutable call hand-back",
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Link every contract claim to the actual physical definition, including
/// recursive groups. Callers must also run per-unit C on every member.
pub fn check_calls<S: Stage>(
    units: &[LirUnit<S>],
    layouts: &crate::LayoutTable,
) -> Result<(), String> {
    let identities = entry_identities(layouts);
    check_links(units, layouts, Some(&identities))
}
/// Link consistency for ordinary heap code. Missing bodies supply no scoped
/// proof, but retain existing foreign/bodyless acceptance. Qualification uses
/// `check_calls` and requires each contracted body.
pub fn check_heap_links<S: Stage>(
    units: &[LirUnit<S>],
    layouts: &crate::LayoutTable,
) -> Result<(), String> {
    check_links(units, layouts, None)
}
fn check_links<S: Stage>(
    units: &[LirUnit<S>],
    layouts: &crate::LayoutTable,
    identities: Option<&[hirsel_syntax::ContentHash]>,
) -> Result<(), String> {
    let require_bodies = identities.is_some();
    let targets: BTreeMap<_, _> = units
        .iter()
        .map(|u| ((&u.header.definition, u.header.instance), u))
        .collect();
    if targets.len() != units.len() {
        return Err("duplicate scoped body identity".into());
    }
    for u in units {
        let entries =
            u.tables
                .calls
                .iter()
                .filter_map(|call| match call.callee {
                    Callee::Direct(s) | Callee::Bound { code: s, .. } => Some((s, call.abi)),
                    _ => None,
                })
                .chain(u.tables.closures.iter().filter_map(|closure| {
                    closure.code.as_ref().map(|code| (code.symbol, code.abi))
                }));
        for (symbol, claimed) in entries {
            let abi = layouts
                .fn_abis
                .get(claimed.0 as usize)
                .ok_or("scoped entry ABI absent")?;
            let symbol = u
                .tables
                .symbols
                .get(symbol.index())
                .ok_or("scoped symbol absent")?;
            let target = symbol
                .instance
                .and_then(|i| targets.get(&(&symbol.definition, i)));
            let Some(target) = target else {
                if !require_bodies && !has_data_paths(abi, layouts)
                    || abi.nonretaining.is_empty()
                        && abi.nonretaining_captures.is_empty()
                        && abi.scoped_entries.is_empty()
                {
                    continue;
                }
                return Err("scoped contracted body absent".into());
            };
            let actual = layouts
                .fn_abis
                .get(target.header.abi.0 as usize)
                .ok_or("scoped target ABI absent")?;
            if identities
                .is_some_and(|ids| ids[claimed.0 as usize] != ids[target.header.abi.0 as usize])
                || abi.nonretaining != actual.nonretaining
                || abi.nonretaining_captures != actual.nonretaining_captures
                || abi.scoped_contract != actual.scoped_contract
                || abi.scoped_entries != actual.scoped_entries
            {
                return Err(format!(
                    "call to {} mismatches scoped body contract",
                    symbol.definition.0.name
                ));
            }
        }
    }
    super::linkage::check_data_calls(units, layouts)?;
    Ok(())
}

pub(super) fn has_data_paths(abi: &crate::abi_types::FnAbi, table: &crate::LayoutTable) -> bool {
    abi.nonretaining.iter().any(|p| {
        let layout = abi.parameters.get(p.parameter as usize).and_then(|a| {
            p.fields.iter().try_fold(a.layout, |layout, field| {
                table
                    .layouts
                    .get(layout.0 as usize)?
                    .fields
                    .iter()
                    .find(|f| f.ordinal == *field)
                    .map(|f| f.layout)
            })
        });
        layout.is_some_and(|layout| {
            !matches!(
                table.layouts[layout.0 as usize].kind,
                crate::layout_types::LayoutKind::Callable { .. }
            )
        })
    })
}

/// Check a complete physical module for future scoped-proof consumers.
///
/// Per-unit C validates symbolic nonretention. Complete C also reconstructs
/// nested callable entry identities and checks every higher-order actual and
/// indirect invocation. Missing identity, unknown data flow and unrepresented
/// hidden capture contracts refuse proof while preserving ordinary heap code.
/// Every scoped specialization must rerun this check after transformation.
fn entry_identities(table: &crate::LayoutTable) -> Vec<hirsel_syntax::ContentHash> {
    let keys = crate::LayoutKeys::new(table);
    // Derive each ABI signature once, shared by all callers and bodies.
    table
        .fn_abis
        .iter()
        .map(|a| crate::abi::scoped_entry_identity_with_keys(a, &keys))
        .collect()
}

/// Strict per-body proofs retained only inside one compiler invocation. An entry
/// signature alone never identifies a checked body: bind its exact bytes and
/// reached declaration/layout rows too. Linked actuals are always checked fresh.
pub struct ScopedQualification<'a> {
    target: &'a crate::TargetSpec,
    manifest: &'a hirsel_core_contract::Manifest,
    operations: &'a (dyn crate::operation::OperationDeclarations + Sync),
    discharge: Option<&'a crate::discharge::Discharge>,
    checked: BTreeSet<(
        hirsel_syntax::ContentHash,
        hirsel_syntax::ContentHash,
        hirsel_syntax::ContentHash,
    )>,
}
impl<'a> ScopedQualification<'a> {
    pub fn new(
        target: &'a crate::TargetSpec,
        manifest: &'a hirsel_core_contract::Manifest,
        operations: &'a (dyn crate::operation::OperationDeclarations + Sync),
        discharge: Option<&'a crate::discharge::Discharge>,
    ) -> Self {
        Self {
            target,
            manifest,
            operations,
            discharge,
            checked: BTreeSet::new(),
        }
    }
    pub fn qualify<S: Stage>(
        &mut self,
        units: &[LirUnit<S>],
        layouts: &crate::LayoutTable,
    ) -> Result<(), String> {
        let inputs = super::Inputs {
            target: self.target,
            layouts,
            manifest: self.manifest,
            operations: self.operations,
            discharge: self.discharge,
            memo: Default::default(),
        };
        let context = super::ValidationContext::new(&inputs);
        context
            .preflight()
            .map_err(|e| format!("scoped table fails C: {e:?}"))?;
        let identities = entry_identities(layouts);
        for unit in units {
            unit.check_identity(layouts)
                .map_err(|e| format!("scoped body schema: {e:?}"))?;
            // Opaque operation providers/discharge state are not memoized. The
            // ordinary full checker still authenticates every such body.
            let cacheable = unit.tables.operations.is_empty() && inputs.discharge.is_none();
            let key = if cacheable {
                let entry = *identities
                    .get(unit.header.abi.0 as usize)
                    .ok_or("scoped body ABI absent")?;
                let reached =
                    super::UnitInputs::from(&inputs).digest_with_index(unit, context.index())?;
                Some((entry, unit.digest(), reached))
            } else {
                None
            };
            if key.as_ref().is_some_and(|k| self.checked.contains(k)) {
                continue;
            }
            let violations = super::check_with_context(unit, &inputs, &context, true);
            if !violations.is_empty() {
                return Err(format!(
                    "{} fails C: {violations:?}",
                    unit.header.definition.0.name
                ));
            }
            if let Some(key) = key {
                self.checked.insert(key);
            }
        }
        check_links(units, layouts, Some(&identities))?;
        check_entries(units, layouts, &identities)
    }
}

pub fn qualify<S: Stage>(units: &[LirUnit<S>], inputs: &super::Inputs<'_>) -> Result<(), String> {
    ScopedQualification::new(
        inputs.target,
        inputs.manifest,
        inputs.operations,
        inputs.discharge,
    )
    .qualify(units, inputs.layouts)
}

/// Reconstruct entry signatures separately from optimizer code discovery. Unknown
/// storage or an unobserved predecessor poisons the answer. Equal signatures do
/// not identify capture data and never authorize an environment rewrite.
fn check_entries<S: Stage>(
    units: &[LirUnit<S>],
    table: &crate::LayoutTable,
    identities: &[hirsel_syntax::ContentHash],
) -> Result<(), String> {
    use hirsel_syntax::ContentHash;
    let bodies: BTreeMap<_, _> = units
        .iter()
        .map(|u| ((&u.header.definition, u.header.instance), u))
        .collect();
    #[derive(Clone, Default, PartialEq, Eq)]
    struct Entries {
        known: BTreeSet<ContentHash>,
        unknown: bool,
    }
    impl Entries {
        fn join(&mut self, other: &Self) -> bool {
            let before = self.clone();
            self.known.extend(&other.known);
            self.unknown |= other.unknown;
            *self != before
        }
        fn matches(&self, wanted: ContentHash) -> bool {
            !self.unknown && self.known == BTreeSet::from([wanted])
        }
    }
    for u in units {
        let own = &table.fn_abis[u.header.abi.0 as usize];
        // Most units have no callable-parameter entry obligation. No entry SSA
        // reconstruction is needed unless a nested actual or contracted indirect
        // invocation is checked. Ordinary full C above still checks the body.
        if own.scoped_entries.is_empty()
            && !u.tables.calls.iter().any(|c| {
                let abi = &table.fn_abis[c.abi.0 as usize];
                !abi.scoped_entries.is_empty()
                    || matches!(c.callee, Callee::Indirect(_))
                        && (!abi.nonretaining.is_empty() || !abi.nonretaining_captures.is_empty())
            })
        {
            continue;
        }
        let mut rows = vec![Entries::default(); u.values.len()];
        let mut inputs = vec![Vec::new(); u.values.len()];
        for &p in &u.parameters {
            rows[p.index()].unknown = true;
        }
        for entry in &own.scoped_entries {
            if !entry.parameter.fields.is_empty() {
                return Err("unsupported nested entry selector".into());
            }
            let Some(&p) = u.parameters.get(entry.parameter.parameter as usize) else {
                return Err("nested entry parameter absent".into());
            };
            rows[p.index()] = Entries {
                known: BTreeSet::from([entry.identity]),
                unknown: false,
            };
        }
        // Mutably exposed slots never supply an entry identity.
        fn whole<S>(u: &LirUnit<S>, a: AddressId) -> Option<SlotId> {
            let a = &u.addresses[a.index()];
            if a.projection.len != 0 {
                return None;
            }
            match a.base {
                AddressBase::Slot(s) => Some(s),
                AddressBase::Address(a) => whole(u, a),
                _ => None,
            }
        }
        let mutable: BTreeSet<_> = u
            .tables
            .loans
            .iter()
            .filter(|l| l.mutable)
            .filter_map(|l| whole(u, l.address))
            .collect();
        let safe = |a| whole(u, a).is_some_and(|s| !mutable.contains(&s));
        for edge in &u.tables.edges {
            for (&p, &a) in u.blocks[edge.target.index()]
                .parameters
                .get(&u.pools.values)
                .unwrap_or(&[])
                .iter()
                .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
            {
                inputs[p.index()].push(a);
            }
        }
        for inst in &u.instructions {
            let flow = match *inst {
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
                } if safe(address) => Some((result, reaching)),
                Inst::Store {
                    address,
                    value,
                    version,
                }
                | Inst::Replace {
                    address,
                    value,
                    version,
                    ..
                } if safe(address) => Some((version, value)),
                Inst::Weaken { result, value } => Some((result, value)),
                _ => None,
            };
            if let Some((a, b)) = flow {
                inputs[a.index()].push(b);
                continue;
            }
            let entry = match *inst {
                Inst::FunctionRef { result, symbol } => Some((result, symbol, None)),
                Inst::Closure {
                    result, closure, ..
                } => u.tables.closures[closure.index()]
                    .code
                    .map(|c| (result, c.symbol, Some(c.abi))),
                _ => None,
            };
            if let Some((v, s, claimed)) = entry {
                let symbol = &u.tables.symbols[s.index()];
                if let Some(body) = symbol
                    .instance
                    .and_then(|i| bodies.get(&(&symbol.definition, i)))
                {
                    let actual_identity = identities[body.header.abi.0 as usize];
                    if claimed.is_some_and(|a| identities[a.0 as usize] != actual_identity) {
                        return Err("closure nested entry identity mismatch".into());
                    }
                    rows[v.index()].known.insert(actual_identity);
                } else {
                    rows[v.index()].unknown = true;
                }
            } else {
                for d in inst.defs() {
                    rows[d.index()].unknown = true;
                }
            }
        }
        for (i, row) in rows.iter_mut().enumerate() {
            if inputs[i].is_empty() && row.known.is_empty() {
                row.unknown = true;
            }
        }
        loop {
            let mut changed = false;
            for (i, args) in inputs.iter().enumerate() {
                for a in args {
                    let other = rows[a.index()].clone();
                    changed |= rows[i].join(&other);
                }
            }
            if !changed {
                break;
            }
        }
        for call in &u.tables.calls {
            let abi = &table.fn_abis[call.abi.0 as usize];
            if let Callee::Indirect(v) = call.callee {
                // Parameter entry identity cannot authenticate hidden capture
                // selectors on an unknown environment.
                if !abi.nonretaining_captures.is_empty() {
                    return Err("indirect scoped capture contract is unavailable".into());
                }
                if (!abi.nonretaining.is_empty()
                    || !abi.nonretaining_captures.is_empty()
                    || !abi.scoped_entries.is_empty())
                    && !rows[v.index()].matches(identities[call.abi.0 as usize])
                {
                    return Err(format!(
                        "{}: indirect scoped entry contract is unavailable or mismatched",
                        u.header.definition.0.name
                    ));
                }
            }
            for entry in &abi.scoped_entries {
                if !entry.parameter.fields.is_empty() {
                    return Err("unsupported nested call selector".into());
                }
                let actual = call
                    .arguments
                    .get(&u.pools.call_arguments)
                    .unwrap_or(&[])
                    .iter()
                    .find(|a| a.parameter == entry.parameter.parameter);
                if !matches!(actual.map(|a|a.value), Some(ArgumentValue::Value(v)) if rows[v.index()].matches(entry.identity))
                {
                    return Err(format!(
                        "{}: nested callable entry identity is unavailable or mismatched",
                        u.header.definition.0.name
                    ));
                }
            }
        }
    }
    Ok(())
}
