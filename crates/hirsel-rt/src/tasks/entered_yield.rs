//! Once-only scheduling suffix for an entered isolate begin. No production
//! callsite selects this helper until its full retained callchain is checked.
#![allow(dead_code)]

use super::{free, ordered};
use crate::TaskCtx;
use crate::stop::Stopped;

#[derive(Debug, Default)]
pub(crate) enum Yield {
    #[default]
    New,
    Ordered(ordered::yielding::Yield),
    Free,
    YieldWaiting,
    Delivered,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Poll {
    Complete,
    Pending,
}

impl Yield {
    /// # Safety
    /// The running task holds its role and owns this retained record. `leave`
    /// requires the full BoundSite callchain and binding::can_leave proofs,
    /// just as free::try_yield_entered does. No caller-stack owner may escape.
    pub(crate) unsafe fn drive(&mut self, ctx: *mut TaskCtx, leave: bool) -> Stopped<Poll> {
        if matches!(self, Self::New) {
            if !ordered::engine::on(ctx) && !free::enabled(ctx) {
                // The cooperative policy retains its native stack and its
                // ordinary prelude charges/traces the yield exactly once.
                assert!(!leave, "cooperative isolate cannot leave compactly");
                unsafe { super::try_yield(&mut *ctx) }?;
                *self = Self::Delivered;
                return Ok(Poll::Complete);
            }
            #[cfg(feature = "diagnostic")]
            unsafe {
                crate::root_trace::point(&mut *ctx)
            };
            crate::stop::observe(ctx)?;
            if unsafe { (*ctx).run_ref().tasks.is_none() } {
                *self = Self::Delivered;
                return Ok(Poll::Complete);
            }
            if let Some(metrics) = unsafe { &(*ctx).run_ref().worker_metrics } {
                metrics.op();
            }
            *self = if ordered::engine::on(ctx) {
                Self::Ordered(Default::default())
            } else {
                Self::Free
            };
        }
        match self {
            Self::Ordered(yielding) => {
                if !yielding.drive(ctx)? {
                    return Ok(Poll::Pending);
                }
            }
            Self::Free => {
                // All housekeeping and the quantum decision finish before
                // Pending. Reactivation only delivers the original suffix.
                if unsafe { free::try_yield_entered(&mut *ctx, leave) }? {
                    *self = Self::YieldWaiting;
                    return Ok(Poll::Pending);
                }
            }
            Self::YieldWaiting => crate::stop::observe(ctx)?,
            Self::Delivered => panic!("entered isolate yield delivered twice"),
            Self::New => unreachable!(),
        }
        *self = Self::Delivered;
        Ok(Poll::Complete)
    }
}
