//! Independent verification of source-published self-tail witnesses.
use hirsel_tir::*;
use hirsel_types::{TypeId, UseKind};
use std::collections::BTreeSet;
fn plain(body: &Body, place: PlaceId, property: &impl Fn(TypeId, bool) -> bool) -> bool {
    body.places
        .get(place.0 as usize)
        .is_some_and(|p| property(p.ty, false))
}
fn counted_temporary(
    body: &Body,
    place: PlaceId,
    property: &impl Fn(TypeId, bool) -> bool,
) -> bool {
    body.places.get(place.0 as usize).is_some_and(|p| {
        p.projection.is_empty()
            && body
                .locals
                .get(p.root.0 as usize)
                .is_some_and(|l| l.storage == Storage::Temporary)
            && !property(p.ty, false)
            && property(p.ty, true)
    })
}
fn same(body: &Body, a: PlaceId, b: PlaceId) -> bool {
    matches!((body.places.get(a.0 as usize), body.places.get(b.0 as usize)), (Some(a), Some(b)) if a.root == b.root && a.projection == b.projection)
}
fn cleared(body: &Body, place: PlaceId, dead: &BTreeSet<u32>) -> bool {
    body.places
        .get(place.0 as usize)
        .is_some_and(|p| dead.contains(&p.root.0))
}
fn clean(
    body: &Body,
    cleanup: CleanupId,
    dead: &BTreeSet<u32>,
    loop_roots: &BTreeSet<u32>,
    loans: &BTreeSet<u32>,
    property: &impl Fn(TypeId, bool) -> bool,
) -> bool {
    body.cleanups.get(cleanup.0 as usize).is_some_and(|_| {
        body.cleanup_actions(cleanup).all(|a| match a {
            CleanupAction::Drop { place, if_live } => {
                plain(body, *place, property)
                    || *if_live
                        && (cleared(body, *place, dead) || cleared(body, *place, loop_roots))
            }
            CleanupAction::EndLoan { loan, .. } => loans.contains(&loan.0),
            _ => false,
        })
    })
}
/// #46: pending finalization/release excludes a tail call. A listed counted
/// temporary is carried by an internal loop owner, not an old-frame obligation.
pub(crate) fn verify_self_tail(
    definition: &Definition,
    owner: BodyId,
    site: &TailSite,
    property: &impl Fn(TypeId, bool) -> bool,
) -> bool {
    if owner != definition.entry || site.owner_body != owner || site.kind != TailKind::SelfTail {
        return false;
    }
    let Some(body) = definition.bodies.get(owner.0 as usize) else {
        return false;
    };
    let Some(call) = body.blocks.get(site.call_block.0 as usize) else {
        return false;
    };
    let Terminator::Call {
        callee: Callee::Definition { path, .. },
        arguments,
        returns,
        on_failure,
    } = &call.terminator
    else {
        return false;
    };
    if path != &definition.path {
        return false;
    }
    let mut loop_roots = BTreeSet::new();
    let mut loans: BTreeSet<_> = arguments
        .iter()
        .filter(|a| a.value.usage == UseKind::Borrow)
        .filter_map(|a| body.places.get(a.value.place.0 as usize)?.loan.map(|l| l.0))
        .collect();
    for (ordinal, place) in &site.owned_loop_arguments {
        if !counted_temporary(body, *place, property) {
            return false;
        }
        let Some(p) = body.places.get(place.0 as usize) else {
            return false;
        };
        if !loop_roots.insert(p.root.0) {
            return false;
        }
        let matches: Vec<_> = arguments
            .iter()
            .filter(|a| {
                a.parameter == *ordinal
                    && same(body, a.value.place, *place)
                    && a.value.usage == UseKind::Borrow
            })
            .collect();
        if matches.len() != 1 {
            return false;
        }
        if let Some(loan) = body.places[matches[0].value.place.0 as usize].loan {
            loans.insert(loan.0);
        }
        // No second consumer/alias may retain this temporary past the loop.
        if arguments
            .iter()
            .filter(|a| {
                body.places
                    .get(a.value.place.0 as usize)
                    .is_some_and(|x| x.root == p.root)
            })
            .count()
            != 1
        {
            return false;
        }
        for (index, block) in body.blocks.iter().enumerate() {
            let uses_root = |place: PlaceId| {
                body.places
                    .get(place.0 as usize)
                    .is_some_and(|x| x.root == p.root)
            };
            for i in &block.instructions {
                let retained = match &i.kind {
                    InstructionKind::Assign { value, .. } => {
                        body.rvalues.get(value.0 as usize).is_none_or(|value| {
                            rvalue_operands(value).iter().any(|o| uses_root(o.place))
                        })
                    }
                    InstructionKind::Replace { value, .. }
                    | InstructionKind::Snapshot { value, .. }
                    | InstructionKind::Check {
                        condition: value, ..
                    } => uses_root(value.place),
                    InstructionKind::BeginLoan { id, place, .. } => {
                        uses_root(*place) && !loans.contains(&id.0)
                    }
                    InstructionKind::BindOrigin { source, place, .. } => {
                        uses_root(*source) && !same(body, *source, *place)
                    }
                    InstructionKind::BindFields { source, .. } => uses_root(*source),
                    _ => false,
                };
                if retained {
                    return false;
                }
            }
            let retained = match &block.terminator {
                Terminator::Branch { condition, .. }
                | Terminator::Switch {
                    value: condition, ..
                }
                | Terminator::Return {
                    value: condition, ..
                } => uses_root(condition.place),
                Terminator::Call {
                    callee, arguments, ..
                } => {
                    matches!(callee, Callee::Indirect { value, .. } if uses_root(value.place))
                        || index != site.call_block.0 as usize
                            && arguments.iter().any(|a| uses_root(a.value.place))
                }
                _ => false,
            };
            if retained {
                return false;
            }
        }
    }
    let Some(live) = live_at_call(body, site.call_block) else {
        return false;
    };
    let mut dead: BTreeSet<_> = (0..body.locals.len())
        .map(|i| i as u32)
        .filter(|id| !live.contains(id))
        .collect();
    for arg in arguments {
        let Some(place) = body.places.get(arg.value.place.0 as usize) else {
            return false;
        };
        let Some(local) = body.locals.get(place.root.0 as usize) else {
            return false;
        };
        if arg.value.usage == UseKind::Mut {
            return false;
        }
        if arg.value.usage == UseKind::Take && place.projection.is_empty() {
            dead.insert(place.root.0);
        }
        if arg.value.usage == UseKind::Borrow
            && !plain(body, arg.value.place, property)
            && !loop_roots.contains(&place.root.0)
            && local.storage != Storage::Parameter
        {
            return false;
        }
    }
    // A diverging self call (`fn serve() -> Never { ..; serve() }`) has no
    // continuation. The frame keeps nothing past it: every obligation its
    // failure cleanup lists is already plain, moved or carried by the loop.
    let Some(CallReturn { destination, next }) = *returns else {
        return clean(body, *on_failure, &dead, &loop_roots, &loans, property);
    };
    let call_destination = destination;
    let mut values = vec![destination];
    let mut block = next;
    let mut seen = BTreeSet::new();
    while seen.insert(block.0) {
        let Some(b) = body.blocks.get(block.0 as usize) else {
            return false;
        };
        for i in &b.instructions {
            match &i.kind {
                InstructionKind::Assign { destination, value } => {
                    let Some(value) = body.rvalues.get(value.0 as usize) else {
                        return false;
                    };
                    if matches!(value,Rvalue::Constant(Constant::Unit))
                        && body.places.get(call_destination.0 as usize).is_some_and(|p|matches!(definition.types.get(p.ty).map(|t|&t.kind),Some(hirsel_types::TypeKind::Tuple(fields)) if fields.is_empty())) {
                        values.push(*destination);
                        continue;
                    }
                    let Rvalue::Use(operand) = value else {
                        return false;
                    };
                    if !values.iter().any(|v| same(body, *v, operand.place)) {
                        return false;
                    }
                    let Some(src) = body.places.get(operand.place.0 as usize) else {
                        return false;
                    };
                    let Some(dst) = body.places.get(destination.0 as usize) else {
                        return false;
                    };
                    if operand.usage == UseKind::Take && src.projection.is_empty() {
                        dead.insert(src.root.0);
                    }
                    dead.remove(&dst.root.0);
                    values.push(*destination);
                }
                InstructionKind::StorageLive(_)
                | InstructionKind::EndScope(_)
                | InstructionKind::BindOrigin { .. }
                | InstructionKind::BindFields { .. }
                | InstructionKind::EndOrigin(_) => (),
                InstructionKind::Drop { place, if_live }
                | InstructionKind::StorageDead { place, if_live }
                    if plain(body, *place, property)
                        || *if_live
                            && (cleared(body, *place, &dead)
                                || cleared(body, *place, &loop_roots)) =>
                {
                    ()
                }
                InstructionKind::EndLoan { loan, .. } if loans.contains(&loan.0) => (),
                InstructionKind::RunCleanup(c)
                    if clean(body, *c, &dead, &loop_roots, &loans, property) =>
                {
                    ()
                }
                _ => return false,
            }
        }
        match &b.terminator {
            Terminator::Goto(next) => block = *next,
            Terminator::Return { value, cleanup } => {
                if value.usage == UseKind::Take {
                    if let Some(p) = body.places.get(value.place.0 as usize) {
                        if p.projection.is_empty() {
                            dead.insert(p.root.0);
                        }
                    }
                }
                return values.iter().any(|v| same(body, *v, value.place))
                    && clean(body, *cleanup, &dead, &loop_roots, &loans, property);
            }
            _ => return false,
        }
    }
    false
}

fn rvalue_operands(value: &Rvalue) -> Vec<&Operand> {
    match value {
        Rvalue::Use(o) | Rvalue::Unary { value: o, .. } => {
            vec![o]
        }
        Rvalue::Binary { left, right, .. } => vec![left, right],
        Rvalue::Construct { fields, .. } => fields.iter().map(|f| &f.value).collect(),
        Rvalue::Closure { captures, .. } => captures.iter().collect(),
        Rvalue::Constant(_)
        | Rvalue::ReadConstant { .. }
        | Rvalue::FunctionReference { .. }
        | Rvalue::Size(_)
        | Rvalue::Shape(_) => vec![],
    }
}

// Replay physical initialization along normal edges independently of the witness.
// Conditional join cleanups can mention owners already taken before this call.
fn live_at_call(body: &Body, target: BlockId) -> Option<BTreeSet<u32>> {
    use std::collections::VecDeque;
    fn take(body: &Body, live: &mut BTreeSet<u32>, operand: &Operand) {
        if operand.usage == UseKind::Take {
            if let Some(p) = body.places.get(operand.place.0 as usize) {
                if p.projection.is_empty() {
                    live.remove(&p.root.0);
                }
            }
        }
    }
    fn drop_place(body: &Body, live: &mut BTreeSet<u32>, place: PlaceId) {
        if let Some(p) = body.places.get(place.0 as usize) {
            if p.projection.is_empty() {
                live.remove(&p.root.0);
            }
        }
    }
    fn cleanup(body: &Body, live: &mut BTreeSet<u32>, id: CleanupId) {
        if body.cleanups.get(id.0 as usize).is_some() {
            for a in body.cleanup_actions(id) {
                if let CleanupAction::Drop { place, .. } = a {
                    drop_place(body, live, *place);
                }
            }
        }
    }
    let mut incoming = vec![None::<BTreeSet<u32>>; body.blocks.len()];
    *incoming.get_mut(body.entry.0 as usize)? =
        Some(body.parameters.iter().map(|id| id.0).collect());
    let mut todo = VecDeque::from([body.entry]);
    let mut at_target = None::<BTreeSet<u32>>;
    while let Some(id) = todo.pop_front() {
        let b = body.blocks.get(id.0 as usize)?;
        let mut live = incoming.get(id.0 as usize)?.clone()?;
        for i in &b.instructions {
            match &i.kind {
                InstructionKind::StorageLive(local) => {
                    live.remove(&local.0);
                }
                InstructionKind::Assign { destination, value } => {
                    let value = body.rvalues.get(value.0 as usize)?;
                    for operand in rvalue_operands(value) {
                        take(body, &mut live, operand);
                    }
                    live.insert(body.places.get(destination.0 as usize)?.root.0);
                }
                InstructionKind::Replace {
                    destination,
                    value,
                    displaced,
                    ..
                } => {
                    take(body, &mut live, value);
                    live.insert(body.places.get(destination.0 as usize)?.root.0);
                    live.insert(body.places.get(displaced.0 as usize)?.root.0);
                }
                InstructionKind::Snapshot { destination, .. } => {
                    live.insert(body.places.get(destination.0 as usize)?.root.0);
                }
                InstructionKind::Drop { place, .. }
                | InstructionKind::StorageDead { place, .. } => drop_place(body, &mut live, *place),
                InstructionKind::EndScope(scope) => {
                    live.retain(|root| {
                        let mut current = Some(body.locals[*root as usize].scope);
                        while let Some(id) = current {
                            if id == *scope {
                                return false;
                            }
                            current = body.scopes[id.0 as usize].parent;
                        }
                        true
                    });
                }
                InstructionKind::RunCleanup(c) => cleanup(body, &mut live, *c),
                _ => (),
            }
        }
        // This replay conservatively stops at pattern decisions: it does not
        // model the arm's binding transfers.
        if matches!(b.terminator, Terminator::Match { .. }) {
            continue;
        }
        if let Terminator::Call {
            arguments, returns, ..
        } = &b.terminator
        {
            for arg in arguments {
                take(body, &mut live, &arg.value);
            }
            if id == target {
                if let Some(previous) = &mut at_target {
                    previous.extend(&live);
                } else {
                    at_target = Some(live.clone());
                }
            }
            if let Some(CallReturn { destination, .. }) = returns {
                live.insert(body.places.get(destination.0 as usize)?.root.0);
            }
        }
        for next in body.successors(&b.terminator) {
            let slot = incoming.get_mut(next.0 as usize)?;
            let changed = if let Some(previous) = slot {
                let len = previous.len();
                previous.extend(&live);
                previous.len() != len
            } else {
                *slot = Some(live.clone());
                true
            };
            if changed {
                todo.push_back(next);
            }
        }
    }
    at_target
}
