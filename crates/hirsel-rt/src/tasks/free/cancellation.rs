//! Task cancellation and observed checks publish through the running owner.
//! Linked-child traversal and wake storage are prepared before the section;
//! no target row or recorder is borrowed across a physical suspension.

use super::super::*;

#[cfg(test)]
mod tests;

/// # Safety
/// The role is held; the handles name a child in the given live scope.
pub(crate) unsafe fn cancel(ctx: *mut TaskCtx, scope: u64, task: u64) -> Stopped<()> {
    super::record::begin(ctx);
    let s = scheduler(ctx) as *mut Scheduler;
    let scope = unsafe { (*s).scope_handle(scope) };
    let child = unsafe { (*s).task_handle(task, scope) };
    let targets = unsafe { (*s).prepare_cancel(child) };
    let publication = super::record::publish(ctx, Phase::Done, |_| unsafe {
        (*s).cancel_prepared(child, CANCEL_REQUESTED, &targets);
    });
    drop(targets);
    publication?;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held, ctx is the current owner, and out writable.
pub(crate) unsafe fn check(ctx: *mut TaskCtx, out: *mut u32) -> Stopped<()> {
    super::record::begin(ctx);
    let s = scheduler(ctx) as *mut Scheduler;
    let shield = unsafe { (*ctx).shield };
    super::record::publish(ctx, Phase::Done, |writer| {
        let cause = unsafe { (*s).canceled(shield) };
        if shield == 0 {
            if let Some(writer) = writer {
                writer.cancel(cause);
            }
        }
        unsafe { out.write(cause.map_or(0, |cause| cause + 1)) };
    })?;
    unsafe { try_yield(&mut *ctx) }
}
