//! The kernel's `sync` names over loom. Loom models a SeqCst access as AcqRel, so each SC
//! access of the section handshake carries an SC fence on its far side (store; fence and
//! fence; load), which loom does model; the GenMC model checks the plain SC accesses.

use loom::sync::atomic::fence;
pub use loom::sync::atomic::{
    AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
pub use loom::sync::{Arc, Condvar, Mutex, MutexGuard};

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

pub fn sc_store64(word: &AtomicU64, value: u64) {
    word.store(value, Ordering::SeqCst);
    fence(Ordering::SeqCst);
}

pub fn sc_load64(word: &AtomicU64) -> u64 {
    fence(Ordering::SeqCst);
    word.load(Ordering::SeqCst)
}

pub fn sc_fence() {
    fence(Ordering::SeqCst);
}

/// The runtime's allocation trap; loom's own bookkeeping allocates inside modeled sections.
pub fn section_open(_open: bool) {}

pub fn relax() {
    loom::thread::yield_now();
}

/// Explore `f` under loom: at preemption bound 2 with loom's 1000-branch budget unless
/// `LOOM_MAX_PREEMPTIONS` / `LOOM_MAX_BRANCHES` say otherwise (check.sh's bound sweeps).
pub fn model(f: impl Fn() + Sync + Send + 'static) {
    let mut builder = loom::model::Builder::new();
    if std::env::var_os("LOOM_MAX_PREEMPTIONS").is_none() {
        builder.preemption_bound = Some(2);
    }
    builder.check(f);
}

/// Collapse only failed section-gate polls into a blocking wait. Notifications
/// follow the release which an original successful poll must acquire: active=0,
/// cut=0, or stopped=true. Initial Dekker accesses are unchanged. As in loom
/// 0.7.2 sync/notify.rs, each modeled thread retains its own Notify, including
/// its at-most-one spurious wake. No protocol word is changed by this facade.
#[derive(Default)]
pub struct SectionWaits(std::sync::Mutex<WaitState>);
#[derive(Default)]
struct WaitState {
    threads: std::collections::HashMap<loom::thread::ThreadId, usize>,
    words: std::collections::HashMap<usize, Vec<std::sync::Arc<loom::sync::Notify>>>,
}
impl SectionWaits {
    pub fn new() -> Self {
        Self::default()
    }
    fn notifications(
        state: &mut WaitState,
        key: usize,
    ) -> &mut Vec<std::sync::Arc<loom::sync::Notify>> {
        state.words.entry(key).or_insert_with(|| {
            (0..loom::MAX_THREADS)
                .map(|_| std::sync::Arc::new(loom::sync::Notify::new()))
                .collect()
        })
    }
    pub fn wait(&self, word: &AtomicU32, stopped: Option<&AtomicBool>) {
        let notification = {
            // This host mutex protects bookkeeping only, with no modeled
            // synchronization edge. Each possible waiter has its own token
            // even before its first poll, so registration cannot lose a wake.
            let mut state = self.0.lock().unwrap();
            let next = state.threads.len();
            let index = *state
                .threads
                .entry(loom::thread::current().id())
                .or_insert(next);
            assert!(index < loom::MAX_THREADS);
            Self::notifications(&mut state, std::ptr::from_ref(word) as usize)[index].clone()
        };
        // These are only additional failed-loop polls, not read-modify-write
        // operations. A notification is retained if its release happened
        // before this poll but the model still selected an older read.
        if stopped.is_some_and(|stopped| stopped.load(Ordering::Acquire)) || sc_load(word) != 1 {
            return;
        }
        notification.wait();
    }
    pub fn wake<T>(&self, word: &T) {
        let notifications = {
            let mut state = self.0.lock().unwrap();
            Self::notifications(&mut state, std::ptr::from_ref(word) as usize).clone()
        };
        for notification in notifications {
            notification.notify();
        }
    }
}
