//! Loom models of cross-worker storage return (design §6 Heaps, `crates/hirsel-rt/src/remote.rs`).
//! A slab's storage is a loom `UnsafeCell` that every protocol access to the slab reads and its
//! release writes, so loom reports any access to a released slab as a race. Each model also
//! asserts that every freed block is collected exactly once.

hirsel_guard::install!();

use hirsel_heaps_loom::remote::{self, Link, Owner, Page, Returns, Xthread};
use loom::cell::UnsafeCell;
use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::collections::BTreeSet;

/// A block: its link word overlays its storage, owned by whoever holds the block.
struct Block {
    next: UnsafeCell<*mut Block>,
    id: u32,
}

impl Link for Block {
    unsafe fn set_next(node: *mut Block, next: *mut Block) {
        unsafe { (*node).next.with_mut(|p| *p = next) }
    }
    unsafe fn next(node: *mut Block) -> *mut Block {
        unsafe { (*node).next.with(|p| *p) }
    }
}

/// A slab: `storage` stands for its memory, which its release gives back.
struct Slab {
    storage: UnsafeCell<bool>,
    xthread: Xthread<Block>,
    queue_next: UnsafeCell<*mut Slab>,
    blocks: Vec<Block>,
}

impl Slab {
    fn new(blocks: u32) -> Slab {
        Slab {
            storage: UnsafeCell::new(true),
            xthread: Xthread::new(),
            queue_next: UnsafeCell::new(std::ptr::null_mut()),
            blocks: (0..blocks)
                .map(|id| Block {
                    next: UnsafeCell::new(std::ptr::null_mut()),
                    id,
                })
                .collect(),
        }
    }
    /// A protocol access to the slab's memory: a race with its release if unsynchronised.
    fn touch(&self) {
        self.storage
            .with(|alive| assert!(unsafe { *alive }, "a released slab was touched"));
    }
    fn release(&self) {
        self.storage.with_mut(|alive| unsafe { *alive = false });
    }
    fn block(&self, id: u32) -> *mut Block {
        (&self.blocks[id as usize] as *const Block).cast_mut()
    }
}

impl Link for Slab {
    unsafe fn set_next(node: *mut Slab, next: *mut Slab) {
        unsafe {
            (*node).touch();
            (*node).queue_next.with_mut(|p| *p = next)
        }
    }
    unsafe fn next(node: *mut Slab) -> *mut Slab {
        unsafe { (*node).queue_next.with(|p| *p) }
    }
}

impl Page for Slab {
    type Block = Block;
    unsafe fn xthread<'a>(page: *mut Slab) -> &'a Xthread<Block> {
        unsafe {
            (*page).touch();
            &(*page).xthread
        }
    }
}

/// A heap as its collector sees it: the queue other heaps publish to, and the slab's live
/// count, which only the collector reads or writes.
struct Heap {
    queue: Returns<Slab>,
    slab: Slab,
    live: UnsafeCell<u32>,
    released: AtomicBool,
    collected: UnsafeCell<BTreeSet<u32>>,
}

// SAFETY: the model's raw pointers name storage inside the Arc'd heap.
unsafe impl Send for Heap {}
unsafe impl Sync for Heap {}

impl Heap {
    fn new(blocks: u32) -> Heap {
        Heap {
            queue: Returns::new(),
            slab: Slab::new(blocks),
            live: UnsafeCell::new(blocks),
            released: AtomicBool::new(false),
            collected: UnsafeCell::new(BTreeSet::new()),
        }
    }
    fn page(&self) -> *mut Slab {
        (&self.slab as *const Slab).cast_mut()
    }
    /// Another heap frees block `id`.
    fn free_remote(&self, id: u32) {
        unsafe { remote::free_remote(&self.queue, self.page(), self.slab.block(id)) }
    }
    fn take_back(&self, block: *mut Block) {
        let id = unsafe { (*block).id };
        self.collected
            .with_mut(|seen| assert!(unsafe { (*seen).insert(id) }, "block {id} collected twice"));
        self.live.with_mut(|live| unsafe { *live -= 1 });
    }
    /// The collector's pass: the queue, then the direct collection of its current page.
    fn collect(&self) {
        if self.released.load(Ordering::Relaxed) {
            return;
        }
        unsafe {
            remote::collect(&self.queue, |_, block| self.take_back(block));
            let page = self.page();
            let xthread = <Slab as Page>::xthread(page);
            #[cfg(not(feature = "direct-dequeue"))]
            let chain = xthread.take();
            #[cfg(feature = "direct-dequeue")]
            let chain = xthread.dequeue();
            remote::drain(page, chain, &mut |_, block| self.take_back(block));
        }
    }
    /// The trim point: release the slab when it holds no live block and none published.
    fn trim(&self) {
        if self.released.load(Ordering::Relaxed) {
            return;
        }
        self.collect();
        let live = self.live.with(|live| unsafe { *live });
        #[cfg(not(feature = "blind-trim"))]
        let busy = self.slab.xthread.busy();
        #[cfg(feature = "blind-trim")]
        let busy = false;
        if live == 0 && !busy {
            self.slab.release();
            self.released.store(true, Ordering::Relaxed);
        }
    }
}

/// The owner frees one block itself and trims while another heap frees the slab's last block:
/// the slab is released only after the remote free stopped touching it, and both blocks are
/// collected once.
#[test]
fn remote_final_free_against_trim() {
    loom::model(|| {
        let heap = Arc::new(Heap::new(2));
        let remote = {
            let heap = heap.clone();
            loom::thread::spawn(move || heap.free_remote(1))
        };
        // The owner's own free of block 0 is local: it counts down live directly.
        heap.live.with_mut(|live| unsafe { *live -= 1 });
        heap.collected.with_mut(|seen| unsafe { (*seen).insert(0) });
        heap.trim();
        heap.trim();
        remote.join().unwrap();
        heap.trim();
        heap.trim();
        assert!(
            heap.released.load(Ordering::Relaxed),
            "the empty slab was never released"
        );
        heap.collected
            .with(|seen| assert_eq!(unsafe { (*seen).len() }, 2));
    });
}

/// Two other heaps free blocks of one slab while its owner collects: each block comes back
/// once, the slab is queued again after a dequeue, and nothing is lost.
#[test]
fn concurrent_remote_frees_are_collected_once() {
    loom::model(|| {
        let heap = Arc::new(Heap::new(2));
        let frees: Vec<_> = (0..2)
            .map(|id| {
                let heap = heap.clone();
                loom::thread::spawn(move || heap.free_remote(id))
            })
            .collect();
        heap.collect();
        for free in frees {
            free.join().unwrap();
        }
        heap.collect();
        heap.collected
            .with(|seen| assert_eq!(unsafe { (*seen).len() }, 2));
        heap.live.with(|live| assert_eq!(unsafe { *live }, 0));
        assert!(!heap.slab.xthread.busy(), "a collected slab stays busy");
    });
}

/// A worker retires its heap while a live worker adopts orphans and another heap frees into it:
/// at most one worker collects at a time (loom checks the collector's cells), and the block
/// comes back once.
#[test]
fn retired_heap_has_one_collector() {
    loom::model(|| {
        let heap = Arc::new(Heap::new(1));
        let owner = Arc::new(Owner::new(0));
        let remote = {
            let heap = heap.clone();
            loom::thread::spawn(move || heap.free_remote(0))
        };
        let adopter = {
            let (heap, owner) = (heap.clone(), owner.clone());
            loom::thread::spawn(move || {
                if owner.adopt(1) {
                    heap.collect();
                    owner.orphan();
                }
            })
        };
        // The retiring worker's last collection, then its retirement.
        heap.collect();
        owner.orphan();
        remote.join().unwrap();
        adopter.join().unwrap();
        // Any later scheduling point collects what is left.
        assert!(owner.adopt(2));
        heap.collect();
        heap.collected
            .with(|seen| assert_eq!(unsafe { (*seen).len() }, 1));
    });
}

/// A large block or segment return list: two heaps push while the owner takes, and every node
/// is taken exactly once.
#[test]
fn return_list_hands_every_node_over_once() {
    struct Node {
        next: UnsafeCell<*mut Node>,
        id: u32,
    }
    impl Link for Node {
        unsafe fn set_next(node: *mut Node, next: *mut Node) {
            unsafe { (*node).next.with_mut(|p| *p = next) }
        }
        unsafe fn next(node: *mut Node) -> *mut Node {
            unsafe { (*node).next.with(|p| *p) }
        }
    }
    struct List {
        returns: Returns<Node>,
        nodes: [Node; 2],
    }
    unsafe impl Send for List {}
    unsafe impl Sync for List {}
    loom::model(|| {
        let list = Arc::new(List {
            returns: Returns::new(),
            nodes: [0, 1].map(|id| Node {
                next: UnsafeCell::new(std::ptr::null_mut()),
                id,
            }),
        });
        let pushers: Vec<_> = (0..2)
            .map(|i| {
                let list = list.clone();
                loom::thread::spawn(move || unsafe {
                    list.returns
                        .push((&list.nodes[i] as *const Node).cast_mut())
                })
            })
            .collect();
        let taken = AtomicU32::new(0);
        let mut seen = BTreeSet::new();
        let take = |seen: &mut BTreeSet<u32>| {
            let mut node = list.returns.take();
            while !node.is_null() {
                unsafe {
                    assert!(seen.insert((*node).id), "a node was taken twice");
                    node = Node::next(node);
                }
                taken.fetch_add(1, Ordering::Relaxed);
            }
        };
        take(&mut seen);
        for pusher in pushers {
            pusher.join().unwrap();
        }
        take(&mut seen);
        assert_eq!(seen.len(), 2);
    });
}
