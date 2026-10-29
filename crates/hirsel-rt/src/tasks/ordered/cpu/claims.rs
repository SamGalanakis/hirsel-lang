use super::*;
use crate::tasks::registry::TaskLease;
use std::sync::{Arc, OnceLock};

/// Both the logical generation and the physical notification allocation stay
/// retained by every saved publication and delayed notifier using this claim.
pub(crate) struct ClaimLease {
    frame: OnceLock<Arc<crate::pool::free::Frame>>,
    task: TaskLease,
}

pub(crate) type ClaimIdentity = hirsel_free::batch::Identity<ClaimLease>;

impl ClaimLease {
    pub(in crate::tasks::ordered) fn frame_bound(&self) -> bool {
        self.frame.get().is_some()
    }
}

impl Batch {
    pub(in crate::tasks::ordered) fn bind_identity(
        &mut self,
        task: TaskId,
        path: Box<[u32]>,
        lease: TaskLease,
    ) -> Arc<ClaimIdentity> {
        let ordinal = self.ordinal(task).expect("an admitted CPU claim");
        assert_eq!(lease.key().word(), task);
        let key = lease.key();
        let identity = Arc::new(ClaimIdentity::bind(
            hirsel_free::batch::Key {
                slot: key.slot,
                generation: u32::try_from(key.generation).expect("bounded task generation"),
            },
            ordinal,
            path,
            ClaimLease {
                frame: OnceLock::new(),
                task: lease,
            },
        ));
        assert!(
            self.records[ordinal]
                .identity
                .replace(identity.clone())
                .is_none()
        );
        identity
    }

    pub(super) fn identity(&self, task: TaskId) -> Option<Arc<ClaimIdentity>> {
        self.records[self.ordinal(task)?].identity.clone()
    }
}

/// # Safety
/// First physical publication under the lifecycle role. No row/driver borrow
/// crosses this call and the task key came from the selected saved image.
pub(crate) unsafe fn bind_frame(
    run: *mut crate::context::RunShared,
    key: crate::tasks::registry::TaskKey,
    frame: &Arc<crate::pool::free::Frame>,
) {
    let identity = unsafe {
        let scheduler = (*(*run).tasks.get()).as_deref_mut().unwrap();
        let crate::tasks::Logical::Ordered(driver) = &scheduler.logical else {
            return;
        };
        driver
            .cpu_batch
            .as_ref()
            .and_then(|batch| batch.identity(key.word()))
    };
    if let Some(identity) = identity {
        assert_eq!(identity.lease().task.key(), key);
        bind_identity_frame(&identity, frame.clone());
    }
}

pub(super) fn bind_identity_frame(
    identity: &Arc<ClaimIdentity>,
    frame: Arc<crate::pool::free::Frame>,
) {
    assert!(
        identity.lease().frame.set(frame).is_ok(),
        "one physical allocation per CPU claim generation"
    );
}

impl Drop for Batch {
    fn drop(&mut self) {
        for row in &self.records {
            if let Some(identity) = &row.identity {
                identity.retire();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::stretch::Budgets;
    use crate::tasks::registry::TaskKey;

    #[test]
    fn cpu_membership_ends_before_the_holding_child_rest_without_invalidating_its_lease() {
        let grant = hirsel_run_schema::CpuGrant {
            provider: "cpu.scoped@1".into(),
            max_workers: 4,
            max_domains: 1,
            max_physical_bytes: 1 << 20,
            max_batch_bytes: 100,
        };
        let mut state = Settled {
            level: 0,
            limit: 100,
            fuel: 20,
        };
        let mut batch = Batch::new(&grant, 4, 1, state);
        let key = TaskKey::new(7, 3);
        batch.claim(key.word(), 4, 0);
        let identity = batch.bind_identity(
            key.word(),
            vec![1, 4].into_boxed_slice(),
            TaskLease::new(key),
        );
        assert_eq!(identity.path(), &[1, 4]);
        assert_eq!(identity.ordinal(), 0);
        assert_eq!(identity.key().generation, 3);
        assert!(identity.active());
        let frame = crate::pool::free::Frame::unpublished(key, true);
        bind_identity_frame(&identity, frame.clone());
        assert!(identity.lease().frame_bound());
        assert_eq!(
            batch.settle(
                key.word(),
                &Stretch::new(Budgets {
                    fuel: true,
                    memory: true
                }),
                &mut state,
                true
            ),
            Some(Ok(()))
        );
        assert!(!identity.active());
        assert!(
            identity.lease().task.live(),
            "holding rest remains a distinct live logical continuation"
        );
        drop(batch);
        assert_eq!(identity.path(), &[1, 4]);
        assert!(Arc::strong_count(&frame) > 1);
        drop(identity);
        assert_eq!(Arc::strong_count(&frame), 1);
    }
}
