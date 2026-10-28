//! Native TSan mutation of Queued<Image>'s exclusive claim. The second scoped
//! worker resumes the first job's actual saved mapper stack, never a synthetic
//! shared cell. No duplicate reaches cleanup: the campaign requires TSan exit 66.
use super::{Image, Queued};
use crate::TaskCtx;
use std::sync::{Barrier, Condvar, Mutex, OnceLock};

#[derive(Default)]
struct State {
    ctx: usize,
    stolen: bool,
    sp: usize,
    ordinary: bool,
    claimed: bool,
}
static STATE: Mutex<State> = Mutex::new(State {
    ctx: 0,
    stolen: false,
    sp: 0,
    ordinary: false,
    claimed: false,
});
static READY: Condvar = Condvar::new();
static ENTER: Barrier = Barrier::new(2);

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIRSEL_TSAN_DOUBLE_CLAIM").is_some())
}

/// # Safety
/// A queued CPU job, on a scoped worker. Only the sanitizer campaign enables
/// this intentionally invalid second owner; TSan must terminate before return.
pub(super) unsafe fn dispatch(queued: &Queued<Image>) {
    if !enabled() {
        return;
    }
    let ctx = unsafe { (*queued.tsan_unclaimed()).ctx };
    if !unsafe { super::super::cpu_scoped::tsan_job(ctx) } {
        return;
    }
    let mut state = STATE.lock().unwrap();
    if state.ctx == 0 {
        // SAFETY: the queue token has not been started. Keep only its context
        // address, not a borrow of the image, across the real owner's start.
        state.ctx = ctx as usize;
        return;
    }
    if state.stolen || state.ctx == ctx as usize {
        return;
    }
    state.stolen = true;
    while state.sp == 0 {
        state = READY.wait(state).unwrap();
    }
    let sp = state.sp as *mut u8;
    drop(state);
    ENTER.wait();
    let mut idle = std::ptr::null_mut();
    // SAFETY: deliberately violated by this mutant. A real scoped worker
    // takes the *same* mapper stack without the Queued CAS or Option::take.
    // Retain this worker's distinct TSan identity: transferring the exclusive
    // fiber identity a second time would falsely serialize the two owners.
    unsafe { crate::fiber::fiber_switch_raw(&raw mut idle, sp) };
    crate::fault("double-claimed mapper returned without a TSan report");
}

/// Publish the victim's first saved scalar mapper quantum. Deferring the
/// duplicate until here avoids mistaking a scheduler-metadata race for emitted
/// mapper coverage. No job contents, stack bytes or mapper PCs are fabricated.
/// # Safety
/// The legitimate idle owner holds this saved mapper stack, before resumption.
pub(crate) unsafe fn mapper(ctx: *mut TaskCtx, sp: *mut u8) {
    if !enabled() {
        return;
    }
    let mut state = STATE.lock().unwrap();
    if state.ctx != ctx as usize || state.sp != 0 {
        return;
    }
    state.sp = sp as usize;
    READY.notify_one();
    drop(state);
    ENTER.wait();
}

#[repr(C)]
pub(crate) struct Switch {
    save: *mut *mut u8,
    to: *mut u8,
}

/// Prepare the ordinary scalar save after a successful canonical CPU claim.
/// # Safety
/// The running context owns this worker. The naked caller must tail-jump to
/// fiber_switch after refill returns, without retaining any Rust frame.
pub(crate) unsafe extern "C" fn ordinary_prepare(ctx: *mut TaskCtx) -> Switch {
    let none = Switch {
        save: std::ptr::null_mut(),
        to: std::ptr::null_mut(),
    };
    if !enabled() {
        return none;
    }
    let mut state = STATE.lock().unwrap();
    if state.ctx != ctx as usize || !state.claimed || state.sp != 0 || state.ordinary {
        return none;
    }
    state.ordinary = true;
    Switch {
        save: unsafe { &raw mut (*ctx).task_sp },
        to: unsafe { (*super::super::own_exec()).idle.get() },
    }
}

/// Resume the ordinary save without publishing a wake or relinquishing Running.
/// # Safety
/// The dispatcher is on idle and still exclusively owns this context's image.
pub(super) unsafe fn ordinary_saved(ctx: *mut TaskCtx) -> Option<*mut u8> {
    if !enabled() {
        return None;
    }
    {
        let state = STATE.lock().unwrap();
        if state.ctx != ctx as usize || !state.ordinary || state.sp != 0 {
            return None;
        }
    }
    let sp = unsafe { (*ctx).task_sp };
    unsafe { mapper(ctx, sp) };
    Some(sp)
}

/// # Safety
/// The current CPU worker has just successfully claimed its canonical job.
pub(crate) unsafe fn claimed(ctx: *mut TaskCtx) {
    if enabled() {
        let mut state = STATE.lock().unwrap();
        if state.ctx == ctx as usize {
            state.claimed = true;
        }
    }
}
