//! Inline format builders that are consumed by one String append.
//! Discovery accepts a straight sequence of trusted primitive formatter/String
//! appends and total instructions after the first write. Custom formatters and
//! padding stay temporary. The destination's own finish still validates it.
use super::*;

fn text_call(path: &DefPath, name: &str) -> bool {
    path.0.package.package_is("std", "1") && path.0.modules == ["text"] && path.0.name == name
}
fn primitive_formatter(path: &DefPath) -> bool {
    path.0.package.package_is("std", "1")
        && path.0.name == "format_into"
        && matches!(path.0.modules.as_slice(), [module] if matches!(module.as_str(),
            "u8" | "u16" | "u32" | "u64" | "u128" | "i8" | "i16" | "i32" | "i64" | "i128" |
            "f32" | "f64" | "bool" | "char"))
}
// A bounded arena's appends can fail after charging a prefix. A local ordinary
// empty builder, never replaced or passed to an unknown mutator, has no such
// budget; parameter and arena destinations keep the temporary.
fn heap_destination(body: &Body, refs: &mut Refs, target: PlaceId) -> bool {
    let web = refs.web(root(body, target));
    let mut created = 0;
    for local in &web {
        if !matches!(
            body.locals[local.0 as usize].storage,
            Storage::Local | Storage::Temporary
        ) {
            return false;
        }
        for reference in &refs.by_local[local.0 as usize] {
            match reference {
                Ref::Assigned { place, value, .. } if whole(body, *place) => match &body.rvalues
                    [value.0 as usize]
                {
                    Rvalue::Construct {
                        constructor: tir::Constructor::Sequence,
                        fields,
                        destination:
                            hirsel_types::Destination::Heap | hirsel_types::Destination::Inferred,
                    } if fields.is_empty() => created += 1,
                    Rvalue::Use(op)
                        if op.usage == UseKind::Take && web.contains(&root(body, op.place)) => {}
                    _ => return false,
                },
                Ref::Argument {
                    block,
                    usage: UseKind::Mut,
                    place,
                    ..
                } => {
                    let Some(path) = callee_path(&body.blocks[*block].terminator) else {
                        return false;
                    };
                    if !whole(body, *place)
                        || !(primitive_formatter(path)
                            || text_call(path, "append")
                            || ["list.push", "list.reserve", "bytes.append"]
                                .iter()
                                .any(|name| path.is_core(name)))
                    {
                        return false;
                    }
                }
                Ref::Other | Ref::Result { .. } | Ref::Assigned { .. } => return false,
                _ => {}
            }
        }
    }
    created == 1
}

struct Plan {
    list: Vec<LocalId>,
    freeze: usize,
    target: PlaceId,
}

fn discover(program: &Program, def: &Definition, fuser: &mut Fuser) -> Vec<Plan> {
    let body = &def.bodies[def.entry.0 as usize];
    let mut refs = Refs::new(body);
    let mut plans = Vec::new();
    let mut heap = HashMap::new();
    for (freeze, block) in body.blocks.iter().enumerate() {
        let Terminator::Call {
            callee: Callee::Definition { path, .. },
            arguments,
            returns: Some(ret),
            ..
        } = &block.terminator
        else {
            continue;
        };
        if !(text_call(path, "finish_valid") || path.is_core("$intrinsic.TextFinish")) {
            continue;
        }
        let Some(builder) = argument(arguments, 0) else {
            continue;
        };
        let list = refs.web(root(body, builder.place));
        if list.iter().any(|l| {
            !matches!(
                body.locals[l.0 as usize].storage,
                Storage::Local | Storage::Temporary
            )
        }) {
            continue;
        }
        let result = refs.web(root(body, ret.destination));
        let mut consumer = None;
        let mut okay = true;
        for l in &result {
            for r in &refs.by_local[l.0 as usize] {
                match r {
                    Ref::Inert | Ref::Moved => {}
                    Ref::Assigned { value, .. } if matches!(&body.rvalues[value.0 as usize], Rvalue::Use(op) if op.usage == UseKind::Take && result.contains(&root(body, op.place))) =>
                        {}
                    Ref::Result { block } if *block == freeze => {}
                    Ref::Loan { mutable: false, .. } => {}
                    Ref::Argument {
                        block,
                        parameter: 1,
                        usage: UseKind::Borrow,
                        ..
                    } if callee_path(&body.blocks[*block].terminator)
                        .is_some_and(|p| text_call(p, "append"))
                        && consumer.replace(*block).is_none() => {}
                    _ => okay = false,
                }
            }
        }
        let Some(consumer) = consumer else { continue };
        if !okay {
            continue;
        }
        let Terminator::Call { arguments, .. } = &body.blocks[consumer].terminator else {
            continue;
        };
        let Some(target) = argument(arguments, 0) else {
            continue;
        };
        let target = target.place;
        if place(body, target)
            .projection
            .iter()
            .any(|p| matches!(p, tir::Projection::Index(_)))
            || list.contains(&root(body, target))
        {
            continue;
        }
        let target_root = root(body, target).0;
        let allowed = match heap.get(&target_root) {
            Some(value) => *value,
            None => {
                let value = heap_destination(body, &mut refs, target);
                heap.insert(target_root, value);
                value
            }
        };
        if !allowed {
            continue;
        }
        let mut creation = None;
        for l in &list {
            for r in &refs.by_local[l.0 as usize] {
                match r {
                    Ref::Inert | Ref::Moved => {}
                    Ref::Assigned {
                        at: At::Inst(b, i),
                        value,
                        ..
                    } => match &body.rvalues[value.0 as usize] {
                        Rvalue::Construct {
                            constructor: tir::Constructor::Sequence,
                            fields,
                            ..
                        } if fields.is_empty() && creation.replace((*b, *i)).is_none() => {}
                        Rvalue::Use(op)
                            if op.usage == UseKind::Take
                                && list.contains(&root(body, op.place)) => {}
                        _ => okay = false,
                    },
                    Ref::Loan { mutable: true, .. } => {}
                    Ref::Argument {
                        block,
                        parameter: 0,
                        usage: UseKind::Take,
                        ..
                    } if *block == freeze => {}
                    Ref::Argument {
                        block,
                        parameter,
                        usage: UseKind::Mut,
                        ..
                    } => {
                        let Some(path) = callee_path(&body.blocks[*block].terminator) else {
                            okay = false;
                            continue;
                        };
                        if !((*parameter == 1 && primitive_formatter(path))
                            || (*parameter == 0
                                && (text_call(path, "append") || path.is_core("list.reserve"))))
                        {
                            okay = false;
                        }
                    }
                    _ => okay = false,
                }
            }
        }
        let Some((mut b, start)) = creation else {
            continue;
        };
        if !okay {
            continue;
        }
        let mut first = true;
        let mut appended = false;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(b) {
                okay = false;
                break;
            }
            let block = &body.blocks[b];
            for inst in block
                .instructions
                .iter()
                .skip(if first { start + 1 } else { 0 })
            {
                if (appended && instruction_fails(body, &def.types, inst))
                    || operand_places(body, inst)
                        .iter()
                        .any(|p| root(body, *p) == root(body, target))
                {
                    okay = false;
                }
            }
            first = false;
            if b == freeze {
                break;
            }
            match &block.terminator {
                Terminator::Goto(next) => b = next.0 as usize,
                Terminator::Call {
                    callee: Callee::Definition { path, .. },
                    arguments,
                    returns: Some(ret),
                    ..
                } => {
                    if appended && !fuser.total_callee(program, path) {
                        okay = false;
                    }
                    if arguments
                        .iter()
                        .any(|a| root(body, a.value.place) == root(body, target))
                    {
                        okay = false;
                    }
                    appended |= arguments.iter().any(|a| {
                        a.value.usage == UseKind::Mut && list.contains(&root(body, a.value.place))
                    });
                    b = ret.next.0 as usize;
                }
                _ => {
                    okay = false;
                    break;
                }
            }
        }
        // The original append may follow moves/cleanup, but no effect or failure
        // may observe the now-earlier mutation between finishing and appending.
        let loan = place(body, target).loan;
        let mut b = ret.next.0 as usize;
        let mut reached = false;
        for _ in 0..body.blocks.len() {
            let block = &body.blocks[b];
            if block.instructions.iter().any(|i| {
                instruction_fails(body, &def.types, i)
                    || loan.is_none_or(|loan| touches(body, i, root(body, target), loan))
            }) {
                okay = false;
            }
            if b == consumer {
                reached = true;
                break;
            }
            match block.terminator {
                Terminator::Goto(next) => b = next.0 as usize,
                _ => break,
            }
        }
        if okay && appended && reached {
            // The consumer's argument loan begins only after its f-string has
            // been evaluated. Reuse the place that loan will borrow; each
            // formatter keeps its own existing short mutable loan of it.
            let Some(target) = loan.and_then(|loan| begin_place(body, loan)) else {
                continue;
            };
            plans.push(Plan {
                list,
                freeze,
                target,
            });
        }
    }
    plans
}

pub(super) fn fuse(program: &Program, def: &Definition, fuser: &mut Fuser) -> Option<Definition> {
    let plans = discover(program, def, fuser);
    if plans.is_empty() {
        return None;
    }
    let mut def = def.clone();
    let entry = def.entry.0 as usize;
    let body = &mut def.bodies[entry];
    let targets: HashMap<_, _> = plans
        .iter()
        .flat_map(|plan| plan.list.iter().map(|l| (l.0, plan.target)))
        .collect();
    let places: HashSet<u32> = body
        .places
        .iter()
        .enumerate()
        .filter(|(_, p)| targets.contains_key(&p.root.0))
        .map(|(i, _)| i as u32)
        .collect();
    let listed = |p: &PlaceId| places.contains(&p.0);
    // Remove temporary storage in one pass, keeping the short formatter loans.
    for (b, block) in body.blocks.iter_mut().enumerate() {
        let mut retained = Vec::new();
        for (i, inst) in block.instructions.iter().enumerate() {
            let keep = match &inst.kind {
                K::StorageLive(l) => !targets.contains_key(&l.0),
                K::StorageDead { place, .. } | K::EndOrigin(place) | K::Drop { place, .. } => {
                    !listed(place)
                }
                K::BindOrigin { source, place, .. } => !listed(source) && !listed(place),
                K::Assign { destination, .. } => !listed(destination),
                _ => true,
            };
            if keep {
                retained.push(i);
            }
        }
        if let Some(origin) = fuser.origins.get_mut(&def.path) {
            let old = origin.sites[entry][b].clone();
            origin.sites[entry][b] = retained
                .iter()
                .map(|&i| old[i])
                .chain([old[block.instructions.len()]])
                .collect();
        }
        let old = std::mem::take(&mut block.instructions);
        block.instructions = retained.into_iter().map(|i| old[i].clone()).collect();
    }
    let old = std::mem::take(&mut body.cleanup_nodes);
    let mut mapped = vec![None; old.len()];
    fn remap(
        n: Option<CleanupNodeId>,
        old: &[tir::CleanupNode],
        mapped: &mut [Option<Option<CleanupNodeId>>],
        out: &mut Vec<tir::CleanupNode>,
        listed: &impl Fn(&PlaceId) -> bool,
    ) -> Option<CleanupNodeId> {
        let n = n?;
        if let Some(value) = mapped[n.0 as usize] {
            return value;
        }
        let node = &old[n.0 as usize];
        let parent = remap(node.parent, old, mapped, out, listed);
        let result = if matches!(&node.action, CleanupAction::Drop { place, .. } if listed(place)) {
            parent
        } else {
            let id = CleanupNodeId(out.len() as u32);
            out.push(tir::CleanupNode {
                action: node.action.clone(),
                parent,
            });
            Some(id)
        };
        mapped[n.0 as usize] = Some(result);
        result
    }
    for cleanup in &mut body.cleanups {
        cleanup.head = remap(
            cleanup.head,
            &old,
            &mut mapped,
            &mut body.cleanup_nodes,
            &listed,
        );
    }
    let old = body.places.clone();
    for p in &mut body.places {
        if let Some(target) = targets.get(&p.root.0) {
            let target = &old[target.0 as usize];
            p.root = target.root;
            p.projection = target.projection.clone();
            p.binding = target.binding.clone();
            p.ty = target.ty;
            if p.loan.is_none() {
                p.loan = target.loan;
            }
        }
    }
    for plan in plans {
        let Terminator::Call {
            returns: Some(ret),
            on_failure,
            ..
        } = body.blocks[plan.freeze].terminator.clone()
        else {
            unreachable!()
        };
        let empty = RvalueId(body.rvalues.len() as u32);
        body.rvalues
            .push(Rvalue::Constant(Constant::String(String::new())));
        let span = body.blocks[plan.freeze].span;
        body.blocks[plan.freeze].instructions.push(Instruction {
            kind: K::Assign {
                destination: ret.destination,
                value: empty,
            },
            span,
            on_failure,
        });
        body.blocks[plan.freeze].terminator = Terminator::Goto(ret.next);
        if let Some(origin) = fuser.origins.get_mut(&def.path) {
            let sites = &mut origin.sites[entry][plan.freeze];
            sites.pop();
            sites.extend([None, None]);
        }
    }
    compact_rvalues(body);
    Some(def)
}
