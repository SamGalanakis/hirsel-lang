//! Immutable receipt metadata custody, independent of language storage credits.
//!
//! Atomic holder ordering follows Rust alloc::rcs::arc (9cd159cf),
//! arc.rs:2520-2547 and :2970-3010: relaxed retain, release decrement, acquire
//! before destruction, and bounded counts. Unlike Arc, the last holder drains
//! dead children through their own intrusive words, without recursive Drop,
//! scratch allocation, weak holders, user finalizers or source-task allocators.
//! The semantic record is Copy, so destruction cannot invoke user code.
//!
//! This Rust-only primitive does not construct semantic receipts, acquire source
//! storage, validate recipes, or expose a native ABI. Its run identity and node
//! generation are metadata identities, never semantic allocation identities.
use super::RootBytes;
use crate::fault;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub mod journal;
pub mod semantic;

static RUNS: AtomicU64 = AtomicU64::new(1);
static GENERATIONS: AtomicU64 = AtomicU64::new(1);
const MAX_HOLDERS: usize = isize::MAX as usize;

fn fresh(counter: &AtomicU64) -> u64 {
    let id = counter.fetch_add(1, Ordering::Relaxed);
    if id == u64::MAX {
        fault("receipt metadata identity exhausted");
    }
    id
}

/// Stable identity shared only by demanded metadata in one live execution.
/// No task, worker, scope guard, pool or frame pointer is retained here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetadataRun(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    DifferentRun,
    InvalidImageLayout,
}

impl MetadataRun {
    pub fn new() -> Self {
        Self(fresh(&RUNS))
    }

    /// Construct one immutable snapshot. Each child occurrence acquires one
    /// metadata holder, including repeated references to the same node. This
    /// acquires no ordinary or Shared language credit. A record must not be
    /// treated as permission to dereference descriptive source addresses.
    pub fn node<T: Copy>(self, record: T, children: &[Receipt<T>]) -> Result<Receipt<T>, Error> {
        self.make_node(record, children, || None)
    }

    /// Copy initialized inline bytes to stable, aligned metadata storage.
    /// The image owns only bytes: embedded pointer words are descriptive and
    /// acquire no ordinary or Shared credits. It must not be dereferenced as a
    /// source storage permission. Padding must be initialized by the caller.
    /// Even an empty image is present and has the requested alignment.
    pub fn node_with_bytes<T: Copy>(
        self,
        record: T,
        bytes: &[u8],
        alignment: usize,
        children: &[Receipt<T>],
    ) -> Result<Receipt<T>, Error> {
        std::alloc::Layout::from_size_align(bytes.len().max(1), alignment)
            .map_err(|_| Error::InvalidImageLayout)?;
        self.make_node(record, children, || {
            Some(InlineImage {
                // SAFETY: the slice contains exactly len initialized bytes;
                // the preceding Layout check establishes size and alignment.
                storage: unsafe { RootBytes::snapshot(bytes.as_ptr(), bytes.len(), alignment) },
                size: bytes.len(),
            })
        })
    }

    fn make_node<T: Copy>(
        self,
        record: T,
        children: &[Receipt<T>],
        image: impl FnOnce() -> Option<InlineImage>,
    ) -> Result<Receipt<T>, Error> {
        if children.iter().any(|child| child.run() != self) {
            return Err(Error::DifferentRun);
        }
        // Refuse foreign children before allocating/copying an inline image
        // or acquiring any child holder, including a same-run prefix.
        let image = image();
        let children = children
            .iter()
            .map(|child| {
                retain(child.pointer);
                child.pointer
            })
            .collect();
        let node = Box::new(Node {
            holders: AtomicUsize::new(1),
            run: self,
            generation: fresh(&GENERATIONS),
            record,
            children,
            image,
            release_next: UnsafeCell::new(ptr::null_mut()),
        });
        Ok(Receipt {
            pointer: NonNull::from(Box::leak(node)),
        })
    }
}

impl Default for MetadataRun {
    fn default() -> Self {
        Self::new()
    }
}

struct InlineImage {
    storage: RootBytes,
    size: usize,
}

struct Node<T: Copy> {
    holders: AtomicUsize,
    run: MetadataRun,
    generation: u64,
    record: T,
    children: Box<[NonNull<Node<T>>]>,
    image: Option<InlineImage>,
    // Read/written only by the sole last-holder release after count reaches 0.
    release_next: UnsafeCell<*mut Node<T>>,
}

/// One owning metadata holder. Clone duplicates only this private holder, not
/// its children or any language source/storage/Shared ownership.
pub struct Receipt<T: Copy> {
    pointer: NonNull<Node<T>>,
}

// SAFETY: immutable record and child links use stable global allocations.
// Shared holders are atomic; the intrusive word is exclusive to the last
// release. Record bounds are exactly those needed to share immutable T.
unsafe impl<T: Copy + Send + Sync> Send for Receipt<T> {}
unsafe impl<T: Copy + Send + Sync> Sync for Receipt<T> {}

impl<T: Copy> Receipt<T> {
    pub fn view(&self) -> View<'_, T> {
        View {
            pointer: self.pointer,
            lifetime: PhantomData,
        }
    }

    pub fn run(&self) -> MetadataRun {
        self.view().run()
    }

    pub fn generation(&self) -> u64 {
        self.view().generation()
    }

    pub fn record(&self) -> &T {
        // SAFETY: this holder keeps the immutable record live.
        &unsafe { self.pointer.as_ref() }.record
    }

    pub fn bytes(&self) -> Option<&[u8]> {
        self.view().bytes()
    }

    pub fn alignment(&self) -> Option<usize> {
        self.view().alignment()
    }

    pub fn children(&self) -> impl ExactSizeIterator<Item = View<'_, T>> {
        self.view().children()
    }
}

impl<T: Copy> Clone for Receipt<T> {
    fn clone(&self) -> Self {
        retain(self.pointer);
        Self {
            pointer: self.pointer,
        }
    }
}

/// Borrowed metadata, bounded by a live holder or one of its retained edges.
/// Retaining a result/journal entry from a view explicitly acquires a holder.
#[derive(Clone, Copy)]
pub struct View<'a, T: Copy> {
    pointer: NonNull<Node<T>>,
    lifetime: PhantomData<&'a Receipt<T>>,
}

impl<'a, T: Copy> View<'a, T> {
    fn node(self) -> &'a Node<T> {
        // SAFETY: construction ties this view to an owning ancestor's lifetime.
        unsafe { self.pointer.as_ref() }
    }

    pub fn run(self) -> MetadataRun {
        self.node().run
    }

    pub fn generation(self) -> u64 {
        self.node().generation
    }

    pub fn record(self) -> &'a T {
        &self.node().record
    }

    pub fn bytes(self) -> Option<&'a [u8]> {
        self.node().image.as_ref().map(|image| {
            // SAFETY: immutable initialized bytes were copied into this exact
            // allocation; this view retains its ancestor-bounded lifetime.
            unsafe { std::slice::from_raw_parts(image.storage.data.as_ptr(), image.size) }
        })
    }

    pub fn alignment(self) -> Option<usize> {
        self.node()
            .image
            .as_ref()
            .map(|image| image.storage.allocation.align())
    }

    pub fn children(self) -> impl ExactSizeIterator<Item = View<'a, T>> {
        self.node().children.iter().map(|&pointer| View {
            pointer,
            lifetime: PhantomData,
        })
    }

    pub fn retain(self) -> Receipt<T> {
        retain(self.pointer);
        Receipt {
            pointer: self.pointer,
        }
    }
}

fn retain<T: Copy>(pointer: NonNull<Node<T>>) {
    // SAFETY: the caller owns or borrows an existing holder across this retain.
    let node = unsafe { pointer.as_ref() };
    if node.holders.fetch_add(1, Ordering::Relaxed) >= MAX_HOLDERS {
        fault("receipt metadata holder count exhausted");
    }
}

unsafe fn last<T: Copy>(pointer: *mut Node<T>) -> bool {
    // SAFETY: caller consumes exactly one still-live holder or child edge.
    let node = unsafe { &*pointer };
    if node.holders.fetch_sub(1, Ordering::Release) != 1 {
        return false;
    }
    // Rust Arc uses an acquire load under TSan (arc.rs:64-71). Use that form
    // here too: it observes the release sequence without an unsupported fence.
    let _ = node.holders.load(Ordering::Acquire);
    true
}

impl<T: Copy> Drop for Receipt<T> {
    fn drop(&mut self) {
        let mut pending = self.pointer.as_ptr();
        // SAFETY: self owns one holder and will never use this pointer again.
        if !unsafe { last(pending) } {
            return;
        }
        while !pending.is_null() {
            // SAFETY: only the unique last release queues this allocation.
            // No live holder/view can access it once its count reaches zero.
            let node = unsafe { Box::from_raw(pending) };
            // SAFETY: the current release exclusively owns this dead node.
            pending = unsafe { *node.release_next.get() };
            for &child in &node.children {
                // SAFETY: this dead parent's immutable link owns one holder.
                if unsafe { last(child.as_ptr()) } {
                    // SAFETY: last() established exclusive dead-node custody.
                    unsafe { *child.as_ref().release_next.get() = pending };
                    pending = child.as_ptr();
                }
            }
            // Box frees this exact node, child links and any aligned inline
            // image. Children are raw nonowning words; Copy T has no Drop.
            // RootBytes returns its exact original Layout without user code.
            drop(node);
        }
    }
}

#[cfg(test)]
mod tests;
