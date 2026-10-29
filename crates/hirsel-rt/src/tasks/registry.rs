//! Stable task identity, slot reuse and cancellation subscriptions (r6 S1).
//!
//! A row may be logically dead before its storage is reusable. Retirement
//! removes the row at once; its slot returns to allocation only after the
//! dispatch frame acknowledged its save and every frame, request and
//! notification lease left. Generations are per slot and never wrap: a slot
//! whose generation is exhausted is never reused, so a stale key can never
//! name a later occupant. Semantic identity stays the spawn path; keys and
//! words are run-local.
//!
//! The lease is Arc-counted (Tokio task harness `Header` reference counts,
//! tokio/src/runtime/task/state.rs at ce685ee6916549ed, REF_ONE), with the
//! row itself and each retained holder owning one reference. A lease grants
//! no TaskCtx or row access; holders validate the key under the role.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::pool::free::Frame;

/// A task row's identity: its slot and that slot's generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct TaskKey {
    pub(crate) slot: u32,
    pub(crate) generation: u64,
}

/// A scope row's identity, as for [`TaskKey`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct ScopeKey {
    pub(crate) slot: u32,
    pub(crate) generation: u64,
}

impl TaskKey {
    /// Row `slot` at its row `generation`.
    pub(crate) fn new(slot: u32, generation: u32) -> Self {
        Self {
            slot,
            generation: u64::from(generation),
        }
    }

    /// The run-local Task word: generation above slot. Row generations stay
    /// within 32 bits, so the word round-trips.
    pub(crate) fn word(self) -> u64 {
        word(self.slot, self.generation)
    }

    pub(crate) fn from_word(word: u64) -> Self {
        Self {
            slot: word as u32,
            generation: word >> 32,
        }
    }
}

impl ScopeKey {
    /// The run-local Scope word, as for [`TaskKey::word`].
    pub(crate) fn word(self) -> u64 {
        word(self.slot, self.generation)
    }

    pub(crate) fn from_word(word: u64) -> Self {
        Self {
            slot: word as u32,
            generation: word >> 32,
        }
    }
}

fn word(slot: u32, generation: u64) -> u64 {
    assert!(
        generation <= u64::from(u32::MAX),
        "a row generation above 32 bits"
    );
    generation << 32 | u64::from(slot)
}

/// An object cell's identity, as for [`TaskKey`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct CellKey {
    pub(crate) slot: u32,
    pub(crate) generation: u64,
}

/// Stable per-task control state, Send + Sync. It never exposes frame bytes
/// or the task's context.
#[derive(Debug)]
pub(crate) struct TaskControl {
    key: TaskKey,
    retired: AtomicBool,
}

/// A retained reference to a task's control block. While any lease other
/// than the row's own is outstanding, the task's slot is not reused.
#[derive(Clone, Debug)]
pub(crate) struct TaskLease(Arc<TaskControl>);

impl TaskLease {
    pub(crate) fn new(key: TaskKey) -> Self {
        Self(Arc::new(TaskControl {
            key,
            retired: AtomicBool::new(false),
        }))
    }

    pub(crate) fn key(&self) -> TaskKey {
        self.0.key
    }

    /// Whether the named row is still logically live. A false answer is
    /// final; a true one is only current while the caller holds the role.
    pub(crate) fn live(&self) -> bool {
        !self.0.retired.load(Ordering::Acquire)
    }

    fn retire(&self) {
        self.0.retired.store(true, Ordering::Release);
    }

    fn sole(&self) -> bool {
        Arc::strong_count(&self.0) == 1
    }

    #[cfg(test)]
    pub(crate) fn downgrade(&self) -> std::sync::Weak<TaskControl> {
        Arc::downgrade(&self.0)
    }
}

pub(crate) use crate::slots::Slots;

/// A retired task slot whose storage may still be owned elsewhere.
struct Retiring {
    slot: u32,
    generation: u32,
    lease: Option<TaskLease>,
    frame: Option<Arc<Frame>>,
}

impl Retiring {
    /// A row whose control block was never leased has no holder to wait for.
    fn reclaimable(&self) -> bool {
        self.lease.as_ref().is_none_or(TaskLease::sole)
            && self
                .frame
                .as_ref()
                .is_none_or(|frame| Arc::strong_count(frame) == 1 && frame.is_saved())
    }
}

/// Bounded reclamation work per allocation: a retained slot costs at most
/// this many checks per spawn, never a scan of every retired row.
const RECLAIM_STEP: usize = 2;

/// Task slots: [`Slots`] plus retirement that waits for save and leases.
pub(crate) struct TaskSlots {
    slots: Slots<{ u32::MAX }>,
    retiring: VecDeque<Retiring>,
}

impl Default for TaskSlots {
    fn default() -> Self {
        Self {
            slots: Slots::default(),
            retiring: VecDeque::new(),
        }
    }
}

impl TaskSlots {
    /// The next task slot and generation, after a bounded reclamation step.
    #[inline]
    pub(crate) fn take(&mut self, len: usize) -> (u32, u32) {
        if !self.retiring.is_empty() {
            self.reclaim(RECLAIM_STEP);
        }
        self.slots.take(len)
    }

    /// Capacity for `additional` retirements inside a recording section.
    pub(crate) fn reserve(&mut self, additional: usize) {
        self.retiring.reserve(additional);
        self.slots.reserve(additional);
    }

    /// The row left its table, still holding its own `lease` and `frame`
    /// references. Its slot waits for the frame's save acknowledgement and
    /// for every other reference; with none outstanding it is free at once.
    pub(crate) fn retire(
        &mut self,
        slot: u32,
        generation: u32,
        lease: Option<&TaskLease>,
        frame: Option<&Arc<Frame>>,
    ) {
        if let Some(lease) = lease {
            lease.retire();
        }
        let held = lease.is_some_and(|lease| !lease.sole())
            || frame.is_some_and(|frame| Arc::strong_count(frame) != 1 || !frame.is_saved());
        if held {
            self.retiring.push_back(Retiring {
                slot,
                generation,
                lease: lease.cloned(),
                frame: frame.cloned(),
            });
        } else {
            self.slots.release(slot, generation);
        }
    }

    fn reclaim(&mut self, budget: usize) {
        for _ in 0..budget.min(self.retiring.len()) {
            let entry = self.retiring.pop_front().unwrap();
            if entry.reclaimable() {
                self.slots.release(entry.slot, entry.generation);
            } else {
                self.retiring.push_back(entry);
            }
        }
    }

    /// Fixture precondition for a law that must exercise a reused slot.
    /// Does not reclaim it: allocation still runs the production reclaim step.
    #[cfg(test)]
    pub(crate) fn reuse_ready(&self, slot: u32) -> bool {
        self.retiring
            .iter()
            .filter(|entry| entry.slot == slot)
            .all(Retiring::reclaimable)
    }

    /// Retired slots still waiting for an owner to leave.
    #[cfg(test)]
    pub(crate) fn retained(&self) -> Vec<u32> {
        self.retiring.iter().map(|entry| entry.slot).collect()
    }

    #[cfg(test)]
    pub(crate) fn reclaim_all(&mut self) {
        self.reclaim(self.retiring.len());
    }
}

/// One subscription of a waiter to its task's cancellation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CancelSub {
    pub(crate) token: u64,
    pub(crate) registration: u64,
}

/// The result of a subscription, with the post-registration recheck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CancelRegistration {
    pub(crate) key: TaskKey,
    pub(crate) registration: u64,
    /// The task's cancellation as of registration: already canceled waiters
    /// act now, since no later cancellation event names them again.
    pub(crate) canceled: Option<u32>,
}

/// A cancellation event touched this subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CancelNotice {
    pub(crate) task: TaskKey,
    pub(crate) token: u64,
    pub(crate) registration: u64,
}

#[cfg(test)]
mod tests;
