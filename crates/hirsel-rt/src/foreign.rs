//! PROG-4 callbacks: C calls a callback on the thread of the extern call that handed it
//! over, so each extern call records its run context for that thread's callback thunks.
use crate::context::RunCtx;
use std::cell::Cell;

thread_local! {
    static CONTEXT: Cell<*mut RunCtx> = const { Cell::new(std::ptr::null_mut()) };
}

/// Records `ctx` as this thread's context for the callbacks of the extern call that follows.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_foreign_enter(ctx: *mut RunCtx) {
    CONTEXT.with(|c| c.set(ctx));
}

/// The run context of this thread's extern call. A callback reached on a thread without one
/// (C called it after the call or from another thread) breaks its contract: the process ends.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_callback_context() -> *mut RunCtx {
    let ctx = CONTEXT.with(Cell::get);
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
pub extern "C" fn hirsel_rt_v2_callback_failed(_ctx: *mut RunCtx) {
    eprintln!("hirsel-rt: PROG-4: a callback failed; the process ends");
    std::process::abort()
}
