//! Independent verification of source-published self-tail witnesses.
use super::{ReadBody, ReadDefinition};
use hirsel_syntax::DefPath;
use hirsel_tir::*;
use hirsel_types::{TypeId, UseKind};
use std::collections::BTreeSet;
fn plain(body: &ReadBody<'_>, place: PlaceId, property: &impl Fn(TypeId, bool) -> bool) -> bool {
    body.places
        .get(place.0 as usize)
        .is_some_and(|p| property(p.ty, false))
}
fn counted_temporary(
    body: &ReadBody<'_>,
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
fn same(body: &ReadBody<'_>, a: PlaceId, b: PlaceId) -> bool {
    matches!((body.places.get(a.0 as usize), body.places.get(b.0 as usize)), (Some(a), Some(b)) if a.root == b.root && a.projection == b.projection)
}
fn cleared(body: &ReadBody<'_>, place: PlaceId, dead: &BTreeSet<u32>) -> bool {
    body.places
        .get(place.0 as usize)
        .is_some_and(|p| dead.contains(&p.root.0))
}
fn clean(
    body: &ReadBody<'_>,
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
    t: &hirsel_types::TypeInterner,
    definition: &ReadDefinition<'_>,
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
    if path != definition.path.path() {
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
                        && body.places.get(call_destination.0 as usize).is_some_and(|p|matches!(t.get(p.ty).map(|t|&t.kind),Some(hirsel_types::TypeKind::Tuple(fields)) if fields.is_empty())) {
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
        Rvalue::Construct {
            constructor: Constructor::SortedMap { by },
            fields,
            ..
        } => std::iter::once(by)
            .chain(fields.iter().map(|field| &field.value))
            .collect(),
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
fn live_at_call(body: &ReadBody<'_>, target: BlockId) -> Option<BTreeSet<u32>> {
    use std::collections::VecDeque;
    fn take(body: &ReadBody<'_>, live: &mut BTreeSet<u32>, operand: &Operand) {
        if operand.usage == UseKind::Take {
            if let Some(p) = body.places.get(operand.place.0 as usize) {
                if p.projection.is_empty() {
                    live.remove(&p.root.0);
                }
            }
        }
    }
    fn drop_place(body: &ReadBody<'_>, live: &mut BTreeSet<u32>, place: PlaceId) {
        if let Some(p) = body.places.get(place.0 as usize) {
            if p.projection.is_empty() {
                live.remove(&p.root.0);
            }
        }
    }
    fn cleanup(body: &ReadBody<'_>, live: &mut BTreeSet<u32>, id: CleanupId) {
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
        // A pattern decision consumes no storage: each arm's binding transfers
        // are explicit instructions in its guard or target, replayed above.
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

/// LANG-10 constructor-tail facts V reads from declarations, not from the
/// witness: whether a nominal is an ordinary data record or variant with no
/// layout promise, `distinct` fact, resource, finalizer or capability.
pub(crate) type OrdinaryConstructor<'a> = dyn Fn(&DefPath) -> bool + 'a;

/// The constructor chain a self call's continuation builds around its result,
/// as declaration field ordinals from the returned constructor down to the
/// call, or None when the continuation is anything else. Each level takes the
/// previous level's value as its last source-evaluated field; every other
/// field is a whole temporary already evaluated before the call (LANG-3), so
/// it is part of the partial result. Only `box`, tuples and ordinary records
/// and variants are levels, built at an inferred (never arena) destination.
pub(crate) fn constructor_path(
    t: &hirsel_types::TypeInterner,
    definition: &ReadDefinition<'_>,
    owner: BodyId,
    call_block: BlockId,
    property: &impl Fn(TypeId, bool) -> bool,
    ordinary: &OrdinaryConstructor<'_>,
) -> Option<Vec<u32>> {
    if owner != definition.entry {
        return None;
    }
    let body = definition.bodies.get(owner.0 as usize)?;
    let call = body.blocks.get(call_block.0 as usize)?;
    let Terminator::Call {
        callee: Callee::Definition {
            path,
            arguments: generics,
        },
        arguments,
        returns: Some(CallReturn { destination, next }),
        ..
    } = &call.terminator
    else {
        return None;
    };
    // Monomorphic self recursion with data parameters and a global data
    // result: no step of the loop holds a capability, so none can suspend.
    if path != definition.path.path() || !generics.is_empty() {
        return None;
    }
    if t.get(body.result)
        .is_none_or(|ty| ty.modes.relation != hirsel_types::Relation::global())
        || !property(body.result, true)
    {
        return None;
    }
    for p in &body.parameters {
        let local = body.locals.get(p.0 as usize)?;
        if !property(local.ty, true) {
            return None;
        }
    }
    if arguments.iter().any(|a| a.value.usage == UseKind::Mut) {
        return None;
    }
    let live = live_at_call(body, call_block)?;
    let mut dead: BTreeSet<u32> = (0..body.locals.len() as u32)
        .filter(|id| !live.contains(id))
        .collect();
    for arg in arguments {
        let place = body.places.get(arg.value.place.0 as usize)?;
        if arg.value.usage == UseKind::Take && place.projection.is_empty() {
            dead.insert(place.root.0);
        }
    }
    let mut values = vec![*destination];
    let mut path = Vec::new();
    let mut block = *next;
    let mut seen = BTreeSet::new();
    while seen.insert(block.0) {
        let b = body.blocks.get(block.0 as usize)?;
        for i in &b.instructions {
            match &i.kind {
                InstructionKind::Assign { destination, value } => {
                    let dst = body.places.get(destination.0 as usize)?;
                    match body.rvalues.get(value.0 as usize)? {
                        Rvalue::Use(operand)
                            if values.iter().any(|v| same(body, *v, operand.place)) =>
                        {
                            let src = body.places.get(operand.place.0 as usize)?;
                            if operand.usage == UseKind::Take && src.projection.is_empty() {
                                dead.insert(src.root.0);
                            }
                        }
                        Rvalue::Construct {
                            constructor,
                            fields,
                            destination:
                                hirsel_types::Destination::Inferred | hirsel_types::Destination::Heap,
                        } => {
                            let level = match constructor {
                                Constructor::Box | Constructor::Tuple => true,
                                Constructor::Record(p)
                                | Constructor::Variant { definition: p, .. } => ordinary(p),
                                _ => false,
                            };
                            let (last, earlier) = fields.split_last()?;
                            if !level || !values.iter().any(|v| same(body, *v, last.value.place)) {
                                return None;
                            }
                            // The call is the payload of the innermost `box`.
                            if path.is_empty() && *constructor != Constructor::Box {
                                return None;
                            }
                            for f in fields {
                                let p = body.places.get(f.value.place.0 as usize)?;
                                if !p.projection.is_empty()
                                    || !matches!(f.value.usage, UseKind::Take | UseKind::Copy)
                                {
                                    return None;
                                }
                            }
                            // An earlier field was evaluated before the call
                            // and is consumed whole by this level.
                            for f in earlier {
                                let p = body.places.get(f.value.place.0 as usize)?;
                                let local = body.locals.get(p.root.0 as usize)?;
                                if !live.contains(&p.root.0)
                                    || local.storage != Storage::Temporary
                                    || values.iter().any(|v| same(body, *v, f.value.place))
                                {
                                    return None;
                                }
                                if f.value.usage == UseKind::Take {
                                    dead.insert(p.root.0);
                                }
                            }
                            let src = body.places.get(last.value.place.0 as usize)?;
                            if last.value.usage == UseKind::Take && src.projection.is_empty() {
                                dead.insert(src.root.0);
                            }
                            path.push(last.field);
                            values.clear();
                        }
                        _ => return None,
                    }
                    dead.remove(&dst.root.0);
                    values.push(*destination);
                }
                InstructionKind::BindOrigin { source, place, .. } => {
                    if values.iter().any(|v| same(body, *v, *source)) {
                        values.push(*place);
                    }
                }
                InstructionKind::StorageLive(_)
                | InstructionKind::EndScope(_)
                | InstructionKind::BindFields { .. }
                | InstructionKind::EndOrigin(_) => (),
                InstructionKind::Drop { place, if_live }
                    if plain(body, *place, property)
                        || *if_live && cleared(body, *place, &dead) => {}
                // Storage of a field already moved into the chain ends empty.
                InstructionKind::StorageDead { place, .. }
                    if plain(body, *place, property) || cleared(body, *place, &dead) => {}
                InstructionKind::RunCleanup(c)
                    if clean(
                        body,
                        *c,
                        &dead,
                        &BTreeSet::new(),
                        &BTreeSet::new(),
                        property,
                    ) => {}
                _ => return None,
            }
        }
        match &b.terminator {
            Terminator::Goto(n) => block = *n,
            Terminator::Return { value, cleanup } => {
                if value.usage == UseKind::Take
                    && let Some(p) = body.places.get(value.place.0 as usize)
                    && p.projection.is_empty()
                {
                    dead.insert(p.root.0);
                }
                path.reverse();
                return (!path.is_empty()
                    && values.iter().any(|v| same(body, *v, value.place))
                    && clean(
                        body,
                        *cleanup,
                        &dead,
                        &BTreeSet::new(),
                        &BTreeSet::new(),
                        property,
                    ))
                .then_some(path);
            }
            _ => return None,
        }
    }
    None
}

/// The locals a constructor-tail chain consumes as earlier fields: owners
/// pending at the call by design, since they become parts of the partial
/// result. Whether the chain qualifies is `constructor_path`'s question.
pub(crate) fn chain_fields(body: &ReadBody<'_>, call_block: BlockId) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    let Some(Terminator::Call {
        returns: Some(CallReturn { destination, next }),
        ..
    }) = body
        .blocks
        .get(call_block.0 as usize)
        .map(|b| &b.terminator)
    else {
        return out;
    };
    let mut values = vec![*destination];
    let mut block = *next;
    let mut seen = BTreeSet::new();
    while seen.insert(block.0) {
        let Some(b) = body.blocks.get(block.0 as usize) else {
            return out;
        };
        for i in &b.instructions {
            match &i.kind {
                InstructionKind::Assign { destination, value } => {
                    let derived = |o: &Operand| values.iter().any(|v| same(body, *v, o.place));
                    match body.rvalues.get(value.0 as usize) {
                        Some(Rvalue::Use(o)) if derived(o) => values.push(*destination),
                        Some(Rvalue::Construct { fields, .. })
                            if fields.last().is_some_and(|f| derived(&f.value)) =>
                        {
                            for f in &fields[..fields.len() - 1] {
                                if let Some(p) = body.places.get(f.value.place.0 as usize) {
                                    out.insert(p.root.0);
                                }
                            }
                            values = vec![*destination];
                        }
                        _ => {}
                    }
                }
                InstructionKind::BindOrigin { source, place, .. }
                    if values.iter().any(|v| same(body, *v, *source)) =>
                {
                    values.push(*place)
                }
                _ => {}
            }
        }
        match &b.terminator {
            Terminator::Goto(n) => block = *n,
            _ => return out,
        }
    }
    out
}

/// LANG-10: a published constructor-tail witness names exactly the chain V
/// derives, and its call is otherwise a legal self restart: no mut argument,
/// and a borrowed counted argument is a parameter or a listed loop temporary.
pub(crate) fn verify_constructor_hole(
    t: &hirsel_types::TypeInterner,
    definition: &ReadDefinition<'_>,
    owner: BodyId,
    site: &TailSite,
    property: &impl Fn(TypeId, bool) -> bool,
    ordinary: &OrdinaryConstructor<'_>,
) -> bool {
    let TailKind::ConstructorHole(witness) = &site.kind else {
        return false;
    };
    if site.owner_body != owner {
        return false;
    }
    let Some(path) = constructor_path(t, definition, owner, site.call_block, property, ordinary)
    else {
        return false;
    };
    if &path != witness {
        return false;
    }
    let body = &definition.bodies[owner.0 as usize];
    let Terminator::Call { arguments, .. } = &body.blocks[site.call_block.0 as usize].terminator
    else {
        return false;
    };
    let mut carried = BTreeSet::new();
    for (ordinal, place) in &site.owned_loop_arguments {
        let Some(p) = body.places.get(place.0 as usize) else {
            return false;
        };
        if !counted_temporary(body, *place, property)
            || !carried.insert(p.root.0)
            || arguments
                .iter()
                .filter(|a| {
                    body.places
                        .get(a.value.place.0 as usize)
                        .is_some_and(|x| x.root == p.root)
                })
                .count()
                != 1
            || !arguments.iter().any(|a| {
                a.parameter == *ordinal
                    && same(body, a.value.place, *place)
                    && a.value.usage == UseKind::Borrow
            })
        {
            return false;
        }
    }
    arguments.iter().all(|arg| {
        let Some(place) = body.places.get(arg.value.place.0 as usize) else {
            return false;
        };
        arg.value.usage != UseKind::Borrow
            || plain(body, arg.value.place, property)
            || carried.contains(&place.root.0)
            || body
                .locals
                .get(place.root.0 as usize)
                .is_some_and(|l| l.storage == Storage::Parameter)
    })
}

/// The declaration facts behind [`OrdinaryConstructor`].
pub(crate) fn ordinary_constructor(reader: &crate::input::Reader<'_>, path: &DefPath) -> bool {
    reader
        .get(hirsel_syntax::DefId::intern(path))
        .is_some_and(|s| match &s.kind {
            hirsel_types::SignatureKind::Type(t) => {
                t.layout == hirsel_types::Layout::default()
                    && !t.distinct
                    && !t.resource
                    && t.fact.is_none()
                    && t.capability.is_none()
                    && t.drop.is_none()
            }
            _ => false,
        })
}
