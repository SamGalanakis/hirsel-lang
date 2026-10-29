//! The scheduler role's exclusive owner, with task identity for reentry.
//! Acquire publishes the previous owner's tables; release follows its final
//! access. A claim-to-context replacement retains ownership without a gap.
use crate::parking_sync::{AtomicUsize, Ordering};

const UNOWNED: usize = usize::MAX;
pub(crate) struct Owner(AtomicUsize);
impl Default for Owner {
    fn default() -> Self {
        Self(AtomicUsize::new(UNOWNED))
    }
}
impl Owner {
    pub(crate) fn is(&self, task: usize) -> bool {
        self.0.load(Ordering::Relaxed) == task
    }
    pub(crate) fn try_acquire(&self, task: usize) -> bool {
        self.0
            .compare_exchange(UNOWNED, task, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
    pub(crate) fn release(&self) {
        self.0.store(UNOWNED, Ordering::Release);
    }
    pub(crate) fn replace(&self, claim: usize, task: usize) {
        assert!(self.is(claim));
        self.0.store(task, Ordering::Relaxed);
    }
    #[cfg(test)]
    pub(crate) fn current(&self) -> Option<usize> {
        match self.0.load(Ordering::Acquire) {
            UNOWNED => None,
            task => Some(task),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parking::Event;
    use crate::parking_sync::{Arc, model, spawn};
    use hirsel_free::sync::Cell;

    fn acquire(owner: &Owner, event: &Event, task: usize) {
        loop {
            let ticket = event.ticket();
            if owner.try_acquire(task) {
                return;
            }
            event.wait(ticket);
        }
    }

    #[test]
    fn late_wake_and_slot_reuse_are_serialized_by_the_role() {
        use hirsel_free::frame;
        model(|| {
            let owner = Arc::new(Owner::default());
            let event = Arc::new(Event::default());
            let (old, queued) = frame::prepared(1u32);
            let running = queued.start();
            let (new, queued) = frame::prepared(2u32);
            let fresh = queued.start();
            let row = Arc::new(Cell::new((1u32, old.clone())));
            let late = spawn({
                let (owner, event, row) = (owner.clone(), event.clone(), row.clone());
                move || {
                    acquire(&owner, &event, 16);
                    // The production dispatch lookup checks the generation
                    // under this role before accessing a row's notification.
                    unsafe {
                        row.with(|(generation, wake)| {
                            if *generation == 1 {
                                assert!(wake.notify().is_none());
                            }
                        })
                    };
                    owner.release();
                    event.notify();
                }
            });
            drop(running);
            acquire(&owner, &event, 8);
            assert!(owner.is(8));
            owner.replace(8, 24);
            unsafe { row.with_mut(|row| *row = (2, new.clone())) };
            owner.release();
            event.notify();
            late.join().unwrap();
            assert!(old.notify().is_none());
            assert!(!new.notified(), "a stale wake reached the new occupant");
            assert!(new.notify().is_none());
            assert!(
                fresh.save(false).is_some(),
                "the live wake must be retained"
            );
        });
    }
}
