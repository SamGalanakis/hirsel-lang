//! Run-owned segment membership, independent of a worker's mutable pool.
//! Mutex serializes intrusive-list edits; no guard crosses allocation, a
//! scheduler call, or user code. Ordinary idle lists and remote returns remain
//! collector owned. A segment remembers this stable allocation, never a task
//! frame or a mutable RegionPool reference.
use super::Segment;
use std::ptr;
use std::sync::Mutex;

#[derive(Default)]
pub(super) struct LentList(Mutex<State>);

struct State {
    active: *mut Segment,
    sealed: *mut Segment,
    active_bytes: u64,
    sealed_bytes: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            active: ptr::null_mut(),
            sealed: ptr::null_mut(),
            active_bytes: 0,
            sealed_bytes: 0,
        }
    }
}

// SAFETY: the mutex is the sole reader/writer of membership links and heads;
// payload and region-chain ownership stay with the segment's affine holder.
unsafe impl Send for State {}

impl LentList {
    #[cfg(any(test, feature = "diagnostic", feature = "measurement"))]
    pub(super) fn active_bytes(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_bytes
    }

    #[cfg(any(test, feature = "diagnostic", feature = "measurement"))]
    pub(super) fn owned_bytes(&self) -> u64 {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.active_bytes + state.sealed_bytes
    }

    /// # Safety
    /// `segment` belongs to this home, is live, and has no list membership.
    pub(super) unsafe fn insert(&self, segment: *mut Segment) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            (*segment)._reserved = 0;
            link(&mut state.active, segment);
            state.active_bytes += (*segment).size;
        }
    }

    /// Move membership to sealed custody without borrowing the home pool.
    /// # Safety
    /// The caller exclusively owns `segment`'s active region storage.
    pub(super) unsafe fn seal(&self, segment: *mut Segment) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            assert_eq!((*segment)._reserved, 0, "segment adopted twice");
            unlink(&mut state.active, segment);
            state.active_bytes -= (*segment).size;
            (*segment)._reserved = 1;
            link(&mut state.sealed, segment);
            state.sealed_bytes += (*segment).size;
        }
    }

    /// # Safety
    /// `segment` belongs to this home and its owner has relinquished it once.
    pub(super) unsafe fn remove(&self, segment: *mut Segment) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            if (*segment)._reserved == 0 {
                unlink(&mut state.active, segment);
                state.active_bytes -= (*segment).size;
            } else {
                assert_eq!((*segment)._reserved, 1, "invalid segment custody");
                unlink(&mut state.sealed, segment);
                state.sealed_bytes -= (*segment).size;
            }
            (*segment)._reserved = 2;
        }
    }

    #[cfg(feature = "measurement")]
    pub(super) fn inspect<R>(&self, visit: impl FnOnce(*mut Segment, *mut Segment) -> R) -> R {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        visit(state.active, state.sealed)
    }

    /// Run teardown, after all workers quiesce. Return both disjoint chains.
    pub(super) fn take_all(&self) -> [*mut Segment; 2] {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let heads = [state.active, state.sealed];
        *state = State::default();
        heads
    }
}

unsafe fn link(head: &mut *mut Segment, segment: *mut Segment) {
    unsafe {
        (*segment).lent_prev = ptr::null_mut();
        (*segment).lent_next = *head;
        if !(*head).is_null() {
            (**head).lent_prev = segment;
        }
        *head = segment;
    }
}

unsafe fn unlink(head: &mut *mut Segment, segment: *mut Segment) {
    unsafe {
        if (*segment).lent_prev.is_null() {
            *head = (*segment).lent_next;
        } else {
            (*(*segment).lent_prev).lent_next = (*segment).lent_next;
        }
        if !(*segment).lent_next.is_null() {
            (*(*segment).lent_next).lent_prev = (*segment).lent_prev;
        }
        (*segment).lent_prev = ptr::null_mut();
        (*segment).lent_next = ptr::null_mut();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn segment(home: &LentList) -> Box<Segment> {
        Box::new(Segment {
            prev: ptr::null_mut(),
            size: 4096,
            lent_prev: ptr::null_mut(),
            lent_next: ptr::null_mut(),
            owner: 0,
            _reserved: 2,
            return_next: ptr::null_mut(),
            home,
        })
    }

    #[test]
    fn adoption_moves_only_named_membership_and_release_removes_it_once() {
        let home = LentList::default();
        let mut outer = segment(&home);
        let mut inner = segment(&home);
        unsafe {
            home.insert(&mut *outer);
            home.insert(&mut *inner);
        }
        assert_eq!(home.active_bytes(), 8192);
        unsafe {
            home.seal(&mut *outer);
        }
        assert_eq!(home.active_bytes(), 4096);
        assert_eq!(home.owned_bytes(), 8192);
        unsafe {
            home.remove(&mut *inner);
        }
        assert_eq!(home.active_bytes(), 0);
        assert_eq!(home.owned_bytes(), 4096);
        unsafe {
            home.remove(&mut *outer);
        }
        assert_eq!(home.owned_bytes(), 0);
        assert_eq!(home.take_all(), [ptr::null_mut(); 2]);
    }

    #[test]
    fn run_teardown_retains_both_disjoint_chains_until_quiescence() {
        let home = LentList::default();
        let mut a = segment(&home);
        let mut b = segment(&home);
        unsafe {
            home.insert(&mut *a);
            home.insert(&mut *b);
            home.seal(&mut *a);
        }
        assert_eq!(
            home.take_all(),
            [&mut *b as *mut Segment, &mut *a as *mut Segment]
        );
        assert_eq!(home.active_bytes(), 0);
        assert_eq!(home.owned_bytes(), 0);
        assert!(a.lent_next.is_null() && b.lent_next.is_null());
    }

    #[test]
    fn receiver_detaches_without_borrowing_the_collectors_pool() {
        struct Transfer(Box<Segment>);
        // SAFETY: the segment payload and region-chain links move affinely;
        // home membership links are accessed only under the shared lock.
        unsafe impl Send for Transfer {}
        impl Transfer {
            fn adopt(mut self) -> Self {
                unsafe {
                    (&*self.0.home).seal(&mut *self.0);
                }
                self
            }
            fn release(mut self) {
                unsafe {
                    (&*self.0.home).remove(&mut *self.0);
                }
            }
        }
        let home = Arc::new(LentList::default());
        let mut moved = Transfer(segment(&home));
        unsafe {
            home.insert(&mut *moved.0);
        }
        let worker_home = home.clone();
        let receiver = std::thread::spawn(move || {
            let moved = moved.adopt();
            assert!(worker_home.owned_bytes() >= 4096);
            moved
        });
        for _ in 0..16 {
            let mut local = segment(&home);
            unsafe {
                home.insert(&mut *local);
                home.remove(&mut *local);
            }
        }
        let moved = receiver.join().unwrap();
        assert_eq!(home.active_bytes(), 0);
        assert_eq!(home.owned_bytes(), 4096);
        moved.release();
        assert_eq!(home.owned_bytes(), 0);
    }
}
