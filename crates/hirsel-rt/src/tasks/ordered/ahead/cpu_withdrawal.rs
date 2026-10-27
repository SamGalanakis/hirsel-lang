//! Backing pressure parks exact younger owners through the shared dispatcher.
//! Saved mappers retain all storage and charges. Reclamation needs a complete
//! saved-root recipe; a save acknowledgement alone is never proof of liveness.
use super::super::engine;
use crate::TaskCtx;
use crate::ordered::footprint::TaskId;
use crate::tasks::{Logical, TaskKey, role};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

pub(crate) struct Withdrawal {
    head: TaskId,
    targets: BTreeSet<TaskId>,
    pending: BTreeSet<TaskId>,
}

impl Withdrawal {
    pub(in crate::tasks::ordered) fn new(
        head: TaskId,
        targets: BTreeSet<TaskId>,
        pending: BTreeSet<TaskId>,
    ) -> Self {
        Self {
            head,
            targets,
            pending,
        }
    }

    pub(in crate::tasks::ordered) fn blocks(&self, task: TaskId) -> bool {
        self.targets.contains(&task) || (task == self.head && !self.pending.is_empty())
    }

    /// Only driver::after_save, after the actual dispatch save, calls this.
    pub(in crate::tasks::ordered) fn saved(&mut self, task: TaskId) {
        self.pending.remove(&task);
    }
}

/// Idle with the role held, after a head's physical backing refusal. No
/// pointed-to mapper state, charge or block is changed by the save request.
pub(super) fn request(ctx: *mut TaskCtx) {
    let head = engine::current(ctx);
    role::with_role(ctx, |s| {
        let Logical::Ordered(driver) = &s.logical else {
            unreachable!()
        };
        if !driver.head_ready(head) || driver.cpu_withdrawal.is_some() {
            return;
        }
        let candidates: Vec<_> = driver
            .cpu_batch
            .as_ref()
            .map_or_else(Vec::new, |batch| batch.younger(head))
            .into_iter()
            .filter(|task| !driver.reserved.contains_key(&(*task as u32)))
            .collect();
        let mut targets = BTreeSet::new();
        for task in candidates {
            if let Some(row) = super::physical(s, TaskKey::from_word(task)) {
                // Local and its flags outlive the physical owner. Only atomics
                // are accessed while that owner's mapper may still be running.
                let local = unsafe { (*row.ctx).ahead.as_ref() };
                if let Some(local) = local {
                    local.save_requested.store(true, Ordering::Release);
                    targets.insert(task);
                }
            }
        }
        if targets.is_empty() {
            return;
        }
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        // Saved is produced only by driver::after_save for this full identity.
        // A task that already saved is equally quiescent, and selection below
        // cannot start it again while the head retains the withdrawal.
        let pending = targets
            .iter()
            .copied()
            .filter(|task| !driver.physically_saved(*task))
            .collect();
        driver.cpu_withdrawal = Some(Withdrawal::new(head, targets, pending));
    });
}

pub(super) fn acknowledged(ctx: *mut TaskCtx) -> bool {
    let task = engine::current(ctx);
    engine::with_driver(ctx, |driver| {
        driver
            .cpu_withdrawal
            .as_ref()
            .is_none_or(|withdrawal| withdrawal.head != task || withdrawal.pending.is_empty())
    })
}

/// Successful backing admission releases the parked younger frames. This
/// never returns a pending or summarized allocation charge to REQUESTED.
pub(super) fn release(ctx: *mut TaskCtx) {
    let head = engine::current(ctx);
    role::with_role(ctx, |s| {
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        if driver
            .cpu_withdrawal
            .as_ref()
            .is_none_or(|withdrawal| withdrawal.head != head)
        {
            return;
        }
        let withdrawal = driver.cpu_withdrawal.take().unwrap();
        assert!(
            withdrawal.pending.is_empty(),
            "backing retry follows physical acknowledgements"
        );
        for task in withdrawal.targets {
            let row = super::physical(s, TaskKey::from_word(task))
                .expect("a withdrawn physical owner stays live");
            unsafe {
                (*row.ctx)
                    .ahead
                    .as_ref()
                    .unwrap()
                    .save_requested
                    .store(false, Ordering::Release)
            };
        }
    });
}
