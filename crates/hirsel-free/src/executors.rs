//! The registered executor slots of one recording: which section words have an owner. An
//! executor registers once as it starts serving a recorded run and retires as it leaves, after
//! its section word is acknowledged inactive; a starting executor that finds every word held
//! blocks until one retires (or the run stops), never spinning. `proofs/free` compiles this
//! module against loom through [`crate::sync`].

use crate::sync::{Arc, AtomicBool, AtomicUsize, Condvar, Mutex, Ordering, sc_fence};

/// The home executor's section word.
pub const HOME: usize = 0;

/// A recording's executor registry; clones share it. Slot [`HOME`] belongs to the run's home
/// executor: only a caller preferring it takes it, so home never waits behind another
/// executor's registration.
#[derive(Clone)]
pub struct Executors(Arc<Words>);

struct Words {
    held: Box<[AtomicBool]>,
    /// Executors blocked in [`Executors::register`]; a retirement wakes them only when nonzero.
    waiting: AtomicUsize,
    retired: (Mutex<()>, Condvar),
}

impl Executors {
    pub fn new(slots: usize) -> Executors {
        Executors(Arc::new(Words {
            held: (0..slots).map(|_| AtomicBool::new(false)).collect(),
            waiting: AtomicUsize::new(0),
            retired: (Mutex::new(()), Condvar::new()),
        }))
    }

    /// Register an executor on section word `index`, outside any section. A second owner
    /// cannot register it until the first retires (drops its slot).
    pub fn executor(&self, index: usize) -> Option<ExecutorSlot> {
        let held = self.0.held.get(index)?;
        held.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        Some(ExecutorSlot {
            words: self.0.clone(),
            index,
            executor: std::marker::PhantomData,
        })
    }

    /// Register an executor on the preferred section word, or another free one when an old
    /// retiring executor still owns it ([`HOME`] only when preferred). No section is open while
    /// searching. `None` when every word is held: a caller that may hold a scheduler role or
    /// section never waits for one, and an executor starting outside both uses [`register`].
    ///
    /// [`register`]: Self::register
    pub fn available(&self, preferred: usize) -> Option<ExecutorSlot> {
        self.executor(preferred).or_else(|| {
            (0..self.0.held.len())
                .filter(|&index| index != preferred && index != HOME)
                .find_map(|index| self.executor(index))
        })
    }

    /// Register a starting executor once, for its whole lifetime, outside any scheduler role or
    /// section. When every word is held by live or retiring executors it blocks until one
    /// retires, never spinning; `None` once `stopped`, when no section can open again (the
    /// stop calls [`wake`](Self::wake) after it).
    pub fn register(&self, preferred: usize, stopped: impl Fn() -> bool) -> Option<ExecutorSlot> {
        if let Some(slot) = self.available(preferred) {
            return Some(slot);
        }
        let words = &*self.0;
        let mut guard = words.retired.0.lock().unwrap();
        words.waiting.fetch_add(1, Ordering::Relaxed);
        // Paired with `release`; a later release wakes under this lock.
        sc_fence();
        let slot = loop {
            if stopped() {
                break None;
            }
            if let Some(slot) = self.available(preferred) {
                break Some(slot);
            }
            guard = words.retired.1.wait(guard).unwrap();
        };
        words.waiting.fetch_sub(1, Ordering::Relaxed);
        slot
    }

    /// The section words registered now: live, retiring or nested executors.
    pub fn registered(&self) -> Vec<usize> {
        (0..self.0.held.len())
            .filter(|&index| self.0.held[index].load(Ordering::Acquire))
            .collect()
    }

    /// Wake every waiting registration to search again: the stop.
    pub fn wake(&self) {
        self.0.wake();
    }
}

impl Words {
    fn release(&self, index: usize) {
        self.held[index].store(false, Ordering::Release);
        // Paired with `register`'s fence: a waiter either sees this word free in its search,
        // or is counted here and woken.
        sc_fence();
        if self.waiting.load(Ordering::Relaxed) != 0 {
            self.wake();
        }
    }

    fn wake(&self) {
        drop(self.retired.0.lock().unwrap());
        self.retired.1.notify_all();
    }
}

/// One registered physical executor's section word, owned for the executor's registered
/// lifetime: a compute worker, the home thread, a blocking or pool thread. It is not `Send`,
/// so it never migrates; a migrating task moves its encoder, never the slot. Phase entry
/// borrows it mutably, so a slot runs one section at a time, and dropping it (the executor's
/// retirement) cannot happen while a section on it is open. A nested callback on an executor
/// whose slot is checked out registers a distinct slot instead of reusing it.
pub struct ExecutorSlot {
    words: Arc<Words>,
    index: usize,
    executor: std::marker::PhantomData<*const ()>,
}

impl ExecutorSlot {
    /// The section word this slot owns.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Whether this slot was registered with `executors`.
    pub fn belongs(&self, executors: &Executors) -> bool {
        Arc::ptr_eq(&self.words, &executors.0)
    }
}

impl Drop for ExecutorSlot {
    fn drop(&mut self) {
        self.words.release(self.index);
    }
}
