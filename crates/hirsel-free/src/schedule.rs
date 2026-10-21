//! Free-mode worker policy (design §5 Mechanism).
//!
//! Ported from Tokio's multi-thread scheduler: a FIFO local queue with a last-woken
//! LIFO slot, displaced slots go to the FIFO, and a thief takes ceil(n/2) from the
//! victim's oldest work. See `tokio/src/runtime/scheduler/multi_thread/queue.rs:417-512`
//! and `worker.rs:270,728-766,1386-1407` in the reference checkout. Hirsel queues
//! task identifiers, including started, suspended fibers; a fiber is published only
//! after its stack pointer has been saved. Affine foreign frames stay outside these
//! queues. Neither task identity nor queue placement appears on tape.
//!
//! The bounded deque is [`crate::queue`]; notifications use the task state
//! word here, and are published only after saving the fiber.

use crate::sync::{AtomicU32, Ordering};

/// Tokio's per-task cooperative operation budget (`task/coop/mod.rs:116`).
pub const OP_BUDGET: u8 = 128;
/// Tokio's bound on consecutive LIFO-slot polls (`worker.rs:270`).
pub const LIFO_POLLS: u8 = 3;

/// Owner-local; it travels with the task across a suspension or migration.
#[derive(Debug)]
pub struct Budget(u8);

impl Default for Budget {
    fn default() -> Self {
        Self(OP_BUDGET)
    }
}

impl Budget {
    /// A completed immediate op may keep running until the 128th op. The
    /// semantic op and its publication have already happened at this point.
    pub fn proceed(&mut self) -> bool {
        self.0 -= 1;
        if self.0 == 0 {
            self.reset();
            false
        } else {
            true
        }
    }

    /// A task that parks gives up the rest of its quantum.
    pub fn reset(&mut self) {
        self.0 = OP_BUDGET;
    }
}

const QUEUED: u32 = 1;
const RUNNING: u32 = 2;
const NOTIFIED: u32 = 4;
const COMPLETE: u32 = 8;

/// One scheduling state word per task. Like Tokio's `runtime/task/state.rs`,
/// a notification racing the running task's park is remembered on the running
/// state, rather than enqueuing a fiber whose stack is still in use.
///
/// Only a successful [`notify`](Self::notify) or [`suspend`](Self::suspend)
/// returning `true` permits publishing an identifier to a queue. The wake
/// publisher must also wake the driver's eventfd after publishing it.
pub struct TaskState(AtomicU32);

impl Default for TaskState {
    fn default() -> Self {
        Self(AtomicU32::new(0))
    }
}

impl TaskState {
    /// Notify once. `true` transfers one queue publication to the caller;
    /// `false` coalesces a notification or ignores a completed task.
    pub fn notify(&self) -> bool {
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            if state & (COMPLETE | QUEUED | NOTIFIED) != 0 {
                return false;
            }
            let next = if state & RUNNING != 0 {
                state | NOTIFIED
            } else {
                QUEUED
            };
            match self
                .0
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return next == QUEUED,
                Err(actual) => state = actual,
            }
        }
    }

    /// Consume the queue's ownership. At most one worker can start the task.
    pub fn start(&self) -> bool {
        self.0
            .compare_exchange(QUEUED, RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// After the stack pointer is saved, relinquish execution ownership.
    /// A notification during execution makes the suspended task ready. An
    /// explicit yield makes it ready even with no notification.
    pub fn suspend(&self, yielded: bool) -> bool {
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            assert_ne!(state & RUNNING, 0, "only a running task suspends");
            let ready = yielded || state & NOTIFIED != 0;
            let next = if ready { QUEUED } else { 0 };
            match self
                .0
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return ready,
                Err(actual) => state = actual,
            }
        }
    }

    /// The running task finished; subsequent notifications cannot resurrect it.
    pub fn complete(&self) {
        let previous = self.0.swap(COMPLETE, Ordering::AcqRel);
        assert_ne!(previous & RUNNING, 0, "only a running task completes");
    }
}

/// Worker-owned LIFO slot and its fairness bound. A replacement returns the
/// displaced task for FIFO publication. An explicit yield never enters it.
#[derive(Debug)]
pub struct Lifo<T> {
    task: Option<T>,
    polls: u8,
}

impl<T> Default for Lifo<T> {
    fn default() -> Self {
        Self {
            task: None,
            polls: 0,
        }
    }
}

impl<T> Lifo<T> {
    pub fn push(&mut self, task: T) -> Option<T> {
        if self.polls == LIFO_POLLS {
            Some(task)
        } else {
            self.task.replace(task)
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        let task = self.task.take()?;
        self.polls += 1;
        Some(task)
    }

    /// Called before polling the FIFO, or while retiring a worker. Any
    /// remaining slot becomes visible to thieves through the FIFO.
    pub fn to_fifo(&mut self) -> Option<T> {
        self.polls = 0;
        self.task.take()
    }
}
