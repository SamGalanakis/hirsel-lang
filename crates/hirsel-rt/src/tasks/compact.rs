//! Compact frames under the shared physical dispatcher (resume-dispatch-51).
//!
//! A resumable task body saved at a resume point can leave its stack for
//! good: `pool::free::leave_compact` switches away, and the idle continuation
//! moves the task's context to a stable home ([`saved`]) and gives the stack
//! back to its worker. The dispatcher owns the saved frame alone until some
//! worker claims it and lends it a stack of its own ([`activate`]), on which
//! [`entry`] continues the dispatcher at the saved state and then ends the
//! task exactly as the task entry does.
//!
//! A frame saves only where its task would wait or is published to another
//! worker ([`parks`]). The leave is physical. Ordered keeps the task's turn,
//! log and queue position and selects the saved frame as it selects any (a
//! published non-head by its moved claim); Free keeps the row
//! ready and remembers the task's own notification until the save, as its
//! physical migration does. No operation, charge or observation is added.
//!
//! Rayon join's ownership (`rayon-core/src/join/mod.rs:132-165`) is the model:
//! the job is pushed once, popped once by whichever worker finds it, and its
//! result is published only after it ran. Hirsel adds the custody step Rayon
//! does not need: the frame is published only after the leaving worker has
//! acknowledged its save, and the stack is reused only after that unwind.

use super::*;
use crate::context::RunShared;

/// A context's stable home once its first stack is gone. The context is
/// destroyed in place, as one lying at a stack's top is; dropping the home
/// only frees its memory.
#[repr(C, align(64))]
pub(super) struct ContextHome(std::cell::UnsafeCell<std::mem::MaybeUninit<TaskCtx>>);

/// Whether the running task, at a resume point, can leave its stack at all:
/// only from the stack its task entry started (not one lent by its parent),
/// owned by any compute worker, outside callbacks and private ahead
/// execution. Every other chain (unknown, foreign, pinned) keeps the general
/// stackful path.
///
/// # Safety
/// The running task is at a resume point of the body its task entry called.
pub(crate) unsafe fn movable(ctx: *mut TaskCtx) -> bool {
    // SAFETY: caller contract.
    unsafe {
        // An ahead body saves only as an admitted v4 resumable body, whose
        // ahead code hands the saved frame to its ordinary dispatcher.
        if !crate::pool::free::enabled((*ctx).run_ref())
            || (*ctx).callback_depth != 0
            || ((*ctx).ahead.is_some() || !(*ctx).ahead_physical.is_null())
                && !ordered::ahead::native::resumable(ctx)
        {
            return false;
        }
    }
    let ordered = ordered::engine::on(ctx);
    role::with_role(ctx, |s| {
        let me = s.current;
        let row = s.task(me);
        row.scope.is_some()
            && row.stack.is_some()
            && !row.inline_running
            && row.inline_context.is_none()
            && row.owner == Owner::Any
            && !row.pinned
            && match s.logical {
                Logical::Ordered(_) => ordered,
                Logical::Free(_) => !ordered,
                Logical::Cooperative => false,
            }
    })
}

/// Whether this stable extent belongs to the running continuation's root
/// allocation, including a nested callee frame. No pointer is retained here.
pub(crate) fn contains_frame(ctx: *mut TaskCtx, start: usize, bytes: usize) -> bool {
    role::with_role(ctx, |s| {
        let me = s.current;
        s.task(me).compact_frame.is_some_and(|(base, extent)| {
            start >= base
                && start
                    .checked_add(bytes)
                    .is_some_and(|end| end <= base + extent)
        })
    })
}

/// How a non-head can issue a boundary's operation without waiting for head.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Ahead {
    /// Every op it performs is logged (class D).
    pub(crate) logged: bool,
    /// Every op commits early (§4.1 E) or is logged.
    pub(crate) early: bool,
}

/// Whether the running task saves its frame at this resume point: it is
/// published to another worker (forced migration, which saves every frame,
/// in place where it cannot leave), or it can leave its stack and would wait
/// here before issuing the boundary's operation (Ordered: it is not a ready
/// head, and the frontier would neither log the operation nor commit it
/// early now). `ahead` says how a non-head can issue it. Free waits only
/// inside its operations, so a Free frame saves only when published. Reads
/// only; the save itself is [`leave`].
///
/// # Safety
/// The running task is at a resume point of the body its task entry called.
pub(crate) unsafe fn parks(ctx: *mut TaskCtx, ahead: Ahead) -> bool {
    // SAFETY: caller contract.
    if crate::pool::free::migrating(unsafe { (*ctx).run_ref() }) {
        return true;
    }
    // SAFETY: caller contract.
    (unsafe { movable(ctx) }) && ordered::engine::on(ctx) && ordered::engine::waits(ctx, ahead)
}

/// Bind the running task's compact frame image while its resumable body
/// runs, or clear it.
pub(crate) fn bind_frame(ctx: *mut TaskCtx, frame: Option<(usize, usize)>) {
    role::with_role(ctx, |s| {
        let me = s.current;
        s.task(me).compact_frame = frame;
    });
}

/// Whether the running task's end can hold its result from a saved compact
/// frame instead of parking on its stack: its result's `size` bytes at
/// `slot` lie in its compact frame image, which outlives the stack, and the
/// task can leave its stack. Its end then returns `STATUS_SUSPENDED` to the
/// boundary call, the frame leaves, and the reissued end continues after the
/// hold (Tokio's harness goes idle only on `Pending`,
/// `runtime/task/harness.rs:193-231`; here the end's first half is that poll).
///
/// # Safety
/// The running task is in its task end, called from its body.
pub(crate) unsafe fn can_hold(ctx: *mut TaskCtx, slot: *mut u8, size: u64) -> bool {
    let inside = role::with_role(ctx, |s| {
        let me = s.current;
        s.task(me).compact_frame.is_some_and(|(base, bytes)| {
            let start = slot as usize;
            start >= base && start.saturating_add(size as usize) <= base + bytes
        })
    });
    // SAFETY: caller contract.
    inside && unsafe { movable(ctx) }
}

/// Record that the running task's end holds its result from its saved
/// frame: [`leave`] publishes no wake of its own (only the parent's hand-off
/// resumes it), and the reissued end continues after the hold.
pub(crate) fn hold(ctx: *mut TaskCtx) {
    role::with_role(ctx, |s| {
        let me = s.current;
        assert!(!std::mem::replace(&mut s.task(me).hold_left, true));
    });
}

/// Whether this end is the reissue of one that held its result from a saved
/// frame; it then continues after the hold.
pub(crate) fn held(ctx: *mut TaskCtx) -> bool {
    role::with_role(ctx, |s| {
        let me = s.current;
        std::mem::take(&mut s.task(me).hold_left)
    })
}

/// The op count a resume point starts: 0 when its boundary's operation can
/// leave from its first op (`RESUME_LEAVES`), else saturated, so no op
/// before the next such point is ever its first. A task starts saturated:
/// ops begun before its first resume point, such as those of a closure
/// some library body runs, have no boundary call to reissue.
pub(crate) fn point(leaves: bool) -> u32 {
    if leaves { 0 } else { u32::MAX }
}

/// Whether the running op's replay wait can leave its stack: the task runs
/// a compact frame bound by its entry, can leave its stack, its resume
/// point's operation can leave (`RESUME_LEAVES`), and it began no op since
/// that point but this one. The op then parks logically and returns
/// `STATUS_SUSPENDED` to the boundary call, directly or through its library
/// body (`channel.send`'s, `mutex.with`'s); the wake activates the frame,
/// which reissues that call (Go's `gopark` parks the goroutine, not its
/// caller's stack, `proc.go:457-492`).
///
/// # Safety
/// The running task holds the role inside an op of its boundary call.
pub(crate) unsafe fn can_wait(ctx: *mut TaskCtx) -> bool {
    let bound = role::with_role(ctx, |s| {
        let me = s.current;
        s.task(me).compact_frame.is_some()
    });
    // SAFETY: caller contract.
    bound && unsafe { (*ctx).point_ops == 1 && movable(ctx) }
}

/// Record that the running op left its stack in its replay wait.
pub(crate) fn wait(ctx: *mut TaskCtx) {
    // SAFETY: the running task owns its context.
    assert!(!std::mem::replace(unsafe { &mut (*ctx).wait_left }, true));
}

/// Whether this op is the reissue of one that left its stack in its replay
/// wait; it then continues that wait.
pub(crate) fn waited(ctx: *mut TaskCtx) -> bool {
    // SAFETY: the running task owns its context.
    std::mem::take(unsafe { &mut (*ctx).wait_left })
}

/// Whether the running task can leave its stack at this resume point. If it
/// can, its logical policy records the physical leave, and the caller must
/// leave: Ordered marks the frame saving (a wait resumes at head; a published
/// frame keeps its claim and may move on before it), Free remembers its own
/// wake.
///
/// # Safety
/// The running task holds the role at a resume point of the body its task
/// entry called.
pub(crate) unsafe fn leave(ctx: *mut TaskCtx) -> bool {
    // SAFETY: caller contract.
    if !unsafe { movable(ctx) } {
        return false;
    }
    let holding = role::with_role(ctx, |s| {
        let me = s.current;
        s.task(me).hold_left
    });
    if ordered::engine::on(ctx) {
        let published = crate::pool::free::migrating(unsafe { (*ctx).run_ref() });
        // A held end resumes when its parent calls its rest, as its park would.
        ordered::engine::transfer(ctx, !published && !holding);
    } else if holding || unsafe { (*ctx).wait_left } {
        // Only the parent's hand-off makes a holding task Ready again, and
        // only its dependency's wake a waiting one.
    } else {
        // SAFETY: the role is held and no table borrow survives this call.
        unsafe {
            let me = super::running_key(ctx);
            crate::pool::free::schedule_saved((*ctx).run_ptr(), me, Owner::Any);
        }
    }
    true
}

/// The idle continuation of a compact leave, after the switch away from the
/// task's stack: the context moves once to its stable home, and the stack is
/// returned for the caller to give back to its worker.
///
/// # Safety
/// The calling idle worker owns `task`'s exclusive Running image, whose leave
/// switched away from its stack for good, and holds no role.
pub(crate) unsafe fn saved(run: *mut RunShared, task: TaskKey) -> (*mut TaskCtx, Stack) {
    // SAFETY: the worker stays busy and retains its lease meanwhile.
    let _role = unsafe { role::IdleGuard::new(run) };
    // SAFETY: IdleGuard exclusively owns the role-protected rows.
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut() }.expect("a run with tasks");
    let row = ordered::ahead::physical(s, task)
        .unwrap_or_else(|| fault("a compact leave of a stale task key"));
    let stack = row
        .stack
        .take()
        .unwrap_or_else(|| fault("a compact leave without its task's stack"));
    if row.context_home.is_none() {
        let home = Box::new(ContextHome(std::cell::UnsafeCell::new(
            std::mem::MaybeUninit::uninit(),
        )));
        let to = home.0.get().cast::<TaskCtx>();
        // SAFETY: the context at the stack's top is the task's alone while
        // its image is Running; no pointer into it survives this move but the
        // row's, rebound here, and the image's, which the caller rebinds.
        // Every heap view it carries is rebound at its next activation.
        unsafe { to.copy_from_nonoverlapping(row.ctx, 1) };
        row.ctx = to;
        row.context_home = Some(home);
    }
    // SAFETY: as above; the old stack pointer names a stack that is gone.
    unsafe { (*row.ctx).task_sp = std::ptr::null_mut() };
    // An ahead body's physical record owns nothing once its frame continued
    // in the ordinary dispatcher: the activation is an ordinary compact one.
    unsafe {
        if !(*row.ctx).ahead_physical.is_null() {
            (*row.ctx).ahead_physical = std::ptr::null_mut();
            (*row.ctx).ahead = None;
        }
    }
    (row.ctx, stack)
}

/// Lend `stack` to `task`'s compact frame, which the calling worker claimed:
/// the stack pointer that starts [`entry`] on it.
///
/// # Safety
/// The caller holds the role for `task`, whose exclusive saved image it owns.
pub(crate) unsafe fn activate(run: *mut RunShared, task: TaskKey, stack: Stack) -> *mut u8 {
    // SAFETY: the role exclusively owns the rows.
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut() }.expect("a run with tasks");
    let row = ordered::ahead::physical(s, task)
        .unwrap_or_else(|| fault("a compact activation of a stale task key"));
    assert!(row.context_home.is_some(), "a compact frame off its home");
    let ctx = row.ctx;
    let sp = stack.prepare(entry, ctx.cast(), std::ptr::null_mut());
    // SAFETY: the claimed task's context is exclusively the claimant's.
    unsafe { (*ctx).task_sp = sp };
    assert!(
        row.stack.replace(stack).is_none(),
        "a compact frame on two stacks"
    );
    sp
}

/// The fiber entry of a compact frame on a lent stack: the rest of its body
/// from the saved state, then the task entry's suffix.
unsafe extern "C" fn entry(ctx: *mut u8, _: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    // SAFETY: this lent fiber owns its task's stable context.
    let _role = unsafe { role::Guard::physical(ctx) };
    recycle(unsafe { &mut *ctx });
    #[cfg(feature = "measurement")]
    profile::resume(ctx);
    // SAFETY: the dispatcher claimed this frame for this activation.
    let (mut status, environment, site) = unsafe { crate::continuation::resume_compact(ctx) };
    let index = unsafe { (*ctx).task_id };
    unsafe { ordered::ahead::barrier(ctx) };
    status |= unsafe {
        crate::release::release_block(&mut *ctx, environment, std::ptr::null(), site, site)
    };
    unsafe { complete(&mut *ctx, index, status) }
}
