//! Ownership of a saved, started task between workers.
//!
//! Tokio `runtime/task/harness.rs:160-229` (ce685ee6916549ed7e52dbfd5430858bbdc734d3)
//! transfers a notification reference into running ownership and returns it to
//! the scheduler only after polling has stopped. Hirsel must additionally save
//! a fiber before returning that ownership. `Running` therefore owns the frame
//! image until the idle continuation calls `save`; a wake never reads it.
//!
//! Queue tokens cannot be cloned. Notifications can: while running they only
//! set the remembered wake bit. Discarding either kind of ownership completes
//! the state before dropping the image, so a late wake cannot reach its context.

use crate::schedule::TaskState;
use crate::sync::{Arc, Cell};

struct Frame<T> {
    state: TaskState,
    saved: Cell<Option<T>>,
}

/// A wake handle, without access to the saved image or running context.
pub struct Notification<T>(Arc<Frame<T>>);

impl<T> Clone for Notification<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// Exactly one queue publication. It can migrate, but cannot be duplicated.
pub struct Queued<T>(Option<Arc<Frame<T>>>);

/// The sole owner of a running task's frame image. Keep this on its worker's
/// idle stack, not a Hirsel stack that may be abandoned on completion.
pub struct Running<T> {
    frame: Arc<Frame<T>>,
    value: Option<T>,
}

/// Admit a newly prepared frame. Its queue token owns the first dispatch;
/// subsequent dispatches come from `Notification::notify` or `Running::save`.
pub fn prepared<T>(value: T) -> (Notification<T>, Queued<T>) {
    let frame = Arc::new(Frame {
        state: TaskState::default(),
        saved: Cell::new(Some(value)),
    });
    assert!(frame.state.notify());
    (Notification(frame.clone()), Queued(Some(frame)))
}

impl<T> Notification<T> {
    /// Return a token only for a saved idle frame. A running wake stays
    /// remembered until `save`; an already queued or completed wake coalesces.
    pub fn notify(&self) -> Option<Queued<T>> {
        self.0.state.notify().then(|| Queued(Some(self.0.clone())))
    }
}

impl<T> Notification<T> {
    /// See [`TaskState::idle`].
    pub fn idle(&self) -> bool {
        self.0.state.idle()
    }

    /// See [`TaskState::notified`].
    pub fn notified(&self) -> bool {
        self.0.state.notified()
    }
}

impl<T> Queued<T> {
    /// Consume publication ownership before touching the image. The successful
    /// state transition acquires everything written by its previous worker.
    pub fn start(mut self) -> Running<T> {
        let frame = self.0.take().expect("one queue token");
        assert!(frame.state.start(), "a queue token owns one dispatch");
        // SAFETY: only the running owner can access saved after this CAS.
        let value = unsafe { frame.saved.with_mut(Option::take) }.expect("a saved frame");
        Running {
            frame,
            value: Some(value),
        }
    }
}

impl<T> Drop for Queued<T> {
    fn drop(&mut self) {
        if let Some(frame) = self.0.take() {
            frame.state.discard_queued();
            // SAFETY: this token owned the only dispatch. Completion prevents
            // every future notification from accessing the saved image.
            let value = unsafe { frame.saved.with_mut(Option::take) };
            drop(value);
        }
    }
}

impl<T> Running<T> {
    pub fn value(&self) -> &T {
        self.value.as_ref().expect("running frame")
    }

    pub fn value_mut(&mut self) -> &mut T {
        self.value.as_mut().expect("running frame")
    }

    /// The idle continuation calls this after saving the task's stack and
    /// updating the image. A parked frame is requeued only if notified; a
    /// voluntary yield always returns a token. Publish that token, then wake
    /// the driver. This method itself neither queues nor wakes a worker.
    pub fn save(mut self, yielded: bool) -> Option<Queued<T>> {
        let value = self.value.take().expect("running frame");
        // SAFETY: RUNNING still excludes every other image access. The
        // release CAS in suspend publishes this write before a thief starts.
        unsafe {
            self.frame.saved.with_mut(|saved| {
                assert!(saved.is_none());
                *saved = Some(value);
            });
        }
        self.frame
            .state
            .suspend(yielded)
            .then(|| Queued(Some(self.frame.clone())))
    }
}

impl<T> Drop for Running<T> {
    fn drop(&mut self) {
        if self.value.is_some() {
            self.frame.state.complete();
            // The image is dropped after completion. A concurrent wake can
            // observe only RUNNING or COMPLETE, never a new queue token.
        }
    }
}
