//! Predicate notifications, after Rust std's futex Parker and Rayon's sleep
//! event counter. Owners retain these words until all notifying threads exit.
use crate::parking_sync::{AtomicU32, Ordering, Wait};

/// One consumer, any number of publishers. A notification remains a token
/// until consumed, including between the consumer's final check and park.
pub(crate) struct Parker {
    state: AtomicU32,
    wait: Wait,
}
const EMPTY: u32 = 0;
const PARKED: u32 = u32::MAX;
const NOTIFIED: u32 = 1;

impl Default for Parker {
    fn default() -> Self {
        Self {
            state: AtomicU32::new(EMPTY),
            wait: Wait::default(),
        }
    }
}
impl Parker {
    pub(crate) fn notify(&self) {
        if self.state.swap(NOTIFIED, Ordering::Release) == PARKED {
            self.wait.wake(&self.state);
        }
    }
    pub(crate) fn wait(&self) {
        for _ in 0..crate::parking_sync::SPINS {
            if self
                .state
                .compare_exchange(NOTIFIED, EMPTY, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            crate::parking_sync::spin();
        }
        // Rust std thread_parking/futex.rs:48-57 changes NOTIFIED to EMPTY
        // or EMPTY to PARKED in one operation, retaining a concurrent token.
        if self.state.fetch_sub(1, Ordering::Acquire) == NOTIFIED {
            return;
        }
        loop {
            self.wait.wait(&self.state, PARKED);
            if self
                .state
                .compare_exchange(NOTIFIED, EMPTY, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }
    /// Bound the existing publication laws by their original deadlines.
    #[cfg(test)]
    pub(crate) fn wait_timeout(&self, timeout: std::time::Duration) -> bool {
        if self.state.fetch_sub(1, Ordering::Acquire) == NOTIFIED {
            return true;
        }
        let started = std::time::Instant::now();
        loop {
            let Some(left) = timeout.checked_sub(started.elapsed()) else {
                return self.state.swap(EMPTY, Ordering::Acquire) == NOTIFIED;
            };
            self.wait.wait_timeout(&self.state, PARKED, left);
            if self
                .state
                .compare_exchange(NOTIFIED, EMPTY, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }
}

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
    fn a_notification_between_the_predicate_check_and_park_is_retained() {
        model(|| {
            let parker = Arc::new(Parker::default());
            let published = Arc::new(AtomicU32::new(0));
            let worker = {
                let parker = parker.clone();
                let published = published.clone();
                crate::parking_sync::spawn(move || {
                    if published.load(Ordering::Acquire) == 0 {
                        parker.wait();
                    }
                    assert_eq!(published.load(Ordering::Acquire), 1);
                })
            };
            published.store(1, Ordering::Release);
            parker.notify();
            worker.join().unwrap();
        });
    }

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
