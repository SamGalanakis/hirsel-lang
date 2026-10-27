//! The task-local size-class small-object allocator and the counted block header v2.
//!
//! Requests of at most [`SMALL_MAX`] payload bytes with alignment at most [`SMALL_ALIGN_MAX`]
//! round up to a class in [`CLASS_SIZES`]: 16-byte steps to 256 bytes, then quarter-power-of-two
//! steps to 4 KiB. Blocks come from [`SLAB_SIZE`] slabs, each one backing allocation aligned to
//! its size so a block finds its slab by masking. Following mimalloc's page-local lists
//! (`alloc.c`, `page.c`, `free.c`; MIT, see third-party/NOTICES.md), allocation pops
//! the current slab's `free` list; free only pushes its separate `local_free` list.
//! An exhausted allocation list extends sequentially through fresh page capacity before
//! collecting local frees. No allocation-list head lives in the size-class table.
//! Larger or more aligned requests take the large-object path to the backing allocator.
//! The slab-derived capacity is the class capacity, so a reuse
//! token's exact `(capacity, alignment, domain)` key matches by class.
//!
//! A slab with no live block is idle. At each trim point (region exit and the explicit trim
//! entry) idle slabs above the context's idle cap go back to the backing allocator; free blocks
//! inside a slab that still holds a live block are not returnable and are reported as
//! fragmentation. Task exit returns every slab and large block.
//!
//! Each worker has its own heap, and heaps live in their run's [`crate::heaps::HeapRegistry`]
//! until the run ends (design §6 Heaps). A slab, large block and region segment carries its
//! heap's id, and a free on another heap publishes the storage to its owner instead of touching
//! the owner's lists ([`crate::remote`]): a small block on its slab's `xthread` list, a large
//! block or segment on the owner heap's return list. The owner collects at its slow paths, trim
//! points and scheduling points; a slab goes back only when it holds no live block and no
//! published one. Semantic destruction always runs on the releasing task; only storage
//! reclamation is deferred.
//!
//! The `system-allocator` archive has no slabs. A block of a fixed-size layout
//! ([`crate::LAYOUT_NO_EXTENT`] without [`crate::release::LAYOUT_WIDE_CURSOR`]), at most
//! 8-aligned, is *bare*: one libc allocation of its header and class capacity, of kind
//! [`KIND_SMALL`], freed by one `free` of its header. Its capacity is its descriptor's size class, so it carries no
//! metadata of its own, and compiled code never queries a fixed layout's capacity. A dynamic,
//! wide-cursor or over-aligned block keeps the large-object prefix: its capacity is a
//! per-block fact (compiled code reads it through the prefix pointer), a dead wide cursor needs
//! the prefix's shadow word, and malloc only guarantees eight-byte alignment to the header.
//! Production cannot list bare blocks, so task exit leaves a bare block still live (a leak or
//! an immortal) to the process; the diagnostic and measurement archives list them for their
//! accounting and return them at exit.

use std::alloc::{Layout as HostLayout, alloc as host_alloc, dealloc as host_dealloc};
use std::ptr;

use crate::context::TaskCtx;
#[cfg(feature = "memcheck")]
use crate::memcheck;
use crate::memcheck::SLAB_REDZONE;
use crate::release::check_layout;
use crate::remote::{self, Link, Owner, Page, Returns, Xthread};
use crate::{STATUS_OK, environmental, fault};

#[cfg(not(target_pointer_width = "64"))]
compile_error!("hirsel_rt_v2 headers assume 64-bit pointers");

/// Bytes of the counted block header v2; the payload starts this far after the block reference.
pub const HEADER_SIZE: usize = 8;
/// The largest payload served by a size class.
pub const SMALL_MAX: u64 = 4096;
/// The largest alignment served by a size class; class payloads are always 16-byte aligned.
pub const SMALL_ALIGN_MAX: u64 = 16;
/// The largest payload extent a request may name; a larger one exceeds the host address
/// space and terminates environmentally ([`check_request`]).
pub const EXTENT_MAX: u64 = isize::MAX as u64 / 2;
/// Bytes of one slab, which is also its alignment.
pub const SLAB_SIZE: usize = 64 * 1024;
/// Number of size classes.
pub const CLASS_COUNT: usize = 32;
/// `Header::class` of a large-object block.
pub const LARGE_CLASS: u8 = 0xff;
/// Usable payload bytes of each class.
pub const CLASS_SIZES: [u32; CLASS_COUNT] = [
    16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, // 16-byte steps
    320, 384, 448, 512, 640, 768, 896, 1024, 1280, 1536, 1792, 2048, 2560, 3072, 3584, 4096,
];

/// Storage domain of a counted heap block (`StorageOwner::Heap`).
pub const DOMAIN_HEAP: u8 = 1;
/// A static block: compiler-emitted, count [`crate::COUNT_IMMORTAL`], never freed.
pub const DOMAIN_STATIC: u8 = 2;
/// A compiler-reserved cleanup continuation for an inline value (not a block).
pub const DOMAIN_REGION: u8 = 3;
pub const KIND_SMALL: u32 = 0;
pub const KIND_LARGE: u32 = 1;
pub const KIND_STATIC: u32 = 2;
pub const KIND_REGION: u32 = 3;
pub const INDEX_MASK: u32 = (1 << 29) - 1;
pub const KIND_SHIFT: u32 = 29;
pub const WIDE_DEAD: u32 = 1 << 31;

/// Free-list storage, or a finished cleanup continuation.
pub const STATE_FREE: u8 = 0;
/// A live value; `count` holds its holders.
pub const STATE_LIVE: u8 = 1;
/// Dead and on the worklist; its own finalizer has not started.
pub const STATE_FINALIZE_PENDING: u8 = 2;
/// Its own finalizer is running (DROP-2's `Finalizing`); it cannot be entered again.
pub const STATE_FINALIZING: u8 = 3;
/// Its own finalizer is done or absent; the drop program's cursor walks the fields.
pub const STATE_FIELDS_PENDING: u8 = 4;
/// Raw storage held by a reuse token.
pub const STATE_RAW: u8 = 5;

/// Payload poison written on free in diagnostic mode and checked on reuse.
pub const FREE_POISON: u8 = 0xdf;
/// Payload poison written on allocation in diagnostic mode (uninitialized bytes).
pub const UNINIT_POISON: u8 = 0xa5;

/// The sole counted representation. A reference always points eight bytes before its payload.
#[repr(C, align(8))]
#[derive(Debug)]
pub struct Header {
    pub count: u32,
    pub descriptor: u32,
}
const _: () = assert!(size_of::<Header>() == HEADER_SIZE);

/// Immutable extent metadata immediately before static and regional immortal headers.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Extent {
    pub capacity: u64,
    pub align: u64,
}

impl Header {
    #[inline(always)]
    pub fn kind(&self) -> u32 {
        (self.descriptor >> KIND_SHIFT) & 3
    }
    #[inline(always)]
    /// # Safety
    /// The header is live/raw heap storage or an immortal owner, or a valid suspended dead block.
    pub unsafe fn index(&self) -> u32 {
        if self.descriptor & WIDE_DEAD == 0 {
            self.descriptor & INDEX_MASK
        } else {
            unsafe { self.shadow().read() & INDEX_MASK }
        }
    }
    #[inline(always)]
    pub fn domain(&self) -> u8 {
        match self.kind() {
            KIND_SMALL | KIND_LARGE => DOMAIN_HEAP,
            KIND_STATIC => DOMAIN_STATIC,
            _ => DOMAIN_REGION,
        }
    }
    #[inline]
    /// # Safety
    /// The header belongs to a live/raw block with its matching slab or prefix metadata. An
    /// arena block of a [`crate::LAYOUT_NO_EXTENT`] layout and a bare system block have none:
    /// see [`Heap::usable`].
    pub unsafe fn capacity(&self) -> u64 {
        unsafe {
            match self.kind() {
                KIND_SMALL => {
                    #[cfg(feature = "system-allocator")]
                    fault("a bare block's capacity is its descriptor's");
                    #[cfg(not(feature = "system-allocator"))]
                    {
                        class_capacity((*slab_of(self as *const Self as *mut Self)).pool as usize)
                    }
                }
                KIND_LARGE => (*self.large_prefix()).capacity,
                _ => (*self.extent()).capacity,
            }
        }
    }
    #[inline]
    /// # Safety
    /// As for [`Header::capacity`].
    pub unsafe fn align(&self) -> u64 {
        unsafe {
            match self.kind() {
                KIND_SMALL => {
                    #[cfg(feature = "system-allocator")]
                    fault("a bare block's alignment is its descriptor's");
                    #[cfg(not(feature = "system-allocator"))]
                    {
                        (*slab_of(self as *const Self as *mut Self)).align as u64
                    }
                }
                KIND_LARGE => (*self.large_prefix()).align,
                _ => (*self.extent()).align,
            }
        }
    }
    unsafe fn extent(&self) -> *const Extent {
        (self as *const Self)
            .cast::<u8>()
            .wrapping_sub(size_of::<Extent>())
            .cast()
    }
    unsafe fn large_prefix(&self) -> *mut LargePrefix {
        unsafe {
            (self as *const Self)
                .cast::<u8>()
                .sub(8)
                .cast::<*mut LargePrefix>()
                .read()
        }
    }
    #[inline]
    pub(crate) unsafe fn shadow(&self) -> *mut u32 {
        unsafe {
            match self.kind() {
                #[cfg(feature = "system-allocator")]
                KIND_SMALL => fault("a bare block cannot hold a wide dead cursor"),
                #[cfg(not(feature = "system-allocator"))]
                KIND_SMALL => {
                    let block = self as *const Self as *mut Self;
                    let slab = slab_of(block);
                    (*slab).shadow.add(slot_of(slab, block))
                }
                KIND_LARGE => &raw mut (*self.large_prefix()).shadow,
                _ => fault("an immortal block cannot hold a dead cursor"),
            }
        }
    }
    #[inline]
    pub(crate) fn cursor(&self) -> u64 {
        if self.descriptor & WIDE_DEAD == 0 {
            return self.count as u64;
        }
        unsafe {
            let bits = (self as *const Self).cast::<u64>().read();
            (bits & ((1u64 << 61) - 1)) | ((*self.shadow() as u64 >> 29) << 61)
        }
    }
    #[inline]
    pub(crate) fn set_cursor(&mut self, cursor: u64, wide: bool) {
        if !wide {
            self.count = cursor as u32;
            return;
        }
        unsafe {
            let index = self.index();
            let kind = self.kind();
            self.shadow().write(index | ((cursor >> 61) as u32) << 29);
            (self as *mut Self)
                .cast::<u64>()
                .write(cursor & ((1u64 << 61) - 1) | (kind as u64) << 61 | 1u64 << 63);
        }
    }
    pub(crate) fn restore_descriptor(&mut self) {
        self.descriptor = unsafe { self.index() } | self.kind() << KIND_SHIFT;
    }
}

/// The capacity a heap request of `extent` payload bytes at `align` receives: its class's, or
/// the rounded extent. A reuse token's capacity key is the capacity of its request.
#[inline]
pub(crate) fn request_capacity(extent: u64, align: u64) -> u64 {
    if extent <= SMALL_MAX && align <= SMALL_ALIGN_MAX {
        class_capacity(class_of(extent))
    } else {
        extent.next_multiple_of(16)
    }
}

/// Usable bytes of a small pool, shared by allocation, request-derived capacity and metadata.
#[inline]
fn class_capacity(pool: usize) -> u64 {
    CLASS_SIZES[pool % CLASS_COUNT] as u64
}

/// `value` rounded up to `align`, a power of two: a mask, never a division.
#[inline(always)]
pub(crate) fn align_up(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (value + (align - 1)) & !(align - 1)
}

/// The offset of a sequence payload's capacity word (runtime ABI 5): the u64 number of
/// elements its storage holds, after the u64 count. It is never more than the storage's real
/// slots, and every [`hirsel_rt_v2_seq_grow`] leaves it exact, so compiled code tests a push
/// against it with one load. The runtime's copy of `hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET`.
pub const SEQUENCE_CAPACITY_OFFSET: u64 = 8;

/// The offset of a sequence payload's first element of alignment `element_align`: past its
/// u64 count and u64 capacity, aligned to the element (runtime ABI 5). The runtime's copy of
/// `hirsel_lir::layout::sequence_elements_offset`, which it cannot depend on;
/// `tests/it/alloc.rs` checks the two agree.
pub fn sequence_elements_offset(element_align: u64) -> u64 {
    align_up(16, element_align)
}

/// The start word of a slice over a whole sequence of alignment `element_align`: its first
/// element's offset past the u64 word a slice's start counts from (the slice data offset,
/// `align_up(8, element_align)`). The runtime's copy of
/// `hirsel_lir::layout::sequence_slice_start`.
pub fn sequence_slice_start(element_align: u64) -> u64 {
    sequence_elements_offset(element_align) - align_up(8, element_align)
}

/// The elements of `element_size` bytes the storage of the sequence block `block` holds past
/// its first element at `elements`: every slot, so the capacity word it sets is exact.
/// Zero-sized elements need no storage, so every capacity is held.
///
/// # Safety
/// `block` is a live block with its allocator metadata.
pub(crate) unsafe fn sequence_slots(
    heap: &Heap,
    block: *const Header,
    elements: u64,
    element_size: u64,
) -> u64 {
    if element_size == 0 {
        return u64::MAX;
    }
    // SAFETY: caller contract.
    slots_in_bytes(
        unsafe { heap.capacity(block) }.saturating_sub(elements),
        element_size,
    )
}

/// Convert bytes available for elements into exact slots, including zero-sized elements.
#[inline]
fn slots_in_bytes(bytes: u64, element_size: u64) -> u64 {
    // Byte sequences need no general division or zero-size branch on their common path.
    if element_size == 1 {
        return bytes;
    }
    if element_size == 0 {
        u64::MAX
    } else {
        bytes / element_size
    }
}

/// Exact slots of fresh storage: use the allocator's request-to-class mapping, never a
/// second lookup through the returned block's metadata. Existing storage still needs
/// [`sequence_slots`], since its class need not be the class of the current request.
#[inline]
fn requested_sequence_slots(extent: u64, align: u64, elements: u64, element_size: u64) -> u64 {
    slots_in_bytes(
        request_capacity(extent, align).saturating_sub(elements),
        element_size,
    )
}

/// Write `slots` as the capacity word of the sequence block `block`.
///
/// # Safety
/// `block` is a live, writable sequence block.
pub(crate) unsafe fn set_sequence_capacity(block: *mut Header, slots: u64) {
    // SAFETY: caller contract; the word follows the count.
    unsafe {
        payload(block)
            .add(SEQUENCE_CAPACITY_OFFSET as usize)
            .cast::<u64>()
            .write(slots)
    }
}

/// A fresh, unique heap sequence block of the descriptor at `index` holding `count` elements
/// of `element_size` bytes at `element_align` (runtime ABI 5): its count and exact capacity
/// are written, its elements left for the caller to fill from
/// [`sequence_elements_offset`]. An extent beyond u64 is an invariant fault; host exhaustion
/// terminates environmentally.
///
/// # Safety
/// `ctx` is a live run context and `index` names a sequence descriptor of this element.
pub(crate) unsafe fn allocate_sequence(
    ctx: *mut TaskCtx,
    index: u32,
    count: u64,
    element_size: u64,
    element_align: u64,
) -> *mut Header {
    let elements = sequence_elements_offset(element_align);
    let extent = count
        .checked_mul(element_size)
        .and_then(|bytes| bytes.checked_add(elements))
        .unwrap_or_else(|| fault("a sequence extent overflows"));
    // SAFETY: caller contract.
    unsafe {
        let block = hirsel_rt_v2_alloc(ctx, index, extent, element_align.max(8), DOMAIN_HEAP, 0);
        payload(block).cast::<u64>().write(count);
        let slots = requested_sequence_slots(extent, element_align.max(8), elements, element_size);
        set_sequence_capacity(block, slots);
        block
    }
}

/// A layout's bare blocks in the system-allocator archive: the extent a bare request has and
/// the capacity its block receives. A layout whose blocks cannot be bare (dynamic, wide-cursor
/// or over-aligned) has `size == u64::MAX`, which no valid extent equals.
#[cfg(feature = "system-allocator")]
#[derive(Clone, Copy)]
struct BareLayout {
    size: u64,
    capacity: u64,
}

#[cfg(feature = "system-allocator")]
impl BareLayout {
    fn of(layout: &crate::Layout) -> BareLayout {
        let flags = crate::LAYOUT_NO_EXTENT | crate::release::LAYOUT_WIDE_CURSOR;
        if layout.flags & flags == crate::LAYOUT_NO_EXTENT && layout.align <= 8 {
            BareLayout {
                size: layout.size,
                capacity: request_capacity(layout.size, 8),
            }
        } else {
            BareLayout {
                size: u64::MAX,
                capacity: 0,
            }
        }
    }
}

/// The size class serving `extent` payload bytes (which must be at most [`SMALL_MAX`]).
#[inline]
pub const fn class_of(extent: u64) -> usize {
    if extent <= 256 {
        (extent.saturating_sub(1) / 16) as usize
    } else {
        // For 256 < n <= 4096: 2^p < n <= 2^(p+1) with p in 8..=11, split into four quarters.
        let above = extent - 1;
        let p = 63 - above.leading_zeros() as u64;
        let quarter = (above - (1 << p)) >> (p - 2);
        (16 + (p - 8) * 4 + quarter) as usize
    }
}

/// Slab bookkeeping at the start of every slab.
#[repr(C)]
pub(crate) struct Slab {
    /// The class's next slab (every slab of the class).
    next: *mut Slab,
    /// The class's next available slab: not current, with free blocks.
    available: *mut Slab,
    /// Blocks ready for allocation, exclusively popped by allocation.
    free: *mut Header,
    live: u32,
    doomed: u32,
    pool: u32,
    align: u32,
    first: u32,
    stride: u32,
    slots: u32,
    multiplier: u32,
    shadow: *mut u32,
    states: *mut u8,
    /// Deferred frees; never changes the allocation list head.
    local_free: *mut Header,
    /// Untouched storage, extended in sequential batches of at most 8 KiB.
    bump: *mut u8,
    end: *mut u8,
    /// The id of the heap whose lists this slab is on (runtime ABI 26): a free from another
    /// heap publishes to `xthread` instead.
    owner: u32,
    _reserved: u32,
    /// Blocks other heaps freed, with the queued flag (`crate::remote`).
    xthread: Xthread<Header>,
    /// The next slab on its heap's queue of slabs with published blocks.
    queue_next: *mut Slab,
}

impl Link for Header {
    unsafe fn set_next(node: *mut Header, next: *mut Header) {
        unsafe { write_free(node, next) }
    }
    unsafe fn next(node: *mut Header) -> *mut Header {
        unsafe { free_next(node) }
    }
}

impl Link for Slab {
    unsafe fn set_next(node: *mut Slab, next: *mut Slab) {
        unsafe { (*node).queue_next = next }
    }
    unsafe fn next(node: *mut Slab) -> *mut Slab {
        unsafe { (*node).queue_next }
    }
}

impl Page for Slab {
    type Block = Header;
    unsafe fn xthread<'a>(page: *mut Slab) -> &'a Xthread<Header> {
        unsafe { &(*page).xthread }
    }
}

const SLAB_HEADER: usize = 128;
/// Task-context and page words compiled fast paths read and write (runtime ABI 4,
/// [`Compiled`]); a task context begins with its [`View`], and the view with them.
pub const CTX_DUP_MAX_OFFSET: usize = std::mem::offset_of!(Compiled, dup_max);
pub const CTX_FREE_INLINE_OFFSET: usize = std::mem::offset_of!(Compiled, free_inline);
pub const CTX_IDLE_SLABS_OFFSET: usize = std::mem::offset_of!(Compiled, idle_slabs);
pub const CTX_PAGES_OFFSET: usize = std::mem::offset_of!(Compiled, pages);
pub const CTX_FUEL_OFFSET: usize = std::mem::offset_of!(Compiled, fuel);
pub const CTX_HEAP_ID_OFFSET: usize = std::mem::offset_of!(Compiled, heap_id);
pub const PAGE_FREE_OFFSET: usize = std::mem::offset_of!(Slab, free);
pub const PAGE_LIVE_OFFSET: usize = std::mem::offset_of!(Slab, live);
pub const PAGE_LOCAL_FREE_OFFSET: usize = std::mem::offset_of!(Slab, local_free);
pub const PAGE_OWNER_OFFSET: usize = std::mem::offset_of!(Slab, owner);
pub const PAGE_XTHREAD_FREE_OFFSET: usize = std::mem::offset_of!(Slab, xthread);
const _: () = assert!(CTX_HEAP_ID_OFFSET == CTX_FUEL_OFFSET + 8);
const _: () = assert!(PAGE_OWNER_OFFSET == 96 && PAGE_XTHREAD_FREE_OFFSET == 104);
const _: () = assert!(size_of::<Xthread<Header>>() == 8);
const _: () = assert!(std::mem::offset_of!(View, compiled) == 0);
/// Size-class pools: each class at 8-byte and at 16-byte alignment.
pub const POOL_COUNT: usize = CLASS_COUNT * 2;
const _: () = assert!(size_of::<Slab>() <= SLAB_HEADER);

/// Bookkeeping in front of every large block.
#[repr(C)]
struct LargePrefix {
    prev: *mut LargePrefix,
    next: *mut LargePrefix,
    total: u64,
    align: u64,
    capacity: u64,
    shadow: u32,
    /// The id of the heap whose large list holds this block.
    owner: u32,
    /// The next block on the owner heap's return list.
    return_next: *mut LargePrefix,
    #[cfg(feature = "system-allocator")]
    allocation: *mut u8,
}

impl Link for LargePrefix {
    unsafe fn set_next(node: *mut LargePrefix, next: *mut LargePrefix) {
        unsafe { (*node).return_next = next }
    }
    unsafe fn next(node: *mut LargePrefix) -> *mut LargePrefix {
        unsafe { (*node).return_next }
    }
}

const LARGE_PREFIX: u64 = size_of::<LargePrefix>() as u64;

// The public read-only capacity contract must survive bookkeeping changes.
const _: () = assert!(std::mem::offset_of!(Slab, pool) == 32);
const _: () = assert!(std::mem::offset_of!(LargePrefix, capacity) == 32);
const _: () = assert!(size_of::<*mut LargePrefix>() == 8);
const _: () = assert!(size_of::<Extent>() == 16 && std::mem::offset_of!(Extent, capacity) == 0);

/// One size class. A slab is current, available (on the `available` list: it has free blocks),
/// or full (no free block and no untouched tail); a full slab's first freed block makes it
/// available again.
#[derive(Clone, Copy)]
struct Class {
    current: *mut Slab,
    available: *mut Slab,
    slabs: *mut Slab,
}

const EMPTY_CLASS: Class = Class {
    current: ptr::null_mut(),
    available: ptr::null_mut(),
    slabs: ptr::null_mut(),
};

/// A slab the diagnostic registry knows: live (its blocks' states are in the slab) or retired
/// (its storage went back, and every block of it reads as `state`).
struct SlabRecord {
    state: u8,
    first: u32,
    stride: u32,
    slots: u32,
}

impl Diag {
    /// The state byte of `block` if this registry knows it, and whether its record is live (an
    /// allocated slab's, or a large or bare block's that is not free) rather than retired.
    /// The pointer is valid while the registry stays locked.
    unsafe fn find(&mut self, block: *mut Header) -> Option<(bool, *mut u8)> {
        if let Some(state) = self.large_states.get_mut(&block.addr()) {
            return Some((*state != STATE_FREE, state as *mut u8));
        }
        let slab = slab_of(block);
        let record = self.slabs.get_mut(&slab.addr())?;
        let delta = block
            .addr()
            .wrapping_sub(slab.addr() + record.first as usize);
        if delta >= record.slots as usize * record.stride as usize
            || !delta.is_multiple_of(record.stride as usize)
        {
            // A retired slab's memory may since back another heap's large block.
            if record.state != STATE_LIVE {
                return None;
            }
            fault("invalid slab slot");
        }
        if record.state == STATE_LIVE {
            // SAFETY: an allocated slab's state array has a byte per slot.
            Some((true, unsafe { (*slab).states.add(slot_of(slab, block)) }))
        } else {
            Some((false, &raw mut record.state))
        }
    }
}

/// A heap's diagnostic registries, behind its lock so a free on another heap can find a
/// block's state (design §6 Heaps). Empty in production: only diagnostic mode records states,
/// and only the system allocator's correctness and measurement archives list bare blocks.
#[derive(Default)]
struct Diag {
    /// States of blocks outside slabs: large and bare blocks.
    large_states: std::collections::BTreeMap<usize, u8>,
    slabs: std::collections::BTreeMap<usize, SlabRecord>,
    /// The live bare blocks.
    #[cfg(all(
        feature = "system-allocator",
        any(feature = "diagnostic", feature = "measurement")
    ))]
    bare: std::collections::HashSet<usize>,
}

/// The words compiled code reads at fixed offsets from the task context (runtime ABI 4,
/// `hirsel_rt_v2.h`; the context begins with its [`View`] and the view with these). Each is the
/// state behind one inline fast path, or a value that sends every operation to its entry:
/// diagnostic mode and measurement builds count or check every allocation, free and dup, and
/// the system allocator has no pages.
#[repr(C)]
pub(crate) struct Compiled {
    /// A dup of a block whose count is below this stores `count + 1` inline (T03 Dup); 0 sends
    /// every dup to `hirsel_rt_v2_dup`. Otherwise the context's finite count maximum, so the
    /// inline path never reaches saturation or the immortal sentinel.
    pub(crate) dup_max: u32,
    /// Nonzero when compiled code may push a small block onto its page's `local_free` list.
    pub(crate) free_inline: u32,
    /// Slabs with no live block. An allocation that makes an idle page's live count 1 takes
    /// one, a free that makes it 0 adds one, inline or not; trim points read it.
    pub(crate) idle_slabs: u64,
    /// Per pool, the current allocation page, or null where compiled code calls
    /// `hirsel_rt_v2_alloc`. Mirrors [`Class::current`].
    pub(crate) pages: [*mut Slab; POOL_COUNT],
    /// The running task's fuel chunk (`budgets.limits@2`, `crate::budget`): a fuel build's
    /// code subtracts one at each fuel point and, when the result is negative, calls
    /// `hirsel_rt_v2_fuel_refill`, which stores the next chunk here and returns.
    pub(crate) fuel: i64,
    /// The id of the heap whose pages these are (runtime ABI 26): compiled code frees a small
    /// block inline only when its slab's `owner` equals it.
    pub(crate) heap_id: u32,
    _reserved: u32,
}

/// A task's copy of its worker's [`Compiled`] words, at the start of its context, and the
/// heap and heap epoch whose pages it holds. Switching a task in on a worker ([`Heap::bind`])
/// copies the pages only when that stamp differs, and always hands over the counters; switching
/// it out ([`Heap::unbind`]) folds them back. Only the bound view changes while its task runs:
/// a slow path that moves a current page updates the worker's words and the view together.
#[repr(C)]
pub(crate) struct View {
    pub(crate) compiled: Compiled,
    heap: u32,
    epoch: u64,
}

impl View {
    /// A private reservation cannot allocate or release storage through an
    /// inline path. Force preflight before its first such operation; invalidate
    /// the stamp so canonical switch-in restores the worker's ordinary view.
    pub(crate) fn private(&mut self) {
        self.heap = 0;
        self.epoch = 0;
        self.compiled.pages.fill(ptr::null_mut());
        self.compiled.dup_max = 0;
        self.compiled.free_inline = 0;
    }

    /// A view no worker has bound yet: its first binding copies every word.
    pub(crate) fn new() -> View {
        View {
            compiled: Compiled {
                dup_max: 0,
                free_inline: 0,
                idle_slabs: 0,
                pages: [ptr::null_mut(); POOL_COUNT],
                fuel: 0,
                heap_id: 0,
                _reserved: 0,
            },
            heap: 0,
            epoch: 0,
        }
    }
}

/// One worker's heap domain.
pub(crate) struct Heap {
    /// The worker's own [`Compiled`] words. While a task runs its bound view holds the live
    /// counters (`idle_slabs`, `fuel`); these hold them between tasks, and the pages always.
    compiled: Compiled,
    /// This heap's id in its run's registry (nonzero, never reused within the run), and the
    /// epoch of its pages, which every page change bumps.
    pub(crate) id: u32,
    epoch: u64,
    /// The running task's view ([`Heap::bind`]), or [`Heap::parked`] while no task runs here.
    view: *mut View,
    /// The view of a heap no task is bound to: its collector's, between tasks or once its
    /// worker retired.
    parked: View,
    /// The worker collecting this heap (`crate::remote::Owner`).
    pub(crate) owner: Owner,
    /// Slabs with blocks other heaps freed, and large blocks other heaps freed.
    queue: Returns<Slab>,
    large_returns: Returns<LargePrefix>,
    /// The worker's region segment pool, which lives and returns with its heap.
    pub(crate) regions: crate::region::RegionPool,
    /// The run's registry, through which a free finds another heap; null for a heap outside
    /// any run (unit tests), all of whose storage is its own.
    registry: *const crate::heaps::HeapRegistry,
    /// The linked program's layout descriptors, by index.
    layouts: Box<[*const crate::Layout]>,
    /// Each layout's bare blocks, by index, so a bare allocation reads one entry.
    #[cfg(feature = "system-allocator")]
    bare_layouts: Box<[BareLayout]>,
    classes: [Class; POOL_COUNT],
    large: *mut LargePrefix,
    diag: std::sync::Mutex<Diag>,
    pub(crate) diagnostic: bool,
    pub(crate) idle_cap: u64,
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) slab_count: u64,
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) large_bytes: u64,
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) backing_allocations: u64,
    #[cfg(feature = "measurement")]
    pub(crate) measurement: crate::measurement::Accounting,
    /// The run's memory budget, shared with its region pool.
    pub(crate) meter: std::sync::Arc<crate::budget::Meter>,
    /// Whether the run has a memory budget, so every allocation and free reaches the meter.
    metered: bool,
    cpu_allocation: Option<crate::pool::cpu_scoped::ledger::HeapReservation>,
}

#[inline]
fn block_size(pool: usize) -> usize {
    (HEADER_SIZE + class_capacity(pool) as usize + SLAB_REDZONE).next_multiple_of(pool_align(pool))
}
fn pool_align(pool: usize) -> usize {
    if pool < CLASS_COUNT { 8 } else { 16 }
}
fn pool_of(extent: u64, align: u64) -> usize {
    class_of(extent) + if align > 8 { CLASS_COUNT } else { 0 }
}
#[inline]
unsafe fn slot_of(slab: *mut Slab, block: *mut Header) -> usize {
    unsafe {
        (((block as usize - slab as usize - (*slab).first as usize) as u64
            * (*slab).multiplier as u64)
            >> 32) as usize
    }
}
/// A free block's link overlays its header. Memcheck keeps free blocks unaddressable, so the
/// runtime opens the link word for exactly its own read or write.
#[inline(always)]
unsafe fn free_next(block: *mut Header) -> *mut Header {
    #[cfg(feature = "memcheck")]
    memcheck::defined(block.cast(), size_of::<*mut Header>());
    let next = unsafe { block.cast::<*mut Header>().read() };
    #[cfg(feature = "memcheck")]
    memcheck::noaccess(block.cast(), size_of::<*mut Header>());
    next
}
#[inline(always)]
unsafe fn write_free(block: *mut Header, next: *mut Header) {
    #[cfg(feature = "memcheck")]
    memcheck::undefined(block.cast(), size_of::<*mut Header>());
    unsafe { block.cast::<*mut Header>().write(next) };
    #[cfg(feature = "memcheck")]
    memcheck::noaccess(block.cast(), size_of::<*mut Header>());
}

#[inline]
fn slab_of(block: *mut Header) -> *mut Slab {
    (block as usize & !(SLAB_SIZE - 1)) as *mut Slab
}

#[inline]
fn slab_layout() -> HostLayout {
    // SAFETY: SLAB_SIZE is a nonzero power of two and fits isize.
    unsafe { HostLayout::from_size_align_unchecked(SLAB_SIZE, SLAB_SIZE) }
}

// glibc malloc/malloc.c:2254-2285,4395-4412 (04e750e7): large chunks use
// page-rounded mmap/munmap, but freeing one raises the process-wide adaptive
// mmap threshold. A service's transient sequence buffers then enter malloc
// arenas and can remain resident indefinitely behind unrelated live chunks.
// Keep the runtime's large buffers out of that policy. mmap guarantees at least
// 4 KiB alignment on our supported Linux targets; rarer larger alignments keep
// their existing backing path. No allocation-kind bit or ABI field is needed:
// the immutable prefix size/alignment determines the matching release.
#[cfg(all(target_os = "linux", not(feature = "system-allocator")))]
fn mapped_large(total: u64, align: u64) -> bool {
    total >= 128 * 1024 && align <= 4096
}

#[cfg(not(feature = "system-allocator"))]
unsafe fn allocate_large(host: HostLayout) -> *mut u8 {
    #[cfg(target_os = "linux")]
    if mapped_large(host.size() as u64, host.align() as u64) {
        // SAFETY: nonzero validated size; anonymous private pages belong only to this block.
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                host.size(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        return if base == libc::MAP_FAILED {
            ptr::null_mut()
        } else {
            base.cast()
        };
    }
    // SAFETY: caller passes the nonzero validated layout of one large block.
    unsafe { host_alloc(host) }
}

/// Offset of a large block's header from its backing allocation.
#[inline]
fn large_header_offset(align: u64) -> u64 {
    align_up(LARGE_PREFIX + 8 + HEADER_SIZE as u64, align) - HEADER_SIZE as u64
}

/// The header of the large block behind `prefix`.
#[inline]
unsafe fn large_block(prefix: *mut LargePrefix) -> *mut Header {
    // SAFETY: `prefix` is a large block's prefix, whose `align` placed its header.
    let offset = unsafe { large_header_offset((*prefix).align) };
    prefix.cast::<u8>().wrapping_add(offset as usize).cast()
}

/// The payload address of a block.
#[inline]
pub(crate) fn payload(block: *mut Header) -> *mut u8 {
    block.cast::<u8>().wrapping_add(HEADER_SIZE)
}

impl Heap {
    /// Physical backing spans for the opt-in measurement-only RSS sampler. Counts and
    /// capacities remain the ctx_stats API's logical view; these spans are not additive RSS.
    #[cfg(feature = "measurement")]
    pub(crate) fn write_residency(&self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        writeln!(
            out,
            "hirsel-rss metadata heap={} layouts={}",
            size_of::<Self>(),
            self.layouts.len() * size_of::<*const crate::Layout>()
        )?;
        for (class, state) in self.classes.iter().enumerate() {
            let mut slab = state.slabs;
            // SAFETY: this worker owns every slab in these lists and is not executing code.
            unsafe {
                while !slab.is_null() {
                    writeln!(
                        out,
                        "hirsel-rss slab {} {} {} {} {} {}",
                        slab.addr(),
                        SLAB_SIZE,
                        class_capacity(class),
                        pool_align(class),
                        (*slab).live,
                        (*slab).slots
                    )?;
                    slab = (*slab).next;
                }
            }
        }
        let mut large = self.large;
        // SAFETY: this worker owns the large-block list.
        unsafe {
            while !large.is_null() {
                writeln!(out, "hirsel-rss large {} {}", large.addr(), (*large).total)?;
                large = (*large).next;
            }
        }
        self.regions.write_residency(out)
    }
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    #[inline(never)]
    fn record_backing_allocation(&mut self) {
        self.backing_allocations += 1;
    }
    pub(crate) fn new(
        layouts: Box<[*const crate::Layout]>,
        diagnostic: bool,
        idle_cap: u64,
    ) -> Heap {
        let meter = std::sync::Arc::<crate::budget::Meter>::default();
        Heap {
            compiled: Compiled {
                dup_max: 0,
                free_inline: Self::inline(diagnostic) as u32,
                idle_slabs: 0,
                pages: [ptr::null_mut(); POOL_COUNT],
                fuel: 0,
                heap_id: 1,
                _reserved: 0,
            },
            id: 1,
            epoch: 0,
            view: ptr::null_mut(),
            parked: View::new(),
            owner: Owner::new(0),
            queue: Returns::new(),
            large_returns: Returns::new(),
            regions: crate::region::RegionPool::new(meter.clone(), 1),
            registry: ptr::null(),
            // SAFETY: the run context validated every descriptor.
            #[cfg(feature = "system-allocator")]
            bare_layouts: layouts
                .iter()
                .map(|&layout| BareLayout::of(unsafe { &*layout }))
                .collect(),
            layouts,
            classes: [EMPTY_CLASS; POOL_COUNT],
            large: ptr::null_mut(),
            diag: Default::default(),
            diagnostic,
            idle_cap,
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            slab_count: 0,
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            large_bytes: 0,
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            backing_allocations: 0,
            #[cfg(feature = "measurement")]
            measurement: Default::default(),
            meter,
            metered: false,
            cpu_allocation: None,
        }
    }

    /// Make this heap the registry's `id`, collected by `worker`, charging the run's `meter`.
    /// Called once, before any storage is taken.
    pub(crate) fn join(
        &mut self,
        id: u32,
        worker: u32,
        registry: *const crate::heaps::HeapRegistry,
        meter: std::sync::Arc<crate::budget::Meter>,
    ) {
        self.id = id;
        self.compiled.heap_id = id;
        self.owner = Owner::new(worker);
        self.registry = registry;
        self.regions = crate::region::RegionPool::new(meter.clone(), id);
        self.meter = meter;
        if self.meter.metered() {
            self.set_metered();
        }
    }

    /// The run has a memory budget: compiled code allocates and frees nothing inline from here
    /// on, so the meter sees every block (design §6 Budgets). Its views refresh at their next
    /// binding, and the bound one now.
    pub(crate) fn set_metered(&mut self) {
        self.metered = true;
        self.compiled.free_inline = 0;
        self.compiled.pages = [ptr::null_mut(); POOL_COUNT];
        self.epoch += 1;
        if !self.view.is_null() {
            // SAFETY: as for `view`.
            let v = unsafe { &mut *self.view };
            v.compiled.free_inline = 0;
            v.compiled.pages = [ptr::null_mut(); POOL_COUNT];
            v.epoch = self.epoch;
        }
    }

    /// The block's logical lifetime ends here, on the freeing task, wherever its storage
    /// returns (design §6 Budgets): credit its logical charge.
    /// # Safety
    /// `block` is a live or raw heap block of this heap's program.
    #[inline(always)]
    unsafe fn credit(&self, block: *mut Header) {
        if let Some(ledger) = self.meter.cpu.get() {
            ledger.blocks.lock().unwrap().release(block);
        }
        if self.metered {
            // SAFETY: caller contract.
            unsafe { self.credit_block(block) }
        }
    }
    #[cold]
    #[inline(never)]
    unsafe fn credit_block(&self, block: *mut Header) {
        // SAFETY: caller contract.
        let charge = unsafe {
            match (*block).kind() {
                KIND_LARGE => crate::budget::large_charge(self.capacity(block)),
                _ => crate::budget::small_charge(self.capacity(block)),
            }
        };
        self.meter.credit(charge);
    }

    /// Bind the heap's own parked view: no task runs on it from here on, and its collector's
    /// slow paths (local frees of collected blocks, trims) update the parked counters.
    pub(crate) fn park(&mut self) {
        let parked = &raw mut self.parked;
        if !self.view.is_null() && self.view != parked {
            self.unbind();
        }
        self.parked.compiled.pages = self.compiled.pages;
        self.parked.compiled.idle_slabs = self.compiled.idle_slabs;
        self.parked.compiled.fuel = self.compiled.fuel;
        self.parked.compiled.heap_id = self.id;
        self.parked.heap = self.id;
        self.parked.epoch = self.epoch;
        self.view = parked;
    }

    /// Bind the parked view without reading the bound one, which may be gone: run teardown.
    pub(crate) fn forget_view(&mut self) {
        self.parked.compiled.pages = self.compiled.pages;
        self.view = &raw mut self.parked;
    }

    /// The heap with id `id` in this heap's run.
    fn heap_of(&self, id: u32) -> *mut Heap {
        if self.registry.is_null() {
            fault("storage of another heap outside any run");
        }
        // SAFETY: the registry outlives every heap in it.
        unsafe { (*self.registry).heap(id) }
    }

    /// The running task's compiled words.
    #[inline(always)]
    pub(crate) fn view(&mut self) -> &mut Compiled {
        // SAFETY: a task's view is bound before any heap operation runs on its behalf, and its
        // context outlives the binding.
        unsafe { &mut (*self.view).compiled }
    }

    /// Switch-in: `view`'s task runs on this heap's worker. Its pages are refreshed when they
    /// were copied from another worker or before a page change, and it takes the counters.
    ///
    /// # Safety
    /// `view` is a live task context's view that stays live while bound.
    #[inline]
    pub(crate) unsafe fn bind(&mut self, view: *mut View) {
        if self.view == &raw mut self.parked {
            self.unbind();
        }
        // SAFETY: caller contract.
        let v = unsafe { &mut *view };
        if v.heap != self.id || v.epoch != self.epoch {
            v.compiled.pages = self.compiled.pages;
            v.compiled.dup_max = self.compiled.dup_max;
            v.compiled.free_inline = self.compiled.free_inline;
            v.compiled.heap_id = self.id;
            v.heap = self.id;
            v.epoch = self.epoch;
        }
        v.compiled.idle_slabs = self.compiled.idle_slabs;
        v.compiled.fuel = self.compiled.fuel;
        self.view = view;
    }

    /// Whether binding `view` refreshes it: it last bound another heap or an older epoch.
    pub(crate) fn stale(&self, view: &View) -> bool {
        view.heap != self.id || view.epoch != self.epoch
    }

    /// Switch-out: fold the running view's counters, which compiled code changed inline, back
    /// into the worker's words.
    #[inline]
    pub(crate) fn unbind(&mut self) {
        let v = self.view();
        let (idle_slabs, fuel) = (v.idle_slabs, v.fuel);
        self.compiled.idle_slabs = idle_slabs;
        self.compiled.fuel = fuel;
    }

    /// The bound view.
    pub(crate) fn bound(&self) -> *mut View {
        self.view
    }

    /// The inline dup bound every view of this heap starts with.
    pub(crate) fn set_dup_max(&mut self, dup_max: u32) {
        self.compiled.dup_max = dup_max;
        self.epoch += 1;
    }

    /// The fuel this heap holds between tasks.
    pub(crate) fn parked_fuel(&self) -> i64 {
        self.compiled.fuel
    }
    /// Hold `fuel` between tasks: the next task bound takes it (`crate::pool` hands the run's
    /// fuel to the thread it hands the run to).
    pub(crate) fn park_fuel(&mut self, fuel: i64) {
        self.compiled.fuel = fuel;
    }

    /// Set the running task's fuel, which it hands on at its switch-out.
    pub(crate) fn set_fuel(&mut self, fuel: i64) {
        self.compiled.fuel = fuel;
        self.view().fuel = fuel;
    }

    /// Change the page compiled code allocates `pool` from: in the worker's words and the bound
    /// view, under a new epoch, so every other view refreshes when it is next bound.
    #[inline]
    fn set_page(&mut self, pool: usize, slab: *mut Slab) {
        self.compiled.pages[pool] = slab;
        self.epoch += 1;
        // SAFETY: as for `view`.
        let v = unsafe { &mut *self.view };
        v.compiled.pages[pool] = slab;
        v.epoch = self.epoch;
    }

    /// The number of descriptors in this heap's registry.
    pub(crate) fn layout_count(&self) -> u32 {
        self.layouts.len() as u32
    }

    #[inline]
    pub(crate) unsafe fn descriptor(&self, index: u32) -> *const crate::Layout {
        if index as usize >= self.layouts.len() {
            fault("descriptor index outside the linked program");
        }
        unsafe { *self.layouts.get_unchecked(index as usize) }
    }

    /// The capacity of a bare block for a request of layout `index`, or None when the request
    /// is not bare: another extent than the layout's size, or an alignment above eight.
    #[cfg(feature = "system-allocator")]
    #[inline(always)]
    pub(crate) fn bare_capacity(&self, index: u32, extent: u64, align: u64) -> Option<u64> {
        let Some(bare) = self.bare_layouts.get(index as usize) else {
            fault("descriptor index outside the linked program");
        };
        (bare.size == extent && align.wrapping_sub(1) < 8 && align & (align - 1) == 0)
            .then_some(bare.capacity)
    }

    /// The usable bytes and physical alignment of any live block: its own metadata, or its
    /// descriptor's for a block that carries none (a bare system block, or an arena block of a
    /// [`crate::LAYOUT_NO_EXTENT`] layout).
    /// # Safety
    /// `block` is a live or raw block of this heap's program.
    pub(crate) unsafe fn usable(&self, block: *const Header) -> (u64, u64) {
        unsafe {
            let header = &*block;
            match header.kind() {
                #[cfg(feature = "system-allocator")]
                KIND_SMALL => match self.bare_layouts.get(header.index() as usize) {
                    Some(bare) if bare.capacity != 0 => (bare.capacity, 8),
                    _ => fault("a bare block's layout cannot be bare"),
                },
                KIND_REGION => {
                    let layout = &*self.descriptor(header.index());
                    if layout.flags & crate::LAYOUT_NO_EXTENT != 0 {
                        (layout.size, layout.align.max(8))
                    } else {
                        (header.capacity(), header.align())
                    }
                }
                _ => (header.capacity(), header.align()),
            }
        }
    }

    /// [`Heap::usable`]'s bytes.
    /// # Safety
    /// As for [`Heap::usable`].
    pub(crate) unsafe fn capacity(&self, block: *const Header) -> u64 {
        unsafe { self.usable(block).0 }
    }

    /// Whether compiled code may allocate and free small blocks inline: never in diagnostic
    /// mode, in measurement builds (which count every class allocation and free), with the
    /// system allocator (which has no pages) or in the memcheck archive (whose allocation and
    /// free tell Valgrind about the block).
    fn inline(diagnostic: bool) -> bool {
        !diagnostic
            && !cfg!(feature = "measurement")
            && !cfg!(feature = "system-allocator")
            && !cfg!(feature = "memcheck")
    }

    /// Make `slab` (or none) the current allocation page of `class`, for the runtime's own
    /// pop and, when inline paths are on, compiled code's.
    #[inline]
    fn set_current(&mut self, class: usize, slab: *mut Slab) {
        self.classes[class].current = slab;
        if self.compiled.free_inline != 0 {
            self.set_page(class, slab);
        }
    }

    pub(crate) fn forget_foreign_record(&mut self, address: usize) {
        if self.diagnostic {
            self.diag().large_states.remove(&address);
        }
    }

    /// This heap's diagnostic registries.
    fn diag(&self) -> std::sync::MutexGuard<'_, Diag> {
        self.diag
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// This heap, then the other heaps of its run: where a block's diagnostic state may be.
    /// It allocates nothing, since releases run under allocation-counting tests.
    fn related_heaps(&self) -> impl Iterator<Item = *const Heap> + '_ {
        let this = self as *const Heap;
        // SAFETY: the registry outlives every heap in it.
        let others = unsafe { self.registry.as_ref() }
            .into_iter()
            .flat_map(|registry| registry.heaps())
            .map(|heap| heap.cast_const())
            .filter(move |&heap| heap != this);
        std::iter::once(this).chain(others)
    }

    /// Run `f` on the diagnostic state byte of `block`, under the lock of the heap whose
    /// registry holds it; None when no heap of the run knows the block. A slab or large block
    /// that a heap gave back and another heap took again at the same address may still have a
    /// retired record in the first heap, so a live record anywhere wins over a retired one.
    unsafe fn with_state<R>(&self, block: *mut Header, f: impl FnOnce(*mut u8) -> R) -> Option<R> {
        let mut retired = None;
        for heap in self.related_heaps() {
            // SAFETY: the run's heaps are live.
            let mut diag = unsafe { (*heap).diag() };
            match unsafe { diag.find(block) } {
                Some((true, state)) => return Some(f(state)),
                Some((false, _)) if retired.is_none() => retired = Some(heap),
                _ => {}
            }
        }
        let heap = retired?;
        // SAFETY: as above.
        let mut diag = unsafe { (*heap).diag() };
        let (_, state) = unsafe { diag.find(block) }?;
        Some(f(state))
    }

    pub(crate) unsafe fn state(&self, block: *mut Header) -> u8 {
        unsafe {
            match self.with_state(block, |state| state.read()) {
                Some(state) => state,
                None if (*block).domain() == DOMAIN_STATIC => STATE_LIVE,
                None => fault("block does not belong to the context"),
            }
        }
    }
    #[inline]
    pub(crate) unsafe fn set_state(&self, block: *mut Header, state: u8) {
        if self.diagnostic {
            unsafe {
                self.write_state(block, state);
            }
        }
    }
    #[cold]
    #[inline(never)]
    unsafe fn write_state(&self, block: *mut Header, state: u8) {
        unsafe {
            if self.with_state(block, |slot| slot.write(state)).is_none() {
                fault("block does not belong to the context");
            }
        }
    }

    /// Blocks not on a free list (live values, dead values being drained and raw tokens) and
    /// their capacity bytes, from the slabs' live counts and the large blocks.
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    #[inline(never)]
    pub(crate) fn live(&self) -> (u64, u64) {
        let (mut blocks, mut bytes) = (0, 0);
        for (class, state) in self.classes.iter().enumerate() {
            let mut slab = state.slabs;
            // SAFETY: the slab list holds this heap's live slabs.
            unsafe {
                while !slab.is_null() {
                    blocks += (*slab).live as u64;
                    bytes += (*slab).live as u64 * class_capacity(class);
                    slab = (*slab).next;
                }
            }
        }
        let mut prefix = self.large;
        // SAFETY: the large list holds this heap's large blocks.
        unsafe {
            while !prefix.is_null() {
                blocks += 1;
                bytes += (*large_block(prefix)).capacity();
                prefix = (*prefix).next;
            }
        }
        #[cfg(feature = "system-allocator")]
        for &block in &self.diag().bare {
            blocks += 1;
            // SAFETY: a listed bare block is live or raw, with its descriptor index.
            bytes += unsafe { self.capacity(block as *const Header) };
        }
        (blocks, bytes)
    }

    /// Bytes currently obtained from the backing allocator for this heap.
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) fn backing_bytes(&self) -> u64 {
        self.slab_count * SLAB_SIZE as u64 + self.large_bytes
    }

    fn charge_cpu_backing(&mut self, bytes: u64) {
        if self.meter.cpu.get().is_some() {
            self.cpu_allocation
                .as_mut()
                .expect("CPU allocation must preflight its backing")
                .consume(bytes);
        }
    }

    /// Collect returns before checking the existing page lists. The next
    /// allocation can need at most one new slab, or its one large/bare block.
    unsafe fn cpu_backing_request(&mut self, index: u32, extent: u64, align: u64) -> u64 {
        #[cfg(feature = "system-allocator")]
        {
            if let Some(capacity) = self.bare_capacity(index, extent, align) {
                return cpu_bare_charge(capacity);
            }
        }
        let _ = index;
        if cfg!(feature = "system-allocator") || extent > SMALL_MAX || align > SMALL_ALIGN_MAX {
            return cpu_large_charge(request_capacity(extent, align), align.max(8));
        }
        self.collect();
        let class = &self.classes[pool_of(extent, align)];
        let current = class.current;
        // A published return after this check can only reduce backing needed.
        if !class.available.is_null()
            || (!current.is_null()
                && unsafe {
                    !(*current).free.is_null()
                        || !(*current).local_free.is_null()
                        || (*current).bump < (*current).end
                })
        {
            0
        } else {
            cpu_slab_charge(self.diagnostic)
        }
    }

    /// T01 Alloc — fresh unpublished storage with one holder and its exact usable bytes.
    /// `extent` and `align` are validated.
    /// A bare system request is one malloc; any other system request takes the large path.
    #[cfg(feature = "system-allocator")]
    #[inline(always)]
    pub(crate) unsafe fn allocate(
        &mut self,
        index: u32,
        extent: u64,
        align: u64,
    ) -> (*mut Header, u64) {
        // SAFETY: the index is checked; fresh storage is owned here.
        unsafe {
            if let Some(capacity) = self.bare_capacity(index, extent, align) {
                let block = self.allocate_bare(index, capacity);
                self.set_state(block, STATE_LIVE);
                return (block, capacity);
            }
            let (block, capacity) = self.allocate_large(extent, align);
            (*block).count = 1;
            (*block).descriptor = index | KIND_LARGE << KIND_SHIFT;
            self.set_state(block, STATE_LIVE);
            (block, capacity)
        }
    }

    /// A bare block: one malloc of the header and `capacity`, the class capacity of a fixed
    /// layout's size. Its header is the allocation, eight-byte aligned like its payload.
    #[cfg(feature = "system-allocator")]
    #[inline(always)]
    pub(crate) unsafe fn allocate_bare(&mut self, index: u32, capacity: u64) -> *mut Header {
        self.meter
            .charge_allocation(crate::budget::small_charge(capacity));
        self.charge_cpu_backing(cpu_bare_charge(capacity));
        unsafe {
            let block = libc::malloc(HEADER_SIZE + capacity as usize).cast::<Header>();
            if block.is_null() {
                environmental("the system allocator could not supply a block");
            }
            block.write(Header {
                count: 1,
                descriptor: index | KIND_SMALL << KIND_SHIFT,
            });
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            self.list_bare(block, capacity);
            #[cfg(feature = "measurement")]
            self.measurement.allocate(capacity);
            block
        }
    }

    #[cfg(all(
        feature = "system-allocator",
        any(feature = "diagnostic", feature = "measurement")
    ))]
    #[inline(never)]
    fn list_bare(&mut self, block: *mut Header, capacity: u64) {
        self.record_backing_allocation();
        self.large_bytes += HEADER_SIZE as u64 + capacity;
        let diagnostic = self.diagnostic;
        let mut diag = self.diag();
        diag.bare.insert(block.addr());
        if diagnostic {
            diag.large_states.insert(block.addr(), STATE_FREE);
        }
    }

    /// T01 Alloc — fresh unpublished storage with one holder and its exact usable bytes.
    /// `extent` and `align` are validated.
    /// The common case pops the page-local allocation list; extension and collection are out of line.
    #[cfg(not(feature = "system-allocator"))]
    #[inline(always)]
    pub(crate) unsafe fn allocate(
        &mut self,
        index: u32,
        extent: u64,
        align: u64,
    ) -> (*mut Header, u64) {
        // SAFETY: class blocks lie in live slabs of this heap; fresh storage is owned here.
        unsafe {
            let small = extent <= SMALL_MAX && align <= SMALL_ALIGN_MAX;
            let (block, capacity) = if small {
                let class = pool_of(extent, align);
                if self.metered {
                    self.meter
                        .charge_allocation(crate::budget::small_charge(class_capacity(class)));
                }
                let block = match self.pop(class) {
                    Some(block) => block,
                    None => {
                        let block = self.refill(class);
                        self.take(slab_of(block));
                        block
                    }
                };
                (block, class_capacity(class))
            } else {
                self.allocate_large(extent, align)
            };
            #[cfg(feature = "memcheck")]
            if small {
                let bytes = HEADER_SIZE + capacity as usize;
                memcheck::pool_alloc(slab_of(block).cast(), block.cast(), bytes);
                // Diagnostic allocation checks the free poison the slab still holds.
                if self.diagnostic {
                    memcheck::defined(block.cast(), bytes);
                }
            }
            // `link` is meaningful only once dead or free; `push` and `free` set it.
            (*block).count = 1;
            (*block).descriptor = index
                | if small {
                    KIND_SMALL << KIND_SHIFT
                } else {
                    KIND_LARGE << KIND_SHIFT
                };
            self.set_state(block, STATE_LIVE);
            (block, capacity)
        }
    }

    /// Pop the current page allocation list. Frees never write this head.
    #[inline(always)]
    pub(crate) unsafe fn pop(&mut self, class: usize) -> Option<*mut Header> {
        // SAFETY: `class` is below POOL_COUNT; the list holds free blocks of the current slab.
        unsafe {
            let state = self.classes.get_unchecked_mut(class);
            let slab = state.current;
            if slab.is_null() {
                return None;
            }
            let block = (*slab).free;
            if block.is_null() {
                return None;
            }
            (*slab).free = free_next(block);
            self.take(slab);
            Some(block)
        }
    }

    /// Count a class block taken off a free list as live in its slab.
    #[inline(always)]
    unsafe fn take(&mut self, slab: *mut Slab) {
        // SAFETY: class blocks lie in live slabs of this heap.
        unsafe {
            let live = (*slab).live;
            (*slab).live = live + 1;
            // As in mimalloc's page-used accounting, allocation updates the
            // page counter. Global idle accounting changes only at zero.
            if live == 0 {
                self.view().idle_slabs -= 1;
            }
            #[cfg(feature = "measurement")]
            self.measurement
                .allocate(class_capacity((*slab).pool as usize));
        }
    }

    /// Refill one page's allocation list. Extend fresh capacity before collecting local
    /// frees, then use an available page or obtain a new page. As in mimalloc, the
    /// slow path hands a whole list to allocation, keeping free off its dependency chain.
    #[cold]
    #[inline(never)]
    unsafe fn refill(&mut self, class: usize) -> *mut Header {
        unsafe {
            self.collect();
            let mut slab = self.classes[class].current;
            if !slab.is_null() {
                // mimalloc's `_mi_page_free_collect`: the current page's published blocks
                // join its local frees before the page counts as exhausted.
                self.collect_page(slab);
                if page_refill(slab) {
                    return page_pop(slab);
                }
            }
            let state = &mut self.classes[class];
            slab = state.available;
            if slab.is_null() {
                self.new_slab(class);
                slab = self.classes[class].current;
            } else {
                state.available = (*slab).available;
                self.set_current(class, slab);
            }
            if !page_refill(slab) {
                fault("an available allocation page has no free storage");
            }
            page_pop(slab)
        }
    }

    unsafe fn new_slab(&mut self, class: usize) {
        self.charge_cpu_backing(cpu_slab_charge(self.diagnostic));
        // SAFETY: nonzero size.
        let base = unsafe { host_alloc(slab_layout()) };
        if base.is_null() {
            environmental("the backing allocator could not supply a slab");
        }
        if self.diagnostic {
            // Fresh storage is free storage: allocation checks its poison like a reused block's.
            unsafe { ptr::write_bytes(base, FREE_POISON, SLAB_SIZE) };
        }
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.record_backing_allocation();
        }
        let slab = base.cast::<Slab>();
        let stride = block_size(class);
        let align = pool_align(class);
        let first = (SLAB_HEADER + HEADER_SIZE).next_multiple_of(align) - HEADER_SIZE;
        let slots = (SLAB_SIZE - first) / (stride + 4);
        let end = base.wrapping_add(first + slots * stride);
        // Every slot and its redzone is unaddressable until allocated.
        #[cfg(feature = "memcheck")]
        {
            memcheck::noaccess(base.wrapping_add(first), slots * stride);
            memcheck::create_pool(base);
        }
        let states = if self.diagnostic {
            let data = unsafe { host_alloc(HostLayout::from_size_align_unchecked(slots, 1)) };
            if data.is_null() {
                environmental("cannot allocate diagnostic slab states");
            }
            unsafe {
                data.write_bytes(STATE_FREE, slots);
            }
            data
        } else {
            ptr::null_mut()
        };
        let next = self.classes[class].slabs;
        // SAFETY: `base` is a fresh slab.
        unsafe {
            slab.write(Slab {
                next,
                available: ptr::null_mut(),
                free: ptr::null_mut(),
                live: 0,
                doomed: 0,
                pool: class as u32,
                align: align as u32,
                first: first as u32,
                stride: stride as u32,
                slots: slots as u32,
                multiplier: (1u64 << 32).div_ceil(stride as u64) as u32,
                shadow: end.cast(),
                states,
                local_free: ptr::null_mut(),
                bump: base.wrapping_add(first),
                end,
                owner: self.id,
                _reserved: 0,
                xthread: Xthread::new(),
                queue_next: ptr::null_mut(),
            });
            if self.diagnostic {
                self.diag().slabs.insert(
                    slab.addr(),
                    SlabRecord {
                        state: STATE_LIVE,
                        first: first as u32,
                        stride: stride as u32,
                        slots: slots as u32,
                    },
                );
            }
        }
        self.classes[class].slabs = slab;
        self.set_current(class, slab);
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.slab_count += 1;
        }
        self.view().idle_slabs += 1;
    }

    #[cold]
    #[inline(never)]
    unsafe fn allocate_large(&mut self, extent: u64, align: u64) -> (*mut Header, u64) {
        let align = align.max(8);
        let offset = large_header_offset(align);
        let capacity = request_capacity(extent, align);
        let Some(total) = (offset + HEADER_SIZE as u64).checked_add(capacity) else {
            environmental("a large allocation's size overflows")
        };
        self.meter
            .charge_allocation(crate::budget::large_charge(capacity));
        self.charge_cpu_backing(cpu_large_charge(capacity, align));
        #[cfg(not(feature = "system-allocator"))]
        let Ok(host) = HostLayout::from_size_align(total as usize, align as usize) else {
            environmental("a large allocation exceeds the host address space")
        };
        // SAFETY: nonzero size.
        #[cfg(not(feature = "system-allocator"))]
        let base = unsafe { allocate_large(host) };
        // malloc's alignment is sufficient for the prefix; overallocate to align the payload
        // ourselves, retaining the original allocation for exactly one matching free.
        #[cfg(feature = "system-allocator")]
        let (base, allocation, total) = unsafe {
            let total = total
                .checked_add(align - 1)
                .unwrap_or_else(|| environmental("a system allocation's size overflows"));
            let allocation = libc::malloc(total as usize).cast::<u8>();
            let base = allocation.wrapping_add(allocation.align_offset(align as usize));
            (base, allocation, total)
        };
        if base.is_null() {
            environmental("the backing allocator could not supply a large block");
        }
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.record_backing_allocation();
        }
        let prefix = base.cast::<LargePrefix>();
        let block = base.wrapping_add(offset as usize).cast::<Header>();
        // SAFETY: fresh storage; the list holds this heap's large blocks.
        unsafe {
            prefix.write(LargePrefix {
                prev: ptr::null_mut(),
                next: self.large,
                total,
                align,
                capacity,
                shadow: 0,
                owner: self.id,
                return_next: ptr::null_mut(),
                #[cfg(feature = "system-allocator")]
                allocation,
            });
            if !self.large.is_null() {
                (*self.large).prev = prefix;
            }
            block
                .cast::<u8>()
                .sub(8)
                .cast::<*mut LargePrefix>()
                .write(prefix);
            (*block).descriptor = KIND_LARGE << KIND_SHIFT;
        }
        if self.diagnostic {
            self.diag().large_states.insert(block.addr(), STATE_FREE);
        }
        self.large = prefix;
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.large_bytes += total;
        }
        #[cfg(feature = "measurement")]
        self.measurement.allocate(capacity);
        (block, capacity)
    }

    /// T16 Free. Diagnostic preparation is cold; the production small path calls nothing.
    #[inline(always)]
    pub(crate) unsafe fn free(&mut self, block: *mut Header) {
        unsafe {
            if self.diagnostic {
                self.prepare_free_diagnostic(block);
            }
            match (*block).kind() {
                KIND_SMALL => self.free_small(block),
                KIND_LARGE => self.free_large(block),
                _ => fault("an immortal block reached heap free"),
            }
        }
    }
    #[cold]
    #[inline(never)]
    unsafe fn prepare_free_diagnostic(&self, block: *mut Header) {
        unsafe {
            if self.state(block) == STATE_FREE || (*block).domain() != DOMAIN_HEAP {
                fault("a block is freed twice or is not a heap block");
            }
            // A raw token's payload is unaddressable until its reuse or dispose.
            #[cfg(feature = "memcheck")]
            memcheck::undefined(payload(block), self.capacity(block) as usize);
            ptr::write_bytes(payload(block), FREE_POISON, self.capacity(block) as usize);
            self.write_state(block, STATE_FREE);
            #[cfg(feature = "diagnostic")]
            crate::root_trace::retired(block);
        }
    }
    /// A bare system block's free is its one matching `free`; nothing lists it in production.
    #[cfg(feature = "system-allocator")]
    #[inline(always)]
    pub(crate) unsafe fn free_small(&mut self, block: *mut Header) {
        unsafe {
            if let Some(cpu) = self.meter.cpu.get() {
                cpu.credit_heap(cpu_bare_charge(self.capacity(block)));
            }
            self.credit(block);
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            self.unlist_bare(block);
            libc::free(block.cast());
        }
    }

    #[cfg(all(
        feature = "system-allocator",
        any(feature = "diagnostic", feature = "measurement")
    ))]
    #[inline(never)]
    unsafe fn unlist_bare(&mut self, block: *mut Header) {
        // A bare block freed on another heap is listed by the heap that took it.
        let listed = self.related_heaps().any(|heap| {
            // SAFETY: the run's heaps are live.
            unsafe { (*heap).diag().bare.remove(&block.addr()) }
        });
        if !listed {
            fault("a bare block is not listed by its heap");
        }
        // SAFETY: the block is live or raw, with its descriptor index.
        let capacity = unsafe { self.capacity(block) };
        self.large_bytes -= HEADER_SIZE as u64 + capacity;
        #[cfg(feature = "measurement")]
        self.measurement.free(capacity);
    }

    #[cfg(not(feature = "system-allocator"))]
    #[inline(always)]
    pub(crate) unsafe fn free_small(&mut self, block: *mut Header) {
        unsafe {
            self.credit(block);
            let slab = slab_of(block);
            if (*slab).owner != self.id {
                return self.free_remote(slab, block);
            }
            #[cfg(feature = "measurement")]
            self.measurement.free((*block).capacity());
            #[cfg(feature = "memcheck")]
            memcheck::pool_free(slab.cast(), block.cast());
            self.free_local(slab, block);
        }
    }

    /// Put the dead `block` on its slab's `local_free` list: a free on this heap, or a block
    /// another heap published that this heap's collector took.
    #[cfg(not(feature = "system-allocator"))]
    #[inline(always)]
    unsafe fn free_local(&mut self, slab: *mut Slab, block: *mut Header) {
        unsafe {
            let head = (*slab).local_free;
            write_free(block, head);
            (*slab).local_free = block;
            (*slab).live -= 1;
            if (*slab).live == 0 {
                self.view().idle_slabs += 1;
            }
            if head.is_null() {
                let state = self.classes.get_unchecked_mut((*slab).pool as usize);
                if slab == state.current {
                    return;
                }
                (*slab).available = state.available;
                state.available = slab;
            }
        }
    }

    /// Free a block of another heap's slab: publish it to the owner (`crate::remote`).
    #[cfg(not(feature = "system-allocator"))]
    #[cold]
    #[inline(never)]
    unsafe fn free_remote(&mut self, slab: *mut Slab, block: *mut Header) {
        unsafe {
            #[cfg(feature = "measurement")]
            {
                self.measurement.free((*block).capacity());
                self.measurement.remote_frees += 1;
            }
            #[cfg(feature = "memcheck")]
            memcheck::pool_free(slab.cast(), block.cast());
            let owner = self.heap_of((*slab).owner);
            remote::free_remote(&(*owner).queue, slab, block);
        }
    }

    /// Collect what other heaps published to this one: blocks of its slabs, its large blocks
    /// and its region segments. Only this heap's collector calls it (`crate::remote::Owner`).
    pub(crate) fn collect(&mut self) {
        #[cfg(not(feature = "system-allocator"))]
        if !self.queue.is_empty() {
            let queue: *const Returns<Slab> = &self.queue;
            // SAFETY: this is the heap's collector, and the queue lives as long as the heap.
            let _collected =
                unsafe { remote::collect(&*queue, |slab, block| self.free_local(slab, block)) };
            #[cfg(feature = "measurement")]
            {
                self.measurement.remote_collected += _collected;
            }
        }
        if !self.large_returns.is_empty() {
            let mut prefix = self.large_returns.take();
            while !prefix.is_null() {
                // SAFETY: a returned large block is this heap's, freed once by its releaser.
                unsafe {
                    let next = LargePrefix::next(prefix);
                    self.release_large_block(prefix);
                    prefix = next;
                }
                #[cfg(feature = "measurement")]
                {
                    self.measurement.remote_collected += 1;
                }
            }
        }
        if !self.regions.returns.is_empty() {
            let _collected = self.regions.collect();
            #[cfg(feature = "measurement")]
            {
                self.measurement.remote_collected += _collected;
            }
        }
    }

    /// The direct collection of `slab`, a page this heap allocates from: its published blocks
    /// join its local frees. Its queued flag stays for the queue's collection to clear.
    #[cfg(not(feature = "system-allocator"))]
    unsafe fn collect_page(&mut self, slab: *mut Slab) {
        unsafe {
            let chain = (*slab).xthread.take();
            if !chain.is_null() {
                let _collected =
                    remote::drain(slab, chain, &mut |slab, block| self.free_local(slab, block));
                #[cfg(feature = "measurement")]
                {
                    self.measurement.remote_collected += _collected;
                }
            }
        }
    }

    #[cfg(feature = "system-allocator")]
    unsafe fn collect_page(&mut self, _slab: *mut Slab) {}

    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn free_large(&mut self, block: *mut Header) {
        // SAFETY: `block` is a large block; its prefix precedes it.
        unsafe {
            self.credit(block);
            let prefix = (*block).large_prefix();
            #[cfg(feature = "measurement")]
            self.measurement.free((*prefix).capacity);
            if (*prefix).owner != self.id {
                // Another heap's: publish it to its owner's return list.
                #[cfg(feature = "measurement")]
                {
                    self.measurement.remote_frees += 1;
                }
                (*self.heap_of((*prefix).owner)).large_returns.push(prefix);
                return;
            }
            self.release_large_block(prefix);
        }
    }

    /// Unlink this heap's large block behind `prefix` and give its storage back.
    unsafe fn release_large_block(&mut self, prefix: *mut LargePrefix) {
        // SAFETY: `prefix` is a large block of this heap.
        unsafe {
            let align = (*prefix).align;
            let total = (*prefix).total;
            if (*prefix).prev.is_null() {
                self.large = (*prefix).next;
            } else {
                (*(*prefix).prev).next = (*prefix).next;
            }
            if !(*prefix).next.is_null() {
                (*(*prefix).next).prev = (*prefix).prev;
            }
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            {
                self.large_bytes -= total;
            }
            if let Some(cpu) = self.meter.cpu.get() {
                cpu.credit_heap(cpu_large_charge((*prefix).capacity, align));
            }
            release_large(prefix, total, align);
        }
    }

    /// Return idle slabs above the idle cap to the backing allocator. Free blocks of returned
    /// slabs leave their class free lists; no other free block moves.
    pub(crate) fn trim(&mut self) {
        self.collect();
        let keep = self.idle_cap / SLAB_SIZE as u64;
        let idle_slabs = self.view().idle_slabs;
        if idle_slabs <= keep {
            return;
        }
        let mut excess = idle_slabs - keep;
        for class in 0..POOL_COUNT {
            if excess == 0 {
                break;
            }
            let mut doomed = 0;
            let mut slab = self.classes[class].slabs;
            // SAFETY: the slab list holds this heap's live slabs.
            unsafe {
                while !slab.is_null() && excess > 0 {
                    // A slab still queued after the collection above waits for the next one.
                    if (*slab).live == 0 && !(*slab).xthread.busy() {
                        (*slab).doomed = 1;
                        doomed += 1;
                        excess -= 1;
                    }
                    slab = (*slab).next;
                }
            }
            if doomed > 0 {
                unsafe { self.release_doomed(class) };
            }
        }
    }

    unsafe fn release_doomed(&mut self, class: usize) {
        let state = &mut self.classes[class];
        // SAFETY: the class's lists hold its slabs; doomed slabs hold no live block, and the class
        // list holds only the current slab's blocks.
        unsafe {
            if !state.current.is_null() && (*state.current).doomed != 0 {
                self.set_current(class, ptr::null_mut());
            }
            let state = &mut self.classes[class];
            let mut link: *mut *mut Slab = &mut state.available;
            while !(*link).is_null() {
                let slab = *link;
                if (*slab).doomed != 0 {
                    *link = (*slab).available;
                } else {
                    link = &mut (*slab).available;
                }
            }
            let mut link: *mut *mut Slab = &mut state.slabs;
            while !(*link).is_null() {
                let slab = *link;
                if (*slab).doomed != 0 {
                    *link = (*slab).next;
                    self.retire_record(slab);
                    if let Some(cpu) = self.meter.cpu.get() {
                        cpu.credit_heap(cpu_slab_charge(self.diagnostic));
                    }
                    release_slab(slab);
                    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
                    {
                        self.slab_count -= 1;
                    }
                    self.view().idle_slabs -= 1;
                } else {
                    link = &mut (*slab).next;
                }
            }
        }
    }

    /// A slab goes back: its diagnostic record now reads every block as free.
    fn retire_record(&self, slab: *mut Slab) {
        if self.diagnostic
            && let Some(record) = self.diag().slabs.get_mut(&slab.addr())
        {
            record.state = STATE_FREE;
        }
    }

    /// Task exit: return every slab and large block, live or not, and every listed bare block.
    /// Storage other heaps published back is still on this heap's lists, so the published
    /// lists are only forgotten.
    pub(crate) fn release_all(&mut self) {
        self.cpu_allocation = None;
        self.queue.take();
        self.large_returns.take();
        #[cfg(all(
            feature = "system-allocator",
            any(feature = "diagnostic", feature = "measurement")
        ))]
        {
            let bare = std::mem::take(&mut self.diag().bare);
            for block in bare {
                let block = block as *mut Header;
                // SAFETY: each listed bare block is this heap's, live or raw, and is freed once.
                unsafe {
                    if let Some(state) = self.diag().large_states.get_mut(&block.addr()) {
                        *state = STATE_FREE;
                    }
                    #[cfg(feature = "measurement")]
                    self.measurement.free(self.capacity(block));
                    if let Some(cpu) = self.meter.cpu.get() {
                        cpu.credit_heap(cpu_bare_charge(self.capacity(block)));
                    }
                    libc::free(block.cast());
                }
            }
        }
        for class in 0..POOL_COUNT {
            let mut slab = self.classes[class].slabs;
            while !slab.is_null() {
                // SAFETY: each slab is this heap's and is freed once.
                unsafe {
                    let next = (*slab).next;
                    #[cfg(feature = "measurement")]
                    self.measurement
                        .free_many(class_capacity(class), (*slab).live as u64);
                    self.retire_record(slab);
                    if let Some(cpu) = self.meter.cpu.get() {
                        cpu.credit_heap(cpu_slab_charge(self.diagnostic));
                    }
                    release_slab(slab);
                    slab = next;
                }
            }
            self.classes[class] = EMPTY_CLASS;
            if !self.compiled.pages[class].is_null() {
                self.set_page(class, ptr::null_mut());
            }
        }
        let mut prefix = self.large;
        while !prefix.is_null() {
            // SAFETY: each large block is this heap's and is freed once.
            unsafe {
                let next = (*prefix).next;
                if self.diagnostic
                    && let Some(state) = self
                        .diag()
                        .large_states
                        .get_mut(&large_block(prefix).addr())
                {
                    *state = STATE_FREE;
                }
                #[cfg(feature = "measurement")]
                self.measurement.free((*prefix).capacity);
                if let Some(cpu) = self.meter.cpu.get() {
                    cpu.credit_heap(cpu_large_charge((*prefix).capacity, (*prefix).align));
                }
                release_large(prefix, (*prefix).total, (*prefix).align);
                prefix = next;
            }
        }
        self.large = ptr::null_mut();
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.slab_count = 0;
        }
        self.view().idle_slabs = 0;
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.large_bytes = 0;
        }
    }
}

unsafe fn release_large(prefix: *mut LargePrefix, _total: u64, _align: u64) {
    #[cfg(feature = "system-allocator")]
    unsafe {
        libc::free((*prefix).allocation.cast());
    }
    #[cfg(not(feature = "system-allocator"))]
    unsafe {
        #[cfg(target_os = "linux")]
        if mapped_large(_total, _align) {
            if libc::munmap(prefix.cast(), _total as usize) != 0 {
                fault("cannot unmap a large block");
            }
            return;
        }
        host_dealloc(
            prefix.cast(),
            HostLayout::from_size_align_unchecked(_total as usize, _align as usize),
        );
    }
}

/// mimalloc's sequential `mi_page_free_list_extend`, restricted to one task's page.
/// At most 8 KiB is touched per extension, matching MI_MAX_EXTEND_SIZE. Untouched
/// capacity precedes reuse; unlike mimalloc we need no thread-free or secure shuffle.
unsafe fn page_refill(slab: *mut Slab) -> bool {
    unsafe {
        debug_assert!((*slab).free.is_null());
        let stride = (*slab).stride as usize;
        let remaining = ((*slab).end as usize - (*slab).bump as usize) / stride;
        if remaining > 0 {
            let extend = remaining.min((8 * 1024 / stride).max(1));
            let first = (*slab).bump.cast::<Header>();
            let mut block = first;
            for _ in 1..extend {
                let next = block.cast::<u8>().add(stride).cast::<Header>();
                write_free(block, next);
                block = next;
            }
            write_free(block, ptr::null_mut());
            (*slab).bump = (*slab).bump.add(extend * stride);
            (*slab).free = first;
        } else {
            (*slab).free = (*slab).local_free;
            (*slab).local_free = ptr::null_mut();
        }
        !(*slab).free.is_null()
    }
}

/// The page has a nonempty allocation list.
#[inline]
unsafe fn page_pop(slab: *mut Slab) -> *mut Header {
    unsafe {
        let block = (*slab).free;
        (*slab).free = free_next(block);
        block
    }
}

unsafe fn release_slab(slab: *mut Slab) {
    #[cfg(feature = "memcheck")]
    memcheck::destroy_pool(slab.cast());
    unsafe {
        if !(*slab).states.is_null() {
            host_dealloc(
                (*slab).states,
                HostLayout::from_size_align_unchecked((*slab).slots as usize, 1),
            );
        }
        // mimalloc src/prim/unix/prim.c:549-558 (31d034d94cdb) uses
        // MADV_DONTNEED when decommitting to lower RSS immediately. libc free
        // may retain these aligned 64 KiB allocations in its arena. All bytes
        // here belong to the retired slab; allocator metadata is outside it.
        // Do this after the last metadata read and before returning the backing.
        #[cfg(target_os = "linux")]
        libc::madvise(slab.cast(), SLAB_SIZE, libc::MADV_DONTNEED);
        host_dealloc(slab.cast(), slab_layout());
    }
}

/// Fault unless a fresh block's free storage kept its poison. Only slab blocks are reused
/// storage; large and bare blocks come fresh from the backing allocator.
unsafe fn check_poison(block: *mut Header) {
    if cfg!(feature = "system-allocator") || unsafe { (*block).kind() } != KIND_SMALL {
        return;
    }
    // SAFETY: the block's payload is owned free storage of its capacity.
    let bytes = unsafe { std::slice::from_raw_parts(payload(block), (*block).capacity() as usize) };
    if bytes.iter().any(|&byte| byte != FREE_POISON) {
        fault("freed storage was written after free (poison damaged)");
    }
}

/// Validate a compiled extent and alignment before converting them to host sizes.
#[inline]
pub(crate) fn check_request(extent: u64, align: u64) {
    if align == 0 || !align.is_power_of_two() || align > (1 << 30) {
        fault("an allocation alignment is not a power of two in range");
    }
    if extent > EXTENT_MAX {
        environmental("an allocation extent exceeds the host address space");
    }
}

/// T01 Alloc. Size-class allocation for small heap blocks, backing allocation otherwise; returns
/// the new block, which has one holder and is unpublished. `destination` must be
/// [`DOMAIN_HEAP`]; region storage comes from `hirsel_rt_v2_region_reserve`. `site` is reserved
/// for future bounded arena charging and is not interpreted. It cannot fail: host exhaustion
/// terminates environmentally, which no program observes (runtime ABI 3).
///
/// # Safety
/// `ctx` is a live run context and `layout` a valid descriptor whose size is at most `extent`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_alloc(
    ctx: *mut TaskCtx,
    index: u32,
    extent: u64,
    align: u64,
    destination: u8,
    site: u64,
) -> *mut Header {
    unsafe {
        allocation_preflight(ctx, index, extent, align);
        let block = alloc_prepared(ctx, index, extent, align, destination, site);
        (*ctx).heap().cpu_allocation = None;
        block
    }
}

pub(crate) unsafe fn allocation_preflight(ctx: *mut TaskCtx, index: u32, extent: u64, align: u64) {
    unsafe { ordered_preflight(ctx, index, extent, align) };
    // No heap reference survives refusal handling. CPU ahead work can use
    // this same preflight result to save/withdraw instead of selecting a stop.
    if unsafe { cpu_preflight(ctx, index, extent, align) }.is_err() {
        crate::budget::exceeded(hirsel_run_schema::BudgetKind::Memory);
    }
}

/// Internal allocating operations use the same admission as T01. No caller
/// may hold a heap borrow across this call, which can settle Ordered work.
pub(crate) unsafe fn allocate_heap(
    ctx: *mut TaskCtx,
    index: u32,
    extent: u64,
    align: u64,
) -> (*mut Header, u64) {
    unsafe {
        allocation_preflight(ctx, index, extent, align);
        let heap = (*ctx).heap();
        let result = heap.allocate(index, extent, align);
        heap.cpu_allocation = None;
        result
    }
}

pub(crate) unsafe fn alloc_prepared(
    ctx: *mut TaskCtx,
    index: u32,
    extent: u64,
    align: u64,
    destination: u8,
    site: u64,
) -> *mut Header {
    unsafe {
        let heap = &mut (*ctx).heap();
        // A bare system block needs its layout's bare entry, so the index is checked.
        #[cfg(feature = "system-allocator")]
        if !heap.diagnostic
            && destination == DOMAIN_HEAP
            && let Some(capacity) = heap.bare_capacity(index, extent, align)
        {
            return heap.allocate_bare(index, capacity);
        }
        // The unsafe ABI guarantees a linked descriptor index. Production allocation only
        // installs that index; resolution and full validation belong to the cold path/drain.
        if cfg!(feature = "system-allocator")
            || cfg!(feature = "memcheck")
            || heap.diagnostic
            || heap.metered
            || extent > SMALL_MAX
            || align.wrapping_sub(1) >= SMALL_ALIGN_MAX
            || align & (align - 1) != 0
            || destination != DOMAIN_HEAP
        {
            return alloc_checked(ctx, index, extent, align, destination, site);
        }
        let pool = pool_of(extent, align);
        let Some(block) = heap.pop(pool) else {
            return alloc_refilled(heap, pool, index);
        };
        block.write(Header {
            count: 1,
            descriptor: index,
        });
        block
    }
}

/// Refusal is physical only. The caller handles it after this short heap
/// borrow ends, before entering the allocation path or retaining heap pointers.
pub(crate) unsafe fn cpu_preflight(
    ctx: *mut TaskCtx,
    index: u32,
    extent: u64,
    align: u64,
) -> Result<(), crate::pool::cpu_scoped::ledger::BackingFull> {
    let heap = unsafe { (*ctx).heap() };
    if heap.meter.cpu.get().is_none() {
        return Ok(());
    }
    check_request(extent, align);
    assert!(
        heap.cpu_allocation.is_none(),
        "one CPU allocation preflight"
    );
    let bytes = unsafe { heap.cpu_backing_request(index, extent, align) };
    if bytes != 0 {
        heap.cpu_allocation = Some(heap.meter.cpu.get().unwrap().reserve_heap(bytes)?);
    }
    Ok(())
}

/// A prepared idle-side CPU request obtained its backing. Return unused
/// reservation capacity before the task can save or migrate again.
pub(crate) unsafe fn cpu_allocation_done(ctx: *mut TaskCtx) {
    unsafe { (*ctx).heap().cpu_allocation = None };
}

/// Reserve ordered speculative storage before borrowing a worker heap. The
/// charge mirrors Heap::allocate, including the optional bare-system path.
/// No worker reference may survive this call, since it may migrate the task.
pub(crate) unsafe fn ordered_preflight(ctx: *mut TaskCtx, _index: u32, extent: u64, align: u64) {
    if !unsafe {
        (*ctx)
            .run_ref()
            .ordered_memory
            .load(std::sync::atomic::Ordering::Acquire)
    } && unsafe {
        (*ctx)
            .run_ref()
            .ordered_allowance
            .load(std::sync::atomic::Ordering::Acquire)
    } == u64::MAX
    {
        return;
    }
    check_request(extent, align);
    let bytes = unsafe { allocation_charge(ctx, _index, extent, align) };
    unsafe { ordered_reserve(ctx, bytes) };
}

pub(crate) unsafe fn allocation_charge(
    ctx: *mut TaskCtx,
    _index: u32,
    extent: u64,
    align: u64,
) -> u64 {
    let _ = ctx;
    {
        #[cfg(feature = "system-allocator")]
        let bare = unsafe { (*ctx).heap_ref().bare_capacity(_index, extent, align) };
        #[cfg(not(feature = "system-allocator"))]
        let bare: Option<u64> = None;
        if let Some(capacity) = bare {
            crate::budget::small_charge(capacity)
        } else if !cfg!(feature = "system-allocator")
            && extent <= SMALL_MAX
            && align <= SMALL_ALIGN_MAX
        {
            crate::budget::small_charge(class_capacity(pool_of(extent, align)))
        } else {
            crate::budget::large_charge(request_capacity(extent, align))
        }
    }
}

pub(crate) unsafe fn ordered_reserve(ctx: *mut TaskCtx, bytes: u64) {
    if !unsafe {
        (*ctx)
            .run_ref()
            .ordered_memory
            .load(std::sync::atomic::Ordering::Acquire)
    } && unsafe {
        (*ctx)
            .run_ref()
            .ordered_allowance
            .load(std::sync::atomic::Ordering::Acquire)
    } == u64::MAX
    {
        return;
    }
    let result = {
        let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
        crate::tasks::ordered::engine::reserve_allocation(ctx, bytes)
    };
    unsafe { crate::stop::check(ctx, result) };
}

#[cold]
#[inline(never)]
unsafe fn alloc_refilled(heap: &mut Heap, pool: usize, index: u32) -> *mut Header {
    unsafe {
        let block = heap.refill(pool);
        heap.take(slab_of(block));
        block.write(Header {
            count: 1,
            descriptor: index,
        });
        block
    }
}

#[cold]
#[inline(never)]
unsafe fn alloc_checked(
    ctx: *mut TaskCtx,
    index: u32,
    extent: u64,
    align: u64,
    destination: u8,
    _site: u64,
) -> *mut Header {
    unsafe {
        check_request(extent, align);
        if destination != DOMAIN_HEAP {
            fault("alloc destination is not the task heap");
        }
        let ctx = &mut *ctx;
        let layout = ctx.descriptor(index);
        if ctx.heap().diagnostic && (*layout).size > extent {
            fault("an allocation extent is smaller than its layout size");
        }
        let (block, capacity) = ctx.heap().allocate(index, extent, align);
        if ctx.heap_ref().diagnostic {
            ctx.run_ref()
                .region_owners
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&block.addr());
        }
        if ctx.heap().diagnostic {
            check_poison(block);
            ptr::write_bytes(payload(block), UNINIT_POISON, capacity as usize);
        }
        block
    }
}

/// T14 Dispose. Shallow-free a reuse token's raw storage exactly once; null (None) does nothing.
/// Never scans the stale payload or runs a finalizer.
///
/// # Safety
/// `token` is null or a token produced by `hirsel_rt_v2_reset` and not yet consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_dispose(ctx: *mut TaskCtx, token: *mut Header) {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if token.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    // SAFETY: caller contract.
    unsafe {
        if ctx.heap().diagnostic && ctx.heap().state(token) != STATE_RAW {
            fault("dispose of storage that is not a raw reuse token");
        }
        ctx.heap().free(token);
    }
}

/// T13 Reuse. Install a new value in a token's raw storage, or allocate when the token is null.
/// The token's key must match the request exactly: the class capacity for `extent` (or the
/// rounded large capacity), the physical alignment and the heap domain.
///
/// # Safety
/// As `hirsel_rt_v2_alloc`; `token` is null or an unconsumed reuse token.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_reuse(
    ctx: *mut TaskCtx,
    token: *mut Header,
    index: u32,
    extent: u64,
    align: u64,
    out_block: *mut *mut Header,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if token.is_null() {
        #[cfg(feature = "measurement")]
        unsafe {
            (*ctx).heap().measurement.record("reuse_misses", 1);
            (&mut (*ctx).heap().measurement).reuse_misses += 1;
        }
        unsafe {
            out_block.write(hirsel_rt_v2_alloc(
                ctx,
                index,
                extent,
                align,
                DOMAIN_HEAP,
                0,
            ))
        };
        return STATUS_OK;
    }
    let ctx = unsafe { &mut *ctx };
    let layout = unsafe { ctx.descriptor(index) };
    check_request(extent, align);
    let (capacity, physical_align) = (request_capacity(extent, align), align.max(8));
    // SAFETY: caller contract.
    unsafe {
        // The payload keeps the donor's bytes (`Reuse.kept` below), whose definedness the reset
        // forgot.
        #[cfg(feature = "memcheck")]
        memcheck::defined(payload(token), ctx.heap().capacity(token) as usize);
        if ctx.heap().diagnostic {
            check_layout(layout);
            if ctx.heap().state(token) != STATE_RAW {
                fault("reuse of storage that is not a raw reuse token");
            }
            if (*layout).size > extent {
                fault("a reuse extent is smaller than its layout size");
            }
        }
        if (*token).domain() != DOMAIN_HEAP
            || ctx.heap().usable(token) != (capacity, physical_align)
        {
            fault("a reuse token's (capacity, alignment, domain) key does not match the request");
        }
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("reuse_hits", 1);
            ctx.heap().measurement.reuse_hits += 1;
        }
        // A bare token's capacity is its layout's, so a request that cannot be bare moves the
        // token's bytes into a prefixed block of the same capacity.
        #[cfg(feature = "system-allocator")]
        if (*token).kind() == KIND_SMALL && ctx.heap().bare_capacity(index, extent, align).is_none()
        {
            let (block, _) = allocate_heap(ctx, index, extent, align);
            ptr::copy_nonoverlapping(payload(token), payload(block), capacity as usize);
            ctx.heap().free(token);
            out_block.write(block);
            return STATUS_OK;
        }
        (*token).count = 1;
        (*token).descriptor = index | (*token).kind() << KIND_SHIFT;
        ctx.heap().set_state(token, STATE_LIVE);
        // The payload keeps its bytes: a reuse may skip the stores of fields whose bits the
        // donor's moved-out children left in place (LIR schema revision 5, `Reuse.kept`).
        out_block.write(token);
    }
    STATUS_OK
}

/// Sequence growth (LIR schema revision 7, `Inst::SeqReserve`). Make the sequence block `block`
/// hold at least `capacity` elements: a u64 count at payload offset 0, the u64 capacity word at
/// 8, then elements `element_size` bytes apart from the first offset aligned for them
/// (runtime ABI 5). A unique heap block that already holds them is left alone. Otherwise the
/// capacity doubles or meets the request, whichever is larger, within the host extent bound.
/// The count and counted elements move bitwise into fresh storage of the class that fits
/// (so the new capacity is the class's), the old storage is freed shallowly, never scanned, and
/// returns the new block. Every path leaves the returned block's capacity word exact: every
/// slot its storage holds.
///
/// A block that is not unique (shared, immortal, or static such as the empty list's) is
/// detached once at the requested capacity, so compiled code does not detach it first: its
/// count and elements are copied into a fresh unique block of the class that fits `capacity`
/// (or the count, if larger), each counted element gains a holder, and the caller's credit of
/// the original is given back (none to give for an immortal block). A dead block and a size
/// that overflows are invariant faults; host exhaustion terminates environmentally.
///
/// A tracked arena block (MEM-6, runtime ABI 11) is returned unchanged: `list.reserve` is only
/// a hint there, and its pushes make their own room (`hirsel_rt_v2_arena_push`).
///
/// # Safety
/// `ctx` is a live run context and `block` is a live sequence block whose
/// payload layout has this element size and alignment, of which the caller holds one credit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_seq_grow(
    ctx: *mut TaskCtx,
    block: *mut Header,
    element_size: u64,
    element_align: u64,
    capacity: u64,
) -> *mut Header {
    // SAFETY: caller contract.
    unsafe {
        if crate::region::is_tracked(block) {
            return block;
        }
        if (*block).count != 1 || (*block).domain() != DOMAIN_HEAP {
            return detach_reserved(ctx, block, element_size, element_align, capacity);
        }
        unique_heap_block(ctx, block, element_align);
        let elements = sequence_elements_offset(element_align);
        // A block that already holds them is left alone, its capacity word made exact.
        // Zero-sized elements need no storage: every capacity is already held.
        let slots = sequence_slots((*ctx).heap_ref(), block, elements, element_size);
        if slots >= capacity {
            set_sequence_capacity(block, slots);
            return block;
        }
        let Some(extent) = capacity
            .checked_mul(element_size)
            .and_then(|bytes| bytes.checked_add(elements))
        else {
            fault("a sequence capacity overflows its extent");
        };
        // Rust RawVec::grow_amortized: an incremental reserve must not copy
        // the entire sequence on every append. Keep the requested extent if
        // doubling would exceed the host bound; tracked/shared blocks took
        // their existing paths above.
        let geometric = slots
            .checked_mul(2)
            .and_then(|slots| slots.checked_mul(element_size))
            .and_then(|bytes| bytes.checked_add(elements))
            .filter(|extent| *extent <= EXTENT_MAX)
            .unwrap_or(extent);
        let extent = extent.max(geometric);
        let count = payload(block).cast::<u64>().read();
        let used = elements + count * element_size;
        let (grown, bytes) = grow(ctx, block, used, extent, element_align);
        // The validated request includes the prefix, and the chosen class holds the request.
        set_sequence_capacity(grown, slots_in_bytes(bytes - elements, element_size));
        grown
    }
}

/// Block growth (LIR schema revision 19, `Inst::SeqAppend`). Make the unique heap block, or
/// unshared tracked arena block (runtime ABI 15), `block` hold at least `extent` payload bytes,
/// aligned to `align`; a tracked one grows in its arena. A block that already
/// holds them is left alone. Otherwise its first `keep` payload bytes move bitwise into fresh
/// storage of the class that fits, the old storage is freed shallowly, never scanned, and the
/// new block is returned. Faults and exhaustion are as for [`hirsel_rt_v2_seq_grow`].
///
/// # Safety
/// `ctx` is a live run context and `block` is a live block whose payload layout has this
/// alignment and whose first `keep` payload bytes are initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_block_grow(
    ctx: *mut TaskCtx,
    block: *mut Header,
    keep: u64,
    extent: u64,
    align: u64,
) -> *mut Header {
    // SAFETY: caller contract.
    unsafe {
        // An unshared tracked arena block (MEM-6, runtime ABI 15) grows in its arena,
        // uncharged; its abandoned storage is emptied, holding no reference.
        if crate::region::is_tracked(block) && (*block).count == crate::COUNT_ARENA_UNIQUE {
            if align == 0 || !align.is_power_of_two() {
                fault("a sequence element alignment is not a power of two");
            }
            let (capacity, _) = (*ctx).heap().usable(block);
            if capacity >= extent {
                return block;
            }
            if keep > capacity || keep > extent {
                fault("a sequence count exceeds its storage");
            }
            let grown = crate::region::hirsel_rt_v2_arena_regrow(ctx, block, extent, align.max(8));
            ptr::copy_nonoverlapping(payload(block), payload(grown), keep as usize);
            payload(block).cast::<u64>().write(0);
            return grown;
        }
        unique_heap_block(ctx, block, align);
        if (*ctx).heap().capacity(block) >= extent {
            return block;
        }
        grow(ctx, block, keep, extent, align).0
    }
}

/// `hirsel_rt_v2_seq_grow` of a block that is not unique: a detach at the requested capacity.
///
/// # Safety
/// As `hirsel_rt_v2_seq_grow`.
unsafe fn detach_reserved(
    ctx: *mut TaskCtx,
    block: *mut Header,
    element_size: u64,
    element_align: u64,
    capacity: u64,
) -> *mut Header {
    // SAFETY: caller contract.
    unsafe {
        let heap = (*ctx).heap_ref();
        if (*block).count == 0
            || ((*block).domain() == DOMAIN_HEAP
                && heap.diagnostic
                && heap.state(block) != STATE_LIVE)
        {
            fault("seq_reserve of a block that is not live");
        }
        if element_align == 0 || !element_align.is_power_of_two() {
            fault("a sequence element alignment is not a power of two");
        }
        let elements = sequence_elements_offset(element_align);
        let count = payload(block).cast::<u64>().read();
        if count == 0 && (*block).domain() == DOMAIN_STATIC {
            // An empty literal has no existing elements, holders or heap credit. Size
            // its fresh storage directly; neither an old-capacity lookup nor a copy
            // walk is needed. Keep the same checked request and exact class capacity.
            let extent = capacity
                .checked_mul(element_size)
                .and_then(|bytes| bytes.checked_add(elements))
                .unwrap_or_else(|| fault("a sequence capacity overflows its extent"));
            let align = element_align.max(8);
            check_request(extent, align);
            let (fresh, bytes) = allocate_heap(ctx, (*block).index(), extent, align);
            if (*ctx).heap().diagnostic {
                ptr::write_bytes(payload(fresh), UNINIT_POISON, bytes as usize);
            }
            payload(fresh).cast::<u64>().write(0);
            set_sequence_capacity(fresh, slots_in_bytes(bytes - elements, element_size));
            return fresh;
        }
        let used = count
            .checked_mul(element_size)
            .and_then(|bytes| bytes.checked_add(elements))
            .filter(|&used| used <= heap.capacity(block))
            .unwrap_or_else(|| fault("a sequence count exceeds its storage"));
        let Some(extent) = capacity
            .max(count)
            .checked_mul(element_size)
            .and_then(|bytes| bytes.checked_add(elements))
        else {
            fault("a sequence capacity overflows its extent");
        };
        let align = element_align.max(8);
        check_request(extent, align);
        let (detached, bytes) = crate::copy::detach_sized(ctx, block, used, extent, align);
        // The validated request includes the prefix, and the chosen class holds the request.
        set_sequence_capacity(detached, slots_in_bytes(bytes - elements, element_size));
        detached
    }
}

/// Fault unless `block` is a live, unique heap block and `align` a power of two.
unsafe fn unique_heap_block(ctx: *mut TaskCtx, block: *mut Header, align: u64) {
    // SAFETY: the caller's block is a live or freed header.
    unsafe {
        let heap = (*ctx).heap_ref();
        if (heap.diagnostic && heap.state(block) != STATE_LIVE)
            || (*block).domain() != DOMAIN_HEAP
            || (*block).count != 1
        {
            fault("seq_reserve of a block that is not a live, unique heap block");
        }
        if align == 0 || !align.is_power_of_two() {
            fault("a sequence element alignment is not a power of two");
        }
    }
}

/// Move the first `keep` payload bytes of `old` into fresh storage holding `extent` bytes,
/// returning the block and its exact usable bytes from the allocation's chosen class.
unsafe fn grow(
    ctx: *mut TaskCtx,
    old: *mut Header,
    keep: u64,
    extent: u64,
    align: u64,
) -> (*mut Header, u64) {
    // SAFETY: `old` is a live unique heap block (`unique_heap_block`).
    unsafe {
        let align = align.max(8);
        check_request(extent, align);
        if keep > (*ctx).heap_ref().capacity(old) || keep > extent {
            fault("a sequence count exceeds its storage");
        }
        let (new, bytes) = allocate_heap(ctx, (*old).index(), extent, align);
        let heap = &mut (*ctx).heap();
        if heap.diagnostic {
            check_poison(new);
            ptr::write_bytes(payload(new), UNINIT_POISON, bytes as usize);
        }
        ptr::copy_nonoverlapping(payload(old), payload(new), keep as usize);
        heap.free(old);
        (new, bytes)
    }
}

/// The payload address of a block reference.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_payload(block: *mut Header) -> *mut u8 {
    payload(block)
}

/// Metadata-derived usable bytes, independent of the live or raw count word.
/// # Safety
/// The block is a live or raw counted block, or an immortal block with an extent (static, or
/// regional of a layout without `LAYOUT_NO_EXTENT`). A block of a `LAYOUT_NO_EXTENT` layout
/// may carry no metadata (an arena block, or a bare block of the system-allocator archive):
/// its capacity is its descriptor's, and compiled code never queries it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_capacity(block: *mut Header) -> u64 {
    unsafe { (*block).capacity() }
}

#[cfg(test)]
#[path = "alloc/cpu_tests.rs"]
mod cpu_tests;

#[cfg(test)]
mod sequence_tests {
    use super::*;

    /// Request-derived slots must equal a separate read through the allocated block's
    /// metadata, at both ends of every size class and on the large/over-aligned paths.
    /// Source programs cannot observe the capacity word or the allocator's metadata.
    #[test]
    fn request_derived_sequence_slots_match_metadata_for_every_class() {
        for diagnostic in [false, true] {
            let layout = &crate::text::hirsel_rt_v2_bytes_descriptor.0 as *const crate::Layout;
            let mut heap = Heap::new(Box::new([layout]), diagnostic, 0);
            let mut view = View::new();
            // SAFETY: the view outlives the heap's use in this test.
            unsafe { heap.bind(&mut view) };
            let mut lower = 0;
            for upper in CLASS_SIZES.map(u64::from).into_iter().chain([4097, 8193]) {
                for extent in [lower, upper] {
                    for element_align in [1, 8, 16, 32, 64] {
                        let align = element_align.max(8);
                        let elements = sequence_elements_offset(element_align);
                        // Whole elements, trailing bytes, and zero-sized elements.
                        for element_size in [0, element_align, element_align * 3] {
                            unsafe {
                                let (block, bytes) = heap.allocate(0, extent, align);
                                assert_eq!(bytes, heap.capacity(block));
                                assert_eq!(
                                    requested_sequence_slots(extent, align, elements, element_size),
                                    sequence_slots(&heap, block, elements, element_size),
                                    "extent={extent} align={element_align} size={element_size} diagnostic={diagnostic}"
                                );
                                heap.free(block);
                            }
                        }
                    }
                }
                lower = upper + 1;
            }
        }
    }
}

#[cfg(all(test, not(feature = "system-allocator")))]
mod page_tests {
    use super::*;

    // Port mimalloc's page accounting invariant from mi_page_is_valid_init:
    // used + |free| + |local_free| == committed capacity. We also count the
    // not-yet-extended part of our reserved slab.
    unsafe fn check_page(slab: *mut Slab) {
        unsafe {
            let mut seen = std::collections::BTreeSet::new();
            let mut count_list = |mut block: *mut Header| {
                let mut count = 0;
                while !block.is_null() {
                    assert_eq!(slab_of(block), slab);
                    assert!(seen.insert(block.addr()), "duplicate free-list entry");
                    count += 1;
                    assert!(count <= (*slab).slots);
                    block = free_next(block);
                }
                count
            };
            let free = count_list((*slab).free);
            let local = count_list((*slab).local_free);
            let untouched = ((*slab).end.addr() - (*slab).bump.addr()) / (*slab).stride as usize;
            assert_eq!(
                (*slab).live as usize + free as usize + local as usize + untouched,
                (*slab).slots as usize
            );
        }
    }

    #[test]
    fn page_lists_partition_reserved_storage_through_exhaustion_and_trim() {
        for pool in 0..POOL_COUNT {
            let mut heap = Heap::new(Box::new([]), false, 0);
            let mut view = View::new();
            // SAFETY: the view outlives the heap's use in this test.
            unsafe { heap.bind(&mut view) };
            let align = pool_align(pool) as u64;
            let extent = CLASS_SIZES[pool % CLASS_COUNT] as u64;
            unsafe {
                let (a, _) = heap.allocate(0, extent, align);
                let slab = slab_of(a);
                let head = (*slab).free;
                heap.free(a);
                assert_eq!((*slab).free, head, "free changes only local_free");
                check_page(slab);
                let (b, _) = heap.allocate(0, extent, align);
                assert_ne!(b, a);
                assert_eq!((*slab).local_free, a, "allocation leaves local_free alone");
                let mut live = vec![b];
                // Fill more than one page and retain every allocation. Every
                // refill/collection must preserve disjoint lists and page counts.
                for i in 0..(*slab).slots + 3 {
                    live.push(heap.allocate(0, extent, align).0);
                    if i.is_multiple_of(64) || i + 1 == (*slab).slots + 3 {
                        let mut page = heap.classes[pool].slabs;
                        while !page.is_null() {
                            check_page(page);
                            page = (*page).next;
                        }
                    }
                }
                for block in live {
                    heap.free(block);
                }
                let mut page = heap.classes[pool].slabs;
                while !page.is_null() {
                    check_page(page);
                    page = (*page).next;
                }
                heap.trim();
                assert!(heap.classes[pool].current.is_null());
                assert!(heap.classes[pool].available.is_null());
                assert!(heap.classes[pool].slabs.is_null());
                let (fresh, _) = heap.allocate(0, extent, align);
                check_page(slab_of(fresh));
                heap.free(fresh);
                heap.release_all();
            }
        }
    }
}

fn cpu_slab_charge(diagnostic: bool) -> u64 {
    SLAB_SIZE as u64 * if diagnostic { 2 } else { 1 }
}
fn cpu_large_charge(capacity: u64, align: u64) -> u64 {
    capacity
        .saturating_add(large_header_offset(align))
        .saturating_add(HEADER_SIZE as u64)
        .saturating_add(align)
        .saturating_add(4095)
}

#[cfg(feature = "system-allocator")]
fn cpu_bare_charge(capacity: u64) -> u64 {
    capacity
        .saturating_add(HEADER_SIZE as u64)
        .saturating_add(32)
        .next_multiple_of(16)
}
