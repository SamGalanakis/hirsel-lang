//! One stable owner for a task's fixed depth scratch. Like Tokio ce685ee6's
//! runtime/task/core.rs:234-278, ownership and allocation move independently.
//! Unlike Tokio's task cell, TaskCtx itself moves between a box, fibers and
//! compact homes. Only its scratch stays here; native aliases never move.
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use hirsel_core_contract::depth::{Activation, CLEANUP_LIMIT};

#[repr(C)]
struct Storage {
    sites: [MaybeUninit<[u64; 2]>; super::INVOCATION_CAPACITY],
    tokens: [MaybeUninit<Activation>; super::INVOCATION_CAPACITY],
    cleanup: [MaybeUninit<Activation>; CLEANUP_LIMIT as usize],
}

/// Non-owning extent. Its initialized prefix belongs exclusively to TaskCtx.
/// The two-word representation preserves the context's existing field widths.
#[repr(C)]
pub(crate) struct Slots<T> {
    ptr: NonNull<MaybeUninit<T>>,
    length: usize,
}

impl<T> Slots<T> {
    pub(crate) fn as_ptr(&self) -> *const MaybeUninit<T> {
        self.ptr.as_ptr()
    }
    pub(crate) fn as_mut_ptr(&mut self) -> *mut MaybeUninit<T> {
        self.ptr.as_ptr()
    }
    pub(crate) fn len(&self) -> usize {
        self.length
    }
}

/// Owner in the existing site-array field; raw ownership keeps the native
/// pointers valid without reborrowing an owning Box after publication.
#[repr(C)]
pub(crate) struct Sites {
    owner: NonNull<Storage>,
    length: usize,
}

impl Sites {
    /// # Safety
    /// Keep the three extents together in one TaskCtx. The owning Sites must
    /// outlive every use of the token and cleanup extents, including after a move.
    pub(crate) unsafe fn new() -> (Slots<Activation>, Cleanup, Self) {
        let owner = Box::into_raw(Box::<Storage>::new_uninit()).cast::<Storage>();
        // SAFETY: every field is MaybeUninit, so field addresses require no
        // initialization. The one allocation outlives both non-owning extents.
        unsafe {
            let tokens = Slots {
                ptr: NonNull::new_unchecked(std::ptr::addr_of_mut!((*owner).tokens).cast()),
                length: super::INVOCATION_CAPACITY,
            };
            let cleanup = Cleanup {
                slots: Slots {
                    ptr: NonNull::new_unchecked(std::ptr::addr_of_mut!((*owner).cleanup).cast()),
                    length: CLEANUP_LIMIT as usize,
                },
                length: 0,
            };
            (
                tokens,
                cleanup,
                Self {
                    owner: NonNull::new_unchecked(owner),
                    length: super::INVOCATION_CAPACITY,
                },
            )
        }
    }

    pub(crate) fn as_ptr(&self) -> *const MaybeUninit<[u64; 2]> {
        self.owner.as_ptr().cast()
    }
    pub(crate) fn as_mut_ptr(&mut self) -> *mut MaybeUninit<[u64; 2]> {
        self.owner.as_ptr().cast()
    }
    pub(crate) fn len(&self) -> usize {
        self.length
    }
}

impl std::ops::Index<usize> for Sites {
    type Output = MaybeUninit<[u64; 2]>;
    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len());
        // SAFETY: the extent is allocated, even outside its initialized prefix.
        unsafe { &*self.as_ptr().add(index) }
    }
}

impl Drop for Sites {
    fn drop(&mut self) {
        // SAFETY: this is the unique owner; no element needs drop glue. Retired
        // and abandoned slots remain uninitialized for destruction purposes.
        unsafe {
            drop(Box::from_raw(
                self.owner.as_ptr().cast::<MaybeUninit<Storage>>(),
            ))
        };
    }
}

#[repr(C)]
pub(crate) struct Cleanup {
    slots: Slots<Activation>,
    length: usize,
}

impl Cleanup {
    pub(crate) fn len(&self) -> usize {
        self.length
    }
    pub(crate) fn capacity(&self) -> usize {
        self.slots.len()
    }
    pub(crate) fn push(&mut self, token: Activation) {
        assert!(self.length < self.capacity());
        // SAFETY: this slot is outside the initialized prefix and in bounds.
        unsafe {
            self.slots
                .as_mut_ptr()
                .add(self.length)
                .write(MaybeUninit::new(token))
        };
        self.length += 1;
    }
    pub(crate) fn pop(&mut self) -> Option<Activation> {
        self.length = self.length.checked_sub(1)?;
        // SAFETY: remove the unique last initialized token before reading it.
        Some(unsafe { self.slots.as_ptr().add(self.length).read().assume_init() })
    }
}

const _: () = {
    assert!(size_of::<Slots<Activation>>() == size_of::<Box<[MaybeUninit<Activation>]>>());
    assert!(size_of::<Sites>() == size_of::<Box<[MaybeUninit<[u64; 2]>]>>());
    assert!(size_of::<Cleanup>() == size_of::<Vec<Activation>>());
    assert!(align_of::<Slots<Activation>>() == align_of::<Box<[MaybeUninit<Activation>]>>());
    assert!(align_of::<Sites>() == align_of::<Box<[MaybeUninit<[u64; 2]>]>>());
    assert!(align_of::<Cleanup>() == align_of::<Vec<Activation>>());
};
