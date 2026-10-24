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
    let (child, targets) = role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        let child = s.task_handle(task, scope);
        (child, s.prepare_cancel(child))
    });
    let publication = super::record::publish(ctx, Phase::Done, |_| unsafe {
        role::with_held(ctx, |s| {
            s.cancel_prepared(child, CANCEL_REQUESTED, &targets)
        });
    });
    drop(targets);
    publication?;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held, ctx is the current owner, and out writable.
pub(crate) unsafe fn check(ctx: *mut TaskCtx, out: *mut u32) -> Stopped<()> {
    super::record::begin(ctx);
    let shield = unsafe { (*ctx).shield };
    let replay = super::replay::token(ctx);
    let publication = super::record::publish(ctx, Phase::Done, |writer| {
        let mut cause = unsafe { role::with_held(ctx, |s| s.canceled(shield)) };
        if shield == 0 {
            if let Some(writer) = writer {
                writer.cancel(cause);
            } else if let Some(replay) = &replay {
                cause = replay.cancel(cause)?;
            }
        }
        unsafe { out.write(cause.map_or(0, |cause| cause + 1)) };
        Ok(())
    });
    drop(replay);
    publication??;
    unsafe { try_yield(&mut *ctx) }
}
