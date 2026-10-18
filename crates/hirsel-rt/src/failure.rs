//! Failure state: the primary failure and its ordered cause chain (FAIL-3).
//!
//! The first failure recorded while none is active becomes primary. A failure recorded while one
//! is active (a finalizer failing during cleanup) extends the cause chain in order; it never
//! replaces the primary. Nothing unwinds: callers propagate the returned status. Bookkeeping
//! allocation exhaustion is environmental, never a language failure. Each task has its own state
//! (the scheduler swaps it in with the task); a joined child's failures keep its spawn path.

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
    pub operation_site: u64,
}

#[derive(Default)]
pub(crate) struct FailureState {
    primary: Option<FailureRecord>,
    causes: Vec<FailureRecord>,
    /// Each record's spawn path, primary first: `None` for a failure of the task this state
    /// belongs to, the child's path for one a join delivered (FAIL-2).
    spawns: Vec<Option<Vec<u32>>>,
}

/// One failure of a completed task's chain, with the spawn path of the task it occurred in.
pub(crate) type Chained = (FailureRecord, Vec<u32>);

impl FailureState {
    pub(crate) fn record(&mut self, record: FailureRecord) {
        self.record_from(record, None);
    }

    fn record_from(&mut self, record: FailureRecord, spawn: Option<Vec<u32>>) {
        if self.primary.is_none() {
            self.primary = Some(record);
        } else {
            self.causes.push(record);
        }
        self.spawns.push(spawn);
    }

    /// The whole chain in order, each record's own failures attributed to `own`; the state is
    /// left empty.
    pub(crate) fn take_chain(&mut self, own: &[u32]) -> Vec<Chained> {
        let state = std::mem::take(self);
        state
            .primary
            .into_iter()
            .chain(state.causes)
            .zip(state.spawns)
            .map(|(record, spawn)| (record, spawn.unwrap_or_else(|| own.to_vec())))
            .collect()
    }

    /// The whole chain in order, each record's own failures attributed to `own`; the state is
    /// kept (FAIL-4: a dead mutex keeps the chain of the body that killed it).
    pub(crate) fn chain(&self, own: &[u32]) -> Vec<Chained> {
        self.primary
            .iter()
            .chain(&self.causes)
            .zip(&self.spawns)
            .map(|(record, spawn)| (*record, spawn.clone().unwrap_or_else(|| own.to_vec())))
            .collect()
    }

    /// The records from the `start`-th on, in order, each of this state's own failures
    /// attributed to `own`; they are forgotten (FAIL-2: `task.isolate` catches them).
    pub(crate) fn split_off(&mut self, start: usize, own: &[u32]) -> Vec<Chained> {
        if start == 0 {
            return self.take_chain(own);
        }
        let causes = self.causes.split_off((start - 1).min(self.causes.len()));
        let spawns = self.spawns.split_off(start.min(self.spawns.len()));
        causes
            .into_iter()
            .zip(spawns)
            .map(|(record, spawn)| (record, spawn.unwrap_or_else(|| own.to_vec())))
            .collect()
    }

    /// Continue the chain with a joined child's failures, in their order.
    pub(crate) fn append_chain(&mut self, chain: Vec<Chained>) {
        for (record, spawn) in chain {
            self.record_from(record, Some(spawn));
        }
    }

    /// A scope's join (FAIL-2): the chain whose first failure has the earlier spawn path leads,
    /// and the other continues it. `own` is the path of this state's task.
    pub(crate) fn merge_chain(&mut self, own: &[u32], chain: Vec<Chained>) {
        let Some((_, first)) = chain.first() else {
            return;
        };
        let leading = match self.spawns.first() {
            None => true,
            Some(spawn) => first.as_slice() < spawn.as_deref().unwrap_or(own),
        };
        if leading {
            let earlier = std::mem::take(self).take_chain(own);
            self.append_chain(chain);
            self.append_chain(earlier);
        } else {
            self.append_chain(chain);
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
    operation_site: u64,
) -> u8 {
    unsafe {
        (*ctx).failure.record(FailureRecord {
            kind,
            rule,
            site,
            operation_site,
        })
    };
    STATUS_FAILURE
}

/// Record a failure the runtime raises itself, at its fixed runtime code
/// (`hirsel_core_contract::RUNTIME_RULES`). Site 0 is anchored by the caller's
/// failure edge. The kind is FAIL-1's `bug`, the one category.
pub(crate) fn record_runtime(ctx: *mut RunCtx, rule: &str, site: u64, operation_site: u64) {
    let rule = hirsel_core_contract::runtime_rule_code(rule)
        .unwrap_or_else(|| crate::fault("a rule the runtime does not record"));
    unsafe {
        (*ctx).failure.record(FailureRecord {
            kind: 1,
            rule,
            site,
            operation_site,
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
pub unsafe extern "C" fn hirsel_rt_v2_failure_reanchor(
    ctx: *mut RunCtx,
    site: u64,
    operation_site: u64,
) {
    if site == 0 {
        return;
    }
    let state = unsafe { &mut (*ctx).failure };
    for failure in state.primary.iter_mut().chain(&mut state.causes) {
        if failure.site == 0 {
            failure.site = site;
        }
        if failure.operation_site == 0 {
            failure.operation_site = operation_site;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(site: u64) -> FailureRecord {
        FailureRecord {
            kind: 1,
            rule: 1,
            site,
            operation_site: site,
        }
    }

    #[test]
    fn an_isolation_takes_the_records_from_its_start_and_keeps_the_rest() {
        let mut state = FailureState::default();
        state.record(record(1));
        state.append_chain(vec![(record(2), vec![0])]);
        state.record(record(3));
        let caught = state.split_off(1, &[7]);
        assert_eq!(caught, vec![(record(2), vec![0]), (record(3), vec![7])]);
        assert_eq!(state.count(), 1);
        assert_eq!(state.chain(&[7]), vec![(record(1), vec![7])]);
        // From the start: the whole chain, and no failure stays active.
        let caught = state.split_off(0, &[7]);
        assert_eq!(caught, vec![(record(1), vec![7])]);
        assert!(!state.active());
    }
}
