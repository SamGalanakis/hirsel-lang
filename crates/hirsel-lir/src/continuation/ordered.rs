//! Retained result cells for the reviewed Ordered host entries. This is a
//! storage proof; only the independently checked sleep entry is admitted.
use super::*;
use crate::abi_types::CallableShape;

pub(super) fn host_kind(u: &LirUnit<Ready>, call: CallId) -> Option<RuntimeOpKind> {
    let site = u.tables.calls.get(call.index())?;
    let Callee::Operation(op) = site.callee else {
        return None;
    };
    let name = core_name(&u.tables.operations.get(op.index())?.definition)?;
    Some(match name.as_str() {
        "clock.sleep_until" => RuntimeOpKind::OrderedSleepUntil,
        "stdin.read" => RuntimeOpKind::OrderedStdinRead,
        "watcher.next" => RuntimeOpKind::OrderedWatchNext,
        "out.print" => RuntimeOpKind::OrderedPrint,
        _ => return None,
    })
}

/// Derive a dedicated uninitialized ABI result cell after all other frame
/// storage. It becomes initialized only on the intrinsic's success edge.
/// Borrowed results and mut arguments need a different lifetime proof.
pub fn ordered_host_result(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    call: CallId,
    after: u64,
) -> Option<RuntimeResultSlot> {
    result_cell(u, table, call, after, false)
}

pub(super) fn result_cell(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    call: CallId,
    after: u64,
    read: bool,
) -> Option<RuntimeResultSlot> {
    host_kind(u, call)?;
    let site = u.tables.calls.get(call.index())?;
    let abi = table.fn_abis.get(site.abi.0 as usize)?;
    if abi.shape != CallableShape::Definition || site.pretransfer_failure.is_some() {
        return None;
    }
    let result = abi.result.as_ref()?;
    if !result.borrowed_parameters.is_empty() {
        return None;
    }
    let value = site.result?;
    let ty = u.types.get(u.values.get(value.index())?.ty.index())?;
    if ty.layout != result.layout || ty.repr == ValueRepr::Hole {
        return None;
    }
    let arguments = site.arguments.get(&u.pools.call_arguments)?;
    if !read
        && arguments
            .iter()
            .any(|a| matches!(a.value, ArgumentValue::Address { .. }))
    {
        return None;
    }
    let (bytes, align) = fixed(table, result.layout)?;
    let offset = after.checked_next_multiple_of(align)?;
    let end = offset.checked_add(bytes)?.checked_next_multiple_of(8)?;
    if align > 16 || end > MAX_FRAME_BYTES {
        return None;
    }
    Some(RuntimeResultSlot {
        source: RuntimeResultSource::CallResult(value),
        layout: result.layout,
        offset,
        bytes,
        align,
    })
}

/// Independently reconstruct the runtime result cell at import time. Do not
/// trust the producer's claimed value, ABI layout, size, alignment or offset.
/// `after` is the already-validated end of the other live frame storage.
pub fn check_ordered_host_result(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    call: CallId,
    after: u64,
    claimed: &RuntimeResultSlot,
) -> bool {
    check_result_cell(u, table, call, after, claimed, false)
}

pub(super) fn check_result_cell(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    call: CallId,
    after: u64,
    claimed: &RuntimeResultSlot,
    read: bool,
) -> bool {
    if host_kind(u, call).is_none() {
        return false;
    }
    let Some(site) = u.tables.calls.get(call.index()) else {
        return false;
    };
    let Some(abi) = table.fn_abis.get(site.abi.0 as usize) else {
        return false;
    };
    let Some(result) = abi.result.as_ref() else {
        return false;
    };
    let Some(value) = site.result else {
        return false;
    };
    let Some(info) = u.values.get(value.index()) else {
        return false;
    };
    let Some(ty) = u.types.get(info.ty.index()) else {
        return false;
    };
    let Some(layout) = table.layout(ty.layout) else {
        return false;
    };
    let SizeSpec::Fixed(bytes) = layout.size else {
        return false;
    };
    let align = u64::from(layout.align.max(1));
    let Some(offset) = after.checked_next_multiple_of(align) else {
        return false;
    };
    let Some(end) = offset
        .checked_add(bytes)
        .and_then(|n| n.checked_next_multiple_of(8))
    else {
        return false;
    };
    let Some(arguments) = site.arguments.get(&u.pools.call_arguments) else {
        return false;
    };
    abi.shape == CallableShape::Definition
        && site.pretransfer_failure.is_none()
        && result.borrowed_parameters.is_empty()
        && result.layout == ty.layout
        && ty.repr != ValueRepr::Hole
        && (read
            || !arguments
                .iter()
                .any(|a| matches!(a.value, ArgumentValue::Address { .. })))
        && align <= 16
        && end <= MAX_FRAME_BYTES
        && claimed.source == RuntimeResultSource::CallResult(value)
        && claimed.layout == ty.layout
        && claimed.offset == offset
        && claimed.bytes == bytes
        && claimed.align == align
}

pub(super) fn sleep(u: &LirUnit<Ready>, call: CallId) -> bool {
    host_kind(u, call) == Some(RuntimeOpKind::OrderedSleepUntil)
}

pub(super) fn plan(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    state: &mut ResumeState,
) -> Result<(), String> {
    if !sleep(u, state.boundary.call) {
        return Ok(());
    }
    if state.callee.is_some() || state.runtime.is_some() {
        return Err("nested or entered frame at clock.sleep_until".into());
    }
    let result = ordered_host_result(u, table, state.boundary.call, state.bytes)
        .ok_or("unproved clock.sleep_until result cell")?;
    state.bytes = result
        .offset
        .checked_add(result.bytes)
        .and_then(|n| n.checked_next_multiple_of(8))
        .ok_or("clock.sleep_until frame overflow")?;
    state.runtime = Some(RuntimeOpPlan {
        kind: RuntimeOpKind::OrderedSleepUntil,
        helper: "hirsel_rt_v2_ordered_sleep_until".into(),
        results: vec![result],
        scope: None,
    });
    state.action = ResumeAction::ResumeRuntime(state.boundary.call);
    Ok(())
}

pub(super) fn check_sleep(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    state: &ResumeState,
    after: u64,
) -> bool {
    let Some(runtime) = &state.runtime else {
        return false;
    };
    // Reconstruct identity from the actual operation, never from the claimed
    // runtime kind. The result-cell checker reconstructs the ABI and extent.
    let Some(site) = u.tables.calls.get(state.boundary.call.index()) else {
        return false;
    };
    let Callee::Operation(op) = site.callee else {
        return false;
    };
    let Some(operation) = u.tables.operations.get(op.index()) else {
        return false;
    };
    operation.definition.is_core("clock.sleep_until")
        && state.callee.is_none()
        && runtime.kind == RuntimeOpKind::OrderedSleepUntil
        && runtime.helper == "hirsel_rt_v2_ordered_sleep_until"
        && runtime.scope.is_none()
        && runtime.results.len() == 1
        && check_ordered_host_result(u, table, state.boundary.call, after, &runtime.results[0])
}
