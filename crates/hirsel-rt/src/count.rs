//! Plain task-local reference counts (MEM-1, OWN-6).
//!
//! A count is a non-atomic `u32` in the block header. The configured bound `max` is D's model
//! parameter (`proofs/rc`, `Config.max`): finite counts are `1 .. max - 1`, and a dup that would
//! reach `max` makes the block immortal instead. [`COUNT_IMMORTAL`] is sticky, never wraps, never
//! tests unique and ignores later dups and releases. Uncounted storage (region and arena) has
//! no header and never reaches these operations; `Shared` interiors keep their headers, made
//! immortal while frozen (`shared`). Transitions cite the model's IDs: T03 dup, T04 release.

use crate::alloc::{Header, STATE_LIVE};
use crate::context::RunCtx;
use crate::fault;

/// The sticky immortal sentinel.
pub const COUNT_IMMORTAL: u32 = u32::MAX;
/// The count of a block with one holder: its next release kills it.
pub const COUNT_UNIQUE: u32 = 1;

/// The largest finite count under the bound `max` (`Config::count_max`; 0 selects the full
/// width, whose all-ones word is the sentinel).
pub const fn finite_count_max(max: u64) -> u32 {
    if max == 0 {
        u32::MAX - 1
    } else {
        (max - 1) as u32
    }
}

/// Reject an operation on a block that is not a live counted block (diagnostic mode).
pub(crate) unsafe fn check_live_block(ctx: &RunCtx, block: *mut Header) {
    if block.is_null() {
        fault("a null block reference");
    }
    // SAFETY: caller passes a block reference.
    unsafe {
        if ctx.diagnostic_state(block) != STATE_LIVE {
            fault("a count operation on a block that is not live (released twice?)");
        }
        if (*block).descriptor & crate::alloc::WIDE_DEAD != 0 {
            fault("a live block carries a dead wide cursor");
        }
        if (*block).count == 0 {
            fault("a live block has a zero count");
        }
    }
}

/// Add one holder credit (T03 Dup). At the finite maximum the block becomes immortal instead
/// (T03 DupSaturate); an immortal block is unchanged (T03 DupImmortal). Never allocates or fails.
///
/// # Safety
/// `ctx` is live and the caller holds a credit of the live block `block`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_dup(ctx: *mut RunCtx, block: *mut Header) {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        #[cfg(feature = "measurement")]
        {
            ctx.heap.measurement.record("count_increments", 1);
            ctx.heap.measurement.count_increments += 1;
        }
        if ctx.heap.diagnostic {
            check_live_block(ctx, block);
            if (*ctx.descriptor((*block).index())).finalizer.is_some() {
                fault("a dup of a block whose layout has a finalizer: resources are noncopyable");
            }
        }
        let count = (*block).count;
        if count == COUNT_IMMORTAL {
            return;
        }
        if count >= ctx.count_max {
            (*block).count = COUNT_IMMORTAL;
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            {
                ctx.immortalized += 1;
            }
        } else {
            (*block).count = count + 1;
        }
    }
}
