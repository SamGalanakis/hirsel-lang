//! Interior mutability for fields owned by the scheduler role.
//!
//! A shared reference to RunShared may be held by a local or concurrent
//! service while the scheduler role changes another field. Each such field
//! must stop that shared borrow at UnsafeCell. This wrapper is deliberately
//! not Sync: crossing worker threads is the runtime driver's
//! unsafe operation, whose contract requires role ownership for every borrow.

use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};

#[repr(transparent)]
pub(crate) struct RoleCell<T>(UnsafeCell<T>);

impl<T> RoleCell<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// Project the field without creating an exclusive borrow of its run.
    /// Dereferencing requires role ownership, admission, or quiescence.
    pub(crate) fn get(&self) -> *mut T {
        self.0.get()
    }
}

impl<T: Default> Default for RoleCell<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> Deref for RoleCell<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: safe Rust cannot transfer this !Sync wrapper to another
        // thread or mutate it while borrowed. Runtime raw-pointer users must
        // separately obey their scheduler-role and no-borrow-across-leave
        // contracts, as for every TaskCtx/RunShared pointer they dereference.
        unsafe { &*self.0.get() }
    }
}

impl<T> DerefMut for RoleCell<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_run_borrow_does_not_freeze_a_role_owned_field() {
        struct Run {
            seed: u64,
            queue: RoleCell<Vec<u64>>,
        }
        let mut run = Run {
            seed: 7,
            queue: RoleCell::default(),
        };
        let raw = &raw mut run;
        let shared = unsafe { &*raw };
        // This is the field-projection pattern used under scheduler ownership.
        unsafe { (*(*raw).queue.get()).push(42) };
        assert_eq!(shared.seed, 7);
        assert_eq!(&**shared.queue, &[42]);
    }
}
