//! Backing pressure parks exact younger owners through the shared dispatcher.
//! The actual save binds complete storage custody and collects the saving
//! worker's unreachable backing before acknowledging the withdrawal.
use super::super::engine;
use crate::TaskCtx;
use crate::ordered::footprint::TaskId;
use crate::tasks::{Logical, TaskKey, role};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;

pub(crate) struct Withdrawal {
    head: TaskId,
    targets: BTreeSet<TaskId>,
    pending: BTreeSet<TaskId>,
    prefixes: Option<Vec<super::super::cpu::Withdrawn>>,
    locals: BTreeMap<TaskId, crate::budget::stretch::Stretch>,
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
            prefixes: None,
            locals: BTreeMap::new(),
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

/// Ordinary canonical allocation has Rust frames on its own task stack. It
/// can suspend through the shared dispatcher, retaining no allocator borrow.
pub(in crate::tasks::ordered) fn wait_for_backing(ctx: *mut TaskCtx) {
    if !engine::on(ctx) {
        return;
    }
    let task = engine::current(ctx);
    if !engine::with_driver(ctx, |driver| {
        driver.head_ready(task)
            && driver
                .cpu_batch
                .as_ref()
                .is_some_and(|batch| batch.limit(task).is_some())
    }) {
        return;
    }
    request(ctx);
    if !engine::with_driver(ctx, |driver| {
        driver
            .cpu_withdrawal
            .as_ref()
            .is_some_and(|withdrawal| withdrawal.head == task)
    }) {
        return;
    }
    while !acknowledged(ctx) {
        engine::suspend(ctx, task, true);
    }
}

pub(crate) fn backing_admitted(ctx: *mut TaskCtx) {
    if engine::on(ctx) {
        let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
        release(ctx);
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
        let Some(withdrawal) = driver.cpu_withdrawal.as_mut() else {
            return true;
        };
        if withdrawal.head != task {
            return true;
        }
        if !withdrawal.pending.is_empty() {
            return false;
        }
        if withdrawal.prefixes.is_none() {
            for target in &withdrawal.targets {
                if let Some(prefix) = driver.stretches.remove(target) {
                    assert!(withdrawal.locals.insert(*target, prefix).is_none());
                }
            }
            withdrawal.prefixes = Some(
                driver
                    .cpu_batch
                    .as_mut()
                    .unwrap()
                    .withdraw(task, &withdrawal.targets),
            );
        }
        true
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
        driver.cpu_batch.as_mut().unwrap().restore(
            withdrawal
                .prefixes
                .expect("acknowledged withdrawal transfers its prefixes"),
        );
        for (task, prefix) in withdrawal.locals {
            assert!(driver.stretches.insert(task, prefix).is_none());
        }
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
