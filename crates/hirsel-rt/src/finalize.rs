//! Finalizers: shielded user drop bodies with field continuation (FAIL-3, DROP-2).
//!
//! A resource moves `Live -> Finalizing -> FieldsPending -> Dead`. Its user drop body runs with
//! borrowed self while the context is shielded (cancellation checks are suppressed; deterministic
//! failures are not). If the body fails, the failure is already recorded; the runtime still
//! releases the remaining fields and siblings, and reports the failure after cleanup. A failing
//! finalizer of a type declared in an `unsafe ffi` package ends the process instead.

use crate::alloc::Header;
use crate::context::TaskCtx;
use crate::release::{LAYOUT_UNSAFE_FFI, Layout, drain, push};
use crate::{STATUS_FAILURE, STATUS_OK, fault};

pub(crate) mod entered;

/// A user drop body: borrowed `value`, the cleanup `anchor`, and the status protocol. On failure
/// it records the failure with `hirsel_rt_v2_failure_begin` after cleaning its own locals.
pub type DropThunk =
    unsafe extern "C" fn(ctx: *mut TaskCtx, value: *mut u8, anchor: u64, operation_site: u64) -> u8;

/// The Io adapter's handle close for a dropped handle: the context, the handle and the cleanup
/// `anchor`, with the drop thunk's status protocol.
pub type CloseThunk =
    unsafe extern "C" fn(ctx: *mut TaskCtx, handle: u64, anchor: u64, operation_site: u64) -> u8;

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
    ctx: &mut TaskCtx,
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
    if flags & crate::release::LAYOUT_IMPLEMENTATION_FINALIZER == 0 {
        let Some(activation) = ctx.activations.admit_finalizer() else {
            ctx.cleanup_rejections = ctx
                .cleanup_rejections
                .checked_add(1)
                .unwrap_or_else(|| fault("finalizer scope count overflow"));
            crate::failure::record_runtime(ctx, "LANG-10", anchor, operation_site);
            if flags & LAYOUT_UNSAFE_FFI != 0 {
                terminate_unsafe_ffi();
            }
            return STATUS_FAILURE;
        };
        // All admitted units share the same 64-unit allowance; this cannot grow.
        if ctx.cleanup_invocations.len() == ctx.cleanup_invocations.capacity() {
            fault("finalizer token storage exceeds its task allowance");
        }
        ctx.cleanup_invocations.push(activation);
    }
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
    ctx: &mut TaskCtx,
    handle: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // This close step owns its semantic entry before dispatching a built-in
    // or adapter. shielded retains cancellation scope and lends the role only
    // while the chosen cleanup runs; every branch shares this outer guard.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if handle & crate::sync::MUTEX != 0 && handle & crate::channels::ENDPOINT == 0 {
        // SAFETY: the wrapper owns the cell; detach before invoking user code.
        return shielded(ctx, |ctx| unsafe {
            crate::tasks::close_mutex(ctx, handle, anchor, operation_site)
        });
    }
    if handle & crate::channels::ENDPOINT != 0 {
        // SAFETY: the dropped value owned this endpoint of the run.
        return shielded(ctx, |ctx| unsafe {
            crate::tasks::close_endpoint(ctx, handle, anchor, operation_site)
        });
    }
    let Some(close) = ctx.run_ref().close_handle else {
        fault("a handle drop in a context without an Io adapter's close_handle");
    };
    // SAFETY: the adapter receives this context and the handle the dropped value owned.
    shielded(ctx, |ctx| unsafe {
        close(ctx, handle, anchor, operation_site)
    })
}

/// Run user or adapter cleanup code shielded (cancellation suppressed) and check its status
/// protocol: 0, or 1 with a failure recorded; no release work may be left pending.
fn shielded(ctx: &mut TaskCtx, call: impl FnOnce(*mut TaskCtx) -> u8) -> u8 {
    // The shield is lifecycle state other tasks read: publish it under the
    // role, which a role-free drain enters only here.
    // SAFETY: this task is running; no scheduler borrow is live.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let work = ctx.work;
    ctx.shield += 1;
    // SAFETY: this semantic entry owns the role, or W1's run token.
    unsafe { crate::tasks::shield_changed(ctx) };
    let status = {
        // SAFETY: the finalizer receives only task-local work, with no live
        // scheduler borrow. Its semantic entries acquire their own role.
        let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
        call(ctx)
    };
    ctx.shield -= 1;
    // SAFETY: Segment reacquired the role before returning above.
    unsafe { crate::tasks::shield_changed(ctx) };
    if ctx.work != work {
        fault("a finalizer returned with release work still pending");
    }
    match status {
        STATUS_OK => STATUS_OK,
        STATUS_FAILURE => {
            if ctx.heap().diagnostic && !ctx.failure.active() {
                fault("a finalizer failed without recording a failure");
            }
            STATUS_FAILURE
        }
        _ => fault("a finalizer returned a status other than 0 or 1"),
    }
}

/// FAIL-3: a failure inside the drop of a type from an `unsafe ffi` package ends the process.
#[cold]
pub(crate) fn terminate_unsafe_ffi() -> ! {
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
    ctx: *mut TaskCtx,
    index: u32,
    value: *mut u8,
    live_mask: *const u64,
    continuation: *mut Cleanup,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // The drain frees task-owned storage; a step that runs user or adapter
    // code or releases a root enters the role itself.
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    unsafe { crate::tasks::role::task_owned(ctx) };
    if let Some(status) = unsafe { crate::tasks::free::tail::invoke_finalize(ctx, index, value) } {
        assert!(
            live_mask.is_null(),
            "a bound inline cleanup has an unproved live mask"
        );
        return status;
    }
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
pub unsafe extern "C" fn hirsel_rt_v2_finalize_begin(ctx: *mut TaskCtx, _anchor: u64) {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    // SAFETY: caller contract.
    unsafe {
        (*ctx).shield += 1;
        (*ctx).cleanup_depth += 1;
        crate::tasks::shield_changed(ctx);
    }
}

/// T15 FinalizeEnd: the drop body called after [`hirsel_rt_v2_finalize_begin`] has returned,
/// on success or failure, and the shield it raised comes down. An end without a begin is an
/// invariant fault.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_finalize_end(ctx: *mut TaskCtx, _anchor: u64) {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        if ctx.shield == 0 {
            fault("a finalizer end without its begin: the context is not shielded");
        }
        ctx.shield -= 1;
        ctx.cleanup_depth = ctx
            .cleanup_depth
            .checked_sub(1)
            .unwrap_or_else(|| fault("cleanup end without cleanup entry"));
        crate::tasks::shield_changed(ctx);
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
    ctx: *mut TaskCtx,
    handle: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    // SAFETY: caller contract.
    unsafe { run_close(&mut *ctx, handle, anchor, operation_site) }
}

/// 1 while a finalizer runs: cancellation checks are suppressed.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_shielded(ctx: *mut TaskCtx) -> u8 {
    unsafe { ((*ctx).shield > 0) as u8 }
}
