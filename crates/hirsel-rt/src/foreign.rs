//! PROG-4 callbacks: C calls a callback on the thread of the extern call that handed it
//! over, so each extern call records its task context for that thread's callback thunks. The
//! thread's scope belongs to the running task: a task switch saves it into the task's context
//! and restores the next task's (`TaskCtx::switch_in`).
use crate::context::TaskCtx;
use std::cell::Cell;
use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ForeignScope {
    context: *mut TaskCtx,
    site: u64,
}

impl ForeignScope {
    /// No extern call is active.
    pub(crate) const NONE: ForeignScope = ForeignScope {
        context: std::ptr::null_mut(),
        site: 0,
    };
}

// Caller-owned storage in hirsel_rt_v2.h, transcribed in runtime_abi.
const _: () = assert!(std::mem::size_of::<ForeignScope>() == 16);
const _: () = assert!(std::mem::align_of::<ForeignScope>() == 8);

thread_local! {
    static CURRENT: Cell<ForeignScope> = const { Cell::new(ForeignScope::NONE) };
}

/// Whether an extern call is active on this thread: C frames sit between the running Hirsel
/// code and its run's launcher.
#[inline(never)]
pub(crate) fn active() -> bool {
    CURRENT.with(|c| !c.get().context.is_null())
}

/// The thread's scope, which the task switched out keeps.
#[inline(never)]
pub(crate) fn save() -> ForeignScope {
    CURRENT.with(Cell::get)
}

/// Make `scope`, the task switched in's own, the thread's scope.
#[inline(never)]
pub(crate) fn restore(scope: ForeignScope) {
    CURRENT.with(|c| c.set(scope));
}

/// Record this call's context and site, saving its enclosing scope without allocating.
///
/// # Safety
/// `previous` is writable for 16 bytes at alignment 8 until foreign_leave returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_foreign_enter(
    ctx: *mut TaskCtx,
    site: u64,
    previous: *mut c_void,
) {
    let enclosing = CURRENT.with(|c| c.replace(ForeignScope { context: ctx, site }));
    unsafe { previous.cast::<ForeignScope>().write(enclosing) };
}

/// Restore the enclosing extern's site and context after a nested call returns.
///
/// # Safety
/// `previous` is the live storage the matching foreign_enter initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_foreign_leave(previous: *const c_void) {
    let enclosing = unsafe { previous.cast::<ForeignScope>().read() };
    CURRENT.with(|c| c.set(enclosing));
}

/// A null crossed a non-null C boundary. Invalid foreign storage cannot be
/// cleaned up safely: end the process, just as for other foreign violations.
/// A callback thunk passes zero to select its active extern's site.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_foreign_violation(site: u64) -> ! {
    let scope = CURRENT.with(Cell::get);
    let site = if site == 0 { scope.site } else { site };
    let ctx = if scope.context.is_null() {
        crate::stop::running()
    } else {
        scope.context
    };
    eprintln!(
        "hirsel-rt: PROG-4: null at a non-null foreign boundary at {}; the process ends",
        crate::isolate::foreign_location(ctx, site)
    );
    std::process::abort()
}

/// The run context of this thread's extern call. A callback reached on a thread without one
/// (C called it after the call or from another thread) breaks its contract: the process ends.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_callback_context() -> *mut TaskCtx {
    let ctx = CURRENT.with(|c| c.get().context);
    if ctx.is_null() {
        eprintln!(
            "hirsel-rt: PROG-4: C called a callback outside the extern call on its thread; the process ends"
        );
        std::process::abort()
    }
    ctx
}

/// PROG-4: a language failure inside a callback ends the process, since the C frames
/// between it and the extern call cannot run cleanup.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_callback_failed(_ctx: *mut TaskCtx) {
    eprintln!("hirsel-rt: PROG-4: a callback failed; the process ends");
    std::process::abort()
}
