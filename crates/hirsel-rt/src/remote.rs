//! Cross-worker storage return (design §6 Heaps): the lists a worker publishes another heap's
//! storage on, and the word that names a heap's one collector. Ported from mimalloc's
//! thread-free protocol (`free.c:80-94` `mi_free_block_mt`, `page.c` `_mi_page_thread_free_collect`;
//! MIT, see third-party/NOTICES.md), simplified where Hirsel's heaps allow it:
//!
//! - A freed small block of another heap's slab is pushed on that slab's [`Xthread`] word. The
//!   word's low bit is the slab's *queued* flag, set by the same CAS as the push, so a push and
//!   the decision to queue are one atomic step. The first publisher after a dequeue queues the
//!   slab on its heap's [`Returns`] list; that is how the collector finds slabs it does not
//!   allocate from, where mimalloc instead keeps a delayed-free state per page.
//! - A slab is released only by its heap's collector, only when it holds no live block and its
//!   word is zero ([`Xthread::busy`]). A publisher touches the slab after its push only when it
//!   set the flag, and the flag is cleared only by the collector that dequeued the slab, after
//!   the publisher's queue push. So no publisher touches a released slab.
//! - Large blocks and region segments are pushed whole on their owner heap's [`Returns`] lists.
//! - Every list is push-only and taken whole by one swap, so there is no ABA.
//! - [`Owner`] names a heap's collector: a worker id, or [`ORPHAN`] after its worker retired.
//!   Adoption is a CAS from `ORPHAN`, so a heap has at most one collector.
//!
//! The loom model in `proofs/heaps` checks this module's own source.

use std::marker::PhantomData;

use crate::remote_sync::{AtomicU32, AtomicUsize, Ordering};

/// An intrusive singly linked node: storage whose link word its current holder owns.
pub trait Link {
    /// # Safety
    /// `node` is the caller's: no other thread reads or writes its link.
    unsafe fn set_next(node: *mut Self, next: *mut Self);
    /// # Safety
    /// `node` was taken off a list by the caller.
    unsafe fn next(node: *mut Self) -> *mut Self;
}

/// A push-only list any thread pushes to and only its heap's collector takes, whole.
pub struct Returns<T> {
    head: AtomicUsize,
    _nodes: PhantomData<*mut T>,
}

// SAFETY: the list only moves node pointers between threads; its push and take give the
// happens-before that hands each node's storage over.
unsafe impl<T> Send for Returns<T> {}
// SAFETY: as for `Send`.
unsafe impl<T> Sync for Returns<T> {}

impl<T: Link> Returns<T> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Returns<T> {
        Returns {
            head: AtomicUsize::new(0),
            _nodes: PhantomData,
        }
    }

    /// Publish `node` to the collector.
    ///
    /// # Safety
    /// `node` is the caller's and stays valid until the collector takes it.
    pub unsafe fn push(&self, node: *mut T) {
        let mut head = self.head.load(Ordering::Relaxed);
        loop {
            // SAFETY: caller contract.
            unsafe { T::set_next(node, head as *mut T) };
            match self.head.compare_exchange_weak(
                head,
                node as usize,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(current) => head = current,
            }
        }
    }

    /// Take every published node, newest first, as a chain through [`Link::next`]: the
    /// collector's only read.
    pub fn take(&self) -> *mut T {
        self.head.swap(0, Ordering::Acquire) as *mut T
    }

    /// Whether nothing is published: a hint, so a collector skips the swap.
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Relaxed) == 0
    }
}

/// A slab's list of blocks freed by other heaps, with its queued flag in the low bit.
#[repr(transparent)]
pub struct Xthread<T> {
    word: AtomicUsize,
    _blocks: PhantomData<*mut T>,
}

const QUEUED: usize = 1;

impl<T: Link> Xthread<T> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Xthread<T> {
        Xthread {
            word: AtomicUsize::new(0),
            _blocks: PhantomData,
        }
    }

    /// Publish the freed `block` (at least two-byte aligned). True when the caller set the
    /// queued flag: it must then queue the slab on its heap's list, and only then may it stop
    /// touching the slab.
    ///
    /// # Safety
    /// `block` is a dead block of this slab that the caller owns.
    pub unsafe fn push(&self, block: *mut T) -> bool {
        let mut word = self.word.load(Ordering::Relaxed);
        loop {
            // SAFETY: caller contract.
            unsafe { T::set_next(block, (word & !QUEUED) as *mut T) };
            // Acquire as well: a publisher that sets the flag again after a dequeue writes the
            // slab's queue link, which the previous publisher wrote before the collector took
            // the slab off the queue and dequeued it (release). The loom model finds the race
            // without it.
            match self.word.compare_exchange_weak(
                word,
                block as usize | QUEUED,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return word & QUEUED == 0,
                Err(current) => word = current,
            }
        }
    }

    /// Take the published blocks and keep the queued flag: the collector's direct collection
    /// of a slab it allocates from, which may still be on the queue.
    pub fn take(&self) -> *mut T {
        (self.word.fetch_and(QUEUED, Ordering::Acquire) & !QUEUED) as *mut T
    }

    /// Take the published blocks and clear the queued flag: only the collector that took this
    /// slab off its heap's queue.
    pub fn dequeue(&self) -> *mut T {
        // Release: the next publisher to set the flag rewrites the slab's queue link.
        (self.word.swap(0, Ordering::AcqRel) & !QUEUED) as *mut T
    }

    /// Whether the slab is queued or holds published blocks, so its collector must not release
    /// it.
    pub fn busy(&self) -> bool {
        self.word.load(Ordering::Acquire) != 0
    }
}

/// A slab as the remote-free protocol sees it: its published list, and its own link on its
/// heap's queue.
pub trait Page: Link {
    type Block: Link;
    /// # Safety
    /// `page` is a live slab.
    unsafe fn xthread<'a>(page: *mut Self) -> &'a Xthread<Self::Block>;
}

/// Free `block` of `page`, a slab of another heap whose queue is `queue`.
///
/// # Safety
/// `block` is a dead block of the live slab `page` that the caller owns, and `queue` is its
/// heap's, which outlives the call.
pub unsafe fn free_remote<P: Page>(queue: &Returns<P>, page: *mut P, block: *mut P::Block) {
    // SAFETY: caller contract: the slab is live while the caller holds its block, and stays
    // live after the push while its queued flag is set (module comment).
    unsafe {
        if P::xthread(page).push(block) {
            queue.push(page);
        }
    }
}

/// The collector's pass over its heap's queue: dequeue each slab and hand each of its blocks
/// to `each`. Returns the number of blocks collected.
///
/// # Safety
/// The caller is the heap's one collector and `queue` is its heap's.
pub unsafe fn collect<P: Page>(
    queue: &Returns<P>,
    mut each: impl FnMut(*mut P, *mut P::Block),
) -> u64 {
    let mut collected = 0;
    let mut page = queue.take();
    while !page.is_null() {
        // SAFETY: a queued slab is live (it is busy), and the collector took it off the queue.
        unsafe {
            let next = P::next(page);
            collected += drain(page, P::xthread(page).dequeue(), &mut each);
            page = next;
        }
    }
    collected
}

/// Hand each block of the chain `block` of `page` to `each`.
///
/// # Safety
/// The chain was taken from `page`'s word by its collector.
pub unsafe fn drain<P: Page>(
    page: *mut P,
    mut block: *mut P::Block,
    each: &mut impl FnMut(*mut P, *mut P::Block),
) -> u64 {
    let mut collected = 0;
    while !block.is_null() {
        // SAFETY: caller contract.
        let next = unsafe { P::Block::next(block) };
        each(page, block);
        collected += 1;
        block = next;
    }
    collected
}

/// A heap's collector: a worker id, or [`ORPHAN`].
pub struct Owner {
    word: AtomicU32,
}

/// The owner word of a heap whose worker retired: the next worker to adopt it collects it.
pub const ORPHAN: u32 = u32::MAX;

impl Owner {
    pub fn new(worker: u32) -> Owner {
        Owner {
            word: AtomicU32::new(worker),
        }
    }

    /// The collector gives the heap up: everything it did to the heap happens before the
    /// adopter's first access.
    pub fn orphan(&self) {
        self.word.store(ORPHAN, Ordering::Release);
    }

    /// Become the collector of an orphaned heap; false when another worker did, or it is not
    /// orphaned.
    pub fn adopt(&self, worker: u32) -> bool {
        self.word
            .compare_exchange(ORPHAN, worker, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
}
