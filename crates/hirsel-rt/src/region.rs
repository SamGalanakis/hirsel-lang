//! Compiler-local regions (MEM-10, OWN-7).
//!
//! Region storage is uncounted: values placed in a region carry no header. A region bump
//! allocates from segments taken from the task's pool. Its cleanup list, kept in its own storage,
//! records the outgoing heap credits it holds and the inline resources it must finalize; exit
//! runs them newest first, finishing every one even after a failure, then returns the segments
//! to the pool. At each exit the pool keeps at most [`REGION_POOL_IDLE_CAP`] idle bytes (active
//! storage is not capped) and the heap trims its idle slabs; task exit returns everything.
//!
//! An unpublished [`Builder`] reserves capacity and may grow by relocating its contents (or in
//! place when it is the region's newest reservation). Once published it never moves or grows;
//! further growth builds a replacement.

use std::alloc::{Layout as HostLayout, alloc as host_alloc, dealloc as host_dealloc};
use std::ptr;

use crate::alloc::{FREE_POISON, Header, check_request};
use crate::context::RunCtx;
use crate::finalize::{Cleanup, finalize_item};
#[cfg(feature = "memcheck")]
use crate::memcheck;
use crate::memcheck::REGION_REDZONE;
use crate::release::{LAYOUT_NO_EXTENT, drain, push, release_block};
use crate::{STATUS_OK, environmental, fault};

/// Bytes of a standard region segment.
pub const SEGMENT_SIZE: usize = 64 * 1024;
/// MEM-10: idle pooled backing retained per task at each region exit.
pub const REGION_POOL_IDLE_CAP: u64 = 8 * 1024 * 1024;

const SEGMENT_HEADER: usize = 32;
const REGION_ACTIVE: u64 = 0x5245_4741_4354_4956; // "REGACTIV"
const REGION_EXITED: u64 = 0x5245_4745_5849_5444; // "REGEXITD"

/// Segment bookkeeping. `prev` chains a region's segments (or the idle pool); `lent_prev` and
/// `lent_next` link every segment lent to a region, so task exit can return them without
/// reading region records whose frames may be gone.
#[repr(C)]
pub(crate) struct Segment {
    prev: *mut Segment,
    size: u64,
    lent_prev: *mut Segment,
    lent_next: *mut Segment,
}

/// A region record in compiler-reserved storage, initialized by `hirsel_rt_v2_region_enter`.
/// Compiled arena bumps read/write `cursor` at byte 8 and read `end` at byte 16.
/// `end` is null in diagnostic mode, forcing allocation through the runtime.
/// All other fields are private to the runtime.
#[repr(C)]
#[derive(Debug)]
pub struct Region {
    pub segment: *mut u8,
    pub cursor: *mut u8,
    /// Inline allocation limit; null before the first segment and in diagnostic mode.
    pub end: *mut u8,
    pub cleanup: *mut u8,
    pub parent: *mut Region,
    pub state: u64,
    /// A bounded arena's logical bytes left (MEM-6), which compiled code tests and lowers
    /// before each arena construction; `u64::MAX` when unbounded.
    pub remaining: u64,
    /// 0 unbounded, else 1 + the failure count when the bound was set.
    pub bound: u32,
    /// 0, else 1 + the index of the arena's own `OutOfSpace` record.
    pub exhausted: u32,
}

/// An unpublished builder: `capacity` reserved bytes at `data` in `region`.
#[repr(C)]
#[derive(Debug)]
pub struct Builder {
    pub data: *mut u8,
    pub capacity: u64,
    pub align: u64,
    pub region: *mut Region,
    pub published: u64,
}

/// Diagnostic-only owner record; production allocates no such records.
pub(crate) struct OwnerRecord {
    pub region: *mut Region,
    pub active: bool,
}

/// Install an immortal flat owner in storage already reserved from this region.
/// The immutable extent precedes its eight-byte header.
/// # Safety
/// `block - 16 .. block + 8 + capacity` is reserved in the named active region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_region_owner(
    ctx: *mut RunCtx,
    region: *mut Region,
    block: *mut Header,
    index: u32,
    capacity: u64,
    align: u64,
) {
    unsafe {
        check_request(capacity, align);
        let ctx = &mut *ctx;
        check_active(region);
        let layout = &*ctx.descriptor(index);
        if layout.finalizer.is_some() || layout.step_count != 0 || layout.size > capacity {
            fault("a regional immortal owner must have a flat empty drop program");
        }
        let physical = align.max(8);
        if (block as usize + 8) % physical as usize != 0 {
            fault("a regional immortal payload has the wrong alignment");
        }
        if ctx.heap.diagnostic {
            let mut active = ctx.regions.active;
            while !active.is_null() && active != region {
                active = (*active).parent;
            }
            if active.is_null() {
                fault("regional owner belongs to another context");
            }
            let mut segment = (*region).segment.cast::<Segment>();
            let mut contained = false;
            while !segment.is_null() {
                let start = segment.addr() + SEGMENT_HEADER;
                let end = segment.addr() + (*segment).size as usize;
                if block.addr() >= start + 16
                    && block
                        .addr()
                        .checked_add(8)
                        .and_then(|n| n.checked_add(capacity as usize))
                        .is_some_and(|n| n <= end)
                {
                    contained = true;
                    break;
                }
                segment = (*segment).prev;
            }
            if !contained {
                fault("regional owner lies outside its region storage");
            }
            ctx.heap.forget_foreign_record(block.addr());
            ctx.region_owners
                .get_or_insert_with(Default::default)
                .insert(
                    block.addr(),
                    OwnerRecord {
                        region,
                        active: true,
                    },
                );
        }
        block
            .cast::<u8>()
            .sub(16)
            .cast::<crate::alloc::Extent>()
            .write(crate::alloc::Extent {
                capacity,
                align: physical,
            });
        block.write(Header {
            count: crate::COUNT_IMMORTAL,
            descriptor: index | (crate::alloc::KIND_REGION << crate::alloc::KIND_SHIFT),
        });
    }
}

/// Allocate an arena block of `extent` payload bytes aligned to `align` in the arena whose
/// record is `region`: an immortal region-kind header of layout
/// `index`, then the payload, which the caller fills. The immutable extent record precedes
/// the header unless the layout is [`LAYOUT_NO_EXTENT`], whose extent is its size.
/// It has no count, so its drop program never runs from a release; the caller schedules the
/// payload with `hirsel_rt_v2_region_schedule` when it moves a heap credit in. Returns the
/// block. It cannot fail: backing exhaustion terminates environmentally (runtime ABI 3).
///
/// # Safety
/// `ctx` is live and `region` an active arena record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arena_alloc(
    ctx: *mut RunCtx,
    region: *mut Region,
    index: u32,
    extent: u64,
    align: u64,
) -> *mut Header {
    unsafe {
        check_request(extent, align);
        let ctx = &mut *ctx;
        check_active(region);
        let layout = &*ctx.descriptor(index);
        let bare = layout.flags & LAYOUT_NO_EXTENT != 0;
        if bare && (extent != layout.size || align != layout.align) {
            fault("a fixed-layout arena block is not its layout's size and alignment");
        }
        if ctx.heap.diagnostic && layout.size > extent {
            fault("an arena block extent is smaller than its layout size");
        }
        let physical = align.max(8);
        // The payload follows the header (after the 16-byte extent unless bare), at `physical`
        // alignment.
        let prefix = if bare { 8 } else { 24 };
        let start = reserve_in(ctx, region, prefix + extent + (physical - 8), 8, true);
        let payload = start.add(prefix as usize);
        let payload = payload.add(payload.align_offset(physical as usize));
        let block = payload.sub(8).cast::<Header>();
        if ctx.heap.diagnostic {
            ctx.heap.forget_foreign_record(block.addr());
            ctx.region_owners
                .get_or_insert_with(Default::default)
                .insert(
                    block.addr(),
                    OwnerRecord {
                        region,
                        active: true,
                    },
                );
            ptr::write_bytes(payload, crate::alloc::UNINIT_POISON, extent as usize);
        }
        if !bare {
            block
                .cast::<u8>()
                .sub(16)
                .cast::<crate::alloc::Extent>()
                .write(crate::alloc::Extent {
                    capacity: extent,
                    align: physical,
                });
        }
        block.write(Header {
            count: crate::COUNT_IMMORTAL,
            descriptor: index | (crate::alloc::KIND_REGION << crate::alloc::KIND_SHIFT),
        });
        block
    }
}

/// A cleanup entry in region storage: a held heap credit (`block`) or an inline resource to
/// finalize (`cleanup`).
#[repr(C)]
struct Entry {
    prev: *mut Entry,
    block: *mut Header,
    cleanup: Cleanup,
}

/// The task's region segment pool.
pub(crate) struct RegionPool {
    idle: *mut Segment,
    lent: *mut Segment,
    pub(crate) idle_bytes: u64,
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) active_bytes: u64,
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) active_count: u64,
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) backing_allocations: u64,
    /// The innermost entered region of the running task (the scheduler swaps it per task).
    pub(crate) active: *mut Region,
}

fn segment_layout(size: u64) -> HostLayout {
    // SAFETY: sizes are validated multiples of 16 below isize::MAX.
    unsafe { HostLayout::from_size_align_unchecked(size as usize, 16) }
}

impl RegionPool {
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    #[inline(never)]
    fn record_backing_allocation(&mut self) {
        self.backing_allocations += 1;
    }
    pub(crate) fn new() -> RegionPool {
        RegionPool {
            idle: ptr::null_mut(),
            lent: ptr::null_mut(),
            idle_bytes: 0,
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            active_bytes: 0,
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            active_count: 0,
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            backing_allocations: 0,
            active: ptr::null_mut(),
        }
    }

    /// A segment with at least `need` bytes including its header, lent to a region. A standard
    /// request takes a pooled standard segment and an oversized one the first pooled segment
    /// that fits, so small regions never pin large segments; otherwise it allocates.
    fn take(&mut self, need: u64) -> *mut Segment {
        let mut found = self.unlink_idle(need);
        if found.is_null() {
            let size = need.max(SEGMENT_SIZE as u64).next_multiple_of(16);
            // SAFETY: nonzero size.
            found = unsafe { host_alloc(segment_layout(size)) }.cast::<Segment>();
            if found.is_null() {
                environmental("the backing allocator could not supply a region segment");
            }
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            {
                self.record_backing_allocation();
            }
            // SAFETY: fresh storage.
            unsafe { (*found).size = size };
        }
        // SAFETY: `found` is this pool's segment.
        unsafe { self.lend(found) };
        found
    }

    /// An idle standard segment lent to a region, or null when the pool has none: it never
    /// allocates.
    fn take_idle(&mut self) -> *mut Segment {
        let found = self.unlink_idle(SEGMENT_SIZE as u64);
        if !found.is_null() {
            // SAFETY: `found` is this pool's segment.
            unsafe { self.lend(found) };
        }
        found
    }

    /// Unlink and return the pooled segment a request of `need` bytes takes, or null.
    fn unlink_idle(&mut self, need: u64) -> *mut Segment {
        let standard = need <= SEGMENT_SIZE as u64;
        let mut found = ptr::null_mut();
        let mut link: *mut *mut Segment = &mut self.idle;
        // SAFETY: the idle list holds this pool's segments.
        unsafe {
            while !(*link).is_null() {
                let segment = *link;
                let size = (*segment).size;
                if if standard {
                    size == SEGMENT_SIZE as u64
                } else {
                    size >= need
                } {
                    *link = (*segment).prev;
                    self.idle_bytes -= size;
                    found = segment;
                    break;
                }
                link = &mut (*segment).prev;
            }
        }
        found
    }

    /// Put `found` on the lent list.
    ///
    /// # Safety
    /// `found` is this pool's segment and on neither list.
    unsafe fn lend(&mut self, found: *mut Segment) {
        // SAFETY: caller contract.
        unsafe {
            (*found).prev = ptr::null_mut();
            (*found).lent_prev = ptr::null_mut();
            (*found).lent_next = self.lent;
            if !self.lent.is_null() {
                (*self.lent).lent_prev = found;
            }
            self.lent = found;
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            {
                self.active_bytes += (*found).size;
            }
            // A lent segment is a pool of its reservations; the rest of its data is
            // unaddressable.
            #[cfg(feature = "memcheck")]
            {
                let data = found.cast::<u8>().add(SEGMENT_HEADER);
                memcheck::noaccess(data, (*found).size as usize - SEGMENT_HEADER);
                memcheck::create_pool(found.cast());
            }
        }
    }

    /// Take a lent segment back into the idle pool.
    unsafe fn give_back(&mut self, segment: *mut Segment) {
        // SAFETY: `segment` is lent by this pool.
        unsafe {
            if (*segment).lent_prev.is_null() {
                self.lent = (*segment).lent_next;
            } else {
                (*(*segment).lent_prev).lent_next = (*segment).lent_next;
            }
            if !(*segment).lent_next.is_null() {
                (*(*segment).lent_next).lent_prev = (*segment).lent_prev;
            }
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            {
                self.active_bytes -= (*segment).size;
            }
            self.idle_bytes += (*segment).size;
            (*segment).prev = self.idle;
            self.idle = segment;
            #[cfg(feature = "memcheck")]
            {
                memcheck::destroy_pool(segment.cast());
                let data = segment.cast::<u8>().add(SEGMENT_HEADER);
                memcheck::noaccess(data, (*segment).size as usize - SEGMENT_HEADER);
            }
        }
    }

    /// Return idle segments above `cap` to the backing allocator.
    pub(crate) fn trim(&mut self, cap: u64) {
        while self.idle_bytes > cap {
            let segment = self.idle;
            // SAFETY: idle_bytes > 0 implies a pooled segment.
            unsafe {
                self.idle = (*segment).prev;
                self.idle_bytes -= (*segment).size;
                host_dealloc(segment.cast(), segment_layout((*segment).size));
            }
        }
    }

    /// Task exit: free pooled segments and those lent to regions still active. Region records
    /// are not read; they live in frames the task no longer has.
    pub(crate) fn release_all(&mut self) {
        self.trim(0);
        let mut segment = self.lent;
        while !segment.is_null() {
            // SAFETY: each lent segment is freed once.
            unsafe {
                let next = (*segment).lent_next;
                #[cfg(any(feature = "diagnostic", feature = "measurement"))]
                {
                    self.active_bytes -= (*segment).size;
                }
                #[cfg(feature = "memcheck")]
                memcheck::destroy_pool(segment.cast());
                host_dealloc(segment.cast(), segment_layout((*segment).size));
                segment = next;
            }
        }
        self.lent = ptr::null_mut();
        self.active = ptr::null_mut();
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.active_count = 0;
        }
    }
}

unsafe fn check_active(region: *mut Region) {
    // SAFETY: caller passes a region record.
    if region.is_null() || unsafe { (*region).state } != REGION_ACTIVE {
        fault("a region operation on a region that is not active");
    }
}

/// The runtime's actual bound, including when diagnostics disable compiled bumps.
unsafe fn segment_end(region: &Region) -> *mut u8 {
    let segment = region.segment.cast::<Segment>();
    if segment.is_null() {
        ptr::null_mut()
    } else {
        unsafe { segment.cast::<u8>().wrapping_add((*segment).size as usize) }
    }
}

/// The cursor after `bytes` reserved at `start`: every reservation ends on a word, so the
/// cursor stays eight-byte aligned and compiled arena bumps need no alignment step. The
/// memcheck archive leaves an unaddressable redzone after each reservation, so the cursor may
/// pass the segment's end; it is only compared, never dereferenced, there.
fn advance(start: *mut u8, bytes: u64) -> *mut u8 {
    start.wrapping_add((bytes.next_multiple_of(8) + REGION_REDZONE) as usize)
}

/// The lent segment of `region` holding the reservation at `data`.
#[cfg(feature = "memcheck")]
unsafe fn segment_of(region: &Region, data: *mut u8) -> *mut Segment {
    let mut segment = region.segment.cast::<Segment>();
    // SAFETY: a region's segments chain through `prev` to null.
    unsafe {
        while !segment.is_null()
            && !(segment.addr() < data.addr()
                && data.addr() <= segment.addr() + (*segment).size as usize)
        {
            segment = (*segment).prev;
        }
    }
    if segment.is_null() {
        fault("a reservation lies outside its region's segments");
    }
    segment
}

/// Bump-allocate `extent` bytes aligned to `align` in `region`.
unsafe fn reserve(ctx: &mut RunCtx, region: *mut Region, extent: u64, align: u64) -> *mut u8 {
    unsafe { reserve_in(ctx, region, extent, align, false) }
}

/// Bump-allocate as `reserve` does. With `grow`, a full region's next segment
/// grows geometrically, as Zig's `std.heap.ArenaAllocator` sizes a new node:
/// one and a half times the previous segment plus the request (and 16), so
/// an arena of `n` bytes takes O(log n) backing allocations
/// (third-party/NOTICES.md). Compiler-local regions keep standard segments.
unsafe fn reserve_in(
    ctx: &mut RunCtx,
    region: *mut Region,
    extent: u64,
    align: u64,
    grow: bool,
) -> *mut u8 {
    check_request(extent, align);
    // SAFETY: caller checked the region is active.
    unsafe {
        let region = &mut *region;
        let end = segment_end(region);
        if !region.cursor.is_null() {
            let start = region
                .cursor
                .wrapping_add(region.cursor.align_offset(align as usize));
            if (start as usize) <= end as usize && (end as usize - start as usize) as u64 >= extent
            {
                region.cursor = advance(start, extent);
                #[cfg(feature = "memcheck")]
                memcheck::pool_alloc(region.segment, start, extent as usize);
                return start;
            }
        }
        let need = SEGMENT_HEADER as u64 + extent.next_multiple_of(8) + align - 1;
        let previous = region.segment.cast::<Segment>();
        let need = if grow && !previous.is_null() {
            let big_enough = (*previous).size + need + 16;
            need.max(big_enough + big_enough / 2)
        } else {
            need
        };
        let segment = ctx.regions.take(need);
        let cursor = install(ctx, region, segment);
        let start = cursor.wrapping_add(cursor.align_offset(align as usize));
        region.cursor = advance(start, extent);
        #[cfg(feature = "memcheck")]
        memcheck::pool_alloc(region.segment, start, extent as usize);
        start
    }
}

/// Make the lent `segment` `region`'s newest: its limit is the segment's end (null in
/// diagnostic mode and the memcheck archive, which keep every allocation in the runtime).
/// Returns the segment's first data byte, which is word aligned.
unsafe fn install(ctx: &mut RunCtx, region: &mut Region, segment: *mut Segment) -> *mut u8 {
    #[cfg(feature = "measurement")]
    {
        ctx.heap.measurement.region_high_water_bytes = ctx
            .heap
            .measurement
            .region_high_water_bytes
            .max(ctx.regions.active_bytes);
    }
    // SAFETY: caller contract: `segment` is lent to `region`.
    unsafe {
        (*segment).prev = region.segment.cast();
        region.segment = segment.cast();
        let base = segment.cast::<u8>();
        region.end = if ctx.heap.diagnostic || cfg!(feature = "memcheck") {
            ptr::null_mut()
        } else {
            base.wrapping_add((*segment).size as usize)
        };
        base.wrapping_add(SEGMENT_HEADER)
    }
}

/// T18 RegionOpen. Enter a region in compiler-reserved storage. Regions nest; exits are innermost
/// first.
///
/// # Safety
/// `ctx` is live and `region` writable storage that outlives the region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_region_enter(ctx: *mut RunCtx, region: *mut Region) {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        region.write(Region {
            segment: ptr::null_mut(),
            cursor: ptr::null_mut(),
            end: ptr::null_mut(),
            cleanup: ptr::null_mut(),
            parent: ctx.regions.active,
            state: REGION_ACTIVE,
            remaining: u64::MAX,
            bound: 0,
            exhausted: 0,
        });
        ctx.regions.active = region;
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            ctx.regions.active_count += 1;
        }
    }
}

/// Enter an arena's region as `hirsel_rt_v2_region_enter` does,
/// then seed its cursor and limit from an idle standard segment of the pool, when it has one,
/// so the arena's first compiled bump takes the inline path instead of the runtime's. This is
/// the pooled counterpart of a C arena that keeps one chunk and resets its cursor. The segment
/// is the one the first `hirsel_rt_v2_arena_alloc` would take, so growth is unchanged.
///
/// # Safety
/// `ctx` is live and `region` writable storage that outlives the region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arena_enter(ctx: *mut RunCtx, region: *mut Region) {
    // SAFETY: caller contract.
    unsafe {
        hirsel_rt_v2_region_enter(ctx, region);
        let ctx = &mut *ctx;
        let segment = ctx.regions.take_idle();
        if !segment.is_null() {
            let region = &mut *region;
            region.cursor = install(ctx, region, segment);
        }
    }
}

/// Bound the entered arena `region` to `capacity` logical bytes (MEM-6), counted from the
/// failures recorded now.
///
/// # Safety
/// `ctx` is live and `region` an entered arena record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arena_bound(
    ctx: *mut RunCtx,
    region: *mut Region,
    capacity: u64,
) {
    // SAFETY: caller contract.
    unsafe {
        check_active(region);
        let count = (*ctx).failure.count();
        let region = &mut *region;
        region.remaining = capacity;
        region.bound = failure_index(count) + 1;
        region.exhausted = 0;
    }
}

/// Record the bounded arena `region`'s own `OutOfSpace` (`kind` and `rule` at `site`) and
/// mark it as the arena's. Returns [`STATUS_FAILURE`](crate::STATUS_FAILURE).
///
/// # Safety
/// `ctx` is live and `region` an entered arena record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arena_exhausted(
    ctx: *mut RunCtx,
    region: *mut Region,
    kind: u32,
    rule: u32,
    site: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        check_active(region);
        let index = (*ctx).failure.count();
        (*region).exhausted = failure_index(index) + 1;
        crate::failure::hirsel_rt_v2_failure_begin(ctx, kind, rule, site)
    }
}

/// Forget the active failure and return 1 when it is exactly the bounded arena `region`'s own
/// `OutOfSpace`: the one record since its bound was set. Otherwise return 0 and keep it.
///
/// # Safety
/// `ctx` is live and `region` an entered arena record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arena_recover(ctx: *mut RunCtx, region: *mut Region) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        check_active(region);
        let region = &mut *region;
        let failures = &mut (*ctx).failure;
        if region.bound == 0 || region.exhausted != region.bound {
            return 0;
        }
        let start = (region.bound - 1) as usize;
        if failures.count() != start + 1 {
            return 0;
        }
        failures.truncate(start);
        region.exhausted = 0;
        1
    }
}

fn failure_index(count: usize) -> u32 {
    u32::try_from(count)
        .ok()
        .filter(|&n| n < u32::MAX - 1)
        .unwrap_or_else(|| fault("a failure count overflows a bounded arena's record"))
}

/// T19 RegionAlloc. Reserve `extent` uncounted bytes aligned to `align` in `region` and write their
/// address.
/// Backing exhaustion terminates environmentally.
///
/// # Safety
/// `ctx` is live, `region` active and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_region_reserve(
    ctx: *mut RunCtx,
    region: *mut Region,
    extent: u64,
    align: u64,
    out: *mut *mut u8,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        check_active(region);
        out.write(reserve(&mut *ctx, region, extent, align));
    }
    STATUS_OK
}

unsafe fn record(ctx: &mut RunCtx, region: *mut Region, block: *mut Header) -> *mut Entry {
    // SAFETY: caller checked the region is active.
    unsafe {
        let entry = reserve(
            ctx,
            region,
            size_of::<Entry>() as u64,
            align_of::<Entry>() as u64,
        )
        .cast::<Entry>();
        (*entry).prev = (*region).cleanup.cast();
        (*entry).block = block;
        (*region).cleanup = entry.cast();
        entry
    }
}

/// T20 RegionEdgeAdd. Transfer one heap credit of `block` to `region`, which releases it at exit.
///
/// # Safety
/// `ctx` is live, `region` active, and the caller transfers one credit of the live `block`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_region_hold(
    ctx: *mut RunCtx,
    region: *mut Region,
    block: *mut Header,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        check_active(region);
        let ctx = &mut *ctx;
        if ctx.heap.diagnostic {
            crate::count::check_live_block(ctx, block);
        }
        record(ctx, region, block);
    }
    STATUS_OK
}

/// Schedule the inline resource `value` of `layout`, stored in `region`, for finalization at
/// region exit.
///
/// # Safety
/// `ctx` is live, `region` active, and `value` a fully initialized value of `layout` in
/// `region` whose finalization obligation transfers to the region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_region_schedule(
    ctx: *mut RunCtx,
    region: *mut Region,
    index: u32,
    value: *mut u8,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        check_active(region);
        let ctx = &mut *ctx;
        let _ = ctx.descriptor(index);
        let entry = record(ctx, region, ptr::null_mut());
        finalize_item(&mut (*entry).cleanup, index, value);
    }
    STATUS_OK
}

/// T23 RegionClose. Run the region's cleanup newest first (finalize scheduled resources, release
/// held heap credits), finishing all of it even after a failure; then reset its storage, return
/// its segments to the pool and trim the pool and heap. Returns the accumulated status.
///
/// # Safety
/// `ctx` is live and `region` is the innermost active region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_region_exit(
    ctx: *mut RunCtx,
    region: *mut Region,
    anchor: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        check_active(region);
        let ctx = &mut *ctx;
        if ctx.regions.active != region {
            fault("a region exits before a region entered inside it");
        }
        let mut status = STATUS_OK;
        let mut entry = (*region).cleanup.cast::<Entry>();
        while !entry.is_null() {
            let prev = (*entry).prev;
            if (*entry).block.is_null() {
                let item: *mut Header = (&raw mut (*entry).cleanup)
                    .cast::<Header>()
                    .map_addr(|address| address | 1);
                let base = ctx.work;
                push(ctx, item, true);
                status |= drain(ctx, base, item, ptr::null(), false, anchor).status;
            } else {
                status |= release_block(ctx, (*entry).block, ptr::null(), anchor);
            }
            entry = prev;
        }
        if let Some(owners) = &mut ctx.region_owners {
            for owner in owners.values_mut().filter(|owner| owner.region == region) {
                owner.active = false;
            }
        }
        let mut segment = (*region).segment.cast::<Segment>();
        while !segment.is_null() {
            let prev = (*segment).prev;
            if ctx.heap.diagnostic {
                let data = segment.cast::<u8>().add(SEGMENT_HEADER);
                #[cfg(feature = "memcheck")]
                memcheck::undefined(data, (*segment).size as usize - SEGMENT_HEADER);
                ptr::write_bytes(data, FREE_POISON, (*segment).size as usize - SEGMENT_HEADER);
            }
            ctx.regions.give_back(segment);
            #[cfg(feature = "measurement")]
            {
                ctx.heap.measurement.region_pool_high_water_bytes = ctx
                    .heap
                    .measurement
                    .region_pool_high_water_bytes
                    .max(ctx.regions.idle_bytes);
            }
            segment = prev;
        }
        (*region).state = REGION_EXITED;
        ctx.regions.active = (*region).parent;
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            ctx.regions.active_count -= 1;
        }
        ctx.trim();
        status
    }
}

/// Begin an unpublished builder of `capacity` bytes aligned to `align` in `region`.
///
/// # Safety
/// `ctx` is live, `region` active and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_builder_begin(
    ctx: *mut RunCtx,
    region: *mut Region,
    capacity: u64,
    align: u64,
    out: *mut Builder,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        check_active(region);
        let data = reserve(&mut *ctx, region, capacity, align);
        out.write(Builder {
            data,
            capacity,
            align,
            region,
            published: 0,
        });
    }
    STATUS_OK
}

/// Grow an unpublished builder to at least `min_capacity` bytes, keeping its first `used` bytes.
/// It grows in place when it is its region's newest reservation and the segment has room, else
/// it relocates (no live binding or call has observed it yet, MEM-10). Growing a published
/// builder is an invariant fault: published storage never moves.
///
/// # Safety
/// `ctx` is live and `builder` a builder of an active region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_builder_grow(
    ctx: *mut RunCtx,
    builder: *mut Builder,
    used: u64,
    min_capacity: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        let builder = &mut *builder;
        check_active(builder.region);
        if builder.published != 0 {
            fault("a published builder was grown; growth after publication builds a replacement");
        }
        if used > builder.capacity {
            fault("a builder reports more used bytes than its capacity");
        }
        let capacity = min_capacity.max(builder.capacity.saturating_mul(2)).max(16);
        let region = &mut *builder.region;
        let tail = advance(builder.data, builder.capacity);
        let end = segment_end(region);
        if tail == region.cursor && (end as usize - builder.data as usize) as u64 >= capacity {
            region.cursor = advance(builder.data, capacity);
            #[cfg(feature = "memcheck")]
            {
                let grown = builder.data.add(builder.capacity as usize);
                memcheck::undefined(grown, (capacity - builder.capacity) as usize);
                memcheck::pool_resize(region.segment, builder.data, capacity as usize);
            }
            builder.capacity = capacity;
            return STATUS_OK;
        }
        let data = reserve(ctx, builder.region, capacity, builder.align);
        ptr::copy_nonoverlapping(builder.data, data, used as usize);
        if ctx.heap.diagnostic {
            ptr::write_bytes(builder.data, FREE_POISON, builder.capacity as usize);
        }
        // Nothing has observed the old storage (MEM-10): it is unaddressable once moved.
        #[cfg(feature = "memcheck")]
        memcheck::pool_free(
            segment_of(&*builder.region, builder.data).cast(),
            builder.data,
        );
        builder.data = data;
        builder.capacity = capacity;
    }
    STATUS_OK
}

/// Publish a builder whose first `used` bytes are its value: it never moves or mutates again.
/// When it is its region's newest reservation the unused tail returns to the region.
///
/// # Safety
/// `ctx` is live and `builder` an unpublished builder of an active region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_builder_publish(
    _ctx: *mut RunCtx,
    builder: *mut Builder,
    used: u64,
) {
    // SAFETY: caller contract.
    unsafe {
        let builder = &mut *builder;
        check_active(builder.region);
        if builder.published != 0 || used > builder.capacity {
            fault("a builder is published twice or with more bytes than its capacity");
        }
        let region = &mut *builder.region;
        if advance(builder.data, builder.capacity) == region.cursor {
            region.cursor = advance(builder.data, used);
        }
        // The published value is its first `used` bytes; the rest is a region tail.
        #[cfg(feature = "memcheck")]
        {
            let segment = segment_of(region, builder.data);
            memcheck::noaccess(
                builder.data.add(used as usize),
                (builder.capacity - used) as usize,
            );
            memcheck::pool_resize(segment.cast(), builder.data, used as usize);
        }
        builder.capacity = used;
        builder.published = 1;
    }
}
