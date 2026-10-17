//! Copies used only through flat-aware reads can retain their source encoding.
use super::*;

impl Importer<'_, '_> {
    /// A stable source and a nested, read-only receiver see identical bytes.
    /// Record copies still materialize: whole-record calls need an ordinary block.
    pub(super) fn flat_copies(&mut self) -> R<()> {
        use tir::InstructionKind as K;
        let body = self.body;
        let (initializations, disqualified) = view_assignments(body);
        let stable = stable_owned_roots(body, &initializations, &disqualified);
        let scopes = scope_intervals(body);
        let ordinary = ordinary_sequence_uses(body);
        let mut assigned: Vec<Vec<Option<tir::PlaceId>>> = vec![Vec::new(); body.locals.len()];
        for i in body.blocks.iter().flat_map(|b| &b.instructions) {
            if let K::Assign { destination, value } = &i.kind {
                let d = &body.places[destination.0 as usize];
                assigned[d.root.0 as usize].push(match &body.rvalues[value.0 as usize] {
                    tir::Rvalue::Use(o) if o.usage == UseKind::Copy && d.projection.is_empty() => {
                        Some(o.place)
                    }
                    _ => None,
                });
            }
        }
        for (l, sources) in assigned.iter().enumerate() {
            let [Some(source)] = sources.as_slice() else {
                continue;
            };
            if disqualified[l] || !view_storage(body, l) || self.aliases[l].is_some() {
                continue;
            }
            let p = self.place(*source)?;
            let Some((source_ty, _)) = p.source else {
                continue;
            };
            if matches!(self.record(source_ty).kind, LayoutKind::Block { payload }
                if self.layouts.record(payload).kind == LayoutKind::Flat)
            {
                continue;
            }
            let source_place = &body.places[source.0 as usize];
            let root = source_place.root.0 as usize;
            let outer = scopes[body.locals[root].scope.0 as usize];
            let inner = scopes[body.locals[l].scope.0 as usize];
            let owned = stable[root]
                && self.local_slot[root] == Some(p.slot)
                && self.aliases[root].is_none()
                && outer.0 <= inner.0
                && inner.0 < outer.1
                && !source_place
                    .projection
                    .iter()
                    .any(|p| matches!(p, tir::Projection::Index(_)))
                && (!ordinary[l]
                    || matches!(self.record(source_ty).kind, LayoutKind::Slice { .. }));
            if self.borrowed_storage(p.slot) || owned {
                self.aliases[l] = Some(*source);
            }
        }
        Ok(())
    }
}

/// A static assignment in a cycle
/// is not one initialization per frame; a root eligible for a view is never
/// written, moved or mutably lent after its one whole initialization.
fn stable_owned_roots(
    body: &tir::Body,
    assigned: &[Vec<Option<tir::PlaceId>>],
    disqualified: &[bool],
) -> Vec<bool> {
    let adjacent: Vec<Vec<usize>> = body
        .blocks
        .iter()
        .map(|b| {
            body.successors(&b.terminator)
                .iter()
                .map(|b| b.0 as usize)
                .collect()
        })
        .collect();
    let mut cyclic = vec![false; body.blocks.len()];
    for component in hirsel_graph::components(&adjacent) {
        if component.len() > 1 || adjacent[component[0]].contains(&component[0]) {
            for b in component {
                cyclic[b] = true;
            }
        }
    }
    let mut stable = vec![false; body.locals.len()];
    for (b, block) in body.blocks.iter().enumerate() {
        for i in &block.instructions {
            if let tir::InstructionKind::Assign { destination, .. } = i.kind {
                let place = &body.places[destination.0 as usize];
                let l = place.root.0 as usize;
                stable[l] = view_storage(body, l)
                    && assigned[l].len() == 1
                    && !disqualified[l]
                    && place.projection.is_empty()
                    && !cyclic[b];
            }
        }
    }
    stable
}

/// Same iterative interval construction as GenericDeclarations::lexical; TIR
/// validation has already checked this scope forest. Queries are constant time.
fn scope_intervals(body: &tir::Body) -> Vec<(usize, usize)> {
    let mut children = vec![Vec::new(); body.scopes.len()];
    let mut pending = Vec::new();
    for (i, scope) in body.scopes.iter().enumerate() {
        if let Some(parent) = scope.parent {
            children[parent.0 as usize].push(i);
        } else {
            pending.push((i, false));
        }
    }
    let mut intervals = vec![(0, 0); body.scopes.len()];
    let mut clock = 0;
    while let Some((i, exit)) = pending.pop() {
        if exit {
            intervals[i].1 = clock;
            continue;
        }
        intervals[i].0 = clock;
        clock += 1;
        pending.push((i, true));
        pending.extend(children[i].iter().rev().map(|i| (*i, false)));
    }
    intervals
}

/// A flat List has no ordinary block to pass as a whole value. Only its length
/// and projected reads are currently flat-aware. Slice values have FlatView.
fn ordinary_sequence_uses(body: &tir::Body) -> Vec<bool> {
    let mut ordinary = vec![false; body.locals.len()];
    let mark = |op: &tir::Operand, ordinary: &mut Vec<bool>| {
        let p = &body.places[op.place.0 as usize];
        if p.projection.is_empty() {
            ordinary[p.root.0 as usize] = true;
        }
    };
    for rv in &body.rvalues {
        for op in rvalue_operands(rv) {
            mark(op, &mut ordinary);
        }
    }
    for block in &body.blocks {
        match &block.terminator {
            tir::Terminator::Call {
                callee, arguments, ..
            } => {
                let length = matches!(callee, tir::Callee::Definition { path, .. }
                    if core_operation(path, "list", "len"));
                if !length {
                    for a in arguments {
                        mark(&a.value, &mut ordinary);
                    }
                }
            }
            tir::Terminator::Return { value, .. } => mark(value, &mut ordinary),
            _ => {}
        }
    }
    for table in &body.matches {
        let p = &body.places[table.scrutinee.0 as usize];
        if p.projection.is_empty() {
            ordinary[p.root.0 as usize] = true;
        }
    }
    ordinary
}
