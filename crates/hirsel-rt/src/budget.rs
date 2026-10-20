//! Run budgets natively (`budgets.limits@1`, docs/spec/determinism.md "Budgets").
//!
//! Fuel is the `Compiled::fuel` word a fuel build's code charges at each fuel point
//! (`hirsel_lir::fuel`); a negative result calls [`hirsel_rt_v2_fuel_exhausted`]. Memory is
//! the [`Meter`] one context's heap and region pool share: every slab, large or bare block
//! and region segment taken from the backing allocator is charged before it is taken and
//! credited when it goes back. Without a budget the meter's limit is zero and every charge
//! is one test on a cold path. A spent budget stops the run with
//! `Stop::BudgetExceeded`, as a Blocked run stops (`crate::stop`).
use crate::TaskCtx;
use hirsel_run_schema::BudgetKind;
use std::sync::atomic::{AtomicU64, Ordering};

/// The backing bytes a run holds against its `memory_bytes` budget; zero is no budget.
#[derive(Debug, Default)]
pub(crate) struct Meter {
    limit: AtomicU64,
    held: AtomicU64,
}
impl Meter {
    pub(crate) fn set_limit(&self, limit: u64) {
        self.limit.store(limit, Ordering::Relaxed);
    }
    #[inline]
    pub(crate) fn metered(&self) -> bool {
        self.limit.load(Ordering::Relaxed) != 0
    }
    /// Whether `bytes` more stay within the budget.
    #[inline]
    pub(crate) fn fits(&self, bytes: u64) -> bool {
        !self.metered()
            || self.held.load(Ordering::Relaxed).saturating_add(bytes)
                <= self.limit.load(Ordering::Relaxed)
    }
    /// Charge `bytes` about to be taken, or end the run when they do not fit.
    #[inline]
    pub(crate) fn hold(&self, bytes: u64) {
        if self.metered() {
            if !self.fits(bytes) {
                exceeded(BudgetKind::Memory);
            }
            self.held.fetch_add(bytes, Ordering::Relaxed);
        }
    }
    /// Credit `bytes` given back.
    #[inline]
    pub(crate) fn give(&self, bytes: u64) {
        if self.metered() {
            let mut held = self.held.load(Ordering::Relaxed);
            while let Err(current) = self.held.compare_exchange_weak(
                held,
                held.saturating_sub(bytes),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                held = current;
            }
        }
    }
}

/// Stop the running run: the meter is shared by its context's heap and region pool, which
/// only the running context charges.
#[cold]
#[inline(never)]
fn exceeded(kind: BudgetKind) -> ! {
    let ctx = crate::stop::running();
    if ctx.is_null() {
        crate::environmental("a run spent its budget outside its launcher");
    }
    // SAFETY: the running context is live while its run executes.
    unsafe { crate::stop::raise(ctx, crate::stop::Stop::BudgetExceeded(kind)) }
}

/// A fuel point found the run's fuel spent: the run ends with BudgetExceeded (fuel).
///
/// # Safety
/// `ctx` is the running context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_fuel_exhausted(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    unsafe { crate::stop::raise(ctx, crate::stop::Stop::BudgetExceeded(BudgetKind::Fuel)) }
}
