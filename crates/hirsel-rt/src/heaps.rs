//! The run's heap registry (design §6 Heaps): every worker's heap, from its creation until the
//! run ends, so storage a retired worker's heap still lends (blocks other tasks hold, region
//! segments of regions still open) stays valid and returns to a heap that still exists.
//!
//! - A heap's id is its slot index plus one; ids are never reused within a run, and compiled
//!   code compares a slab's owner id with the running context's `heap_id` (runtime ABI 26).
//! - A retiring worker orphans its heap ([`HeapRegistry::retire`]). Any live worker's next
//!   scheduling point adopts each orphan for one collection ([`HeapRegistry::collect_orphans`])
//!   and orphans it again, so at most one worker collects a heap at a time
//!   (`crate::remote::Owner`), and a growing pool re-adopts an orphan for good before it
//!   creates a heap ([`HeapRegistry::attach`]).
//! - Heaps are freed only when the registry is dropped with its run.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use crate::alloc::Heap;
use crate::budget::Meter;
use crate::fault;

/// The most heaps one run creates: workers, the home thread and blocking-pool threads, each
/// once; a re-adopted orphan takes no new slot.
pub(crate) const MAX_HEAPS: usize = 1024;

/// What every heap of a run is made from.
pub(crate) struct Template {
    pub(crate) layouts: Box<[*const crate::Layout]>,
    pub(crate) diagnostic: bool,
    pub(crate) idle_cap: u64,
    pub(crate) dup_max: u32,
    pub(crate) meter: Arc<Meter>,
}

pub(crate) struct HeapRegistry {
    slots: Box<[AtomicPtr<Heap>]>,
    /// Heaps created; slots below it are filled.
    count: AtomicU32,
    /// Worker ids handed out.
    workers: AtomicU32,
    /// Heaps whose worker retired and that no worker took over for good.
    orphans: AtomicU32,
    template: Mutex<Template>,
}

// SAFETY: heaps are reached through their slots by their one collector (`Owner`), or through
// their published lists; the template is behind its lock, and its descriptor pointers name the
// linked program's immutable tables.
unsafe impl Send for HeapRegistry {}
// SAFETY: as for `Send`.
unsafe impl Sync for HeapRegistry {}

impl HeapRegistry {
    pub(crate) fn new(template: Template) -> Box<HeapRegistry> {
        Box::new(HeapRegistry {
            slots: (0..MAX_HEAPS).map(|_| AtomicPtr::default()).collect(),
            count: AtomicU32::new(0),
            workers: AtomicU32::new(0),
            orphans: AtomicU32::new(0),
            template: Mutex::new(template),
        })
    }

    /// The heap with id `id`.
    pub(crate) fn heap(&self, id: u32) -> *mut Heap {
        let heap = self
            .slots
            .get((id as usize).wrapping_sub(1))
            .map_or(std::ptr::null_mut(), |slot| slot.load(Ordering::Acquire));
        if heap.is_null() {
            fault("storage names a heap its run does not have");
        }
        heap
    }

    /// Every heap created so far.
    pub(crate) fn heaps(&self) -> impl Iterator<Item = *mut Heap> + '_ {
        let count = self.count.load(Ordering::Acquire) as usize;
        self.slots[..count]
            .iter()
            .map(|slot| slot.load(Ordering::Acquire))
    }

    /// A new worker's id.
    pub(crate) fn worker_id(&self) -> u32 {
        self.workers.fetch_add(1, Ordering::Relaxed)
    }

    /// The heap of a new worker `worker`: an orphan it re-adopts, or a fresh one.
    pub(crate) fn attach(&self, worker: u32) -> *mut Heap {
        if self.orphans.load(Ordering::Acquire) != 0 {
            for heap in self.heaps() {
                // SAFETY: registered heaps live until the registry drops.
                if unsafe { (*heap).owner.adopt(worker) } {
                    self.orphans.fetch_sub(1, Ordering::Relaxed);
                    #[cfg(feature = "measurement")]
                    unsafe {
                        (&mut (*heap).measurement).heap_adoptions += 1;
                    }
                    return heap;
                }
            }
        }
        let template = self
            .template
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = self.count.load(Ordering::Relaxed) + 1;
        if id as usize > MAX_HEAPS {
            crate::environmental("a run created more heaps than it can register");
        }
        let mut heap = Box::new(Heap::new(
            template.layouts.clone(),
            template.diagnostic,
            template.idle_cap,
        ));
        heap.join(id, worker, self, template.meter.clone());
        if template.dup_max != 0 {
            heap.set_dup_max(template.dup_max);
        }
        heap.park();
        let heap = Box::into_raw(heap);
        self.slots[id as usize - 1].store(heap, Ordering::Release);
        self.count.store(id, Ordering::Release);
        heap
    }

    /// A worker retires: its heap keeps lending its storage, collected by any live worker.
    ///
    /// # Safety
    /// `heap` is the retiring worker's own heap, and no task of it runs any more.
    pub(crate) unsafe fn retire(&self, heap: *mut Heap) {
        // SAFETY: caller contract.
        unsafe {
            // The task last bound to the heap was switched out, which folded its view's
            // counters into the heap, and may be gone (a completed task's stack is reused or
            // freed): the heap parks without reading it, and the collection counts into the
            // parked view.
            (*heap).forget_view();
            (*heap).park();
            (*heap).collect();
            (*heap).owner.orphan();
        }
        self.orphans.fetch_add(1, Ordering::Release);
    }

    /// A scheduling point of worker `worker`: collect each orphaned heap once, adopting it
    /// for the collection only.
    pub(crate) fn collect_orphans(&self, worker: u32) {
        if self.orphans.load(Ordering::Relaxed) == 0 {
            return;
        }
        for heap in self.heaps() {
            // SAFETY: registered heaps live until the registry drops; adoption makes this
            // worker the heap's one collector until it orphans the heap again.
            unsafe {
                if (*heap).owner.adopt(worker) {
                    (*heap).trim();
                    (*heap).owner.orphan();
                }
            }
        }
    }
}

impl Drop for HeapRegistry {
    fn drop(&mut self) {
        for heap in self.heaps().collect::<Vec<_>>() {
            // SAFETY: the run is over, so nothing else reaches its heaps; each is freed once.
            // A heap's last bound view may lie in a freed task stack, so it parks without
            // folding that view's counters.
            unsafe {
                let mut heap = Box::from_raw(heap);
                heap.forget_view();
                heap.regions.release_all();
                heap.release_all();
            }
        }
    }
}

// The region pool's lent-byte and active-region counts exist in the diagnostic and measurement
// archives only.
#[cfg(all(test, any(feature = "diagnostic", feature = "measurement")))]
mod tests {
    use crate::context::{
        Config, TaskCtx, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
        hirsel_rt_v2_worker_collect, hirsel_rt_v2_worker_create, hirsel_rt_v2_worker_retire,
    };
    use crate::region::{
        Region, hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit, hirsel_rt_v2_region_reserve,
    };
    use std::sync::mpsc;

    /// Raw pointers handed between the test's threads.
    struct Handoff(*mut TaskCtx, *mut Region);
    // SAFETY: each handed-over context and region is used by one thread at a time.
    unsafe impl Send for Handoff {}
    struct Ctx(*mut TaskCtx);
    unsafe impl Send for Ctx {}

    /// Enter a region on `ctx` and fill it past one segment.
    unsafe fn fill(ctx: *mut TaskCtx, region: *mut Region) {
        unsafe {
            hirsel_rt_v2_region_enter(ctx, region);
            for _ in 0..3 {
                let mut out = std::ptr::null_mut();
                assert_eq!(
                    hirsel_rt_v2_region_reserve(ctx, region, 40_000, 8, &mut out),
                    0
                );
                out.write_bytes(0x5a, 40_000);
            }
        }
    }

    /// Cross-worker region return (design §6 Heaps; regions cross tasks, `ownership.md:325`):
    /// tasks enter regions on worker A, migrate to worker B and exit them there, while A runs
    /// regions of its own, collects and trims. B returns every segment to A's pool, which
    /// takes them back at its scheduling points, so nothing stays lent, and the run's active
    /// region count (A's and B's shares) is zero.
    #[test]
    fn regions_exited_on_another_worker_return_their_segments() {
        const ROUNDS: usize = 200;
        let config = Config::new();
        let mut root = std::ptr::null_mut();
        // SAFETY: the test owns the run; each context is used by one thread at a time, and a
        // migrating context switches out on A before B attaches it.
        unsafe {
            assert_eq!(hirsel_rt_v2_ctx_create(&config, &mut root), 0);
            let other = hirsel_rt_v2_worker_create(root);
            let (to_b, from_a) = mpsc::channel::<Handoff>();
            let (root_a, other_b) = (Ctx(root), Ctx(other));
            let a = std::thread::spawn(move || {
                let root = root_a;
                let (worker, run) = ((*root.0).worker_ptr(), (*root.0).run_ptr());
                let mut own = Box::new(std::mem::MaybeUninit::<Region>::uninit());
                for _ in 0..ROUNDS {
                    let task = Box::into_raw(Box::new(TaskCtx::new(worker, run)));
                    TaskCtx::attach(task, worker);
                    let region = Box::into_raw(Box::new(std::mem::MaybeUninit::<Region>::uninit()))
                        .cast::<Region>();
                    fill(task, region);
                    (*task).switch_out();
                    TaskCtx::attach(root.0, worker);
                    to_b.send(Handoff(task, region)).unwrap();
                    // A region of A's own, entered and exited here.
                    fill(root.0, own.as_mut_ptr());
                    assert_eq!(hirsel_rt_v2_region_exit(root.0, own.as_mut_ptr(), 0, 0), 0);
                    hirsel_rt_v2_worker_collect(root.0);
                }
                drop(to_b);
            });
            let b = std::thread::spawn(move || {
                let other = other_b;
                let worker = (*other.0).worker_ptr();
                for Handoff(task, region) in from_a {
                    TaskCtx::attach(task, worker);
                    assert_eq!(hirsel_rt_v2_region_exit(task, region, 0, 0), 0);
                    assert!((*task).region.is_null());
                    (*task).switch_out();
                    drop(Box::from_raw(task));
                    drop(Box::from_raw(region));
                    TaskCtx::attach(other.0, worker);
                    hirsel_rt_v2_worker_collect(other.0);
                }
            });
            a.join().unwrap();
            b.join().unwrap();
            hirsel_rt_v2_worker_collect(root);
            let (pool_a, pool_b) = (&(*root).heap_ref().regions, &(*other).heap_ref().regions);
            assert_eq!(pool_a.active_bytes(), 0, "a segment stayed lent");
            assert_eq!(pool_b.active_bytes(), 0);
            assert_eq!(pool_a.active_count.wrapping_add(pool_b.active_count), 0);
            assert!(pool_a.idle_bytes > 0, "A's pool took its segments back");
            assert_eq!(pool_b.idle_bytes, 0, "B pooled a segment it never lent");
            hirsel_rt_v2_worker_retire(other);
            hirsel_rt_v2_ctx_destroy(root);
        }
    }

    #[test]
    fn exit_audit_collects_orphan_returns_and_preserves_a_real_leak() {
        use crate::context::{Audit, CONFIG_DIAGNOSTIC, hirsel_rt_v2_task_exit};
        for leak in [false, true] {
            let mut config = Config::new();
            config.flags = CONFIG_DIAGNOSTIC;
            let mut root = std::ptr::null_mut();
            // SAFETY: the test owns both workers sequentially. The orphan's
            // returned blocks are freed once by root, before task exit.
            unsafe {
                assert_eq!(hirsel_rt_v2_ctx_create(&config, &mut root), 0);
                let other = hirsel_rt_v2_worker_create(root);
                let (small, _) = (*other).heap().allocate(0, 32, 8);
                let (large, _) = (*other).heap().allocate(0, 5000, 8);
                let bytes = if leak {
                    (*other).heap().allocate(0, 48, 8).1
                } else {
                    0
                };
                hirsel_rt_v2_worker_retire(other);
                (*root).heap().free(small);
                (*root).heap().free(large);
                let mut audit = Audit::default();
                hirsel_rt_v2_task_exit(root, &mut audit);
                assert_eq!(audit.live_blocks, u64::from(leak));
                assert_eq!(audit.live_bytes, bytes);
                assert_eq!(audit.retained_bytes, 0);
                hirsel_rt_v2_ctx_destroy(root);
            }
        }
    }
}
