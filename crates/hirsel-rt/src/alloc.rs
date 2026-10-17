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
//! fragmentation. Task exit returns every slab and large block. Nothing here is shared between
//! tasks (OWN-6): the allocator lives in its task's run context.
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

use crate::context::RunCtx;
#[cfg(feature = "memcheck")]
use crate::memcheck;
use crate::memcheck::SLAB_REDZONE;
use crate::release::check_layout;
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
    ctx: *mut RunCtx,
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
}

const SLAB_HEADER: usize = 128;
/// Run-context and page words compiled fast paths read and write (runtime ABI 4,
/// [`Compiled`]); the context begins with its heap, and the heap with them.
pub const CTX_DUP_MAX_OFFSET: usize = std::mem::offset_of!(Compiled, dup_max);
pub const CTX_FREE_INLINE_OFFSET: usize = std::mem::offset_of!(Compiled, free_inline);
pub const CTX_IDLE_SLABS_OFFSET: usize = std::mem::offset_of!(Compiled, idle_slabs);
pub const CTX_PAGES_OFFSET: usize = std::mem::offset_of!(Compiled, pages);
pub const PAGE_FREE_OFFSET: usize = std::mem::offset_of!(Slab, free);
pub const PAGE_LIVE_OFFSET: usize = std::mem::offset_of!(Slab, live);
pub const PAGE_LOCAL_FREE_OFFSET: usize = std::mem::offset_of!(Slab, local_free);
const _: () = assert!(std::mem::offset_of!(Heap, compiled) == 0);
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
    #[cfg(feature = "system-allocator")]
    allocation: *mut u8,
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

struct SlabRecord {
    state: std::cell::Cell<u8>,
    first: u32,
    stride: u32,
    slots: u32,
}

/// The words compiled code reads at fixed offsets from the run context (runtime ABI 4,
/// `hirsel_rt_v2.h`; the context begins with its heap and the heap with these). Each is the
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
}

/// One task's heap domain.
#[repr(C)]
pub(crate) struct Heap {
    /// First, at context offset 0: see [`Compiled`].
    pub(crate) compiled: Compiled,
    /// The linked program's layout descriptors, by index.
    layouts: Box<[*const crate::Layout]>,
    /// Each layout's bare blocks, by index, so a bare allocation reads one entry.
    #[cfg(feature = "system-allocator")]
    bare_layouts: Box<[BareLayout]>,
    classes: [Class; POOL_COUNT],
    large: *mut LargePrefix,
    /// Diagnostic states of blocks outside slabs: large and bare blocks.
    large_states: Option<std::collections::BTreeMap<usize, std::cell::Cell<u8>>>,
    /// The live bare blocks, which only the correctness and measurement archives list.
    #[cfg(all(
        feature = "system-allocator",
        any(feature = "diagnostic", feature = "measurement")
    ))]
    bare: std::collections::HashSet<usize>,
    slab_records: Option<std::collections::BTreeMap<usize, SlabRecord>>,
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
        Heap {
            compiled: Compiled {
                dup_max: 0,
                free_inline: Self::inline(diagnostic) as u32,
                idle_slabs: 0,
                pages: [ptr::null_mut(); POOL_COUNT],
            },
            // SAFETY: the run context validated every descriptor.
            #[cfg(feature = "system-allocator")]
            bare_layouts: layouts
                .iter()
                .map(|&layout| BareLayout::of(unsafe { &*layout }))
                .collect(),
            layouts,
            classes: [EMPTY_CLASS; POOL_COUNT],
            large: ptr::null_mut(),
            large_states: diagnostic.then(Default::default),
            #[cfg(all(
                feature = "system-allocator",
                any(feature = "diagnostic", feature = "measurement")
            ))]
            bare: Default::default(),
            slab_records: diagnostic.then(Default::default),
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
        }
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
            self.compiled.pages[class] = slab;
        }
    }

    pub(crate) fn forget_foreign_record(&mut self, address: usize) {
        if let Some(states) = &mut self.large_states {
            states.remove(&address);
        }
    }

    pub(crate) unsafe fn state_ptr(&self, block: *mut Header) -> *mut u8 {
        unsafe {
            for pool in &self.classes {
                let mut slab = pool.slabs;
                while !slab.is_null() {
                    let start = slab as usize + (*slab).first as usize;
                    let delta = (block as usize).wrapping_sub(start);
                    if delta < (*slab).slots as usize * (*slab).stride as usize {
                        if !delta.is_multiple_of((*slab).stride as usize) {
                            fault("invalid slab slot");
                        }
                        return (*slab).states.add(slot_of(slab, block));
                    }
                    slab = (*slab).next;
                }
            }
            if let Some(state) = self
                .large_states
                .as_ref()
                .and_then(|states| states.get(&block.addr()))
            {
                return state.as_ptr();
            }
            if let Some(record) = self
                .slab_records
                .as_ref()
                .and_then(|records| records.get(&slab_of(block).addr()))
            {
                let delta = block
                    .addr()
                    .wrapping_sub(slab_of(block).addr() + record.first as usize);
                if delta < record.slots as usize * record.stride as usize
                    && delta.is_multiple_of(record.stride as usize)
                {
                    return record.state.as_ptr();
                }
                fault("invalid retired slab slot");
            }
            ptr::null_mut()
        }
    }
    pub(crate) unsafe fn state(&self, block: *mut Header) -> u8 {
        unsafe {
            let state = self.state_ptr(block);
            if !state.is_null() {
                state.read()
            } else if (*block).domain() == DOMAIN_STATIC {
                STATE_LIVE
            } else {
                fault("block does not belong to the context");
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
            let slot = self.state_ptr(block);
            if slot.is_null() {
                fault("block does not belong to the context");
            }
            slot.write(state);
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
        for &block in &self.bare {
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
        self.bare.insert(block.addr());
        if let Some(states) = &mut self.large_states {
            states.insert(block.addr(), std::cell::Cell::new(STATE_FREE));
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
                self.compiled.idle_slabs -= 1;
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
            let state = &mut self.classes[class];
            let mut slab = state.current;
            if !slab.is_null() && page_refill(slab) {
                return page_pop(slab);
            }
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
        let state = &mut self.classes[class];
        // SAFETY: `base` is a fresh slab.
        unsafe {
            slab.write(Slab {
                next: state.slabs,
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
            });
            if let Some(records) = &mut self.slab_records {
                records.insert(
                    slab.addr(),
                    SlabRecord {
                        state: std::cell::Cell::new(STATE_LIVE),
                        first: first as u32,
                        stride: stride as u32,
                        slots: slots as u32,
                    },
                );
            }
        }
        state.slabs = slab;
        self.set_current(class, slab);
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.slab_count += 1;
        }
        self.compiled.idle_slabs += 1;
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
        #[cfg(not(feature = "system-allocator"))]
        let Ok(host) = HostLayout::from_size_align(total as usize, align as usize) else {
            environmental("a large allocation exceeds the host address space")
        };
        // SAFETY: nonzero size.
        #[cfg(not(feature = "system-allocator"))]
        let base = unsafe { host_alloc(host) };
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
        if let Some(states) = &mut self.large_states {
            states.insert(block.addr(), std::cell::Cell::new(STATE_FREE));
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
        }
    }
    /// A bare system block's free is its one matching `free`; nothing lists it in production.
    #[cfg(feature = "system-allocator")]
    #[inline(always)]
    pub(crate) unsafe fn free_small(&mut self, block: *mut Header) {
        unsafe {
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
        if !self.bare.remove(&block.addr()) {
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
            #[cfg(feature = "measurement")]
            self.measurement.free((*block).capacity());
            let slab = slab_of(block);
            #[cfg(feature = "memcheck")]
            memcheck::pool_free(slab.cast(), block.cast());
            let head = (*slab).local_free;
            write_free(block, head);
            (*slab).local_free = block;
            (*slab).live -= 1;
            if (*slab).live == 0 {
                self.compiled.idle_slabs += 1;
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
    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn free_large(&mut self, block: *mut Header) {
        // SAFETY: `block` is a large block of this heap; its prefix precedes it.
        unsafe {
            let align = (*block).align();
            let prefix = (*block).large_prefix();
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
            #[cfg(feature = "measurement")]
            self.measurement.free((*prefix).capacity);
            release_large(prefix, total, align);
        }
    }

    /// Return idle slabs above the idle cap to the backing allocator. Free blocks of returned
    /// slabs leave their class free lists; no other free block moves.
    pub(crate) fn trim(&mut self) {
        let keep = self.idle_cap / SLAB_SIZE as u64;
        if self.compiled.idle_slabs <= keep {
            return;
        }
        let mut excess = self.compiled.idle_slabs - keep;
        for class in 0..POOL_COUNT {
            if excess == 0 {
                break;
            }
            let mut doomed = 0;
            let mut slab = self.classes[class].slabs;
            // SAFETY: the slab list holds this heap's live slabs.
            unsafe {
                while !slab.is_null() && excess > 0 {
                    if (*slab).live == 0 {
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
                    if let Some(record) = self
                        .slab_records
                        .as_ref()
                        .and_then(|records| records.get(&slab.addr()))
                    {
                        record.state.set(STATE_FREE);
                    }
                    release_slab(slab);
                    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
                    {
                        self.slab_count -= 1;
                    }
                    self.compiled.idle_slabs -= 1;
                } else {
                    link = &mut (*slab).next;
                }
            }
        }
    }

    /// Task exit: return every slab and large block, live or not, and every listed bare block.
    pub(crate) fn release_all(&mut self) {
        #[cfg(all(
            feature = "system-allocator",
            any(feature = "diagnostic", feature = "measurement")
        ))]
        for block in std::mem::take(&mut self.bare) {
            let block = block as *mut Header;
            // SAFETY: each listed bare block is this heap's, live or raw, and is freed once.
            unsafe {
                if let Some(state) = self
                    .large_states
                    .as_ref()
                    .and_then(|states| states.get(&block.addr()))
                {
                    state.set(STATE_FREE);
                }
                #[cfg(feature = "measurement")]
                self.measurement.free(self.capacity(block));
                libc::free(block.cast());
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
                    if let Some(record) = self
                        .slab_records
                        .as_ref()
                        .and_then(|records| records.get(&slab.addr()))
                    {
                        record.state.set(STATE_FREE);
                    }
                    release_slab(slab);
                    slab = next;
                }
            }
            self.classes[class] = EMPTY_CLASS;
            self.compiled.pages[class] = ptr::null_mut();
        }
        let mut prefix = self.large;
        while !prefix.is_null() {
            // SAFETY: each large block is this heap's and is freed once.
            unsafe {
                let next = (*prefix).next;
                if let Some(state) = self
                    .large_states
                    .as_ref()
                    .and_then(|states| states.get(&large_block(prefix).addr()))
                {
                    state.set(STATE_FREE);
                }
                #[cfg(feature = "measurement")]
                self.measurement.free((*prefix).capacity);
                release_large(prefix, (*prefix).total, (*prefix).align);
                prefix = next;
            }
        }
        self.large = ptr::null_mut();
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.slab_count = 0;
        }
        self.compiled.idle_slabs = 0;
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
    ctx: *mut RunCtx,
    index: u32,
    extent: u64,
    align: u64,
    destination: u8,
    site: u64,
) -> *mut Header {
    unsafe {
        let heap = &mut (*ctx).heap;
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
    ctx: *mut RunCtx,
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
        if ctx.heap.diagnostic && (*layout).size > extent {
            fault("an allocation extent is smaller than its layout size");
        }
        let (block, capacity) = ctx.heap.allocate(index, extent, align);
        if let Some(owners) = &mut ctx.region_owners {
            owners.remove(&block.addr());
        }
        if ctx.heap.diagnostic {
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
pub unsafe extern "C" fn hirsel_rt_v2_dispose(ctx: *mut RunCtx, token: *mut Header) {
    if token.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    // SAFETY: caller contract.
    unsafe {
        if ctx.heap.diagnostic && ctx.heap.state(token) != STATE_RAW {
            fault("dispose of storage that is not a raw reuse token");
        }
        ctx.heap.free(token);
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
    ctx: *mut RunCtx,
    token: *mut Header,
    index: u32,
    extent: u64,
    align: u64,
    out_block: *mut *mut Header,
) -> u8 {
    if token.is_null() {
        #[cfg(feature = "measurement")]
        unsafe {
            (*ctx).heap.measurement.record("reuse_misses", 1);
            (&mut (*ctx).heap.measurement).reuse_misses += 1;
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
        memcheck::defined(payload(token), ctx.heap.capacity(token) as usize);
        if ctx.heap.diagnostic {
            check_layout(layout);
            if ctx.heap.state(token) != STATE_RAW {
                fault("reuse of storage that is not a raw reuse token");
            }
            if (*layout).size > extent {
                fault("a reuse extent is smaller than its layout size");
            }
        }
        if (*token).domain() != DOMAIN_HEAP || ctx.heap.usable(token) != (capacity, physical_align)
        {
            fault("a reuse token's (capacity, alignment, domain) key does not match the request");
        }
        #[cfg(feature = "measurement")]
        {
            ctx.heap.measurement.record("reuse_hits", 1);
            ctx.heap.measurement.reuse_hits += 1;
        }
        // A bare token's capacity is its layout's, so a request that cannot be bare moves the
        // token's bytes into a prefixed block of the same capacity.
        #[cfg(feature = "system-allocator")]
        if (*token).kind() == KIND_SMALL && ctx.heap.bare_capacity(index, extent, align).is_none() {
            let (block, _) = ctx.heap.allocate(index, extent, align);
            ptr::copy_nonoverlapping(payload(token), payload(block), capacity as usize);
            ctx.heap.free(token);
            out_block.write(block);
            return STATUS_OK;
        }
        (*token).count = 1;
        (*token).descriptor = index | (*token).kind() << KIND_SHIFT;
        ctx.heap.set_state(token, STATE_LIVE);
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
/// count and the counted elements move bitwise into fresh storage of the class that fits (so
/// the new capacity is the class's), the old storage is freed shallowly, never scanned, and
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
/// # Safety
/// `ctx` is a live run context and `block` is a live sequence block whose
/// payload layout has this element size and alignment, of which the caller holds one credit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_seq_grow(
    ctx: *mut RunCtx,
    block: *mut Header,
    element_size: u64,
    element_align: u64,
    capacity: u64,
) -> *mut Header {
    // SAFETY: caller contract.
    unsafe {
        if (*block).count != 1 || (*block).domain() != DOMAIN_HEAP {
            return detach_reserved(ctx, block, element_size, element_align, capacity);
        }
        unique_heap_block(ctx, block, element_align);
        let elements = sequence_elements_offset(element_align);
        // A block that already holds them is left alone, its capacity word made exact.
        // Zero-sized elements need no storage: every capacity is already held.
        let slots = sequence_slots(&(*ctx).heap, block, elements, element_size);
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
        let count = payload(block).cast::<u64>().read();
        let used = elements + count * element_size;
        let (grown, bytes) = grow(ctx, block, used, extent, element_align);
        // The validated request includes the prefix, and the chosen class holds the request.
        set_sequence_capacity(grown, slots_in_bytes(bytes - elements, element_size));
        grown
    }
}

/// Block growth (LIR schema revision 19, `Inst::SeqAppend`). Make the unique heap block
/// `block` hold at least `extent` payload bytes, aligned to `align`. A block that already
/// holds them is left alone. Otherwise its first `keep` payload bytes move bitwise into fresh
/// storage of the class that fits, the old storage is freed shallowly, never scanned, and the
/// new block is returned. Faults and exhaustion are as for [`hirsel_rt_v2_seq_grow`].
///
/// # Safety
/// `ctx` is a live run context and `block` is a live block whose payload layout has this
/// alignment and whose first `keep` payload bytes are initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_block_grow(
    ctx: *mut RunCtx,
    block: *mut Header,
    keep: u64,
    extent: u64,
    align: u64,
) -> *mut Header {
    // SAFETY: caller contract.
    unsafe {
        unique_heap_block(ctx, block, align);
        if (*ctx).heap.capacity(block) >= extent {
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
    ctx: *mut RunCtx,
    block: *mut Header,
    element_size: u64,
    element_align: u64,
    capacity: u64,
) -> *mut Header {
    // SAFETY: caller contract.
    unsafe {
        let heap = &(*ctx).heap;
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
        let (detached, bytes) = if count == 0 && (*block).domain() == DOMAIN_STATIC {
            // An empty list literal's static block has no element, holder or credit to
            // carry: its first reserve is one fresh allocation.
            let (fresh, bytes) = (*ctx).heap.allocate((*block).index(), extent, align);
            if (*ctx).heap.diagnostic {
                ptr::write_bytes(payload(fresh), UNINIT_POISON, bytes as usize);
            }
            payload(fresh).cast::<u64>().write(0);
            (fresh, bytes)
        } else {
            crate::copy::detach_sized(ctx, block, used, extent, align)
        };
        // The validated request includes the prefix, and the chosen class holds the request.
        set_sequence_capacity(detached, slots_in_bytes(bytes - elements, element_size));
        detached
    }
}

/// Fault unless `block` is a live, unique heap block and `align` a power of two.
unsafe fn unique_heap_block(ctx: *mut RunCtx, block: *mut Header, align: u64) {
    // SAFETY: the caller's block is a live or freed header.
    unsafe {
        let heap = &(*ctx).heap;
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
    ctx: *mut RunCtx,
    old: *mut Header,
    keep: u64,
    extent: u64,
    align: u64,
) -> (*mut Header, u64) {
    // SAFETY: `old` is a live unique heap block (`unique_heap_block`).
    unsafe {
        let heap = &mut (*ctx).heap;
        let align = align.max(8);
        check_request(extent, align);
        if keep > heap.capacity(old) || keep > extent {
            fault("a sequence count exceeds its storage");
        }
        let (new, bytes) = heap.allocate((*old).index(), extent, align);
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
