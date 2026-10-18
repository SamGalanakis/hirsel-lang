//! Finalizers: shielded user drop bodies with field continuation (FAIL-3, DROP-2).
//!
//! A resource moves `Live -> Finalizing -> FieldsPending -> Dead`. Its user drop body runs with
//! borrowed self while the context is shielded (cancellation checks are suppressed; deterministic
//! failures are not). If the body fails, the failure is already recorded; the runtime still
//! releases the remaining fields and siblings, and reports the failure after cleanup. A failing
//! finalizer of a type declared in an `unsafe ffi` package ends the process instead.

use crate::alloc::Header;
use crate::context::RunCtx;
use crate::release::{LAYOUT_UNSAFE_FFI, Layout, drain, push};
use crate::{STATUS_FAILURE, STATUS_OK, fault};

/// A user drop body: borrowed `value`, the cleanup `anchor`, and the status protocol. On failure
/// it records the failure with `hirsel_rt_v2_failure_begin` after cleaning its own locals.
pub type DropThunk =
    unsafe extern "C" fn(ctx: *mut RunCtx, value: *mut u8, anchor: u64, operation_site: u64) -> u8;

/// The Io adapter's handle close for a dropped handle: the context, the handle and the cleanup
/// `anchor`, with the drop thunk's status protocol.
pub type CloseThunk =
    unsafe extern "C" fn(ctx: *mut RunCtx, handle: u64, anchor: u64, operation_site: u64) -> u8;

/// Compiler-reserved cleanup continuation storage for an inline value (a frame or region
/// resource). It is a worklist item shaped like a dead header, so dropping an inline value uses
/// the same constant-stack engine as heap release.
#[repr(C)]
#[derive(Debug)]
pub struct Cleanup {
    pub index: u32,
    pub phase: u32,
    pub cursor: u64,
    pub value: *mut u8,
    pub parent: *mut Header,
}

/// Run `layout`'s own finalizer on `value`, shielded. Returns the accumulated status.
pub(crate) unsafe fn run_finalizer(
    ctx: &mut RunCtx,
    layout: *const Layout,
    value: *mut u8,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: `layout` is a valid descriptor.
    let (finalizer, flags) = unsafe { ((*layout).finalizer, (*layout).flags) };
    let Some(thunk) = finalizer else {
        return STATUS_OK;
    };
    // SAFETY: the thunk receives borrowed self and this context.
    let status = shielded(ctx, |ctx| unsafe {
        thunk(ctx, value, anchor, operation_site)
    });
    if status == STATUS_FAILURE && flags & LAYOUT_UNSAFE_FFI != 0 {
        terminate_unsafe_ffi();
    }
    status
}

/// Close the run-context `handle` through the Io adapter's close thunk, shielded like a
/// finalizer; a channel endpoint closes in the task scheduler instead. A handle drop without an
/// adapter is an invariant fault.
pub(crate) unsafe fn run_close(
    ctx: &mut RunCtx,
    handle: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    if handle & crate::channels::ENDPOINT != 0 {
        // SAFETY: the dropped value owned this endpoint of the run.
        return shielded(ctx, |ctx| unsafe {
            crate::tasks::close_endpoint(ctx, handle, anchor, operation_site)
        });
    }
    let Some(close) = ctx.close_handle else {
        fault("a handle drop in a context without an Io adapter's close_handle");
    };
    // SAFETY: the adapter receives this context and the handle the dropped value owned.
    shielded(ctx, |ctx| unsafe {
        close(ctx, handle, anchor, operation_site)
    })
}

/// Run user or adapter cleanup code shielded (cancellation suppressed) and check its status
/// protocol: 0, or 1 with a failure recorded; no release work may be left pending.
fn shielded(ctx: &mut RunCtx, call: impl FnOnce(*mut RunCtx) -> u8) -> u8 {
    let work = ctx.work;
    ctx.shield += 1;
    let status = call(ctx);
    ctx.shield -= 1;
    if ctx.work != work {
        fault("a finalizer returned with release work still pending");
    }
    match status {
        STATUS_OK => STATUS_OK,
        STATUS_FAILURE => {
            if ctx.heap.diagnostic && !ctx.failure.active() {
                fault("a finalizer failed without recording a failure");
            }
            STATUS_FAILURE
        }
        _ => fault("a finalizer returned a status other than 0 or 1"),
    }
}

/// FAIL-3: a failure inside the drop of a type from an `unsafe ffi` package ends the process.
#[cold]
fn terminate_unsafe_ffi() -> ! {
    eprintln!(
        "hirsel-rt: FAIL-3: a finalizer of a type declared in an unsafe ffi package failed; the process ends"
    );
    std::process::abort()
}

/// Drop the inline value at `value`: run `layout`'s finalizer (unless `live_mask` marks it
/// partially initialized), then its drop program, using `continuation` as the worklist item.
/// T15 FinalizeBegin, T15 FinalizeEnd and T16 DropEdge for each released field.
///
/// # Safety
/// `ctx` is live, `value` holds a value of `layout` whose finalization obligation the caller
/// owns, and `continuation` is writable storage that outlives the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_finalize(
    ctx: *mut RunCtx,
    index: u32,
    value: *mut u8,
    live_mask: *const u64,
    continuation: *mut Cleanup,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        let _ = ctx.descriptor(index);
        let item = finalize_item(continuation, index, value);
        let base = ctx.work;
        push(ctx, item, live_mask.is_null());
        drain(ctx, base, item, live_mask, false, anchor, operation_site).status
    }
}

/// Prepare `continuation` as a worklist item for the inline `value`.
pub(crate) unsafe fn finalize_item(
    continuation: *mut Cleanup,
    index: u32,
    value: *mut u8,
) -> *mut Header {
    // SAFETY: caller passes writable continuation storage.
    unsafe {
        continuation.write(Cleanup {
            index,
            phase: 0,
            cursor: 0,
            parent: std::ptr::null_mut(),
            value,
        });
    }
    continuation
        .cast::<Header>()
        .map_addr(|address| address | 1)
}

/// T15 FinalizeBegin for a resource whose drop body compiled code calls itself: the context is
/// shielded until the matching [`hirsel_rt_v2_finalize_end`]. Shields nest.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_finalize_begin(ctx: *mut RunCtx, _anchor: u64) {
    // SAFETY: caller contract.
    unsafe { (*ctx).shield += 1 }
}

/// T15 FinalizeEnd: the drop body called after [`hirsel_rt_v2_finalize_begin`] has returned,
/// on success or failure, and the shield it raised comes down. An end without a begin is an
/// invariant fault.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_finalize_end(ctx: *mut RunCtx, _anchor: u64) {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        if ctx.shield == 0 {
            fault("a finalizer end without its begin: the context is not shielded");
        }
        ctx.shield -= 1;
    }
}

/// Close the run-context `handle` of a dropped inline value through the Io adapter's close
/// thunk, shielded like a drop body. Returns 0, or 1 with the failure recorded; a context
/// without an adapter's `close_handle` is an invariant fault.
///
/// # Safety
/// `ctx` is live and `handle` is a handle the dropped value owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_close_handle(
    ctx: *mut RunCtx,
    handle: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe { run_close(&mut *ctx, handle, anchor, operation_site) }
}

/// 1 while a finalizer runs: cancellation checks are suppressed.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_shielded(ctx: *mut RunCtx) -> u8 {
    unsafe { ((*ctx).shield > 0) as u8 }
}
