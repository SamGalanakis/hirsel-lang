//! All-or-nothing scoped reservations under the existing process permit owner.
//! Like Tokio's batch_semaphore acquire, a reservation owns every requested
//! credit before handing any to workers. Unlike its wait list, CPU admission
//! refuses immediately and leaves existing waiters their capacity.
use super::{Exec, Permits};
use std::sync::{Arc, atomic::Ordering};

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Refusal {
    InvalidWidth,
    WidthUnavailable { requested: u32, available: u32 },
}

/// Unassigned credits. Each assigned credit has an independent lifetime, so a
/// stopped worker can outlive both startup rollback and its caller.
pub(super) struct Reservation {
    permits: Arc<Permits>,
    unassigned: u32,
}

/// Retained on the worker's native stack through its final exit, outside the
/// Hirsel fibers that a stop can abandon. Never store it on a task fiber.
pub(super) struct Assigned {
    permits: Arc<Permits>,
    exec: Arc<Exec>,
}

impl Permits {
    pub(super) fn try_reserve(self: &Arc<Self>, width: u32) -> Result<Reservation, Refusal> {
        if !matches!(width, 1 | 4 | 8 | 16) {
            return Err(Refusal::InvalidWidth);
        }
        let mut state = self.state.lock().unwrap();
        let available = state
            .limit
            .saturating_sub(state.held)
            .saturating_sub(state.waiting);
        if width > available {
            return Err(Refusal::WidthUnavailable {
                requested: width,
                available,
            });
        }
        state.held += width;
        Ok(Reservation {
            permits: Arc::clone(self),
            unassigned: width,
        })
    }
}

impl Reservation {
    /// Bind a credit before starting this fresh worker. A failed OS spawn
    /// drops the returned guard; a successful spawn moves it into the thread.
    pub(super) fn assign(&mut self, exec: Arc<Exec>) -> Option<Assigned> {
        if self.unassigned == 0 {
            return None;
        }
        assert!(!exec.held.load(Ordering::Acquire));
        assert!(!exec.reserved.swap(true, Ordering::AcqRel));
        self.unassigned -= 1;
        Some(Assigned {
            permits: Arc::clone(&self.permits),
            exec,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.permits.state.lock().unwrap();
        state.held -= self.unassigned;
        self.permits.freed.notify_all();
    }
}

impl Drop for Assigned {
    fn drop(&mut self) {
        // The worker has exited compiled code. Its idle transitions did not
        // return this credit to the process, even after capacity contracted.
        let mut state = self.permits.state.lock().unwrap();
        assert!(self.exec.reserved.swap(false, Ordering::AcqRel));
        self.exec.held.store(false, Ordering::Release);
        state.held -= 1;
        if self.exec.lost.swap(false, Ordering::AcqRel) {
            state.lost -= 1;
        }
        self.permits.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::Kind;

    fn exec() -> Arc<Exec> {
        Exec::new(Kind::Compute(0), std::ptr::null_mut(), false)
    }

    #[test]
    fn invalid_widths_and_insufficient_capacity_leave_no_partial_reservation() {
        for width in [0, 2, 3, 17, u32::MAX] {
            let permits = Arc::new(Permits::new(16));
            assert!(matches!(
                permits.try_reserve(width),
                Err(Refusal::InvalidWidth)
            ));
            assert_eq!(permits.available(), 16);
        }
        for width in [1, 4, 8, 16] {
            let permits = Arc::new(Permits::new(width - 1));
            assert!(
                matches!(permits.try_reserve(width), Err(Refusal::WidthUnavailable { requested, available }) if requested == width && available == width - 1)
            );
            assert_eq!(permits.state.lock().unwrap().held, 0);
            permits.state.lock().unwrap().limit = width;
            let reservation = permits.try_reserve(width).unwrap();
            assert_eq!(permits.available(), 0);
            drop(reservation);
            assert_eq!(permits.available(), width);
        }
    }

    #[test]
    fn parent_releases_before_reserving_and_can_resume_after_every_rollback_prefix() {
        for width in [1, 4, 8, 16] {
            for started in 0..=width {
                let permits = Arc::new(Permits::new(width));
                let parent = exec();
                assert!(permits.acquire(&parent, None, None).unwrap());
                assert!(matches!(
                    permits.try_reserve(width),
                    Err(Refusal::WidthUnavailable { .. })
                ));
                permits.release(&parent);
                let mut reservation = permits.try_reserve(width).unwrap();
                let workers: Vec<_> = (0..started)
                    .map(|_| reservation.assign(exec()).unwrap())
                    .collect();
                drop(reservation);
                assert_eq!(permits.state.lock().unwrap().held, started);
                drop(workers);
                assert!(permits.acquire(&parent, None, None).unwrap());
                permits.release(&parent);
                assert_eq!(permits.available(), width);
            }
        }
    }

    #[test]
    fn idle_worker_credit_stays_with_its_domain_until_actual_exit() {
        let permits = Arc::new(Permits::new(1));
        let mut reservation = permits.try_reserve(1).unwrap();
        let worker = exec();
        let assigned = reservation.assign(Arc::clone(&worker)).unwrap();
        assert!(reservation.assign(exec()).is_none());
        drop(reservation);
        for _ in 0..3 {
            assert!(permits.acquire(&worker, None, None).unwrap());
            assert!(!permits.acquire(&worker, None, None).unwrap());
            permits.release(&worker);
            permits.release(&worker);
            assert_eq!(permits.available(), 0);
            assert!(matches!(
                permits.try_reserve(1),
                Err(Refusal::WidthUnavailable { available: 0, .. })
            ));
        }
        drop(assigned);
        assert_eq!(permits.available(), 1);
    }

    #[test]
    fn contraction_and_lost_holder_do_not_mint_replacement_credits() {
        let permits = Arc::new(Permits::new(4));
        let mut reservation = permits.try_reserve(4).unwrap();
        let worker = exec();
        let assigned = reservation.assign(Arc::clone(&worker)).unwrap();
        permits.acquire(&worker, None, None).unwrap();
        {
            let mut state = permits.state.lock().unwrap();
            state.limit = 1;
            state.lost = 1;
            worker.lost.store(true, Ordering::Release);
        }
        drop(reservation);
        assert_eq!(permits.available(), 0);
        assert_eq!(permits.state.lock().unwrap().lost, 1);
        assert!(matches!(
            permits.try_reserve(1),
            Err(Refusal::WidthUnavailable { .. })
        ));
        drop(assigned);
        assert_eq!(permits.state.lock().unwrap().lost, 0);
        assert_eq!(permits.available(), 1);
    }

    #[test]
    fn batch_reservations_do_not_barge_a_waiting_ordinary_worker() {
        let permits = Arc::new(Permits::new(4));
        permits.state.lock().unwrap().waiting = 1;
        assert!(matches!(
            permits.try_reserve(4),
            Err(Refusal::WidthUnavailable { available: 3, .. })
        ));
        assert_eq!(permits.state.lock().unwrap().held, 0);
        permits.state.lock().unwrap().waiting = 0;
        let reservation = permits.try_reserve(4).unwrap();
        drop(reservation);
        assert_eq!(permits.available(), 4);
    }

    #[test]
    fn competing_domains_cannot_each_reserve_the_same_four_credits() {
        let permits = Arc::new(Permits::new(4));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let outcomes = std::thread::scope(|scope| {
            let attempts: Vec<_> = (0..2)
                .map(|_| {
                    let permits = Arc::clone(&permits);
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        let reservation = permits.try_reserve(4);
                        // Keep the winning credits until the other admission
                        // finished; a release followed by success is legitimate.
                        barrier.wait();
                        reservation.is_ok()
                    })
                })
                .collect();
            attempts
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(outcomes.into_iter().filter(|ok| *ok).count(), 1);
        assert_eq!(permits.available(), 4);
    }
}
