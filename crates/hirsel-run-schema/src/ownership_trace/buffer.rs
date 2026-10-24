//! Native observers reserve all storage at admission. Recording a committed
//! transition must not allocate, block, sample a clock or enter the scheduler,
//! including inside a source `@noalloc` finalizer.
//!
//! The single-reservation/single-writer structure follows the runtime's existing
//! operation-latency buffer (`hirsel-rt/src/operation_latency.rs`, Buffer/begin).
//! Unlike that sampler, ownership diagnostics cannot discard overflowing rows:
//! consuming an overflowed buffer fails closed. Consumption takes ownership,
//! so safe callers cannot read a row while its issuer is still writing it.
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A sticky loss of diagnostic events. This is separate from the language's
/// outcome; callers must preserve source execution after a recording failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferFull;
impl std::fmt::Display for BufferFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ownership trace event buffer overflowed")
    }
}
impl std::error::Error for BufferFull {}

/// Fixed storage for plain diagnostic rows. Native hook rows contain scalar
/// ids and immutable metadata references, never owned language values.
///
/// Allocate this once, before program execution. `push` does no allocation or
/// locking. After all issuers end, consume the buffer to decode rows and assign
/// logical shadow tokens. Row order records committed observations; it grants
/// no scheduling authority and is not a replacement for task/effect ordinals.
pub struct Buffer<T: Copy> {
    rows: Box<[UnsafeCell<T>]>,
    next: AtomicUsize,
    overflow: AtomicBool,
}

// SAFETY: fetch_update reserves a unique row for each writer without wrapping.
// No API borrows a row. into_rows consumes the buffer, which excludes all live
// borrowed issuers (including those owning an Arc). Sending the rows between
// threads requires T: Send; Copy prevents a partial row from owning a destructor.
unsafe impl<T: Copy + Send> Sync for Buffer<T> {}

impl<T: Copy> Buffer<T> {
    pub fn new(limit: usize, empty: T) -> Self {
        Self {
            rows: (0..limit).map(|_| UnsafeCell::new(empty)).collect(),
            next: AtomicUsize::new(0),
            overflow: AtomicBool::new(false),
        }
    }

    /// Record once at an already committed transition. A full buffer remains
    /// poisoned even if its caller deliberately ignores this error.
    pub fn push(&self, row: T) -> Result<(), BufferFull> {
        let index = self
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < self.rows.len()).then(|| n + 1)
            });
        let Ok(index) = index else {
            self.overflow.store(true, Ordering::Relaxed);
            return Err(BufferFull);
        };
        // SAFETY: only this reservation can write index; consumers need self by
        // value and therefore cannot run while this borrow remains active.
        unsafe { *self.rows[index].get() = row };
        Ok(())
    }

    /// Call only after execution ends. Ownership, rather than a producer's
    /// claimed footer, proves that no issuer can race this read. The caller
    /// must separately establish that execution reached its complete suffix.
    pub fn into_rows(self) -> Result<Vec<T>, BufferFull> {
        if self.overflow.into_inner() {
            return Err(BufferFull);
        }
        let count = self.next.into_inner();
        Ok(self
            .rows
            .into_vec()
            .into_iter()
            .take(count)
            .map(UnsafeCell::into_inner)
            .collect())
    }
}
