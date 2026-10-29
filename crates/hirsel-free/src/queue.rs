//! Tokio's bounded single-producer/multi-consumer worker deque, ported from
//! `tokio/src/runtime/scheduler/multi_thread/queue.rs:28-101,142-584`, commit
//! ce685ee6916549ed7e52dbfd5430858bbdc734d3 (MIT). The packed head reserves a
//! steal before reading slots and keeps the producer from reusing them until
//! the whole batch has moved. This allows stealing started, suspended tasks.
//!
//! Hirsel differences: generic owned task identifiers replace Notified<T>;
//! overflow and stolen batches return to the engine instead of calling Tokio's
//! injection queue/stats; AtomicU32's relaxed load replaces unsync_load on the
//! sole producer. These keep the protocol independent of a runtime and let the
//! same source run under loom. LIFO slots remain owner-local (schedule.rs).

use crate::sync::{Arc, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Cell, Ordering};
use std::marker::PhantomData;

/// The production capacity (Tokio: 256). Models instantiate four slots.
pub const CAPACITY: usize = 256;

struct Inner<T, const N: usize> {
    head: AtomicU64,
    tail: AtomicU32,
    buffer: Ring<T, N>,
    references: AtomicUsize,
}

/// The last handle of this queue releases its ring independently of the
/// shared metadata arena. The owning marker preserves the original T: Send
/// requirement, including for the atomic pointer's otherwise-erased pointee.
struct Ring<T, const N: usize> {
    pointer: AtomicPtr<Cell<Option<T>>>,
    owns: PhantomData<Box<[Cell<Option<T>>]>>,
}
impl<T, const N: usize> Ring<T, N> {
    fn new() -> Self {
        Self {
            pointer: AtomicPtr::new(std::ptr::null_mut()),
            owns: PhantomData,
        }
    }
    fn get(&self) -> Option<&[Cell<Option<T>>]> {
        let pointer = self.pointer.load(Ordering::Acquire);
        if pointer.is_null() {
            None
        } else {
            // SAFETY: the caller holds a queue handle. Only its last handle
            // clears this initialized N-element slice, after all users leave.
            Some(unsafe { std::slice::from_raw_parts(pointer, N) })
        }
    }
    fn prepare(&self) {
        if self.get().is_none() {
            let slots: Box<[Cell<Option<T>>]> = (0..N).map(|_| Cell::new(None)).collect();
            // The sole producer initializes once, before publishing a tail.
            self.pointer
                .store(Box::into_raw(slots).cast(), Ordering::Release);
        }
    }
    /// # Safety
    /// The caller owns the queue's last handle, or the metadata arena itself.
    unsafe fn clear(&self) {
        let pointer = self.pointer.swap(std::ptr::null_mut(), Ordering::Acquire);
        if !pointer.is_null() {
            // SAFETY: prepare transferred this exact N-element Box. No handle
            // can reach its cells after the last reference's acquire edge.
            drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(pointer, N)) });
        }
    }
}
impl<T, const N: usize> Drop for Ring<T, N> {
    fn drop(&mut self) {
        // SAFETY: every handle retains the arena, so none outlives this Drop.
        unsafe { self.clear() };
    }
}

/// One producer/owner. It may move between threads, but cannot be shared.
pub struct Local<T, const N: usize = CAPACITY> {
    inner: Shared<T, N>,
    /// Cell is !Sync while remaining Send: only the owner pushes/pops.
    owner: PhantomData<std::cell::Cell<()>>,
}

/// Any number of thieves. A steal takes ceil(n/2) from the FIFO's front.
pub struct Steal<T, const N: usize = CAPACITY>(Shared<T, N>);

/// Metadata belongs to the whole ready topology, whose thieves already
/// retain every local queue until run drain. Rings remain owner-initialized.
struct Shared<T, const N: usize> {
    arena: Arc<Vec<Inner<T, N>>>,
    index: usize,
}
impl<T, const N: usize> Shared<T, N> {
    fn new(arena: Arc<Vec<Inner<T, N>>>, index: usize) -> Self {
        // Each queue count is bounded by the arena's Arc count, whose clone
        // performs the standard overflow check before reaching this increment.
        arena[index].references.fetch_add(1, Ordering::Relaxed);
        Self { arena, index }
    }
}
impl<T, const N: usize> Clone for Shared<T, N> {
    fn clone(&self) -> Self {
        Self::new(self.arena.clone(), self.index)
    }
}
impl<T, const N: usize> Drop for Shared<T, N> {
    fn drop(&mut self) {
        let inner = &self.arena[self.index];
        if inner.references.fetch_sub(1, Ordering::AcqRel) == 1 {
            // SAFETY: all producers, thieves and their borrows have ended.
            unsafe { inner.buffer.clear() };
        }
    }
}
impl<T, const N: usize> std::ops::Deref for Shared<T, N> {
    type Target = Inner<T, N>;
    fn deref(&self) -> &Self::Target {
        &self.arena[self.index]
    }
}

fn pack(steal: u32, real: u32) -> u64 {
    (u64::from(steal) << 32) | u64::from(real)
}

fn unpack(word: u64) -> (u32, u32) {
    ((word >> 32) as u32, word as u32)
}

pub fn local<T, const N: usize>() -> (Local<T, N>, Steal<T, N>) {
    locals(1).next().unwrap()
}

/// Construct the fixed topology's metadata once, without eagerly backing any
/// ring. Each index still has exactly one producer and the original head CAS.
pub(crate) fn locals<T, const N: usize>(
    count: usize,
) -> impl Iterator<Item = (Local<T, N>, Steal<T, N>)> {
    assert!(N >= 2 && N.is_power_of_two() && N <= u16::MAX as usize);
    assert!(count != 0);
    let arena = Arc::new(
        (0..count)
            .map(|_| Inner {
                head: AtomicU64::new(0),
                tail: AtomicU32::new(0),
                buffer: Ring::new(),
                references: AtomicUsize::new(0),
            })
            .collect::<Vec<_>>(),
    );
    (0..count).map(move |index| {
        let inner = Shared::new(arena.clone(), index);
        (
            Local {
                inner: inner.clone(),
                owner: PhantomData,
            },
            Steal(inner),
        )
    })
}

impl<T, const N: usize> Clone for Steal<T, N> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T, const N: usize> Inner<T, N> {
    /// # Safety
    /// The caller owns this slot, having claimed it by the head CAS. A
    /// stealer retains its reservation until every read is finished.
    unsafe fn take(&self, index: u32) -> T {
        // SAFETY: caller contract.
        unsafe {
            self.buffer.get().expect("published queue storage")[index as usize & (N - 1)]
                .with_mut(|slot| slot.take().unwrap())
        }
    }

    /// # Safety
    /// Sole producer; index is the unpublished tail, outside the reserved range.
    unsafe fn put(&self, index: u32, task: T) {
        // SAFETY: caller contract.
        unsafe {
            self.buffer.get().expect("published queue storage")[index as usize & (N - 1)].with_mut(
                |slot| {
                    assert!(slot.is_none());
                    *slot = Some(task);
                },
            )
        }
    }
}

impl<T, const N: usize> Local<T, N> {
    /// Back this owner's ring before entering an allocation-free scheduler role.
    /// Thieves touch it only after acquiring the first published tail, so an
    /// unused queue needs no ring. The sole producer initializes it once.
    pub fn prepare(&mut self) {
        self.inner.buffer.prepare();
    }

    /// Push one task. On overflow return the newest half plus this task for
    /// injection; on a concurrent steal return just this task. Publish and
    /// notify the injection batch before allowing the worker to sleep.
    pub fn push(&mut self, task: T) -> Vec<T> {
        self.prepare();
        loop {
            let head = self.inner.head.load(Ordering::Acquire);
            let (steal, real) = unpack(head);
            let tail = self.inner.tail.load(Ordering::Relaxed);
            if tail.wrapping_sub(steal) < N as u32 {
                // SAFETY: sole producer; the reserved steal range is excluded.
                unsafe { self.inner.put(tail, task) };
                self.inner
                    .tail
                    .store(tail.wrapping_add(1), Ordering::Release);
                return Vec::new();
            }
            if steal != real {
                return vec![task];
            }
            // Claim all slots. Publish the retained oldest half at its new
            // logical positions (same physical slots) and inject the newest.
            if self
                .inner
                .head
                .compare_exchange_weak(head, pack(tail, tail), Ordering::Release, Ordering::Relaxed)
                .is_err()
            {
                continue;
            }
            let half = N as u32 / 2;
            self.inner
                .tail
                .store(tail.wrapping_add(half), Ordering::Release);
            let mut overflow = Vec::with_capacity(N / 2 + 1);
            for i in half..N as u32 {
                // SAFETY: the CAS claimed the whole buffer; these slots
                // were excluded from the retained half's published range.
                overflow.push(unsafe { self.inner.take(real.wrapping_add(i)) });
            }
            overflow.push(task);
            return overflow;
        }
    }

    /// Free slots for the owner's next pushes. Thieves only remove work, so
    /// a later push finds at least this many slots (Tokio `remaining_slots`).
    pub fn remaining(&self) -> usize {
        let (steal, _) = unpack(self.inner.head.load(Ordering::Acquire));
        let tail = self.inner.tail.load(Ordering::Relaxed);
        N - tail.wrapping_sub(steal) as usize
    }

    pub fn pop(&mut self) -> Option<T> {
        let mut head = self.inner.head.load(Ordering::Acquire);
        loop {
            let (steal, real) = unpack(head);
            let tail = self.inner.tail.load(Ordering::Relaxed);
            if real == tail {
                return None;
            }
            let next_real = real.wrapping_add(1);
            let next = if steal == real {
                pack(next_real, next_real)
            } else {
                pack(steal, next_real)
            };
            match self.inner.head.compare_exchange_weak(
                head,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // SAFETY: CAS transferred this slot to the sole owner;
                    // no push happens until this pop returns.
                    return Some(unsafe { self.inner.take(real) });
                }
                Err(actual) => head = actual,
            }
        }
    }

    /// Drain before retiring a worker, including a live shrink. A concurrent
    /// thief owns its batch; this drain cannot duplicate it.
    pub fn drain(&mut self) -> Vec<T> {
        let mut out = Vec::new();
        while let Some(task) = self.pop() {
            out.push(task);
        }
        out
    }
}

impl<T, const N: usize> Steal<T, N> {
    /// Claim a batch. Only one stealer per victim proceeds; a racing stealer
    /// returns empty instead of spinning on another worker's reservation.
    pub fn steal_half(&self) -> Vec<T> {
        let mut head = self.0.head.load(Ordering::Acquire);
        let (first, count, reserved) = loop {
            let (steal, real) = unpack(head);
            if steal != real {
                return Vec::new();
            }
            let tail = self.0.tail.load(Ordering::Acquire);
            let count = tail.wrapping_sub(real).div_ceil(2);
            if count == 0 {
                return Vec::new();
            }
            let next = pack(steal, real.wrapping_add(count));
            match self
                .0
                .head
                .compare_exchange_weak(head, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break (real, count, next),
                Err(actual) => head = actual,
            }
        };
        // Validate only after winning the CAS: an old head paired with a
        // newer tail may transiently suggest more than the ring's capacity.
        // Such a reservation always loses its CAS (Tokio queue.rs:514).
        assert!(count <= N as u32 / 2);
        let mut tasks = Vec::with_capacity(count as usize);
        for i in 0..count {
            // SAFETY: the reservation owns these slots and excludes reuse
            // until all the reads below are published by the final CAS.
            tasks.push(unsafe { self.0.take(first.wrapping_add(i)) });
        }
        let mut head = reserved;
        loop {
            let (_, real) = unpack(head);
            match self.0.head.compare_exchange_weak(
                head,
                pack(real, real),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return tasks,
                Err(actual) => head = actual,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unused_rings_remain_unbacked_and_preparation_is_idempotent() {
        crate::sync::model(|| {
            let (mut local, thief) = local::<u32, 4>();
            assert!(local.inner.buffer.get().is_none());
            assert!(thief.steal_half().is_empty());
            assert!(local.inner.buffer.get().is_none());
            local.prepare();
            let storage = local.inner.buffer.get().unwrap().as_ptr();
            local.prepare();
            assert_eq!(local.inner.buffer.get().unwrap().as_ptr(), storage);
            assert!(local.push(7).is_empty());
            assert_eq!(thief.steal_half(), [7]);
            assert!(local.drain().is_empty());
        });
    }

    #[test]
    fn logical_indices_wrap_without_losing_the_reserved_steal_range() {
        crate::sync::model(|| {
            let (mut local, thief) = local::<_, 4>();
            let start = u32::MAX - 1;
            local
                .inner
                .head
                .store(pack(start, start), Ordering::Relaxed);
            local.inner.tail.store(start, Ordering::Relaxed);
            for id in 0..4 {
                assert!(local.push(id).is_empty());
            }
            assert_eq!(thief.steal_half(), [0, 1]);
            assert!(local.push(4).is_empty());
            assert!(local.push(5).is_empty());
            assert_eq!(local.drain(), [2, 3, 4, 5]);
        });
    }
}
