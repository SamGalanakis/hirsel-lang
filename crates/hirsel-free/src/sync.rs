//! The synchronisation primitives the concurrent parts of the protocol use. `proofs/free`
//! compiles the same modules against its own `sync` with loom's types, so every access goes
//! through these names: a [`Cell`] instead of `UnsafeCell`, and [`sc_store`] / [`sc_load`]
//! for the SeqCst accesses of the section handshake (loom models a SeqCst access as AcqRel
//! and needs an SC fence beside it; GenMC checks the plain SeqCst form).

pub use std::sync::atomic::{
    AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
pub use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// Interior mutability whose exclusion the protocol, not the type, guarantees.
pub struct Cell<T>(std::cell::UnsafeCell<T>);

// SAFETY: every access is under the caller contract of `with` / `with_mut`.
unsafe impl<T: Send> Sync for Cell<T> {}

impl<T> Cell<T> {
    pub fn new(value: T) -> Cell<T> {
        Cell(std::cell::UnsafeCell::new(value))
    }
    /// # Safety
    /// No mutable access to this cell is concurrent with `f`.
    pub unsafe fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        // SAFETY: caller contract.
        f(unsafe { &*self.0.get() })
    }
    /// # Safety
    /// No other access to this cell is concurrent with `f`.
    pub unsafe fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: caller contract.
        f(unsafe { &mut *self.0.get() })
    }
}

/// A SeqCst store of the section handshake.
pub fn sc_store(word: &AtomicU32, value: u32) {
    word.store(value, Ordering::SeqCst);
}

/// A SeqCst load of the section handshake.
pub fn sc_load(word: &AtomicU32) -> u32 {
    word.load(Ordering::SeqCst)
}

pub fn sc_store64(word: &AtomicU64, value: u64) {
    word.store(value, Ordering::SeqCst);
}

pub fn sc_load64(word: &AtomicU64) -> u64 {
    word.load(Ordering::SeqCst)
}

pub fn sc_fence() {
    std::sync::atomic::fence(Ordering::SeqCst);
}

/// A section opens or closes on this thread: debug builds trap any allocation or free while
/// one is open (`hirsel_guard::section`).
pub fn section_open(open: bool) {
    hirsel_guard::section(open);
}

/// One step of a bounded wait for another thread's short critical work.
pub fn relax() {
    std::hint::spin_loop();
    std::thread::yield_now();
}

/// Run a unit test directly; the proof crate substitutes loom's model here.
#[cfg(test)]
pub fn model(f: impl FnOnce()) {
    f();
}

/// Wait policy for section-gate polling. This is zero-sized in the runtime;
/// the model substitutes blocking waits to collapse failed-read stuttering.
pub struct SectionWaits;
impl Default for SectionWaits {
    fn default() -> Self {
        Self::new()
    }
}
impl SectionWaits {
    pub fn new() -> Self {
        Self
    }
    pub fn wait(&self, _word: &AtomicU32, _stopped: Option<&AtomicBool>) {
        relax();
    }
    pub fn wake<T>(&self, _word: &T) {}
}
