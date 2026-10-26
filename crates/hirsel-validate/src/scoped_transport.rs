//! Independent source nonretention provenance. No inference receipts, code-target
//! summaries or physical ownership credits take part in this finite union proof.
use super::*;
use hirsel_types::{CaptureContext, PassingMode, SignatureKind, TypeKind};
use std::collections::{BTreeMap, BTreeSet};
type Paths = BTreeMap<Vec<u32>, BTreeSet<(u32, u32)>>;
type State = BTreeMap<usize, Paths>;
fn join_state(to: &mut State, from: &State) -> bool {
    let mut changed = false;
    for (local, paths) in from {
        changed |= join(to.entry(*local).or_default(), paths);
    }
    changed
}
fn join(to: &mut Paths, from: &Paths) -> bool {
    let mut changed = false;
    for (p, roots) in from {
        let row = to.entry(p.clone()).or_default();
        let size = row.len();
        row.extend(roots);
        changed |= row.len() != size;
    }
    changed
}
fn read(t: &TypeInterner, body: &Body, rows: &State, value: &Operand) -> Paths {
    let p = &body.places[value.place.0 as usize];
    if matches!(&t.row(p.ty).kind, TypeKind::Primitive(k)
        if !matches!(k, hirsel_types::Primitive::Bytes | hirsel_types::Primitive::String))
        || matches!(&t.row(p.ty).kind, TypeKind::Tuple(fields) if fields.is_empty())
    {
        return Paths::new();
    }
    let path: Option<Vec<u32>> = p
        .projection
        .iter()
        .map(|p| match p {
            Projection::Field(f) => Some(*f),
            _ => None,
        })
        .collect();
    let mut out = Paths::new();
    for (held, roots) in rows.get(&(p.root.0 as usize)).into_iter().flatten() {
        let at = match &path {
            Some(path) if held.starts_with(path) => held[path.len()..].to_vec(),
            Some(path) if path.starts_with(held) => Vec::new(),
            None => Vec::new(),
            _ => continue,
        };
        out.entry(at).or_default().extend(roots);
    }
    out
}
fn put(body: &Body, rows: &mut State, destination: PlaceId, input: &Paths) -> bool {
    let p = &body.places[destination.0 as usize];
    let path: Option<Vec<u32>> = p
        .projection
        .iter()
        .map(|p| match p {
            Projection::Field(f) => Some(*f),
            _ => None,
        })
        .collect();
    let mut input2 = Paths::new();
    for (q, r) in input {
        let mut at = path.clone().unwrap_or_default();
        at.extend(q);
        let known = at.is_empty()
            || body.places.iter().any(|p| {
                p.root == body.places[destination.0 as usize].root
                    && p.projection.len() >= at.len()
                    && p.projection
                        .iter()
                        .zip(&at)
                        .all(|(p, f)| matches!(p,Projection::Field(g) if g==f))
            });
        input2
            .entry(if known { at } else { Vec::new() })
            .or_default()
            .extend(r);
    }
    let input = input2;
    if input.is_empty() {
        return false;
    }
    join(rows.entry(p.root.0 as usize).or_default(), &input)
}
fn write(body: &Body, rows: &mut State, destination: PlaceId, input: &Paths) {
    let p = &body.places[destination.0 as usize];
    let path: Option<Vec<u32>> = p
        .projection
        .iter()
        .map(|p| match p {
            Projection::Field(f) => Some(*f),
            _ => None,
        })
        .collect();
    if let Some(path) = path {
        if let Some(row) = rows.get_mut(&(p.root.0 as usize)) {
            row.retain(|held, _| !held.starts_with(&path));
            if row.is_empty() {
                rows.remove(&(p.root.0 as usize));
            }
        }
    }
    put(body, rows, destination, input);
}
/// Source counterpart of nested entry selectors. Reconstruct from declared
/// callable parameter types, independently of physical ABI metadata and code
/// discovery. Generic/erased source evidence never invents a nested contract.
fn entry_contracts(t: &TypeInterner, ty: hirsel_types::TypeId) -> Option<Vec<Vec<u32>>> {
    let mut pending = vec![(ty, Vec::new())];
    let mut out = Vec::new();
    while let Some((ty, path)) = pending.pop() {
        let TypeKind::Function(f) = &t.get(ty)?.kind else {
            return None;
        };
        if !f.generics.is_empty() {
            return None;
        }
        for (ordinal, p) in f.parameters.iter().enumerate() {
            if matches!(t.get(p.ty).map(|r| &r.kind), Some(TypeKind::Function(_))) {
                let mut child = path.clone();
                child.push(ordinal as u32);
                if p.passing != PassingMode::Take {
                    out.push(child.clone());
                }
                pending.push((p.ty, child));
            }
        }
    }
    out.sort();
    Some(out)
}

pub(super) fn check(
    definition: &Definition,
    t: &TypeInterner,
    signatures: &Reader<'_>,
) -> Vec<Violation> {
    // Most definitions have no scoped origins. Avoid retaining per-instruction
    // state for these definitions (notably the standard-library image).
    if definition.bodies.iter().all(|body| {
        body.scoped_parameters.is_empty()
            && !body.rvalues.iter().any(|r| {
                matches!(
                    r,
                    Rvalue::Closure {
                        context: CaptureContext::Scoped,
                        ..
                    }
                )
            })
    }) {
        return Vec::new();
    }
    let mut functions = vec![None; definition.bodies.len()];
    if let Some(signature) = signatures.get(definition.path) {
        if let SignatureKind::Function(f) = &signature.kind {
            functions[definition.entry.0 as usize] = Some(f.clone());
        }
    }
    for body in &definition.bodies {
        for inst in body.blocks.iter().flat_map(|b| &b.instructions) {
            if let InstructionKind::Assign { destination, value } = inst.kind {
                if let Rvalue::Closure { body: child, .. } = body.rvalues[value.0 as usize] {
                    if let TypeKind::Function(f) =
                        &t.get(body.places[destination.0 as usize].ty).unwrap().kind
                    {
                        functions[child.0 as usize] = Some(f.clone());
                    }
                }
            }
        }
    }
    let mut rows = vec![State::new(); definition.bodies.len()];
    for (b, body) in definition.bodies.iter().enumerate() {
        for p in &body.scoped_parameters {
            if let Some(l) = body.parameters.get(p.parameter as usize) {
                rows[b]
                    .entry(l.0 as usize)
                    .or_default()
                    .entry(p.fields.clone())
                    .or_default()
                    .insert((b as u32, l.0));
            }
        }
    }
    let mut observed = BTreeMap::new();
    let mut cleanup_states = BTreeMap::new();
    let mut snapshots: Vec<Vec<Option<State>>> = definition
        .bodies
        .iter()
        .map(|b| vec![None; b.blocks.len()])
        .collect();
    loop {
        let mut changed = false;
        for (b, body) in definition.bodies.iter().enumerate() {
            let mut inputs = vec![None; body.blocks.len()];
            inputs[body.entry.0 as usize] = Some(rows[b].clone());
            let mut pending = std::collections::VecDeque::from([body.entry]);
            while let Some(block) = pending.pop_front() {
                let mut state = inputs[block.0 as usize].clone().unwrap();
                for (ordinal, inst) in body.blocks[block.0 as usize]
                    .instructions
                    .iter()
                    .enumerate()
                {
                    observed.insert((b, block.0 as usize, ordinal), state.clone());
                    cleanup_states.insert(
                        (b, block.0 as usize, Some(ordinal as u32)),
                        (inst.on_failure, state.clone()),
                    );
                    match &inst.kind {
                        InstructionKind::Assign { destination, value } => {
                            let mut input = Paths::new();
                            match &body.rvalues[value.0 as usize] {
                                Rvalue::Use(o) | Rvalue::Unary { value: o, .. } => {
                                    input = read(t, body, &state, o)
                                }
                                Rvalue::Construct { fields, .. } => {
                                    for field in fields {
                                        let facts = read(t, body, &state, &field.value);
                                        let embedded = facts
                                            .into_iter()
                                            .map(|(p, r)| {
                                                let mut at = vec![field.field];
                                                at.extend(p);
                                                (at, r)
                                            })
                                            .collect();
                                        join(&mut input, &embedded);
                                    }
                                }
                                Rvalue::Closure {
                                    body: child,
                                    captures,
                                    context,
                                    ..
                                } => {
                                    if *context == CaptureContext::Scoped {
                                        input.entry(Vec::new()).or_default().insert((
                                            b as u32,
                                            body.places[destination.0 as usize].root.0,
                                        ));
                                    }
                                    let locals = definition.bodies[child.0 as usize]
                                        .locals
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, l)| l.storage == Storage::Capture);
                                    for ((i, _), capture) in locals.zip(captures) {
                                        let facts = read(t, body, &state, capture);
                                        for roots in facts.values() {
                                            input.entry(Vec::new()).or_default().extend(roots);
                                        }
                                        if !facts.is_empty() {
                                            changed |= join(
                                                rows[child.0 as usize].entry(i).or_default(),
                                                &facts,
                                            );
                                        }
                                    }
                                }
                                _ => {}
                            }
                            write(body, &mut state, *destination, &input);
                        }
                        InstructionKind::Replace {
                            destination,
                            value,
                            displaced,
                            ..
                        } => {
                            let old = read(
                                t,
                                body,
                                &state,
                                &Operand {
                                    place: *destination,
                                    usage: hirsel_types::UseKind::Borrow,
                                },
                            );
                            let input = read(t, body, &state, value);
                            write(body, &mut state, *destination, &input);
                            write(body, &mut state, *displaced, &old);
                        }
                        InstructionKind::StorageDead { place, .. } => {
                            write(body, &mut state, *place, &Paths::new())
                        }
                        _ => {}
                    }
                }
                // These snapshots are the exact contents at each normal or
                // error return/call, independent of source loan reconstruction.
                snapshots[b][block.0 as usize] = Some(state.clone());
                let bb = &body.blocks[block.0 as usize];
                match &bb.terminator {
                    Terminator::Call { on_failure, .. } => {
                        cleanup_states
                            .insert((b, block.0 as usize, None), (*on_failure, state.clone()));
                    }
                    Terminator::Return { cleanup, .. } | Terminator::Fail { cleanup, .. } => {
                        cleanup_states
                            .insert((b, block.0 as usize, None), (*cleanup, state.clone()));
                    }
                    _ => {}
                }
                let successors: Vec<_> = match &bb.terminator {
                    Terminator::Goto(next) => vec![*next],
                    Terminator::Branch { yes, no, .. } => vec![*yes, *no],
                    Terminator::Switch {
                        arms, otherwise, ..
                    } => arms.iter().map(|(_, b)| *b).chain([*otherwise]).collect(),
                    Terminator::Match { table, .. } => body.matches[table.0 as usize]
                        .arms
                        .iter()
                        .flat_map(|a| a.guard.into_iter().chain([a.target]))
                        .collect(),
                    Terminator::Call {
                        returns: Some(r), ..
                    } => vec![r.next],
                    _ => Vec::new(),
                };
                for next in successors {
                    let mut outgoing = state.clone();
                    if let Terminator::Call {
                        returns: Some(r), ..
                    } = &bb.terminator
                    {
                        write(body, &mut outgoing, r.destination, &Paths::new());
                    }
                    let target = &mut inputs[next.0 as usize];
                    let grew = match target {
                        Some(prior) => join_state(prior, &outgoing),
                        None => {
                            *target = Some(outgoing);
                            true
                        }
                    };
                    if grew && !pending.contains(&next) {
                        pending.push_back(next);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut errors = Vec::new();
    // Construction obligations are checked at the construction, including a
    // forbidden environment that is dropped before the block's terminator.
    for ((b, block, i), state) in &observed {
        let body = &definition.bodies[*b];
        let inst = &body.blocks[*block].instructions[*i];
        let mut retained = Paths::new();
        let mut destination = None;
        match &inst.kind {
            InstructionKind::Assign {
                value,
                destination: to,
            } => {
                destination = Some(*to);
                match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(o) | Rvalue::Unary { value: o, .. } => {
                        retained = read(t, body, state, o)
                    }
                    Rvalue::Construct { fields, .. } => {
                        for field in fields {
                            join(&mut retained, &read(t, body, state, &field.value));
                        }
                    }
                    Rvalue::Closure {
                        captures, context, ..
                    } => {
                        for capture in captures {
                            join(&mut retained, &read(t, body, state, capture));
                        }
                        if matches!(
                            context,
                            CaptureContext::Escaping
                                | CaptureContext::Task
                                | CaptureContext::OwningScoped
                        ) && retained.values().any(|r| !r.is_empty())
                        {
                            errors.push(Violation {
                                body: Some(BodyId(*b as u32)),
                                block: Some(BlockId(*block as u32)),
                                instruction: Some(*i as u32),
                                kind: ViolationKind::TypeMismatch {
                                    entity: "scoped owning capture",
                                },
                            });
                        }
                    }
                    _ => {}
                }
            }
            InstructionKind::Replace {
                destination: to,
                value,
                ..
            } => {
                destination = Some(*to);
                retained = read(t, body, state, value);
            }
            _ => {}
        }
        if let Some(to) = destination {
            let root = body.places[to.0 as usize].root;
            let mut_parameter = body
                .parameters
                .iter()
                .position(|p| *p == root)
                .is_some_and(|p| {
                    functions[*b]
                        .as_ref()
                        .and_then(|f| f.parameters.get(p))
                        .is_some_and(|p| p.passing == PassingMode::Mut)
                });
            if (mut_parameter || body.locals[root.0 as usize].storage == Storage::Capture)
                && retained.values().any(|r| !r.is_empty())
            {
                errors.push(Violation {
                    body: Some(BodyId(*b as u32)),
                    block: Some(BlockId(*block as u32)),
                    instruction: Some(*i as u32),
                    kind: ViolationKind::TypeMismatch {
                        entity: "scoped mutable hand-back",
                    },
                });
            }
        }
    }
    // TIR cleanups cannot install or return content. Drop/loan/region actions
    // are already checked by V's independent ownership/exit replay. In
    // particular no cleanup may smuggle scoped content into an async join.
    for ((b, block, instruction), (cleanup, state)) in cleanup_states {
        let body = &definition.bodies[b];
        for action in body.cleanup_actions(cleanup) {
            match action {
                CleanupAction::JoinTasks { scope, .. } | CleanupAction::CloseArena(scope) => {
                    let value = Operand {
                        place: *scope,
                        usage: hirsel_types::UseKind::Borrow,
                    };
                    if read(t, body, &state, &value)
                        .values()
                        .any(|r| !r.is_empty())
                    {
                        errors.push(Violation {
                            body: Some(BodyId(b as u32)),
                            block: Some(BlockId(block as u32)),
                            instruction,
                            kind: ViolationKind::TypeMismatch {
                                entity: "scoped cleanup crossing",
                            },
                        });
                    }
                }
                CleanupAction::Drop { .. }
                | CleanupAction::EndLoan { .. }
                | CleanupAction::ReleaseRegion(_) => {}
            }
        }
    }
    for (b, body) in definition.bodies.iter().enumerate() {
        for (block, bb) in body.blocks.iter().enumerate() {
            let Some(state) = &snapshots[b][block] else {
                continue;
            };
            let error = |errors: &mut Vec<Violation>, instruction, entity| {
                errors.push(Violation {
                    body: Some(BodyId(b as u32)),
                    block: Some(BlockId(block as u32)),
                    instruction,
                    kind: ViolationKind::TypeMismatch { entity },
                })
            };
            match &bb.terminator {
                Terminator::Return { value, .. } => {
                    if read(t, body, state, value).values().any(|r| !r.is_empty()) {
                        error(&mut errors, None, "scoped nonretaining result");
                    }
                }
                Terminator::Call {
                    callee, arguments, ..
                } => {
                    if let Callee::Indirect { value, .. } = callee {
                        let retains_data =
                            read(t, body, state, value)
                                .values()
                                .flatten()
                                .any(|&(b, local)| {
                                    !matches!(
                                        &t.row(
                                            definition.bodies[b as usize].locals[local as usize].ty
                                        )
                                        .kind,
                                        TypeKind::Function(_)
                                    )
                                });
                        if retains_data {
                            error(&mut errors, None, "scoped data callable environment");
                        }
                    }
                    // Inspect the original declaration before any generic substitution.
                    let function = match callee {
                        Callee::Definition { path, .. } => signatures
                            .get(DefId::intern(path))
                            .and_then(|s| match &s.kind {
                                SignatureKind::Function(f) => Some(f.clone()),
                                _ => None,
                            }),
                        Callee::Indirect { value, .. } => {
                            match &t.get(body.places[value.place.0 as usize].ty).unwrap().kind {
                                TypeKind::Function(f) => Some(f.clone()),
                                _ => None,
                            }
                        }
                    };
                    for a in arguments {
                        if read(t, body, state, &a.value)
                            .values()
                            .all(BTreeSet::is_empty)
                        {
                            continue;
                        }
                        let data_observer = matches!(callee, Callee::Definition { path, .. }
                            if ["list.len", "array.len", "bytes.len", "string.byte_len"].iter().any(|name| path.is_core(name)))
                            && a.parameter.0 == 0
                            && a.value.usage == hirsel_types::UseKind::Borrow;
                        let allowed = data_observer
                            || function
                                .as_ref()
                                .and_then(|f| f.parameters.get(a.parameter.0 as usize))
                                .is_some_and(|p| {
                                    p.passing != PassingMode::Take
                                        && matches!(
                                            t.get(p.ty).map(|t| &t.kind),
                                            Some(TypeKind::Function(_))
                                        )
                                });
                        if !allowed {
                            error(&mut errors, None, "scoped may-retain source call");
                        }
                        if allowed && !data_observer {
                            let expected =
                                function.as_ref().unwrap().parameters[a.parameter.0 as usize].ty;
                            let actual = body.places[a.value.place.0 as usize].ty;
                            if entry_contracts(t, expected).is_none()
                                || entry_contracts(t, expected) != entry_contracts(t, actual)
                            {
                                error(&mut errors, None, "scoped nested source entry contract");
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::*;
    use hirsel_types::{SignatureKind, TypeKind, UseKind};

    fn capture(scoped: bool, context: CaptureContext) -> Vec<Violation> {
        let mut f = Fixture::unit();
        let signature = function_signature("callback", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(function) = signature.kind else {
            unreachable!()
        };
        let p = f.slot(TypeKind::Function(function.clone()), Storage::Parameter);
        let made = f.slot(TypeKind::Function(function), Storage::Temporary);
        f.body().parameters = vec![LocalId(p)];
        if scoped {
            f.body().scoped_parameters = vec![ScopedParameter {
                parameter: 0,
                fields: vec![],
            }];
        }
        let mut child = Fixture::unit().definition.bodies.remove(0);
        child.locals.push(Local {
            ty: f.body().locals[p as usize].ty,
            storage: Storage::Capture,
            scope: ScopeId(0),
            span: Default::default(),
        });
        child.places.push(Place {
            root: LocalId(1),
            projection: vec![].into(),
            ty: f.body().locals[p as usize].ty,
            loan: None,
            binding: None,
        });
        f.definition.bodies.push(child);
        f.assign(
            made,
            Rvalue::Closure {
                context,
                body: BodyId(1),
                captures: vec![operand(p, UseKind::Copy)],
                destination: hirsel_types::Destination::Heap,
            },
            0,
        );
        f.emit(InstructionKind::StorageDead {
            place: PlaceId(made),
            if_live: false,
        });
        check(
            &f.definition,
            interner(),
            &Reader::new(interner(), &f.signatures),
        )
    }
    #[test]
    fn dropped_owning_capture_is_checked_at_construction() {
        assert!(capture(false, CaptureContext::Escaping).is_empty());
        let errors = capture(true, CaptureContext::Escaping);
        assert!(
            errors.iter().any(|v| matches!(
                v.kind,
                ViolationKind::TypeMismatch {
                    entity: "scoped owning capture"
                }
            )),
            "{errors:?}"
        );
    }
    #[test]
    fn dropped_task_capture_is_checked_at_construction() {
        assert!(capture(false, CaptureContext::Task).is_empty());
        let errors = capture(true, CaptureContext::Task);
        assert!(
            errors.iter().any(|v| matches!(
                v.kind,
                ViolationKind::TypeMismatch {
                    entity: "scoped owning capture"
                }
            )),
            "{errors:?}"
        );
    }
}
