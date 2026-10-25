//! The deterministic runtime driver's adapter to the concrete task registry
//! (r6 S1): a real [`Scheduler`] whose rows, generations, save-gated
//! retirement, leases, host-wait parks and cancellation subscriptions are
//! the production ones. The adapter only names a row by its [`TaskKey`] and
//! never decides an outcome; every answer comes from the scheduler.
use super::super::*;
use registry::{CancelRegistration, TaskKey, TaskLease};
use std::sync::Arc;

pub(crate) struct Rows {
    s: Scheduler,
    scope: u32,
}

impl Rows {
    /// A free-mode scheduler with its root task and one open root scope.
    pub(crate) fn new() -> Self {
        let mut s = Scheduler::new(Schedule::default(), std::ptr::null_mut());
        s.logical = Logical::Free(free::Driver::default());
        let (scope, generation) = s.take_scope_slot();
        s.scopes[scope as usize] = Some(Scope {
            generation,
            owner: 0,
            children: Vec::new(),
            canceled: Default::default(),
            deadline: None,
            completions: AtomicU64::new(0),
        });
        s.task(0).scopes.push(scope);
        Self { s, scope }
    }

    /// Spawn a child row through the production slot path.
    pub(crate) fn spawn(&mut self, ordinal: u32) -> TaskKey {
        let (slot, generation) = self.s.take_task_slot();
        let row = Task::new(
            generation,
            std::ptr::null_mut(),
            Some(self.scope),
            vec![ordinal],
            None,
        );
        assert!(self.s.tasks[slot as usize].replace(row).is_none());
        self.s.scope(self.scope).children.push(slot);
        self.s.link_child(0, slot);
        self.s.task_key(slot).unwrap()
    }

    fn slot(&self, key: TaskKey) -> u32 {
        self.s
            .live_task(key)
            .unwrap_or_else(|| panic!("{key:?} is not a live row"))
    }

    /// The row's dispatch frame, as `schedule_saved` installs it.
    pub(crate) fn attach(&mut self, key: TaskKey, frame: Arc<crate::pool::free::Frame>) {
        let slot = self.slot(key);
        assert!(self.s.task(slot).dispatch.replace(frame).is_none());
    }

    pub(crate) fn live(&self, key: TaskKey) -> bool {
        self.s.live_task(key).is_some()
    }

    /// The policy's save gate (`readiness::saved`): a parked row whose frame
    /// has not acknowledged its save is not resumable.
    pub(crate) fn saved(&self, key: TaskKey) -> bool {
        readiness::saved(&self.s, self.slot(key))
    }

    pub(crate) fn lease(&mut self, key: TaskKey) -> TaskLease {
        let slot = self.slot(key);
        self.s.lease(slot)
    }

    /// The completed row leaves its table; its slot waits for its frame's
    /// save acknowledgement and every lease.
    pub(crate) fn retire(&mut self, key: TaskKey) {
        let slot = self.slot(key);
        self.s.reserve_retirements(1);
        let scope = self.scope;
        self.s.scope(scope).children.retain(|&c| c != slot);
        drop(self.s.retire_task(slot));
    }

    /// Retired slots still waiting for an owner to leave.
    pub(crate) fn retained(&self) -> Vec<u32> {
        self.s.task_slots.retained()
    }

    /// Park the row on a host request: Free parking subscribes the request
    /// token to the task's cancellation, with the post-registration recheck.
    /// A shielded wait holds the finalizer shield that keeps its request.
    pub(crate) fn host_wait(
        &mut self,
        key: TaskKey,
        token: u64,
        shield: bool,
    ) -> CancelRegistration {
        let slot = self.slot(key);
        self.s.current = slot;
        self.s.task(slot).shield = u32::from(shield);
        self.s.park(Wait::Host { token });
        self.s.woken.clear();
        self.s.task(slot).host_cancel.unwrap()
    }

    /// The host wait ended: the subscription leaves with it.
    pub(crate) fn host_done(&mut self, key: TaskKey) {
        let slot = self.slot(key);
        self.s.end_wait(slot);
        self.s.task(slot).shield = 0;
    }

    /// Cancel the live row; returns whether this was its first cause.
    pub(crate) fn cancel(&mut self, key: TaskKey) -> bool {
        let slot = self.slot(key);
        let first = self.s.task(slot).canceled.cause().is_none();
        self.s.cancel(slot, CANCEL_REQUESTED);
        self.s.woken.clear();
        first
    }

    /// The row's unshielded cancellation.
    pub(crate) fn canceled(&self, key: TaskKey) -> bool {
        self.s.canceled_task(self.slot(key), 0).is_some()
    }

    /// Cancellation notices queued and not yet drained.
    pub(crate) fn notices(&self) -> usize {
        self.s.cancel_notices.len()
    }

    /// Home's drain of the notices addressed to `mask`/`value` tokens; the
    /// registration check drops a notice after retry or row reuse.
    pub(crate) fn host_cancellations(&mut self, mask: u64, value: u64) -> Vec<u64> {
        self.s.host_cancellations(mask, value)
    }

    /// Requeue a notice exactly as a delayed cancellation would deliver it.
    pub(crate) fn redeliver(&mut self, notice: registry::CancelNotice) {
        self.s.cancel_notices.push(notice);
    }
}
