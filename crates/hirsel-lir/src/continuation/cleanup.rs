//! Instruction coordinates and owned operands for entered cleanup. This table
//! is separate from call states: there is no CallId for a release instruction.
use super::*;

pub(super) fn sites(u: &LirUnit<Ready>, table: &LayoutTable) -> Vec<(BlockId, usize)> {
    u.blocks
        .iter()
        .enumerate()
        .flat_map(|(b, block)| {
            block
                .instructions
                .get(&u.instructions)
                .unwrap_or(&[])
                .iter()
                .enumerate()
                .filter_map(move |(i, inst)| match *inst {
                    Inst::Release {
                        value,
                        layout,
                        live: MaskRef::All,
                        ..
                    } if eligible(u, table, value, layout)
                        && (has_close(table, layout, &mut BTreeSet::new())
                            || has_finalizers(table, layout)) =>
                    {
                        Some((BlockId(b as u32), i))
                    }
                    _ => None,
                })
        })
        .collect()
}

pub(super) fn root_states(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    storage: &[LiveStorage],
    base: u64,
    first: u32,
) -> Result<Vec<CleanupResumeState>, String> {
    let live = crate::liveness::liveness(u);
    let mut out = Vec::new();
    for (b, block) in u.blocks.iter().enumerate() {
        for (i, inst) in block
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let Inst::Release {
                value,
                layout,
                live: MaskRef::All,
                anchor,
                failure,
            } = *inst
            else {
                continue;
            };
            if !eligible(u, table, value, layout)
                || !(has_close(table, layout, &mut BTreeSet::new())
                    || has_finalizers(table, layout))
            {
                continue;
            }
            if u.values[value.index()].holding != Holding::Owned {
                return Err("cleanup root has no owning credit".into());
            }
            let block = BlockId(b as u32);
            if successors_of(u, block)
                .into_iter()
                .any(|next| reaches(u, next, block))
            {
                return Err("cleanup boundary on a cycle".into());
            }
            let (slots, loans) = storage_before(u, block, i);
            if !loans.is_empty() {
                return Err("cleanup with a live unproved loan".into());
            }
            let retained: Vec<_> = storage
                .iter()
                .filter(|s| slots.contains(&s.slot))
                .cloned()
                .collect();
            if slots
                .iter()
                .any(|s| !parameter_slot(u, *s) && !retained.iter().any(|r| r.slot == *s))
            {
                return Err("cleanup storage outside the retained frame".into());
            }
            let mut values: BTreeSet<ValueId> = (0..u.values.len())
                .map(|v| ValueId(v as u32))
                .filter(|v| live.live_after(u, block, i, *v))
                .collect();
            values.insert(value);
            values.extend(carried_from(u, block, Some(i)).1);
            // Current failure disposal belongs to this instruction as well as
            // everything the later suffix needs; a failure must not lose it.
            let f = &u.tables.failures[failure.index()];
            values.extend(f.dispose.get(&u.pools.values).unwrap_or(&[]));
            let target = u.tables.edges[f.cleanup.index()].target;
            values.extend(live.live_in[target.index()].iter().filter(|v| {
                !u.blocks[target.index()]
                    .parameters
                    .get(&u.pools.values)
                    .unwrap_or(&[])
                    .contains(v)
            }));
            values.extend(
                u.tables.edges[f.cleanup.index()]
                    .arguments
                    .get(&u.pools.values)
                    .unwrap_or(&[]),
            );
            if values.iter().any(|v| {
                matches!(u.values[v.index()].holding, Holding::Borrowed(_))
                    || matches!(
                        u.types[u.values[v.index()].ty.index()].repr,
                        ValueRepr::Hole | ValueRepr::Token | ValueRepr::Uncounted
                    )
            }) {
                return Err("cleanup with an unproved interior address or reuse owner".into());
            }
            let values: Vec<_> = values.into_iter().collect();
            let layouts: Vec<_> = values
                .iter()
                .map(|v| u.types[u.values[v.index()].ty.index()].layout)
                .collect();
            if layouts
                .iter()
                .any(|l| fixed(table, *l).is_none_or(|(_, align)| align > 16))
            {
                return Err("cleanup with dynamic or over-aligned live bytes".into());
            }
            let (offsets, bytes) = place(table, base, &layouts).ok_or("dynamic cleanup image")?;
            if bytes > MAX_FRAME_BYTES {
                return Err("oversize cleanup image".into());
            }
            out.push(CleanupResumeState {
                action: ResumeAction::ResumeCleanup {
                    block,
                    instruction: i as u32,
                    anchor,
                },
                block,
                instruction: i as u32,
                value,
                layout,
                anchor,
                failure,
                state: first + out.len() as u32,
                live: values
                    .into_iter()
                    .zip(offsets)
                    .map(|(value, offset)| {
                        let ty = &u.types[u.values[value.index()].ty.index()];
                        LiveOwnedSlot {
                            value,
                            layout: ty.layout,
                            repr: ty.repr,
                            holding: u.values[value.index()].holding,
                            offset,
                        }
                    })
                    .collect(),
                storage: retained,
                bytes,
                finalizers: None,
            });
        }
    }
    Ok(out)
}

// Only ordinary owning representations. Donors, arenas, sealed owners and
// metadata release protocols cannot inherit this instruction's binder.
fn eligible(u: &LirUnit<Ready>, table: &LayoutTable, value: ValueId, layout: LayoutId) -> bool {
    let ty = &u.types[u.values[value.index()].ty.index()];
    table.layout(ty.layout).is_some_and(|l| {
        matches!(
            l.kind,
            LayoutKind::Shared { .. } | LayoutKind::Block { .. } | LayoutKind::Box { .. }
        )
    }) || table.layout(layout).is_some_and(|l| {
        matches!(l.kind, LayoutKind::Resource | LayoutKind::Handle) && l.size.storage().is_some()
    })
}

/// A mechanical release without a reachable close cannot suspend. Opaque
/// user finalizers alone keep their synchronous thunk contract.
fn has_close(table: &LayoutTable, layout: LayoutId, seen: &mut BTreeSet<LayoutId>) -> bool {
    use crate::layout_types::DropStep;
    if !seen.insert(layout) {
        return false;
    }
    let Some(record) = table.layout(layout) else {
        return false;
    };
    fn closes(steps: &[DropStep]) -> bool {
        steps.iter().any(|step| match step {
            DropStep::CloseHandle { .. } => true,
            DropStep::ActiveVariant { variants, .. } => {
                variants.iter().any(|(_, steps)| closes(steps))
            }
            _ => false,
        })
    }
    if closes(&record.drop) {
        return true;
    }
    let child = match record.kind {
        LayoutKind::Block { payload }
        | LayoutKind::Box { payload }
        | LayoutKind::Shared { payload } => Some(payload),
        LayoutKind::Frozen { value } => Some(value),
        LayoutKind::Inline { element, .. }
        | LayoutKind::Sequence { element }
        | LayoutKind::Sequential { element }
        | LayoutKind::SoaColumns { element }
        | LayoutKind::Slice { element }
        | LayoutKind::Iterator { element } => Some(element),
        LayoutKind::Callable { environment } => environment,
        _ => None,
    };
    child.is_some_and(|l| has_close(table, l, seen))
        || record
            .fields
            .iter()
            .any(|field| has_close(table, field.layout, seen))
        || record
            .tag
            .iter()
            .flat_map(|t| &t.variants)
            .flat_map(|v| &v.fields)
            .any(|field| has_close(table, field.layout, seen))
}

pub(super) fn has_finalizers(table: &LayoutTable, root: LayoutId) -> bool {
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(record) = table.layout(id) else {
            return true;
        };
        if record.finalizer.is_some() {
            return true;
        }
        pending.extend(record.references());
    }
    false
}
