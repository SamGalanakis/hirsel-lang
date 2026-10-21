//! FIFO permit state for a runtime mutex, ported from Tokio's
//! sync/batch_semaphore.rs:334-370,432-579 at ce685ee6916549ed7e52dbfd5430858bbdc734d3.
//! Tokio links an acquire once, and assigns returned permits to the oldest
//! waiters before admitting newcomers. Hirsel has one permit and task ids
//! instead of intrusive future nodes. A caller holds the object's lock through
//! this transition, payload movement, version record and phase publication;
//! notification happens after releasing that lock. No cancellation removes an
//! acquisition: mutex take is not a cancellation point (CONC-3).

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Acquired {
    Payload,
    Dead,
    Pending,
}

pub struct Permit {
    available: bool,
    dead: bool,
    waiters: VecDeque<u32>,
    version: u64,
}

impl Permit {
    pub fn new() -> Self {
        Self {
            available: false,
            dead: false,
            waiters: VecDeque::new(),
            version: 0,
        }
    }

    pub fn available(&self) -> bool {
        self.available || self.dead
    }
    pub fn ready_for(&self, task: u32) -> bool {
        self.dead || (self.available && self.waiters.front().is_none_or(|&first| first == task))
    }

    pub fn observe(&mut self) -> u64 {
        self.advance();
        self.version
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn waiting(&self) -> usize {
        self.waiters.len()
    }
    pub fn reserve_waiter(&mut self) {
        self.waiters.reserve(1);
    }

    fn advance(&mut self) {
        self.version = self.version.checked_add(1).expect("mutex version overflow");
    }

    /// `queued` is owner-local and remains on the waiting task's saved frame.
    /// It is false initially, then prevents duplicate links on retries.
    pub fn acquire(&mut self, task: u32, queued: &mut bool) -> Acquired {
        if self.dead {
            *queued = false;
            self.advance();
            return Acquired::Dead;
        }
        if self.available && self.waiters.front().is_none_or(|&first| first == task) {
            if *queued {
                assert_eq!(self.waiters.pop_front(), Some(task));
                *queued = false;
            }
            self.available = false;
            self.advance();
            return Acquired::Payload;
        }
        if !*queued {
            self.waiters.push_back(task);
            *queued = true;
        }
        Acquired::Pending
    }

    /// Publish the returned payload before the selected waiter is notified.
    pub fn release(&mut self) -> Option<u32> {
        assert!(
            !self.available && !self.dead,
            "mutex released twice or after death"
        );
        self.available = true;
        self.advance();
        self.waiters.front().copied()
    }

    /// A dead mutex returns its failure to every waiter, without a payload.
    pub fn kill(&mut self) -> Vec<u32> {
        let mut waiters = Vec::with_capacity(self.waiters.len());
        self.kill_into(&mut waiters);
        waiters
    }

    pub fn kill_into(&mut self, out: &mut Vec<u32>) {
        assert!(
            out.capacity() - out.len() >= self.waiters.len(),
            "wake capacity prepared before the section"
        );
        assert!(!self.available && !self.dead, "only a held mutex can die");
        self.dead = true;
        self.advance();
        out.extend(self.waiters.drain(..));
    }
}

impl Default for Permit {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returning_a_payload_reserves_it_for_the_oldest_waiter() {
        let mut p = Permit::new();
        assert_eq!(p.release(), None);
        let mut owner = false;
        assert_eq!(p.acquire(0, &mut owner), Acquired::Payload);
        let (mut first, mut second, mut newcomer) = (false, false, false);
        assert_eq!(p.acquire(1, &mut first), Acquired::Pending);
        assert_eq!(p.acquire(2, &mut second), Acquired::Pending);
        assert_eq!(p.acquire(1, &mut first), Acquired::Pending);
        assert_eq!(p.release(), Some(1));
        assert_eq!(p.acquire(3, &mut newcomer), Acquired::Pending);
        assert_eq!(p.acquire(2, &mut second), Acquired::Pending);
        assert_eq!(p.acquire(1, &mut first), Acquired::Payload);
        assert_eq!(p.release(), Some(2));
        assert_eq!(p.acquire(2, &mut second), Acquired::Payload);
        assert_eq!(p.release(), Some(3));
        assert_eq!(p.acquire(3, &mut newcomer), Acquired::Payload);
        assert_eq!(p.version(), 8);
    }

    #[test]
    fn death_wakes_every_waiter_and_preserves_a_no_payload_outcome() {
        let mut p = Permit::new();
        let (mut a, mut b) = (false, false);
        assert_eq!(p.acquire(1, &mut a), Acquired::Pending);
        assert_eq!(p.acquire(2, &mut b), Acquired::Pending);
        assert_eq!(p.kill(), [1, 2]);
        assert_eq!(p.acquire(2, &mut b), Acquired::Dead);
        assert_eq!(p.acquire(1, &mut a), Acquired::Dead);
        assert!(!a && !b);
        assert!(p.available());
    }
}
