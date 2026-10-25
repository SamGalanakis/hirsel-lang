//! The production replay wake registry (crates/hirsel-free/src/replay/wakes.rs). A pending
//! wait registers on its object's version cell and rechecks under the registration lock while
//! a publication advances the cell and then takes the registrations it enabled: a wait is never
//! left registered on a version that already passed (the lost wake), and a publication wakes
//! only the waits it enabled. Vectors after Tokio `sync/tests/loom_notify.rs:13-110`
//! (notify before and racing with the waiter's registration).

hirsel_guard::install!();
use hirsel_free_loom::sync::AtomicU64;
use hirsel_free_loom::wakes::{DependencyToken, Subscriber, Version};
use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::sync::{Arc, Notify};

/// A task's waker: it records that it woke and unparks the waiting thread.
struct Task {
    id: usize,
    woken: Arc<AtomicUsize>,
    unpark: Notify,
}

impl std::task::Wake for Task {
    fn wake(self: std::sync::Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.woken.fetch_or(1 << self.id, Ordering::Relaxed);
        self.unpark.notify();
    }
}

fn task(id: usize, woken: &Arc<AtomicUsize>) -> std::sync::Arc<Task> {
    std::sync::Arc::new(Task {
        id,
        woken: woken.clone(),
        unpark: Notify::new(),
    })
}

/// A wait for version 2 registers and parks while another thread advances 1 -> 2 and wakes
/// what the advance took; a wait for version 3, registered before, is not enabled and stays.
#[test]
fn a_publication_wakes_exactly_the_wait_it_enabled() {
    hirsel_free_loom::sync::model(|| {
        let cell = Arc::new(Version::new(1));
        let woken = Arc::new(AtomicUsize::new(0));
        let later = DependencyToken::new(cell.clone(), 3);
        assert!(!later.subscribe(Subscriber {
            waiter: 3,
            waker: task(3, &woken).into(),
        }));
        let publisher = {
            let cell = cell.clone();
            loom::thread::spawn(move || {
                let visits = AtomicU64::new(0);
                let mut wakes = Vec::new();
                assert!(cell.advance(1, &visits, &mut wakes));
                // The engine wakes after the phase is published.
                wakes.into_iter().for_each(std::task::Waker::wake);
            })
        };
        let waiting = task(2, &woken);
        let pending = DependencyToken::new(cell.clone(), 2);
        // As the engine reruns a woken poll: a spurious wake registers the same wait again.
        while !pending.subscribe(Subscriber {
            waiter: 2,
            waker: waiting.clone().into(),
        }) {
            waiting.unpark.wait();
        }
        assert!(pending.ready());
        publisher.join().unwrap();
        assert_eq!(
            woken.load(Ordering::Relaxed) & (1 << 3),
            0,
            "woke a wait it did not enable"
        );
        // Only the version-3 wait stays registered.
        assert_eq!(pending.waiting(), 1);
    });
}

/// Two waits for the same version on two threads, one advance: both wake, neither stays.
#[test]
fn one_publication_wakes_every_wait_it_enabled() {
    hirsel_free_loom::sync::model(|| {
        let cell = Arc::new(Version::new(1));
        let woken = Arc::new(AtomicUsize::new(0));
        let waiters: Vec<_> = (0..2usize)
            .map(|id| {
                let (cell, woken) = (cell.clone(), woken.clone());
                loom::thread::spawn(move || {
                    let waiting = task(id, &woken);
                    let pending = DependencyToken::new(cell, 2);
                    while !pending.subscribe(Subscriber {
                        waiter: id as u64,
                        waker: waiting.clone().into(),
                    }) {
                        waiting.unpark.wait();
                    }
                    assert!(pending.ready());
                })
            })
            .collect();
        let visits = AtomicU64::new(0);
        let mut wakes = Vec::new();
        assert!(cell.advance(1, &visits, &mut wakes));
        wakes.into_iter().for_each(std::task::Waker::wake);
        for waiter in waiters {
            waiter.join().unwrap();
        }
        assert_eq!(DependencyToken::new(cell, 2).waiting(), 0);
    });
}

/// Tokio `notify_waiters` (`sync/tests/loom_notify.rs:29-47`): two waits registered before a
/// publication on another thread; one advance wakes both.
#[test]
fn waits_registered_before_a_publication_both_wake() {
    hirsel_free_loom::sync::model(|| {
        let cell = Arc::new(Version::new(1));
        let woken = Arc::new(AtomicUsize::new(0));
        let waits: Vec<_> = (0..2usize)
            .map(|id| {
                let waiting = task(id, &woken);
                let pending = DependencyToken::new(cell.clone(), 2);
                assert!(!pending.subscribe(Subscriber {
                    waiter: id as u64,
                    waker: waiting.clone().into(),
                }));
                (waiting, pending)
            })
            .collect();
        let publisher = {
            let cell = cell.clone();
            loom::thread::spawn(move || {
                let visits = AtomicU64::new(0);
                let mut wakes = Vec::new();
                // A wait rechecking between the advance and the take removes its own entry.
                assert!(cell.advance(1, &visits, &mut wakes));
                wakes.into_iter().for_each(std::task::Waker::wake);
            })
        };
        for (id, (waiting, pending)) in waits.into_iter().enumerate() {
            while !pending.subscribe(Subscriber {
                waiter: id as u64,
                waker: waiting.clone().into(),
            }) {
                waiting.unpark.wait();
            }
        }
        publisher.join().unwrap();
        assert_eq!(DependencyToken::new(cell, 2).waiting(), 0);
    });
}

/// Tokio `notify_multi` (`sync/tests/loom_notify.rs:82-108`): each woken wait publishes the
/// next version, so a chain of publications on different threads ends with the last one seen.
#[test]
fn a_chain_of_publications_wakes_each_successor() {
    hirsel_free_loom::sync::model(|| {
        let cell = Arc::new(Version::new(1));
        let woken = Arc::new(AtomicUsize::new(0));
        let wait_then_publish = |id: usize, at: u64| {
            let (cell, woken) = (cell.clone(), woken.clone());
            loom::thread::spawn(move || {
                let waiting = task(id, &woken);
                let pending = DependencyToken::new(cell.clone(), at);
                while !pending.subscribe(Subscriber {
                    waiter: id as u64,
                    waker: waiting.clone().into(),
                }) {
                    waiting.unpark.wait();
                }
                let visits = AtomicU64::new(0);
                let mut wakes = Vec::new();
                assert!(cell.advance(at, &visits, &mut wakes));
                wakes.into_iter().for_each(std::task::Waker::wake);
            })
        };
        let chain = [wait_then_publish(0, 2), wait_then_publish(1, 3)];
        let visits = AtomicU64::new(0);
        let mut wakes = Vec::new();
        assert!(cell.advance(1, &visits, &mut wakes));
        wakes.into_iter().for_each(std::task::Waker::wake);
        for link in chain {
            link.join().unwrap();
        }
        assert_eq!(cell.get(), 4);
    });
}
