//! PROG-4 extern calls and their callbacks, dispatched by S-13 (design §6 Extern dispatch).
//!
//! - **Foreign interval.** From `hirsel_rt_v2_foreign_enter` to `hirsel_rt_v2_foreign_leave`
//!   the task cannot migrate, and C calls its callbacks on the interval's thread, which finds
//!   the task's context there: each thread's innermost interval is its scope. A task switch
//!   saves the thread's scope into the task's context and restores the next task's
//!   (`TaskCtx::switch_in`).
//! - **Effective thread, in precedence order.** A call inside an active interval runs on that
//!   interval's thread; otherwise a call to a `home` package runs on the home thread, blocking
//!   or not; otherwise an `@blocking` call runs on a blocking-pool thread (`crate::pool`),
//!   which owns the task until the call returns; otherwise the call runs on the caller's
//!   worker. At width 1 the caller's worker is the home thread; at width > 1 a home call moves
//!   its task from its compute worker to the home thread for the interval (`HOMED`).
//! - **Stops.** An outermost interval's entry and leave observe the run's stop word: a stop
//!   requested while C runs ends the run when the call returns, never inside it.
//! - **The `serial` gate** of each `serial` package is a logical object of the run
//!   (`crate::tasks::gate_enter`): an outermost call's entry takes it before C is entered,
//!   waiting while another call chain holds it; the owner's nested calls re-enter by depth;
//!   the outermost leave frees it. A blocking call takes it, then moves to the pool.
//! - **Permits.** A pool dispatch gives the caller's compute permit back; a callback's Hirsel
//!   code takes one at its entry, unless its thread holds one, and gives it back at its exit.

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

/// One extern call's caller-owned record, from its enter to its leave: the enclosing scope,
/// its own context, its dispatch word with how it was dispatched, and its start when metrics
/// are on.
#[repr(C)]
struct Interval {
    enclosing: ForeignScope,
    context: *mut TaskCtx,
    word: u64,
    started: u64,
}

/// `hirsel_rt_v2_foreign_enter`'s dispatch word (`hirsel_rt_v2.h`, transcribed in
/// `hirsel_lir::runtime_abi`): the callee package's `threads`, `@blocking`, and the package's
/// ordinal, which names its gate.
pub const FOREIGN_THREADS_HOME: u64 = 0;
pub const FOREIGN_THREADS_SERIAL: u64 = 1;
pub const FOREIGN_THREADS_ANY: u64 = 2;
pub const FOREIGN_THREADS_MASK: u64 = 3;
pub const FOREIGN_BLOCKING: u64 = 4;
pub const FOREIGN_PACKAGE_SHIFT: u32 = 8;
/// Bytes and alignment of the caller-owned interval record.
pub const FOREIGN_SCOPE_BYTES: usize = 40;
pub const FOREIGN_SCOPE_ALIGN: usize = 8;

/// `Interval::word` bits beside the dispatch word's own: the call took its package's gate
/// (outermost), re-entered it (nested), or ran on a pool thread.
const ENTERED: u64 = 1 << 3;
const REENTERED: u64 = 1 << 4;
const POOLED: u64 = 1 << 5;
/// The call moved its task to the home thread, at width > 1 (`crate::tasks::to_home`).
const HOMED: u64 = 1 << 6;
const COMPUTE_PINNED: u64 = 1 << 7;
const HOW: u64 = ENTERED | REENTERED | POOLED | HOMED | COMPUTE_PINNED;
const _: () = assert!(FOREIGN_PACKAGE_SHIFT >= 8 && FOREIGN_BLOCKING & HOW == 0);

// Caller-owned storage in hirsel_rt_v2.h, transcribed in runtime_abi.
const _: () = assert!(std::mem::size_of::<Interval>() == FOREIGN_SCOPE_BYTES);
const _: () = assert!(std::mem::align_of::<Interval>() == FOREIGN_SCOPE_ALIGN);

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

/// Nanoseconds since the first call that asked, for the extern row's counters.
fn now_ns() -> u64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let epoch = *EPOCH.get_or_init(std::time::Instant::now);
    u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn ended(message: &str) -> ! {
    eprintln!("hirsel-rt: PROG-4: {message}; the process ends");
    std::process::abort()
}

/// Dispatch an extern call to its effective thread (S-13), take its package's `serial` gate,
/// and record its context and site as that thread's innermost interval. Returns on the
/// thread the call runs on.
///
/// # Safety
/// `ctx` is the running task's; `previous` is writable for `FOREIGN_SCOPE_BYTES` at
/// `FOREIGN_SCOPE_ALIGN` until foreign_leave returns; `dispatch` is the extern's dispatch
/// word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_foreign_enter(
    ctx: *mut TaskCtx,
    site: u64,
    previous: *mut c_void,
    dispatch: u64,
) {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let enclosing = save();
    let nested = !enclosing.context.is_null();
    if !nested && !unsafe { (*ctx).io_data() }.is_null() {
        let mut isolated = false;
        let queried = unsafe {
            crate::pool::call_home(ctx, &mut || {
                isolated = crate::io::Binding::get(ctx).adapter.isolated_clock();
            })
        };
        unsafe { crate::stop::check(ctx, queried) };
        if isolated {
            unsafe {
                crate::stop::raise(
                    ctx,
                    crate::stop::Stop::Refused("foreign call in timed process clock domain".into()),
                )
            };
        }
    }
    let threads = dispatch & FOREIGN_THREADS_MASK;
    let started = if unsafe { (*ctx).run_ref() }.extern_metrics.is_some() {
        now_ns()
    } else {
        0
    };
    let mut word = dispatch & !HOW;
    if threads == FOREIGN_THREADS_HOME && crate::pool::on_pool() {
        // The build refuses a callback that can reach a home package when it is handed to an
        // extern that does not run on the home thread.
        ended("a home package's extern was called on a blocking-pool thread");
    }
    if !nested {
        // SAFETY: the call holds nothing yet, and no C frame lies below.
        unsafe { crate::stop::check(ctx, crate::stop::observe(ctx)) };
    }
    if !nested
        && threads == FOREIGN_THREADS_HOME
        // SAFETY: the context is the running task's; the call holds nothing yet.
        && unsafe { crate::tasks::to_home(&mut *ctx) }
    {
        word |= HOMED;
    }
    let gate = (dispatch >> FOREIGN_PACKAGE_SHIFT) as u32;
    if threads == FOREIGN_THREADS_SERIAL {
        // SAFETY: the context is the running task's; the call holds nothing yet.
        word |= if unsafe { crate::tasks::gate_enter(&mut *ctx, gate, site) } {
            ENTERED
        } else {
            REENTERED
        };
    }
    if crate::tasks::ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, crate::tasks::ordered::foreign::enter(ctx)) };
    }
    if !nested && threads != FOREIGN_THREADS_HOME && dispatch & FOREIGN_BLOCKING != 0 {
        // SAFETY: as above; this returns on the pool thread.
        let slot = unsafe { crate::tasks::to_pool(&mut *ctx, site) };
        word |= POOLED | u64::from(slot) << 48;
    }
    if !nested && word & HOMED == 0 {
        let pinned = if crate::tasks::ordered::engine::on(ctx) {
            crate::tasks::ordered::foreign::pin(ctx)
        } else if crate::tasks::free::enabled(ctx) {
            crate::tasks::free::foreign::pin(ctx)
        } else {
            false
        };
        if pinned {
            word |= COMPUTE_PINNED;
        }
    }
    // C holds no compute permit. Nested calls reacquire on return so a callback's
    // Hirsel code is accounted even when it makes another synchronous extern call.
    crate::pool::release_permit(ctx);
    // The thread the call runs on, which may have changed above.
    restore(ForeignScope { context: ctx, site });
    // SAFETY: caller contract.
    unsafe {
        previous.cast::<Interval>().write(Interval {
            enclosing,
            context: ctx,
            word,
            started,
        })
    };
    crate::tasks::free::replay::start(ctx);
}

/// Restore the enclosing extern's site and context after a call returns, return the task to
/// the caller's worker from a pool thread, and leave its package's gate.
///
/// # Safety
/// `previous` is the live storage the matching foreign_enter initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_foreign_leave(previous: *const c_void) {
    // SAFETY: caller contract.
    let interval = unsafe { previous.cast::<Interval>().read() };
    restore(interval.enclosing);
    let ctx = interval.context;
    // SAFETY: the saved interval belongs to this task and the C call returned.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if interval.word & POOLED != 0 {
        // SAFETY: the context is the running task's, owned by this pool thread; this returns
        // on the home thread.
        unsafe { crate::tasks::from_pool(&mut *ctx, (interval.word >> 48) as u16) };
    }
    if interval.word & (ENTERED | REENTERED) != 0 {
        let gate = ((interval.word & ((1 << 48) - 1)) >> FOREIGN_PACKAGE_SHIFT) as u32;
        crate::tasks::gate_leave(ctx, gate, interval.word & ENTERED != 0);
    }
    if interval.word & HOMED != 0 {
        // SAFETY: the context is the running task's, on the home thread, with no C frame
        // of its call below: a stop requested while C ran ends the run here.
        unsafe {
            crate::stop::check(ctx, crate::stop::observe(ctx));
            crate::tasks::from_home(&mut *ctx);
        }
    }
    if interval.word & COMPUTE_PINNED != 0 {
        if crate::tasks::ordered::engine::on(ctx) {
            crate::tasks::ordered::foreign::unpin_compute(ctx);
        } else {
            crate::tasks::free::foreign::unpin_compute(ctx);
        }
    }
    // The call returned to a supported boundary. A host stop ends it here;
    // otherwise ordinary Hirsel code takes the process permit again.
    unsafe {
        crate::stop::check(ctx, crate::stop::observe(ctx));
    }
    {
        // Another callback can hold the last permit while entering a runtime
        // operation. Never wait for its permit while holding that operation's
        // scheduler role. No runtime borrow survives this segment boundary.
        let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
        crate::pool::acquire_permit(ctx);
    }
    // SAFETY: the context is live.
    if let Some(metrics) = unsafe { &(*ctx).run_ref().extern_metrics } {
        metrics.call(std::time::Duration::from_nanos(
            now_ns().saturating_sub(interval.started),
        ));
    }
    crate::tasks::free::replay::start(ctx);
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

/// The run context of this thread's extern call, whose callback begins: it takes a compute
/// permit unless its thread holds one. A callback reached on a thread without one (C called
/// it after the call or from another thread) breaks its contract: the process ends.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_callback_context() -> *mut TaskCtx {
    let ctx = CURRENT.with(|c| c.get().context);
    if ctx.is_null() {
        ended("C called a callback outside the extern call on its thread");
    }
    unsafe {
        crate::stop::check(ctx, crate::stop::observe(ctx));
    }
    // SAFETY: this is the interval's task, running exclusively on this thread.
    if unsafe { crate::depth::enter_callback(ctx) } != crate::STATUS_OK {
        ended("a callback failed");
    }
    let took = crate::pool::acquire_permit(ctx);
    // SAFETY: the context is the interval's task's, running on this thread.
    unsafe {
        let depth = (*ctx).callback_depth;
        if depth >= u64::BITS {
            ended("callbacks nest deeper than the runtime tracks");
        }
        (*ctx).callback_permits = (*ctx).callback_permits << 1 | u64::from(took);
        (*ctx).callback_depth = depth + 1;
    }
    ctx
}

/// A callback returns to C: the compute permit its entry took goes back.
///
/// # Safety
/// `ctx` is the context `hirsel_rt_v2_callback_context` returned to this callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_callback_leave(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    let took = unsafe {
        if (*ctx).callback_depth == 0 {
            crate::fault("a callback left that never entered");
        }
        let took = (*ctx).callback_permits & 1 != 0;
        (*ctx).callback_permits >>= 1;
        (*ctx).callback_depth -= 1;
        took
    };
    // SAFETY: callback_context admitted this callback before returning ctx.
    unsafe {
        crate::depth::hirsel_rt_v2_depth_leave(ctx);
    }
    if took {
        crate::pool::release_permit(ctx);
    }
}

/// PROG-4: a language failure inside a callback ends the process, since the C frames
/// between it and the extern call cannot run cleanup.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_callback_failed(_ctx: *mut TaskCtx) {
    ended("a callback failed")
}

/// Turn on `ctx`'s run's extern row counters (design §5 Headroom); a run without them pays
/// one branch per call and reads no clock.
///
/// # Safety
/// `ctx` is a live context of a run that has not started.
pub unsafe fn enable_metrics(ctx: *mut TaskCtx) -> std::sync::Arc<crate::pool::ExternMetrics> {
    let metrics = std::sync::Arc::new(crate::pool::ExternMetrics::default());
    // SAFETY: caller contract.
    unsafe { (*ctx).run().extern_metrics = Some(std::sync::Arc::clone(&metrics)) };
    metrics
}

/// Turn on `ctx`'s run's scheduling row counters (design §5 Headroom); a run without them
/// pays one branch per collection point and reads no clock.
///
/// # Safety
/// `ctx` is a live context of a run that has not started.
pub unsafe fn enable_worker_metrics(
    ctx: *mut TaskCtx,
) -> std::sync::Arc<crate::pool::WorkerMetrics> {
    let metrics = std::sync::Arc::new(crate::pool::WorkerMetrics::default());
    // SAFETY: caller contract.
    unsafe { (*ctx).run().worker_metrics = Some(std::sync::Arc::clone(&metrics)) };
    metrics
}

/// `serial` gate `gate`'s version in `ctx`'s run: its outermost entries and leaves so far.
///
/// # Safety
/// `ctx` belongs to the current task, or no thread is running the run's code.
pub unsafe fn gate_version(ctx: *mut TaskCtx, gate: u32) -> u64 {
    crate::tasks::gate_version(ctx, gate)
}
