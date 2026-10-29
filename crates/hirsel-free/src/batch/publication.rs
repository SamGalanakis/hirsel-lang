//! A single generation's saved envelope and fallible head verdict. This ports
//! the producer/consumer release-acquire transfer of Tokio's oneshot channel
//! (tokio/src/sync/oneshot.rs) and retains Hirsel's separate cleanup obligation.
use crate::sync::{Arc, AtomicU8, Cell, Ordering};

const OPEN: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;
const TAKEN: u8 = 3;
const ACCEPTED: u8 = 4;
const REFUSED: u8 = 5;
const DRAINED: u8 = 6;

pub struct Publication<E> {
    phase: AtomicU8,
    envelope: Cell<Option<E>>,
}
impl<E> Default for Publication<E> {
    fn default() -> Self {
        Self {
            phase: AtomicU8::new(OPEN),
            envelope: Cell::new(None),
        }
    }
}
impl<E> Publication<E> {
    /// Idle has acknowledged the saved native continuation. Its owner cannot
    /// touch the envelope or borrowed roots until it acquires the verdict.
    pub fn publish(&self, envelope: E) {
        assert_eq!(
            self.phase
                .compare_exchange(OPEN, WRITING, Ordering::AcqRel, Ordering::Acquire),
            Ok(OPEN)
        );
        // SAFETY: WRITING is exclusive; only READY transfers cell access.
        unsafe { self.envelope.with_mut(|slot| *slot = Some(envelope)) };
        self.phase.store(READY, Ordering::Release);
    }
    /// Called by lexical head authority, or by acknowledged backing withdrawal.
    /// The returned permit retains this allocation across a fallible receipt.
    pub fn take(publication: &Arc<Self>) -> Option<(E, Verdict<E>)> {
        publication
            .phase
            .compare_exchange(READY, TAKEN, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        // SAFETY: the successful acquire claims READY exactly once.
        let envelope = unsafe { publication.envelope.with_mut(|slot| slot.take().unwrap()) };
        Some((
            envelope,
            Verdict {
                publication: publication.clone(),
                completed: false,
            },
        ))
    }
    pub fn verdict(&self) -> Option<bool> {
        match self.phase.load(Ordering::Acquire) {
            ACCEPTED => Some(true),
            REFUSED => Some(false),
            _ => None,
        }
    }
    /// After the physical owner's cleanup has left the native stack. This
    /// observation never substitutes for joining the native thread.
    pub fn drained(&self) {
        let phase = self.phase.load(Ordering::Acquire);
        assert!(matches!(phase, ACCEPTED | REFUSED));
        assert_eq!(
            self.phase
                .compare_exchange(phase, DRAINED, Ordering::AcqRel, Ordering::Acquire),
            Ok(phase)
        );
    }
    pub fn is_drained(&self) -> bool {
        self.phase.load(Ordering::Acquire) == DRAINED
    }
}

/// Dropping unfinished retirement refuses the continuation before returning an
/// error. No batch or scheduler-table borrow spans the receipt callback.
pub struct Verdict<E> {
    publication: Arc<Publication<E>>,
    completed: bool,
}
impl<E> Verdict<E> {
    pub fn accept(mut self) {
        self.finish(true);
    }
    fn finish(&mut self, accepted: bool) {
        assert_eq!(
            self.publication.phase.compare_exchange(
                TAKEN,
                if accepted { ACCEPTED } else { REFUSED },
                Ordering::Release,
                Ordering::Acquire,
            ),
            Ok(TAKEN)
        );
        self.completed = true;
    }
}
impl<E> Drop for Verdict<E> {
    fn drop(&mut self) {
        if !self.completed {
            self.finish(false);
        }
    }
}
