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
                    edges.push((out.edge, state.clone()));
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

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, facts: &Facts, report: &mut Report) {
    let u = cx.unit;
    let abi = &cx.layouts.0.fn_abis[u.header.abi.0 as usize];
    let mut n = vec![Paths::new(); u.values.len()];
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
                }) && cx
                    .layouts
                    .0
                    .layouts
                    .get(layout.0 as usize)
                    .is_some_and(|r| {
                        matches!(r.kind, crate::layout_types::LayoutKind::Callable { .. })
                    });
                if !valid {
                    report.add(
                        Site::UNIT,
                        K::Type("scoped contract capture/parameter path"),
                    );
                }
                n[v.index()].entry(fields(p)).or_default().insert(v);
            } else {
                report.add(Site::UNIT, K::Type("scoped contract ordinal"));
            }
        }
    }
    for inst in &u.instructions {
        if let Inst::Closure {
            result, closure, ..
        } = *inst
        {
            if u.tables.closures[closure.index()].scoped {
                n[result.index()]
                    .entry(Vec::new())
                    .or_default()
                    .insert(result);
            }
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
                        union(
                            &mut n[result.index()],
                            &project(&old[reaching.index()], path),
                        );
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
                Inst::Read { .. }
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
pub fn check_calls<S>(units: &[LirUnit<S>], layouts: &crate::LayoutTable) -> Result<(), String> {
    check_links(units, layouts, true)
}
/// Link consistency for ordinary heap code. Missing bodies supply no scoped
/// proof, but retain existing foreign/bodyless acceptance. Qualification uses
/// `check_calls` and requires each contracted body.
pub fn check_heap_links<S>(
    units: &[LirUnit<S>],
    layouts: &crate::LayoutTable,
) -> Result<(), String> {
    check_links(units, layouts, false)
}
fn check_links<S>(
    units: &[LirUnit<S>],
    layouts: &crate::LayoutTable,
    require_bodies: bool,
) -> Result<(), String> {
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
                if !require_bodies
                    || abi.nonretaining.is_empty() && abi.nonretaining_captures.is_empty()
                {
                    continue;
                }
                return Err("scoped contracted body absent".into());
            };
            let actual = layouts
                .fn_abis
                .get(target.header.abi.0 as usize)
                .ok_or("scoped target ABI absent")?;
            if abi.nonretaining != actual.nonretaining
                || abi.nonretaining_captures != actual.nonretaining_captures
                || abi.scoped_contract != actual.scoped_contract
            {
                return Err(format!(
                    "call to {} mismatches scoped body contract",
                    symbol.definition.0.name
                ));
            }
        }
    }
    Ok(())
}

/// Check a complete physical module for future scoped-proof consumers.
///
/// Per-unit C validates symbolic contracts, but cannot authenticate an unknown
/// indirect entry's nested callable signature from its two-word layout. Until
/// that evidence is represented, such a boundary remains heap-backed and does
/// not qualify. This is deliberately separate from ordinary source/native
/// acceptance: unsupported proof must not become a language restriction.
/// No placement or code-generation path consumes this result in stage 1.
pub fn qualify<S: Stage>(units: &[LirUnit<S>], inputs: &super::Inputs<'_>) -> Result<(), String> {
    let context = super::ValidationContext::new(inputs);
    for unit in units {
        let violations = super::check_with_context(unit, inputs, &context, true);
        if !violations.is_empty() {
            return Err(format!(
                "{} fails C: {violations:?}",
                unit.header.definition.0.name
            ));
        }
    }
    check_calls(units, inputs.layouts)?;
    for unit in units {
        for call in &unit.tables.calls {
            let abi = &inputs.layouts.fn_abis[call.abi.0 as usize];
            if matches!(call.callee, Callee::Indirect(_))
                && (!abi.nonretaining.is_empty() || !abi.nonretaining_captures.is_empty())
            {
                return Err(format!(
                    "{}: indirect scoped entry contract is unavailable",
                    unit.header.definition.0.name
                ));
            }
        }
    }
    Ok(())
}
