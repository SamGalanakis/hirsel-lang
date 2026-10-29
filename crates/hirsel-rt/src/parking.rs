//! Predicate notifications, after Rust std's futex Parker and Rayon's sleep
//! event counter. Owners retain these words until all notifying threads exit.
use crate::parking_sync::{AtomicU32, Ordering, Wait};

/// Multiple predicate waiters. Register before rechecking the event. The SC
/// order makes either registration visible to notify, or its increment visible
/// to the waiter; the futex comparison closes the last-check/park window.
pub(crate) struct Event {
    epoch: AtomicU32,
    sleepers: AtomicU32,
    wait: Wait,
}
impl Default for Event {
    fn default() -> Self {
        Self {
            epoch: AtomicU32::new(0),
            sleepers: AtomicU32::new(0),
            wait: Wait::default(),
        }
    }
}
impl Event {
    pub(crate) fn ticket(&self) -> u32 {
        self.epoch.load(Ordering::Acquire)
    }
    pub(crate) fn notify(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        crate::parking_sync::fence();
        if self.sleepers.load(Ordering::SeqCst) != 0 {
            self.wait.wake(&self.epoch);
        }
    }
    pub(crate) fn wait(&self, ticket: u32) {
        for _ in 0..crate::parking_sync::SPINS {
            if self.ticket() != ticket {
                return;
            }
            crate::parking_sync::spin();
        }
        self.sleepers.fetch_add(1, Ordering::SeqCst);
        crate::parking_sync::fence();
        if self.epoch.load(Ordering::SeqCst) == ticket {
            self.wait.wait(&self.epoch, ticket);
        }
        self.sleepers.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parking_sync::{Arc, AtomicU32, Ordering, model};

    #[test]
    fn startup_publication_closes_the_registration_window() {
        model(|| {
            let event = Arc::new(Event::default());
            let published = Arc::new(AtomicU32::new(0));
            let worker = {
                let event = event.clone();
                let published = published.clone();
                crate::parking_sync::spawn(move || {
                    loop {
                        let ticket = event.ticket();
                        if published.load(Ordering::Acquire) != 0 {
                            break;
                        }
                        event.wait(ticket);
                    }
                    assert_eq!(published.load(Ordering::Acquire), 1);
                })
            };
            published.store(1, Ordering::Release);
            event.notify();
            worker.join().unwrap();
        });
    }
}
