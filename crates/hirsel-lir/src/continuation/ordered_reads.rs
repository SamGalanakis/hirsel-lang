//! The two no-yield reads retain one whole mutable caller slot. This is a
//! specific runtime argument proof, separate from generic ExclusiveSlot.
use super::*;

pub(super) fn candidate(u: &LirUnit<Ready>, call: CallId) -> bool {
    matches!(
        ordered::host_kind(u, call),
        Some(RuntimeOpKind::OrderedStdinRead | RuntimeOpKind::OrderedWatchNext)
    )
}
fn root(u: &LirUnit<Ready>, mut a: AddressId) -> Option<SlotId> {
    for _ in 0..=u.addresses.len() {
        match u.addresses.get(a.index())?.base {
            AddressBase::Slot(s) => return Some(s),
            AddressBase::Address(base) => a = base,
            AddressBase::Value(_) => return None,
        }
    }
    None
}
fn helper(kind: RuntimeOpKind) -> &'static str {
    match kind {
        RuntimeOpKind::OrderedStdinRead => "hirsel_rt_v2_ordered_stdin_read",
        RuntimeOpKind::OrderedWatchNext => "hirsel_rt_v2_ordered_watcher_next",
        _ => unreachable!(),
    }
}
fn typed(u: &LirUnit<Ready>, table: &LayoutTable, kind: RuntimeOpKind, ty: LirTypeId) -> bool {
    let Some(ty) = u.types.get(ty.index()) else {
        return false;
    };
    let Some(layout) = table.layout(ty.layout) else {
        return false;
    };
    if fixed(table, ty.layout) != Some((8, 8)) {
        return false;
    }
    match (kind, ty.repr, &layout.kind) {
        (RuntimeOpKind::OrderedStdinRead, ValueRepr::Block, LayoutKind::Block { payload }) => {
            let Some(payload) = table.layout(*payload) else {
                return false;
            };
            let LayoutKind::Sequence { element } = payload.kind else {
                return false;
            };
            table
                .layout(element)
                .is_some_and(|e| e.kind == LayoutKind::Scalar(ScalarKind::U8))
        }
        (
            RuntimeOpKind::OrderedWatchNext,
            ValueRepr::Handle,
            LayoutKind::Handle | LayoutKind::HostCapability { .. },
        ) => true,
        (RuntimeOpKind::OrderedWatchNext, ValueRepr::Aggregate, LayoutKind::Resource) => true,
        _ => false,
    }
}
// Ready proves the initialized reaching image. Additionally require a single
// whole-slot initializer, never a parameter or projected external carrier.
fn initializer(u: &LirUnit<Ready>, _block: BlockId, slot: SlotId) -> Option<ValueId> {
    let mut stores = u
        .blocks
        .iter()
        .flat_map(|b| b.instructions.get(&u.instructions).unwrap_or(&[]))
        .filter_map(|inst| match *inst {
            Inst::Store { address, value, .. } if root(u, address) == Some(slot) => Some(value),
            _ => None,
        });
    let initial = stores.next()?;
    stores.next().is_none().then_some(initial)
}
fn argument(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    state: &ResumeState,
    kind: RuntimeOpKind,
) -> Option<RuntimeResultSlot> {
    let call = u.tables.calls.get(state.boundary.call.index())?;
    let args = call.arguments.get(&u.pools.call_arguments)?;
    if args.len()
        != if kind == RuntimeOpKind::OrderedStdinRead {
            3
        } else {
            2
        }
    {
        return None;
    }
    let mut targets = args.iter().filter(|a| a.parameter == 1);
    let target = targets.next()?;
    if targets.next().is_some() || target.passing != Passing::Mut {
        return None;
    }
    let ArgumentValue::Address { address, .. } = target.value else {
        return None;
    };
    let info = u.addresses.get(address.index())?;
    let AddressBase::Slot(slot) = info.base else {
        return None;
    };
    if info.projection.len != 0
        || !info.unchecked.is_empty()
        || !matches!(
            u.slots.get(slot.index())?.kind,
            SlotKind::Local | SlotKind::Temporary
        )
        || u.slots[slot.index()].ty != info.ty
        || !typed(u, table, kind, info.ty)
    {
        return None;
    }
    let layout = u.types[info.ty.index()].layout;
    let abi = table.fn_abis.get(call.abi.0 as usize)?;
    let parameter = abi.parameters.iter().find(|p| p.source == 1)?;
    if parameter.passing != ParameterPassing::ExclusiveAddress
        || parameter.layout != layout
        || abi.caller_site
        || abi.environment
        || !abi.context
        || !abi.destination
    {
        return None;
    }
    if args.iter().any(|a| {
        a.parameter != 1
            && (a.passing != Passing::Borrow || !matches!(a.value, ArgumentValue::Value(_)))
    }) {
        return None;
    }
    let (slots, loans) = storage_at(u, state.boundary.block);
    if !slots.contains(&slot) {
        return None;
    }
    let mut reaching = loans
        .iter()
        .filter(|l| root(u, u.tables.loans[l.index()].address) == Some(slot));
    let loan = *reaching.next()?;
    if reaching.next().is_some()
        || u.tables.loans[loan.index()].address != address
        || !u.tables.loans[loan.index()].mutable
    {
        return None;
    }
    let stored = state.storage.iter().find(|s| s.slot == slot)?;
    if stored.layout != layout || stored.repr != u.types[info.ty.index()].repr {
        return None;
    }
    Some(RuntimeResultSlot {
        source: RuntimeResultSource::ArgumentSlot {
            parameter: 1,
            address,
            slot,
            loan,
            initial: initializer(u, state.boundary.block, slot)?,
        },
        layout,
        offset: stored.offset,
        bytes: 8,
        align: 8,
    })
}
pub(super) fn plan(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    state: &mut ResumeState,
) -> Result<(), String> {
    if !candidate(u, state.boundary.call) {
        return Ok(());
    }
    if state.runtime.is_some() || state.callee.is_some() || state.tail.is_some() {
        return Err("nested no-yield read".into());
    }
    let kind = ordered::host_kind(u, state.boundary.call).unwrap();
    let buffer = argument(u, table, state, kind).ok_or("unproved retained read argument")?;
    let result = ordered::result_cell(u, table, state.boundary.call, state.bytes, true)
        .ok_or("unproved retained read result")?;
    if (result.bytes, result.align)
        != (
            if kind == RuntimeOpKind::OrderedStdinRead {
                16
            } else {
                32
            },
            8,
        )
    {
        return Err("read ABI result differs".into());
    }
    state.bytes = (result.offset + result.bytes).next_multiple_of(8);
    state.runtime = Some(RuntimeOpPlan {
        kind,
        helper: helper(kind).into(),
        results: vec![result, buffer],
        scope: None,
    });
    state.action = ResumeAction::ResumeRuntime(state.boundary.call);
    Ok(())
}
// The enclosing validator independently reconstructs live storage and loans
// before this check; claimed descriptors never grant new liveness or custody.
pub(super) fn check(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    state: &ResumeState,
    after: u64,
) -> bool {
    let Some(runtime) = &state.runtime else {
        return false;
    };
    let Some(kind) = ordered::host_kind(u, state.boundary.call) else {
        return false;
    };
    if !candidate(u, state.boundary.call)
        || runtime.kind != kind
        || runtime.helper != helper(kind)
        || runtime.scope.is_some()
        || state.callee.is_some()
        || state.tail.is_some()
        || runtime.results.len() != 2
    {
        return false;
    }
    let result = &runtime.results[0];
    if !ordered::check_result_cell(u, table, state.boundary.call, after, result, true)
        || (result.bytes, result.align)
            != (
                if kind == RuntimeOpKind::OrderedStdinRead {
                    16
                } else {
                    32
                },
                8,
            )
    {
        return false;
    }
    let cell = &runtime.results[1];
    let RuntimeResultSource::ArgumentSlot {
        parameter: 1,
        address,
        slot,
        loan,
        initial,
    } = cell.source
    else {
        return false;
    };
    let Some(call) = u.tables.calls.get(state.boundary.call.index()) else {
        return false;
    };
    let Some(args) = call.arguments.get(&u.pools.call_arguments) else {
        return false;
    };
    if args.len()
        != if kind == RuntimeOpKind::OrderedStdinRead {
            3
        } else {
            2
        }
    {
        return false;
    }
    let targets: Vec<_> = args.iter().filter(|a| a.parameter == 1).collect();
    if targets.len() != 1
        || targets[0].passing != Passing::Mut
        || !matches!(targets[0].value,ArgumentValue::Address {address:a,..} if a==address)
        || args.iter().any(|a| {
            a.parameter != 1
                && (a.passing != Passing::Borrow || !matches!(a.value, ArgumentValue::Value(_)))
        })
    {
        return false;
    }
    let Some(info) = u.addresses.get(address.index()) else {
        return false;
    };
    let Some(place) = u.slots.get(slot.index()) else {
        return false;
    };
    if info.base != AddressBase::Slot(slot)
        || info.projection.len != 0
        || !info.unchecked.is_empty()
        || place.ty != info.ty
        || !matches!(place.kind, SlotKind::Local | SlotKind::Temporary)
        || !typed(u, table, kind, info.ty)
        || initializer(u, state.boundary.block, slot) != Some(initial)
    {
        return false;
    }
    let layout = u.types[info.ty.index()].layout;
    let Some(abi) = table.fn_abis.get(call.abi.0 as usize) else {
        return false;
    };
    if abi.caller_site
        || abi.environment
        || !abi.context
        || !abi.destination
        || !abi.parameters.iter().any(|p| {
            p.source == 1 && p.passing == ParameterPassing::ExclusiveAddress && p.layout == layout
        })
    {
        return false;
    }
    let reaching: Vec<_> = state
        .loans
        .iter()
        .filter(|l| root(u, u.tables.loans[l.index()].address) == Some(slot))
        .copied()
        .collect();
    if reaching != [loan]
        || !u.tables.loans[loan.index()].mutable
        || u.tables.loans[loan.index()].address != address
    {
        return false;
    }
    cell.layout == layout
        && cell.bytes == 8
        && cell.align == 8
        && state.storage.iter().any(|s| {
            s.slot == slot
                && s.layout == layout
                && s.repr == u.types[info.ty.index()].repr
                && s.offset == cell.offset
        })
}
