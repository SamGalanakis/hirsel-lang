//! The kernel's `sync` names over loom. Loom models a SeqCst access as AcqRel, so each SC
//! access of the section handshake carries an SC fence on its far side (store; fence and
//! fence; load), which loom does model; the GenMC model checks the plain SC accesses.

use loom::sync::atomic::fence;
pub use loom::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
pub use loom::sync::{Arc, Mutex, MutexGuard};

/// Interior mutability whose exclusion the protocol guarantees; loom checks every access.
pub struct Cell<T>(loom::cell::UnsafeCell<T>);

// SAFETY: every access is under the caller contract of `with` / `with_mut`, which loom checks.
unsafe impl<T: Send> Sync for Cell<T> {}

impl<T> Cell<T> {
    pub fn new(value: T) -> Cell<T> {
        Cell(loom::cell::UnsafeCell::new(value))
    }
    /// # Safety
    /// No mutable access to this cell is concurrent with `f`.
    pub unsafe fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        // SAFETY: caller contract.
        self.0.with(|p| f(unsafe { &*p }))
    }
    /// # Safety
    /// No other access to this cell is concurrent with `f`.
    pub unsafe fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: caller contract.
        self.0.with_mut(|p| f(unsafe { &mut *p }))
    }
}

pub fn sc_store(word: &AtomicU32, value: u32) {
    word.store(value, Ordering::SeqCst);
    #[cfg(not(feature = "weak"))]
    fence(Ordering::SeqCst);
}

pub fn sc_load(word: &AtomicU32) -> u32 {
    #[cfg(not(feature = "weak"))]
    fence(Ordering::SeqCst);
    word.load(Ordering::SeqCst)
}

pub fn relax() {
    loom::thread::yield_now();
}
