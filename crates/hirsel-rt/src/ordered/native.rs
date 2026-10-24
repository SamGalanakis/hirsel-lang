//! Native channel adapter for the ordered role. ABI message images stay in
//! their channel's ring; the receipt names only the pending queue entry.

use crate::TaskCtx;

/// An E send's outcome and retirement obligation. Dropping a receipt after a
/// terminal cut leaves its message hidden, as required for speculative suffixes.
#[must_use = "a successful early send must be published at its turn's retirement"]
pub struct SendReceipt {
    pub status: u32,
    pending: Option<crate::channels::PendingSend>,
}

impl SendReceipt {
    /// Commit the internal part of an eligible E send, leaving `None` in the
    /// ABI carrier on success. No cancellation check or scheduling point.
    ///
    /// # Safety
    /// `ctx` is live. The caller holds the run's scheduler role and has proved
    /// all four early-commit rules for this sender's queued turn. `sender` is
    /// an open Sender of the run and `message` is its writable `Some` image.
    pub unsafe fn commit(ctx: *mut TaskCtx, sender: u64, message: *mut u8) -> Self {
        let (status, pending) = crate::tasks::role::with_role(ctx, |s| {
            let channel = s.channels.channel(sender, true);
            // SAFETY: caller contract.
            unsafe { s.channels.send_pending(channel, message) }
        });
        Self { status, pending }
    }

    /// Retirement step 3, before the sender's scheduling point. This consumes
    /// the receipt, so the same send cannot be published twice by its adapter.
    ///
    /// # Safety
    /// `ctx` is of the same live run; the caller holds the scheduler role and
    /// has settled this send's preceding stretch and reached its retirement.
    pub unsafe fn retire(self, ctx: *mut TaskCtx) {
        if let Some(send) = self.pending {
            crate::tasks::role::with_role(ctx, |s| s.channels.publish(send));
        }
    }
}
