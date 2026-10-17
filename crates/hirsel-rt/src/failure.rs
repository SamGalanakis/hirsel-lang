//! Failure state: the primary failure and its ordered cause chain (FAIL-3).
//!
//! The first failure recorded while none is active becomes primary. A failure recorded while one
//! is active (a finalizer failing during cleanup) extends the cause chain in order; it never
//! replaces the primary. Nothing unwinds: callers propagate the returned status. Bookkeeping
//! allocation exhaustion is environmental, never a language failure.

use crate::context::RunCtx;
use crate::{STATUS_FAILURE, STATUS_OK};

/// One recorded failure. `kind` and `rule` are compiler-assigned codes (the failure category
/// and the rule ID's index); `site` identifies the source site.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailureRecord {
    pub kind: u32,
    pub rule: u32,
    pub site: u64,
}

#[derive(Default)]
pub(crate) struct FailureState {
    primary: Option<FailureRecord>,
    causes: Vec<FailureRecord>,
}

impl FailureState {
    pub(crate) fn record(&mut self, record: FailureRecord) {
        if self.primary.is_none() {
            self.primary = Some(record);
        } else {
            self.causes.push(record);
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.primary.is_some()
    }

    /// The number of records: the primary and its causes.
    pub(crate) fn count(&self) -> usize {
        usize::from(self.primary.is_some()) + self.causes.len()
    }

    /// Forget every record from the `count`-th on.
    pub(crate) fn truncate(&mut self, count: usize) {
        if count == 0 {
            self.clear();
        } else {
            self.causes.truncate(count - 1);
        }
    }

    /// Forget the failure and return its bookkeeping storage.
    pub(crate) fn clear(&mut self) {
        *self = FailureState::default();
    }
}

/// Record a new failure: primary if none is active, else the next cause. Returns
/// [`STATUS_FAILURE`] so generated code can return it directly.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_failure_begin(
    ctx: *mut RunCtx,
    kind: u32,
    rule: u32,
    site: u64,
) -> u8 {
    unsafe { (*ctx).failure.record(FailureRecord { kind, rule, site }) };
    STATUS_FAILURE
}

/// Record a failure the runtime raises itself, at its fixed runtime code
/// (`hirsel_core_contract::RUNTIME_RULES`). Site 0 is anchored by the caller's
/// failure edge. The kind is FAIL-1's `bug`, the one category.
pub(crate) fn record_runtime(ctx: *mut RunCtx, rule: &str, site: u64) {
    let rule = hirsel_core_contract::runtime_rule_code(rule)
        .unwrap_or_else(|| crate::fault("a rule the runtime does not record"));
    unsafe {
        (*ctx).failure.record(FailureRecord {
            kind: 1,
            rule,
            site,
        })
    };
}

/// Append a failure produced elsewhere (a callback or nested context) with the same rule as
/// `hirsel_rt_v2_failure_begin`.
///
/// # Safety
/// `ctx` is live and `failure` readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_failure_append(
    ctx: *mut RunCtx,
    failure: *const FailureRecord,
) -> u8 {
    unsafe { (*ctx).failure.record(*failure) };
    STATUS_FAILURE
}

/// 1 while a failure is recorded.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_failure_active(ctx: *mut RunCtx) -> u8 {
    unsafe { (*ctx).failure.active() as u8 }
}

/// Write the primary failure to `out` and return 1, or return 0 when none is recorded.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_failure_primary(
    ctx: *mut RunCtx,
    out: *mut FailureRecord,
) -> u8 {
    match unsafe { (*ctx).failure.primary } {
        Some(record) => {
            unsafe { out.write(record) };
            1
        }
        None => STATUS_OK,
    }
}

/// Number of causes recorded after the primary.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_failure_cause_count(ctx: *mut RunCtx) -> u64 {
    unsafe { (*ctx).failure.causes.len() as u64 }
}

/// Write cause `index` (0 is the earliest) to `out` and return 1, or return 0 out of range.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_failure_cause(
    ctx: *mut RunCtx,
    index: u64,
    out: *mut FailureRecord,
) -> u8 {
    let causes = unsafe { &(*ctx).failure.causes };
    match usize::try_from(index)
        .ok()
        .and_then(|index| causes.get(index))
    {
        Some(record) => {
            unsafe { out.write(*record) };
            1
        }
        None => 0,
    }
}

/// Forget the recorded failure once its observer has consumed it.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_failure_clear(ctx: *mut RunCtx) {
    unsafe { (*ctx).failure.clear() };
}

/// Give failures originating in generic library bodies their first source call
/// site. Already anchored source failures and their causes keep their sites.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_failure_reanchor(ctx: *mut RunCtx, site: u64) {
    if site == 0 {
        return;
    }
    let state = unsafe { &mut (*ctx).failure };
    if let Some(primary) = &mut state.primary {
        if primary.site == 0 {
            primary.site = site;
        }
    }
    for cause in &mut state.causes {
        if cause.site == 0 {
            cause.site = site;
        }
    }
}
