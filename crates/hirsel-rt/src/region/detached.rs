//! Storage custody after an arena transaction has discharged its outgoing
//! credits. Segment membership belongs to the run's stable allocator homes;
//! this owner contains no pointer to the source region record or task frame.
use super::{Region, SEGMENT_HEADER, Segment, check_active};
use crate::{TaskCtx, fault};
use std::ptr;

/// Affine ownership of detached region segments, within their original run.
///
/// This is the storage part of publication, not a language `Region[T]` and
/// not a graph evacuation operation. Its caller must release it exactly once
/// before the run ends. Whole-run environmental teardown can reclaim it.
#[must_use = "detached storage must be released by its affine owner"]
pub struct DetachedStorage {
    head: *mut Segment,
}

// SAFETY: storage custody moves affinely. Each segment's membership is protected
// by its separately allocated, run-owned LentList; its payload has one owner.
unsafe impl Send for DetachedStorage {}

impl DetachedStorage {
    /// Rebind diagnostic header ownership without retaining the source frame.
    /// A null scope identifies detached storage; release identifies its headers
    /// by segment interval. The map already contains every arena header, so
    /// publication only updates existing entries and never reserves a vector.
    pub(crate) unsafe fn transfer_diagnostic(&self, ctx: &TaskCtx, region: *mut Region) {
        if ctx.heap_ref().diagnostic {
            let mut owners = ctx
                .run_ref()
                .region_owners
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for owner in owners
                .values_mut()
                .filter(|owner| owner.active && owner.region == region)
            {
                owner.region = ptr::null_mut();
            }
        }
    }

    /// Return this owner's segments to their allocator homes, without allocating.
    /// Remote homes receive them through the existing return queues.
    ///
    /// # Safety
    /// `ctx` is used exclusively on its worker and belongs to this storage's
    /// still-live run. No view or other owner reaches any of these segments;
    /// all outgoing credits have already been discharged by the typed owner.
    pub unsafe fn release(self, ctx: *mut TaskCtx) {
        unsafe {
            let ctx = &mut *ctx;
            let mut segment = self.head;
            while !segment.is_null() {
                let next = (*segment).prev;
                if ctx.heap_ref().diagnostic {
                    let start = segment.addr() + SEGMENT_HEADER;
                    let end = segment.addr() + (*segment).size as usize;
                    let mut owners = ctx
                        .run_ref()
                        .region_owners
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for (&address, owner) in owners.iter_mut() {
                        if owner.region.is_null() && address >= start && address < end {
                            owner.active = false;
                        }
                    }
                    drop(owners);
                    let data = segment.cast::<u8>().add(SEGMENT_HEADER);
                    #[cfg(feature = "memcheck")]
                    crate::memcheck::undefined(data, (*segment).size as usize - SEGMENT_HEADER);
                    ptr::write_bytes(
                        data,
                        crate::alloc::FREE_POISON,
                        (*segment).size as usize - SEGMENT_HEADER,
                    );
                }
                ctx.return_segment(segment);
                segment = next;
            }
        }
    }
}

/// Commit a region's storage to a detached owner, disarming only its storage
/// cleanup. The source guard retains its parent chain and budget boundary and
/// must still exit normally. This step makes no allocation or language call.
///
/// # Safety
/// `region` is live, exclusively owned by an entered transaction, and belongs
/// to a run that outlives the returned owner. No external view reaches it.
/// Graph closure/evacuation and all fallible work have finished; its outgoing
/// cleanup list must already be empty. Caller publishes only after this step.
pub unsafe fn detach_storage(region: *mut Region) -> DetachedStorage {
    unsafe {
        check_active(region);
        if !(*region).cleanup.is_null() {
            fault("region storage detached with outgoing cleanup still pending");
        }
        let head = (*region).segment.cast::<Segment>();
        let mut segment = head;
        while !segment.is_null() {
            (&*(*segment).home).seal(segment);
            segment = (*segment).prev;
        }
        (*region).segment = ptr::null_mut();
        (*region).cursor = ptr::null_mut();
        (*region).end = ptr::null_mut();
        DetachedStorage { head }
    }
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests {
    use super::super::{
        hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit, hirsel_rt_v2_region_reserve,
    };
    use super::*;
    use crate::context::{
        Config, Stats, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy, hirsel_rt_v2_ctx_stats,
        hirsel_rt_v2_worker_collect, hirsel_rt_v2_worker_create, hirsel_rt_v2_worker_retire,
    };
    use std::mem::MaybeUninit;

    unsafe fn context() -> *mut TaskCtx {
        let config = Config {
            flags: crate::context::CONFIG_DIAGNOSTIC,
            ..Config::new()
        };
        let mut ctx = ptr::null_mut();
        assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
        ctx
    }
    unsafe fn stats(ctx: *mut TaskCtx) -> Stats {
        let mut out = Stats::default();
        unsafe { hirsel_rt_v2_ctx_stats(ctx, &mut out) };
        out
    }
    unsafe fn enter(ctx: *mut TaskCtx) -> Box<Region> {
        let mut record = Box::new(MaybeUninit::<Region>::uninit());
        unsafe {
            hirsel_rt_v2_region_enter(ctx, record.as_mut_ptr());
            Box::from_raw(Box::into_raw(record).cast())
        }
    }
    unsafe fn reserve(ctx: *mut TaskCtx, region: &mut Region) -> *mut u8 {
        let mut out = ptr::null_mut();
        assert_eq!(
            unsafe { hirsel_rt_v2_region_reserve(ctx, region, 32, 8, &mut out) },
            0
        );
        out
    }

    #[test]
    fn empty_storage_disarms_storage_but_still_closes_its_guard() {
        unsafe {
            let ctx = context();
            let mut region = enter(ctx);
            let owner = detach_storage(&mut *region);
            assert_eq!(stats(ctx).active_regions, 1);
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *region, 0, 0), 0);
            assert_eq!(stats(ctx).active_regions, 0);
            owner.release(ctx);
            hirsel_rt_v2_ctx_destroy(ctx);
        }
    }

    #[test]
    fn outer_storage_survives_inner_and_source_guard_exit() {
        unsafe {
            let ctx = context();
            let mut outer = enter(ctx);
            let data = reserve(ctx, &mut outer);
            data.cast::<u64>().write(71);
            let mut inner = enter(ctx);
            let inner_data = reserve(ctx, &mut inner);
            inner_data.cast::<u64>().write(29);
            let owner = detach_storage(&mut *outer);
            let held = stats(ctx);
            assert_eq!(held.active_regions, 2);
            assert_eq!(held.region_active_bytes, super::super::SEGMENT_SIZE as u64);
            assert!(held.backing_bytes >= 2 * super::super::SEGMENT_SIZE as u64);
            assert_eq!(inner_data.cast::<u64>().read(), 29);
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *inner, 0, 0), 0);
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *outer, 0, 0), 0);
            drop(inner);
            drop(outer);
            assert_eq!(data.cast::<u64>().read(), 71);
            assert_eq!(stats(ctx).region_active_bytes, 0);
            owner.release(ctx);
            assert_eq!((*ctx).heap_ref().regions.owned_bytes(), 0);
            hirsel_rt_v2_ctx_destroy(ctx);
        }
    }

    #[test]
    fn receiver_reads_after_source_record_and_worker_are_gone_then_returns_once() {
        struct Handoff {
            storage: DetachedStorage,
            data: *mut u8,
            receiver: *mut TaskCtx,
        }
        // SAFETY: all three fields move to the receiver's sole thread.
        unsafe impl Send for Handoff {}
        impl Handoff {
            unsafe fn run(self) {
                unsafe {
                    assert_eq!(self.data.cast::<u64>().read(), 83);
                    self.storage.release(self.receiver);
                    hirsel_rt_v2_worker_collect(self.receiver);
                    hirsel_rt_v2_worker_retire(self.receiver);
                }
            }
        }
        unsafe {
            let root = context();
            let source = hirsel_rt_v2_worker_create(root);
            let receiver = hirsel_rt_v2_worker_create(root);
            let mut record = enter(source);
            let data = reserve(source, &mut record);
            data.cast::<u64>().write(83);
            let storage = detach_storage(&mut *record);
            assert_eq!(hirsel_rt_v2_region_exit(source, &mut *record, 0, 0), 0);
            drop(record);
            hirsel_rt_v2_worker_retire(source);
            let handoff = Handoff {
                storage,
                data,
                receiver,
            };
            std::thread::spawn(move || handoff.run()).join().unwrap();
            hirsel_rt_v2_worker_collect(root);
            assert!(
                (*root)
                    .run_ref()
                    .heaps
                    .heaps()
                    .all(|heap| (*heap).regions.owned_bytes() == 0)
            );
            hirsel_rt_v2_ctx_destroy(root);
        }
    }
}
