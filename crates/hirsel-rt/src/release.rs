//! Layout descriptors and the MEM-3 release engine.
//!
//! Dead blocks use count/cursor words and consumed payload reference slots for reversal,
//! so release uses constant native stack and never allocates. A [`Layout`]'s drop program is a
//! flat list of [`Step`]s in drop order, lowered from the LayoutRecord's nested drop program by
//! `hirsel_lir::rt_program` (the only producer). [`STEP_TAG`] skips an inactive
//! variant's steps, and [`STEP_NONNULL`] the dataful variant's of a null-pointer niche when its
//! word is null; [`STEP_LOOP`] runs its span once per element, highest index first, with a
//! static count or a dynamic `u64` count read from the value; [`STEP_COLUMN`] runs its span on
//! one structure-of-arrays column's element of the enclosing loop. Loops and columns are
//! *frames*: their steps' offsets are from the current element. An inline resource field
//! appears as a [`STEP_FINALIZE`] followed by its own fields' steps.
//!
//! When a child dies, the item is suspended, the child is pushed on top and drained completely,
//! and the item resumes, so resource drops run depth first in DROP-2 order. The suspended walk
//! state lives in the dead header's `count` word: the next step in the low
//! [`STEP_CURSOR_BITS`] bits and, inside loops, the enclosing loops' element indices + 1 in the
//! bits above as one mixed-radix number. Only an outermost loop has a dynamic count (a dynamic
//! extent is never inline in another value), so every inner radix is a static count. Resuming
//! rebuilds the innermost frame from that number by a scan of the enclosing steps, which needs
//! no storage; so does leaving a nested frame.

use std::ptr;

use crate::alloc::{
    DOMAIN_HEAP, Header, STATE_FIELDS_PENDING, STATE_FINALIZE_PENDING, STATE_FINALIZING, STATE_RAW,
    payload,
};
use crate::context::TaskCtx;
use crate::count::{COUNT_IMMORTAL, check_live_block};
use crate::finalize::{Cleanup, DropThunk, run_close, run_finalizer};
use crate::{STATUS_FAILURE, STATUS_OK, fault};

/// `Layout::magic`: "HRL1".
pub const LAYOUT_MAGIC: u32 = 0x484e_4c31;
/// `Layout::version` of this descriptor format.
pub const LAYOUT_VERSION: u32 = 18;
/// `Layout::flags`: the type is declared in an `unsafe ffi` package, so a failure in its
/// finalizer ends the process (FAIL-3).
pub const LAYOUT_UNSAFE_FFI: u32 = 1;
/// `Layout::flags`: the size is dynamic; `Layout::size` is the extent every value has.
pub const LAYOUT_DYNAMIC: u32 = 2;
/// A program that can suspend in a loop/column needs the full 40-bit element cursor.
pub const LAYOUT_WIDE_CURSOR: u32 = 4;
/// `Layout::flags` (version 5): a fixed-size, non-flat layout. Its arena blocks carry no extent
/// record before the header; their capacity is `Layout::size` and their alignment
/// `max(Layout::align, 8)`.
pub const LAYOUT_NO_EXTENT: u32 = 8;
/// `Layout::flags` (version 6): `Layout::logical` is per element, so a block's MEM-6 charge
/// is it times the u64 count at the payload's start.
pub const LAYOUT_LOGICAL_PER_ELEMENT: u32 = 16;
/// `Layout::logical` of a layout with no logical size (a flat encoding, an environment).
pub const LOGICAL_UNSUPPORTED: u64 = u64::MAX;
/// `Layout::flags`: the payload of a `Shared` root block (MEM-11). The blocks its value reaches
/// are immortal while the root lives; its program begins with [`STEP_THAW`] when it holds any.
pub const LAYOUT_FROZEN: u32 = 32;
/// `Layout::flags` (version 7): a sequence's payload. A deep copy into an arena tracks a block
/// of it (MEM-6) unless the copy reached it below a [`LAYOUT_UNTRACKED_BELOW`] block.
pub const LAYOUT_SEQUENCE: u32 = 64;
/// `Layout::flags` (version 7): a SortedMap's, a Slots' or an environment's payload, whose
/// sequences a deep copy into an arena keeps untracked.
pub const LAYOUT_UNTRACKED_BELOW: u32 = 128;
/// `Layout::flags` (version 8): a Map's or Set's payload, its entries List at offset 0, then
/// its index. A deep copy into a bounded arena tracks the block and its entries (MEM-6),
/// charged as any List, and neither tracks nor charges the index; the block's own logical
/// size is 0.
pub const LAYOUT_KEYED: u32 = 256;
/// `Layout::flags` (version 9): an `@soa` List's columns payload, tracked in a bounded arena
/// as a sequence's (MEM-6).
pub const LAYOUT_COLUMNS: u32 = 512;
/// `Layout::flags` (version 10): a sequential sequence's payload, of `Packed` values, tracked in
/// a bounded arena as a sequence's (MEM-6).
pub const LAYOUT_SEQUENTIAL: u32 = 1024;
/// An atomic root wrapping a mutex cell word (ABI 31). Releases apply immediately;
/// its final drop may run user code and fail, through RootDrop.
pub const LAYOUT_MUTEX_ROOT: u32 = 2048;

/// Release the counted block reference stored at `offset`.
pub const STEP_RELEASE: u32 = 1;
/// Run `layout`'s finalizer on the inline value at `offset` (borrowed self, shielded).
pub const STEP_FINALIZE: u32 = 2;
/// Close the run-context handle stored at `offset` through the Io adapter.
pub const STEP_CLOSE_HANDLE: u32 = 3;
/// Unless the `u32` tag at `offset` equals `variant`, skip the next `span` steps. A column
/// tag (nonzero `stride`, runtime ABI 68) is an `@soa` variant row's ordinal, `stride` bytes
/// wide, in a column of its loop's element `i`: at `offset + capacity * count + i * stride`,
/// where `capacity` is the `u64` at `count_offset`.
pub const STEP_TAG: u32 = 4;
/// Run the next `span` steps once per element, highest index first; element `i` is at
/// `offset + i * stride`. The count is `count`, or the `u64` at `count_offset`.
pub const STEP_LOOP: u32 = 5;
/// Run the next `span` steps once on the enclosing loop's element `i` of one column, at
/// `offset + capacity * count + i * stride`, where `capacity` is the `u64` at `count_offset`.
pub const STEP_COLUMN: u32 = 6;
/// Unless the pointer word at `offset` is non-null, skip the next `span` steps: the dataful
/// variant of a null-pointer niche, whose block reference there is never null.
pub const STEP_NONNULL: u32 = 7;
/// Skip the span when `variant` names a clear bit of the persistent u64 live word.
pub const STEP_LIVE_BIT: u32 = 8;
/// Restore ordinary counts in the frozen value at `offset`, read through `layout`, before the
/// steps that release it: the first step of a `Shared` root block's program (MEM-11).
pub const STEP_THAW: u32 = 9;
/// A [`STEP_RELEASE`] after which its value runs nothing more (version 11): the last step of
/// the program, or the last of a variant arm that is itself last. Never inside a loop. When it
/// kills its child, the dying block is freed at once and the child takes its place (a tail
/// release), so a chain drains without suspending a block per link.
pub const STEP_RELEASE_LAST: u32 = 10;
/// Release the `Shared` root reference stored at `offset` (version 13) under the root count
/// contract ([`crate::roots`]): exact, and deferred where the run's mode defers root releases.
/// Never a tail release.
pub const STEP_RELEASE_ROOT: u32 = 11;
/// Retire a language finalizer after all of its fields, including on failure.
pub const STEP_END_FINALIZE: u32 = 12;
/// Consume the private semantic metadata holder at offset (descriptor version 17).
/// This never traverses language counts, invokes user code or suspends.
pub const STEP_RELEASE_RECEIPT: u32 = 13;
/// Consume one opaque affine sealed owner through the run-checked release helper.
pub const STEP_RELEASE_SEALED: u32 = 14;
/// The finalizer is compiler implementation cleanup, adding no invocation itself.
pub const LAYOUT_IMPLEMENTATION_FINALIZER: u32 = 4096;

/// Whether a step releases the counted reference at its offset.
#[inline(always)]
pub(crate) fn releases(kind: u32) -> bool {
    kind == STEP_RELEASE || kind == STEP_RELEASE_LAST
}

/// Step bits of the logical dead cursor. Simple programs use the count word alone;
/// wide programs retain the complete remaining forty mixed-radix bits in header plus shadow.
pub const STEP_CURSOR_BITS: u32 = 24;
const STEP_CURSOR_MASK: u64 = (1 << STEP_CURSOR_BITS) - 1;
const NEST_LIMIT: u64 = 1 << (64 - STEP_CURSOR_BITS);
/// `Step::field` of a step that is never masked out by a partial live mask.
pub const WHOLE_VALUE: u32 = u32::MAX;
/// `Step::count_offset` of a loop with a static count.
pub const NO_OFFSET: u64 = u64::MAX;

/// A layout descriptor: payload size and alignment, the value's own finalizer, and the drop
/// program for its fields.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub magic: u32,
    pub version: u32,
    /// The fixed size, or with [`LAYOUT_DYNAMIC`] the bytes every value has.
    pub size: u64,
    pub align: u64,
    /// The value's own user drop body, run before its fields; absent for non-resources.
    pub finalizer: Option<DropThunk>,
    pub steps: *const Step,
    pub step_count: u64,
    /// Number of top-level fields addressed by a live mask.
    pub field_count: u32,
    pub flags: u32,
    /// The MEM-6 charge of a block of this payload layout (version 6; the runtime README's
    /// table), per element with [`LAYOUT_LOGICAL_PER_ELEMENT`].
    pub logical: u64,
    /// Optional five-word live-entry count program (native descriptor version 13).
    pub logical_count: *const u64,
    /// Native holder scan (version 14), eight words per instruction.
    pub scan: *const u64,
    /// Zero certifies no holders; u64::MAX means metadata is unavailable.
    pub scan_words: u64,
}

/// One drop program step. Outside every loop it is active only when its top-level `field` is
/// live; a live mask never applies inside a loop.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub kind: u32,
    pub field: u32,
    /// [`STEP_TAG`]: the ordinal that keeps the span.
    pub variant: u32,
    /// [`STEP_TAG`], [`STEP_NONNULL`], [`STEP_LOOP`], [`STEP_COLUMN`]: the steps that follow and
    /// belong to it.
    pub span: u32,
    pub offset: u64,
    /// On RELEASE/RELEASE_LAST, 24 marks a String/Bytes slice triple (version 15).
    pub stride: u64,
    pub count: u64,
    pub count_offset: u64,
    /// The inline resource's layout for [`STEP_FINALIZE`].
    pub layout: *const Layout,
}

#[inline]
fn spans(kind: u32) -> bool {
    matches!(
        kind,
        STEP_TAG | STEP_NONNULL | STEP_LIVE_BIT | STEP_LOOP | STEP_COLUMN
    )
}

/// Where a step reads its word in the frame whose element is `base` at loop index `index`:
/// `offset` into it, except a column [`STEP_TAG`]'s ordinal, in a column of the element.
#[inline(always)]
pub(crate) unsafe fn step_at(step: &Step, base: *mut u8, index: u64) -> *mut u8 {
    if step.kind != STEP_TAG || step.stride == 0 {
        return base.wrapping_add(step.offset as usize);
    }
    // SAFETY: a column tag's capacity word lies in its loop's element (`check_frame`).
    let capacity = unsafe {
        base.add(step.count_offset as usize)
            .cast::<u64>()
            .read_unaligned()
    };
    base.wrapping_add(
        step.offset
            .wrapping_add(capacity.wrapping_mul(step.count))
            .wrapping_add(index.wrapping_mul(step.stride)) as usize,
    )
}

/// Whether a [`STEP_TAG`] or [`STEP_NONNULL`] at `at` skips its span: its variant is inactive.
#[inline(always)]
pub(crate) unsafe fn variant_inactive(step: &Step, at: *const u8) -> bool {
    // SAFETY: `at` addresses the step's tag or niche word inside the value being dropped.
    unsafe {
        if step.kind == STEP_TAG {
            let ordinal = match step.stride {
                1 => u32::from(at.read()),
                2 => u32::from(at.cast::<u16>().read_unaligned()),
                _ => at.cast::<u32>().read_unaligned(),
            };
            ordinal != step.variant
        } else if step.kind == STEP_LIVE_BIT {
            at.cast::<u64>().read_unaligned() & (1u64 << step.variant) == 0
        } else {
            at.cast::<*const u8>().read_unaligned().is_null()
        }
    }
}

/// Reject a malformed descriptor (diagnostic mode).
pub(crate) unsafe fn check_layout(layout: *const Layout) {
    if layout.is_null() {
        fault("a null layout descriptor");
    }
    // SAFETY: caller passes a descriptor pointer; its fields are plain data.
    let layout = unsafe { &*layout };
    if layout.magic != LAYOUT_MAGIC || layout.version != LAYOUT_VERSION {
        fault("a layout descriptor has the wrong magic or version");
    }
    if layout.align == 0 || !layout.align.is_power_of_two() {
        fault("a layout descriptor's alignment is not a power of two");
    }
    if layout.flags
        & !(LAYOUT_UNSAFE_FFI
            | LAYOUT_DYNAMIC
            | LAYOUT_WIDE_CURSOR
            | LAYOUT_NO_EXTENT
            | LAYOUT_LOGICAL_PER_ELEMENT
            | LAYOUT_FROZEN
            | LAYOUT_SEQUENCE
            | LAYOUT_UNTRACKED_BELOW
            | LAYOUT_KEYED
            | LAYOUT_COLUMNS
            | LAYOUT_SEQUENTIAL
            | LAYOUT_MUTEX_ROOT
            | LAYOUT_IMPLEMENTATION_FINALIZER)
        != 0
    {
        fault("a layout descriptor has unknown flags");
    }
    if layout.flags & LAYOUT_DYNAMIC != 0 && layout.flags & LAYOUT_NO_EXTENT != 0 {
        fault("a dynamic layout descriptor claims NO_EXTENT");
    }
    if layout.step_count >= STEP_CURSOR_MASK {
        fault("a layout descriptor has more steps than a dead header's cursor can count");
    }
    if layout.step_count > 0 && layout.steps.is_null() {
        fault("a layout descriptor has steps but no step table");
    }
    let steps = if layout.step_count == 0 {
        &[]
    } else {
        // SAFETY: `steps` is nonnull with `step_count` entries.
        unsafe { std::slice::from_raw_parts(layout.steps, layout.step_count as usize) }
    };
    let wide = steps
        .iter()
        .any(|step| matches!(step.kind, STEP_LOOP | STEP_COLUMN));
    if wide && layout.flags & LAYOUT_WIDE_CURSOR == 0 {
        fault("a loop/column descriptor lacks WIDE_CURSOR");
    }
    check_frame(layout, steps, 0, steps.len(), Some(layout.size), 0, false);
    // Runtime-owned mechanical descriptors can have a host cleanup thunk and
    // no language scope. Emitted library descriptors still pair their marker.
    let own_scope = layout.finalizer.is_some()
        && (layout.flags & LAYOUT_IMPLEMENTATION_FINALIZER == 0
            || steps
                .iter()
                .any(|s| s.kind == STEP_END_FINALIZE && s.variant == 1));
    check_finalizer_scopes(layout, steps, 0, steps.len(), own_scope);
}

/// Static scope proof independent of runtime admission. A skipped arm or
/// zero-iteration loop must preserve its caller's active finalizers.
fn check_finalizer_scopes(layout: &Layout, steps: &[Step], start: usize, end: usize, own: bool) {
    let mut active = usize::from(own);
    let mut at = start;
    while at < end {
        let step = &steps[at];
        if spans(step.kind) {
            let close = at + 1 + step.span as usize;
            check_finalizer_scopes(layout, steps, at + 1, close, false);
            at = close;
            continue;
        }
        if step.kind == STEP_FINALIZE {
            active += 1;
        }
        if step.kind == STEP_END_FINALIZE {
            active = active
                .checked_sub(1)
                .unwrap_or_else(|| fault("unmatched finalizer scope end"));
            let mut nesting = 0usize;
            let mut matching = None;
            for earlier in (start..at).rev() {
                match steps[earlier].kind {
                    STEP_END_FINALIZE => nesting += 1,
                    STEP_FINALIZE if nesting == 0 => {
                        matching = Some(&steps[earlier]);
                        break;
                    }
                    STEP_FINALIZE => nesting -= 1,
                    _ => (),
                }
            }
            if let Some(begin) = matching {
                if begin.layout != step.layout
                    || begin.offset != step.offset
                    || begin.field != step.field
                    || step.variant != 0
                {
                    fault("finalizer scope end differs from its entry");
                }
            } else if !own
                || !std::ptr::eq(layout, step.layout)
                || step.offset != 0
                || step.variant != 1
            {
                fault("finalizer scope end differs from its implicit root entry");
            }
        }
        at += 1;
    }
    if active != 0 {
        fault("finalizer scope remains active after its field program");
    }
}

/// Check `steps[start..end]`, one frame's steps, whose element extent is `extent` bytes
/// (`None`: dynamic) inside `loops` enclosing loops; `column` when the frame is a column's.
/// Recursion follows the descriptor's static frame nesting, never the data.
fn check_frame(
    layout: &Layout,
    steps: &[Step],
    start: usize,
    end: usize,
    extent: Option<u64>,
    loops: u32,
    column: bool,
) {
    let fits = |offset: u64, bytes: u64| {
        extent.is_none_or(|extent| offset.checked_add(bytes).is_some_and(|e| e <= extent))
    };
    // Reversal writes a parent pointer into every consumed reference. Headerless
    // control words that a resumed frame may read must be disjoint from those slots.
    // A null-pointer niche deliberately shares its reference word; its guard is read
    // only before entering that arm, never while reconstructing a suspended cursor.
    // Two arms of one ordinal tag are exclusive: a control word inside one is never
    // read on a path that consumes a reference inside another. Diagnostic release
    // runs this check, so it allocates nothing.
    //
    // Visit the frame's steps outside loop/column bodies.
    fn framed(steps: &[Step], start: usize, end: usize, visit: &mut dyn FnMut(usize)) {
        let mut at = start;
        while at < end {
            let step = &steps[at];
            visit(at);
            if spans(step.kind) {
                let close = at + 1 + step.span as usize;
                if close > end {
                    fault("a drop step's span leaves its frame");
                }
                at = if matches!(step.kind, STEP_TAG | STEP_NONNULL | STEP_LIVE_BIT) {
                    at + 1
                } else {
                    close
                };
            } else {
                at += 1;
            }
        }
    }
    let inside = |arm: usize, at: usize| arm < at && at <= arm + steps[arm].span as usize;
    let exclusive = |a: usize, b: usize| {
        let mut found = false;
        framed(steps, start, end, &mut |k| {
            let tag = &steps[k];
            if found || tag.kind != STEP_TAG || !inside(k, a) {
                return;
            }
            framed(steps, start, end, &mut |m| {
                let other = &steps[m];
                found |= other.kind == STEP_TAG
                    && (other.offset, other.count, other.stride)
                        == (tag.offset, tag.count, tag.stride)
                    && other.variant != tag.variant
                    && inside(m, b);
            });
        });
        found
    };
    framed(steps, start, end, &mut |control_at| {
        let step = &steps[control_at];
        let control = match step.kind {
            STEP_TAG if step.stride != 0 => Some((step.count_offset, 8)),
            STEP_TAG => Some((step.offset, 4)),
            STEP_LIVE_BIT => Some((step.offset, 8)),
            STEP_LOOP if step.count_offset != NO_OFFSET => Some((step.count_offset, 8)),
            STEP_COLUMN => Some((step.count_offset, 8)),
            _ => None,
        };
        // Loop/column control words are checked here; their element frames are
        // independently checked below.
        let Some((control, width)) = control else {
            return;
        };
        framed(steps, start, end, &mut |slot_at| {
            let slot = &steps[slot_at];
            if (releases(slot.kind) || matches!(slot.kind, STEP_RELEASE_ROOT | STEP_RELEASE_SEALED))
                && control < slot.offset.saturating_add(8)
                && slot.offset < control.saturating_add(width)
                && !exclusive(control_at, slot_at)
            {
                fault("drop control word aliases a consumed reference slot");
            }
        });
    });
    let mut at = start;
    while at < end {
        let step = &steps[at];
        if step.field != WHOLE_VALUE && step.field >= layout.field_count {
            fault("a drop step names a field outside its layout");
        }
        let body = at + 1;
        let close = body + step.span as usize;
        if spans(step.kind) {
            if close > end {
                fault("a drop step's span leaves its frame");
            }
        } else if step.span != 0 {
            fault("a leaf drop step has a span");
        }
        match step.kind {
            STEP_RELEASE | STEP_RELEASE_LAST | STEP_RELEASE_ROOT | STEP_CLOSE_HANDLE
            | STEP_RELEASE_RECEIPT | STEP_RELEASE_SEALED => {
                if step.kind == STEP_RELEASE_LAST && loops > 0 {
                    fault("a last release inside a loop");
                }
                if !fits(
                    step.offset,
                    if releases(step.kind) && step.stride == 24 {
                        24
                    } else {
                        8
                    },
                ) {
                    fault("a drop step lies outside its layout");
                }
            }
            STEP_THAW => {
                if at != 0 || loops > 0 || layout.flags & LAYOUT_FROZEN == 0 {
                    fault("a thaw step that does not begin a frozen payload's program");
                }
                if step.layout.is_null() {
                    fault("a thaw step has no layout");
                }
                // SAFETY: a nonnull descriptor pointer; its fields are plain data.
                let child = unsafe { &*step.layout };
                if child.magic != LAYOUT_MAGIC || child.version != LAYOUT_VERSION {
                    fault("a thaw step's layout has the wrong magic or version");
                }
                if !fits(step.offset, child.size) {
                    fault("a drop step lies outside its layout");
                }
            }
            STEP_FINALIZE | STEP_END_FINALIZE => {
                let child = step.layout;
                if child.is_null() {
                    fault("a finalize step has no layout");
                }
                // SAFETY: a nonnull descriptor pointer; its fields are plain data.
                let child = unsafe { &*child };
                if child.magic != LAYOUT_MAGIC || child.version != LAYOUT_VERSION {
                    fault("a finalize step's layout has the wrong magic or version");
                }
                if child.finalizer.is_none() {
                    fault("a finalize step's layout has no finalizer");
                }
                if !fits(step.offset, child.size) {
                    fault("a drop step lies outside its layout");
                }
            }
            STEP_TAG | STEP_NONNULL | STEP_LIVE_BIT => {
                if step.kind == STEP_LIVE_BIT && step.variant >= 64 {
                    fault("live bit outside its word");
                }
                if step.kind == STEP_TAG && step.stride != 0 {
                    // A column tag reads its loop's element: never the root's or a column's.
                    if loops == 0 || column || !matches!(step.stride, 1 | 2 | 4) {
                        fault("a column tag outside a loop's element or of another width");
                    }
                    if !fits(step.count_offset, 8) {
                        fault("a column tag's capacity lies outside its layout");
                    }
                } else {
                    let width = if step.kind == STEP_TAG { 4 } else { 8 };
                    if !fits(step.offset, width) {
                        fault("a drop step's tag lies outside its layout");
                    }
                }
                check_frame(layout, steps, body, close, extent, loops, column);
            }
            STEP_LOOP => {
                let element = if step.count_offset == NO_OFFSET {
                    let region = step.count.checked_mul(step.stride);
                    if region.is_none_or(|bytes| !fits(step.offset, bytes)) {
                        fault("a drop loop lies outside its layout");
                    }
                    Some(step.stride)
                } else {
                    if loops > 0 {
                        fault("a drop loop with a dynamic count inside another loop");
                    }
                    if !fits(step.count_offset, 8) {
                        fault("a drop loop's count lies outside its layout");
                    }
                    // Stride 0 holds columns, whose fixed part is the value's remainder.
                    match (step.stride, extent) {
                        (0, Some(extent)) => Some(extent.saturating_sub(step.offset)),
                        (0, None) => None,
                        (stride, _) => Some(stride),
                    }
                };
                check_frame(layout, steps, body, close, element, loops + 1, false);
            }
            STEP_COLUMN => {
                if loops == 0 {
                    fault("a drop column outside a loop");
                }
                if !fits(step.count_offset, 8) {
                    fault("a drop column's capacity lies outside its layout");
                }
                check_frame(layout, steps, body, close, Some(step.stride), loops, true);
            }
            _ => fault("a drop step has an unknown kind"),
        }
        at = if spans(step.kind) { close } else { body };
    }
}

/// Cleanup continuations use a tagged internal work pointer, never a counted reference.
#[inline(always)]
fn is_cleanup(item: *mut Header) -> bool {
    item.addr() & 1 != 0
}
#[inline]
fn cleanup(item: *mut Header) -> *mut Cleanup {
    item.map_addr(|address| address & !1).cast()
}
#[inline]
unsafe fn index_of(item: *mut Header) -> u32 {
    unsafe {
        if is_cleanup(item) {
            (*cleanup(item)).index
        } else {
            (*item).index()
        }
    }
}
#[inline]
unsafe fn cursor_of(item: *mut Header) -> u64 {
    unsafe {
        if is_cleanup(item) {
            (*cleanup(item)).cursor
        } else {
            (*item).cursor()
        }
    }
}
#[inline]
unsafe fn save_cursor(item: *mut Header, cursor: u64, layout: *const Layout) {
    unsafe {
        if is_cleanup(item) {
            (*cleanup(item)).cursor = cursor;
        } else {
            (*item).set_cursor(cursor, (*layout).flags & LAYOUT_WIDE_CURSOR != 0);
        }
    }
}
#[inline]
unsafe fn value_of(item: *mut Header) -> *mut u8 {
    unsafe {
        if is_cleanup(item) {
            (*cleanup(item)).value
        } else {
            payload(item)
        }
    }
}
#[inline]
unsafe fn field_live(mask: *const u64, field: u32) -> bool {
    mask.is_null()
        || field == WHOLE_VALUE
        || unsafe { *mask.add(field as usize / 64) >> (field % 64) & 1 == 1 }
}
/// Start one drain invocation; its stopping boundary is in the caller's activation.
#[inline]
pub(crate) unsafe fn push(ctx: &mut TaskCtx, item: *mut Header, own_finalizer: bool) {
    unsafe {
        if is_cleanup(item) {
            (*cleanup(item)).parent = ctx.work;
            (*cleanup(item)).phase = own_finalizer as u32;
            (*cleanup(item)).cursor = 0;
        } else {
            ctx.heap().set_state(
                item,
                if own_finalizer {
                    STATE_FINALIZE_PENDING
                } else {
                    STATE_FIELDS_PENDING
                },
            );
            save_cursor(item, 0, ctx.descriptor(index_of(item)));
        }
    }
    ctx.work = item;
}

/// Hint that the cache line at `address` will be read soon. Never faults, whatever the address.
#[inline(always)]
fn prefetch(address: *const u8) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: a prefetch hint has no architectural effect and never faults.
    unsafe {
        std::arch::x86_64::_mm_prefetch::<{ std::arch::x86_64::_MM_HINT_T0 }>(address.cast());
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: as above; PRFM writes no register or memory.
    unsafe {
        std::arch::asm!(
            "prfm pldl1keep, [{address}]",
            address = in(reg) address,
            options(nostack, readonly, preserves_flags)
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = address;
}

/// T16 DropEdge — consume the credit of one outgoing edge. Returns whether the child died; the
/// caller then pushes it.
#[inline]
unsafe fn consume_edge(ctx: &mut TaskCtx, child: *mut Header) -> bool {
    // SAFETY: an active reference slot holds a counted block reference.
    unsafe {
        if ctx.heap().diagnostic {
            check_live_block(ctx, child);
            check_not_root(ctx, child);
        }
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("count_decrements", 1);
            ctx.heap().measurement.count_decrements += 1;
        }
        let count = (*child).count;
        if count == 1 {
            return true;
        }
        if count < crate::count::COUNT_ARENA_SHARED {
            (*child).count = count - 1;
        }
        false
    }
}

/// The innermost frame of a drop-program walk: the value itself, or a loop's or column's
/// current element.
#[derive(Clone, Copy)]
struct Frame {
    /// The frame's [`STEP_LOOP`] or [`STEP_COLUMN`] step; unused at the root.
    op: usize,
    /// The first step after the frame.
    end: usize,
    /// The current element.
    base: *mut u8,
    /// The enclosing frame's current element.
    parent: *mut u8,
    /// The innermost loop's element index (a column's is its loop's).
    index: u64,
    /// Loops and columns enclosing the walk, this frame included; 0 at the root.
    depth: u32,
}

impl Frame {
    fn root(value: *mut u8, end: usize) -> Frame {
        Frame {
            op: 0,
            end,
            base: value,
            parent: value,
            index: 0,
            depth: 0,
        }
    }

    /// The frame `step` (a loop at element `index`, or a column) opens at `op` inside `self`.
    #[inline]
    unsafe fn enter(&self, op: usize, step: &Step, index: u64) -> Frame {
        let offset = if step.kind == STEP_COLUMN {
            // SAFETY: the column's capacity lies in the enclosing element.
            let capacity = unsafe {
                self.base
                    .add(step.count_offset as usize)
                    .cast::<u64>()
                    .read_unaligned()
            };
            step.offset
                .wrapping_add(capacity.wrapping_mul(step.count))
                .wrapping_add(self.index.wrapping_mul(step.stride))
        } else {
            step.offset.wrapping_add(index.wrapping_mul(step.stride))
        };
        Frame {
            op,
            end: op + 1 + step.span as usize,
            base: self.base.wrapping_add(offset as usize),
            parent: self.base,
            index: if step.kind == STEP_COLUMN {
                self.index
            } else {
                index
            },
            depth: self.depth + 1,
        }
    }
}

/// Visit the steps that open frames enclosing step `target`, outermost first.
#[inline]
unsafe fn enclosing(steps: *const Step, target: usize, mut visit: impl FnMut(usize, &Step)) {
    let mut at = 0;
    while at < target {
        // SAFETY: `at` precedes `target`, a step of this program.
        let step = unsafe { &*steps.add(at) };
        let span = if spans(step.kind) {
            step.span as usize
        } else {
            0
        };
        if target <= at + span {
            if !matches!(step.kind, STEP_TAG | STEP_NONNULL | STEP_LIVE_BIT) {
                visit(at, step);
            }
            at += 1;
        } else {
            at += 1 + span;
        }
    }
}

/// Rebuild the innermost frame enclosing step `target` from the enclosing loops' mixed-radix
/// element indices `nest`: the outermost loop's index is the most significant digit, and each
/// inner loop's static count is its radix.
#[cold]
unsafe fn locate(
    steps: *const Step,
    end: usize,
    value: *mut u8,
    target: usize,
    nest: u64,
) -> Frame {
    // SAFETY: `target` is a step of this program.
    unsafe {
        let mut weight = 1u64;
        let mut outermost = true;
        enclosing(steps, target, |_, step| {
            if step.kind == STEP_LOOP {
                if !outermost {
                    weight *= step.count;
                }
                outermost = false;
            }
        });
        let mut frame = Frame::root(value, end);
        let mut rest = nest;
        let mut outermost = true;
        enclosing(steps, target, |op, step| {
            let mut index = 0;
            if step.kind == STEP_LOOP {
                if !outermost {
                    weight /= step.count;
                }
                outermost = false;
                index = rest / weight;
                rest %= weight;
            }
            frame = frame.enter(op, step, index);
        });
        frame
    }
}

/// The count word of an item suspended before step `next` in `frame`.
#[inline]
fn suspended(next: usize, frame: &Frame, nest: u64) -> u64 {
    if frame.depth == 0 {
        return next as u64;
    }
    if nest + 1 >= NEST_LIMIT {
        fault("a drop loop nest has more elements than a dead header can count");
    }
    next as u64 | (nest + 1) << STEP_CURSOR_BITS
}

/// Diagnostic mode: a dynamic loop or column over a heap block must lie inside its capacity.
#[cold]
unsafe fn check_extent(item: *mut Header, frame: &Frame, step: &Step, count: u64) {
    // SAFETY: `item` is a dead header.
    unsafe {
        if is_cleanup(item) || (*item).domain() != DOMAIN_HEAP {
            return;
        }
        let start = frame.base as u64 - payload(item) as u64;
        let bytes = if step.kind == STEP_COLUMN {
            count.checked_mul(step.count.saturating_add(step.stride))
        } else {
            count.checked_mul(step.stride)
        };
        let fits = bytes
            .and_then(|b| b.checked_add(step.offset))
            .and_then(|b| b.checked_add(start))
            .is_some_and(|e| e <= (*item).capacity());
        // A column's element must exist: its index stays below the capacity.
        if !fits || step.kind == STEP_COLUMN && frame.index >= count {
            fault("a drop loop's elements lie outside the block's capacity");
        }
    }
}

/// What a drain did with its root.
pub(crate) struct Drained {
    pub(crate) status: u8,
    pub(crate) kept_root: bool,
}

/// Drain the worklist down to `base`. `root` is the item this call pushed; `mask` applies to it
/// alone. With `keep_root` a root that drains without failure becomes raw storage instead of
/// being freed (reset). Failures accumulate: every remaining field and sibling is still
/// released, and the status is returned only after the drain finishes.
///
/// The top item runs its steps until one kills a child, which is pushed and drained first. When
/// that step was a [`STEP_RELEASE_LAST`], nothing of the item remains to run: the item is freed
/// at once and the child replaces it (a tail release), so a list's links are freed head first
/// in one pass. Any other item is freed after its children (post-order). Compiled drop glue
/// frees in the same order (`hirsel-llvm`'s `glue`).
pub(crate) unsafe fn drain(
    ctx: &mut TaskCtx,
    base: *mut Header,
    root: *mut Header,
    mask: *const u64,
    keep_root: bool,
    anchor: u64,
    operation_site: u64,
) -> Drained {
    #[cfg(test)]
    crate::header_tests::count_drain();
    let mut cursor = Cursor::new(base, root, mask, keep_root, anchor, operation_site);
    loop {
        match unsafe { cursor.poll(ctx) } {
            ReleasePoll::Complete(result) => return result,
            ReleasePoll::Close(handle) => {
                let status = unsafe { run_close(ctx, handle, anchor, operation_site) };
                cursor.closed(status);
            }
        }
    }
}

/// All traversal locals that survive a built-in close. A pending cursor is
/// kept at a stable address with its entire work/cursor ancestry; constructing
/// this record alone never authorizes physical stack release.
pub(crate) struct Cursor {
    base: *mut Header,
    root: *mut Header,
    item: *mut Header,
    parent: *mut Header,
    fresh: bool,
    mask: *const u64,
    keep_root: bool,
    anchor: u64,
    operation_site: u64,
    status: u8,
    kept_root: bool,
    close: Option<u64>,
    complete: bool,
    finalizer: Option<Finalizer>,
    finalizer_serial: u64,
    finalizer_own: bool,
}

pub(crate) enum ReleasePoll {
    Close(u64),
    Complete(Drained),
}

/// One original user-finalizer invocation exposed before its first action.
/// The caller must retain the borrowed value and own admission/shield custody;
/// this token grants no scheduling permission and never names a TaskCtx.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Finalizer {
    pub(crate) layout: *const Layout,
    pub(crate) value: *mut u8,
    pub(crate) anchor: u64,
    pub(crate) operation_site: u64,
    serial: u64,
}

/// Opt-in traversal for a checked finalizer driver. Ordinary poll never
/// exposes user code and retains its synchronous status contract.
#[allow(dead_code)]
pub(crate) enum FinalizerPoll {
    Close(u64),
    Finalizer(Finalizer),
    Complete(Drained),
}

impl Cursor {
    /// # Safety
    /// The caller already owns root's consumed credit/inline Cleanup and the
    /// original work watermark. Mask/value/ancestors outlive this cursor.
    pub(crate) fn new(
        base: *mut Header,
        root: *mut Header,
        mask: *const u64,
        keep_root: bool,
        anchor: u64,
        operation_site: u64,
    ) -> Self {
        Self {
            base,
            root,
            item: root,
            parent: ptr::null_mut(),
            fresh: true,
            mask,
            keep_root,
            anchor,
            operation_site,
            status: STATUS_OK,
            kept_root: false,
            close: None,
            complete: false,
            finalizer: None,
            finalizer_serial: 0,
            finalizer_own: false,
        }
    }

    /// Poll the traversal only; an unfinished close is returned at its saved
    /// PC until its own driver supplies a completed 0/1 status. The close's
    /// consumed credit, loop position and pointer-reversal ancestors persist.
    /// # Safety
    /// As for new. No ordinary finalizer called by this walk can compact its
    /// synchronous Rust caller; only the returned close may leave this stack.
    pub(crate) unsafe fn poll(&mut self, ctx: &mut TaskCtx) -> ReleasePoll {
        assert!(
            self.finalizer.is_none(),
            "entered finalizer requires its original driver"
        );
        match unsafe { self.poll_with::<false>(ctx) } {
            FinalizerPoll::Complete(result) => ReleasePoll::Complete(result),
            FinalizerPoll::Close(handle) => ReleasePoll::Close(handle),
            FinalizerPoll::Finalizer(_) => unreachable!("synchronous finalizer exposed"),
        }
    }

    /// Yield the original finalizer before invoking it, retaining the next
    /// mechanical cursor. Repeated polls return that same invocation; fields
    /// and sibling owners cannot advance until finalized consumes its token.
    /// # Safety
    /// As for new; the checked caller additionally retains each exposed
    /// finalizer's borrowed self, nested frame, admission and shield through
    /// Pending. Opaque finalizers must still use synchronous run_finalizer.
    #[allow(dead_code)]
    pub(crate) unsafe fn poll_finalizers(&mut self, ctx: &mut TaskCtx) -> FinalizerPoll {
        unsafe { self.poll_with::<true>(ctx) }
    }

    unsafe fn poll_with<const FINALIZERS: bool>(&mut self, ctx: &mut TaskCtx) -> FinalizerPoll {
        assert!(!self.complete, "release cursor polled after completion");
        if let Some(finalizer) = self.finalizer {
            return FinalizerPoll::Finalizer(finalizer);
        }
        if let Some(handle) = self.close {
            return FinalizerPoll::Close(handle);
        }
        unsafe {
            if self.mask.is_null() && !self.keep_root {
                poll_items::<false, FINALIZERS>(ctx, self)
            } else {
                poll_items::<true, FINALIZERS>(ctx, self)
            }
        }
    }

    fn handoff(&mut self, layout: *const Layout, value: *mut u8, own: bool) -> FinalizerPoll {
        assert!(self.finalizer.is_none() && self.close.is_none());
        self.finalizer_serial = self
            .finalizer_serial
            .checked_add(1)
            .expect("finalizer serial overflow");
        let finalizer = Finalizer {
            layout,
            value,
            anchor: self.anchor,
            operation_site: self.operation_site,
            serial: self.finalizer_serial,
        };
        self.finalizer = Some(finalizer);
        self.finalizer_own = own;
        FinalizerPoll::Finalizer(finalizer)
    }

    /// Complete only the original invocation after its nested body and local
    /// cleanup finish. STEP_END_FINALIZE still owns activation retirement;
    /// this does not re-admit, re-shield or repeat a finalizer.
    /// # Safety
    /// The Active task owns this cursor and completed the token's borrowed
    /// invocation. Failure is already recorded; unsafe-FFI failure is fatal.
    #[allow(dead_code)]
    pub(crate) unsafe fn finalized(&mut self, ctx: &mut TaskCtx, token: Finalizer, status: u8) {
        assert!(
            status == STATUS_OK || status == STATUS_FAILURE,
            "pending is not a finalizer completion"
        );
        assert_eq!(self.finalizer, Some(token), "stale finalizer completion");
        if status == STATUS_FAILURE {
            if ctx.heap().diagnostic && !ctx.failure.active() {
                fault("a finalizer failed without recording a failure");
            }
            if unsafe { (*token.layout).flags } & LAYOUT_UNSAFE_FFI != 0 {
                crate::finalize::terminate_unsafe_ffi();
            }
        }
        self.finalizer = None;
        self.status |= status;
        if self.finalizer_own && !is_cleanup(self.item) {
            unsafe { ctx.heap().set_state(self.item, STATE_FIELDS_PENDING) };
        }
        self.finalizer_own = false;
    }

    pub(crate) fn closed(&mut self, status: u8) {
        assert!(
            self.close.take().is_some(),
            "close completion without a release step"
        );
        assert!(
            status == STATUS_OK || status == STATUS_FAILURE,
            "pending is not a release failure bit"
        );
        self.status |= status;
    }
}

#[derive(Clone, Copy)]
enum Walk {
    Child(*mut Header, u64),
    Close(u64, u64),
    Finalizer(*const Layout, *mut u8, u64),
    Complete,
}

/// [`drain`]. `ROOTED` is false when the root has neither a live mask nor `keep_root`, so every
/// item runs alike: an ordinary release.
#[inline(always)]
unsafe fn poll_items<const ROOTED: bool, const FINALIZERS: bool>(
    ctx: &mut TaskCtx,
    state: &mut Cursor,
) -> FinalizerPoll {
    let Cursor {
        base,
        mut root,
        mut item,
        mut parent,
        mut fresh,
        mask,
        keep_root,
        anchor,
        operation_site,
        mut status,
        mut kept_root,
        ..
    } = *state;
    while !item.is_null() {
        unsafe {
            let layout = ctx.descriptor(index_of(item));
            let value = value_of(item);
            if fresh {
                if item != root || mask.is_null() {
                    if !is_cleanup(item) {
                        ctx.heap().set_state(item, STATE_FINALIZING);
                    }
                    if FINALIZERS && (*layout).finalizer.is_some() {
                        state.root = root;
                        state.item = item;
                        state.parent = parent;
                        state.fresh = false;
                        state.status = status;
                        state.kept_root = kept_root;
                        return state.handoff(layout, value, true);
                    }
                    status |= run_finalizer(ctx, layout, value, anchor, operation_site);
                }
                if !is_cleanup(item) {
                    ctx.heap().set_state(item, STATE_FIELDS_PENDING);
                }
                fresh = false;
            }
            let (item_mask, keep) = if ROOTED && item == root {
                (mask, keep_root)
            } else {
                (ptr::null(), false)
            };
            let steps = (*layout).steps;
            let end = (*layout).step_count as usize;
            let cursor = cursor_of(item);
            let mut pc = (cursor & STEP_CURSOR_MASK) as usize;
            // The child a step killed, and the count word the item resumes from.
            let mut child = ptr::null_mut();
            let mut resume = 0;
            if cursor >> STEP_CURSOR_BITS != 0 {
                // Resume inside loops: rebuild the frame of the step that killed the child.
                let nest = (cursor >> STEP_CURSOR_BITS) - 1;
                let frame = locate(steps, end, value, pc - 1, nest);
                let walked = walk_frames::<FINALIZERS>(
                    ctx,
                    item,
                    steps,
                    frame,
                    pc,
                    nest,
                    item_mask,
                    anchor,
                    operation_site,
                    &mut status,
                );
                match walked {
                    Walk::Child(c, r) => (child, resume) = (c, r),
                    Walk::Complete => {}
                    Walk::Close(handle, next) => {
                        save_cursor(item, next, layout);
                        state.root = root;
                        state.item = item;
                        state.parent = parent;
                        state.fresh = fresh;
                        state.status = status;
                        state.kept_root = kept_root;
                        state.close = Some(handle);
                        return FinalizerPoll::Close(handle);
                    }
                    Walk::Finalizer(finalizer, borrowed, next) => {
                        save_cursor(item, next, layout);
                        state.root = root;
                        state.item = item;
                        state.parent = parent;
                        state.fresh = fresh;
                        state.status = status;
                        state.kept_root = kept_root;

                        return state.handoff(finalizer, borrowed, false);
                    }
                }
            } else {
                // Releases and tags outside every loop, the steps of ordinary data, run here
                // without calls; the first other step continues in `walk_frames`.
                while pc < end {
                    let step = &*steps.add(pc);
                    if ROOTED && !field_live(item_mask, step.field) {
                        pc += 1;
                        if spans(step.kind) {
                            pc += step.span as usize;
                        }
                        continue;
                    }
                    let at = value.wrapping_add(step.offset as usize);
                    if releases(step.kind) {
                        pc += 1;
                        let killed = at.cast::<*mut Header>().read_unaligned();
                        if consume_edge(ctx, killed) {
                            (child, resume) = (killed, pc as u64);
                            // The resumed item's next read is the next release's child header:
                            // start it now so it overlaps the child's drain.
                            if pc < end {
                                let next = &*steps.add(pc);
                                if releases(next.kind)
                                    && (!ROOTED || field_live(item_mask, next.field))
                                {
                                    let at = value.wrapping_add(next.offset as usize);
                                    prefetch(at.cast::<*const u8>().read_unaligned());
                                }
                            }
                            break;
                        }
                    } else if step.kind == STEP_TAG
                        || step.kind == STEP_NONNULL
                        || step.kind == STEP_LIVE_BIT
                    {
                        pc += 1;
                        if variant_inactive(step, at) {
                            pc += step.span as usize;
                        }
                    } else {
                        let frame = Frame::root(value, end);
                        let walked = walk_frames::<FINALIZERS>(
                            ctx,
                            item,
                            steps,
                            frame,
                            pc,
                            0,
                            item_mask,
                            anchor,
                            operation_site,
                            &mut status,
                        );
                        match walked {
                            Walk::Child(c, r) => (child, resume) = (c, r),
                            Walk::Complete => {}
                            Walk::Close(handle, next) => {
                                save_cursor(item, next, layout);
                                state.root = root;
                                state.item = item;
                                state.parent = parent;
                                state.fresh = fresh;
                                state.status = status;
                                state.kept_root = kept_root;
                                state.close = Some(handle);
                                return FinalizerPoll::Close(handle);
                            }
                            Walk::Finalizer(finalizer, borrowed, next) => {
                                save_cursor(item, next, layout);
                                state.root = root;
                                state.item = item;
                                state.parent = parent;
                                state.fresh = fresh;
                                state.status = status;
                                state.kept_root = kept_root;

                                return state.handoff(finalizer, borrowed, false);
                            }
                        }
                        break;
                    }
                }
            }
            if child.is_null() {
                if keep && status == STATUS_OK {
                    (*item).restore_descriptor();
                    ctx.heap().set_state(item, STATE_RAW);
                    kept_root = true;
                } else {
                    finish(ctx, item);
                }
                if parent.is_null() {
                    ctx.work = base;
                    break;
                }
                item = parent;
                let parent_layout = ctx.descriptor(index_of(item));
                let cursor = cursor_of(item);
                let pc = (cursor & STEP_CURSOR_MASK) as usize;
                let value = value_of(item);
                let frame = if cursor >> STEP_CURSOR_BITS == 0 {
                    Frame::root(value, (*parent_layout).step_count as usize)
                } else {
                    locate(
                        (*parent_layout).steps,
                        (*parent_layout).step_count as usize,
                        value,
                        pc - 1,
                        (cursor >> STEP_CURSOR_BITS) - 1,
                    )
                };
                let step = &*(*parent_layout).steps.add(pc - 1);
                let slot = frame.base.add(step.offset as usize).cast::<*mut Header>();
                parent = if item == root {
                    ptr::null_mut()
                } else {
                    slot.read_unaligned()
                };
                ctx.work = item;
                continue;
            }
            // A tail release: the item runs nothing more, so it is freed now and the child takes
            // its place under the same parent. A kept or masked root runs to its end.
            if resume >> STEP_CURSOR_BITS == 0
                && (*steps.add(resume as usize - 1)).kind == STEP_RELEASE_LAST
                && !is_cleanup(item)
                && !(ROOTED && item == root)
            {
                if item == root {
                    root = ptr::null_mut();
                }
                finish(ctx, item);
                item = child;
                let child_layout = ctx.descriptor((*child).index());
                ctx.heap().set_state(child, STATE_FINALIZE_PENDING);
                save_cursor(child, 0, child_layout);
                ctx.work = item;
                fresh = true;
                continue;
            }
            // A dying child's credit has been consumed. Only non-root slots carry a parent link;
            // root donor bytes remain intact, including moved/kept fields used by Reuse.
            save_cursor(item, resume, layout);
            if item != root {
                let pc = (resume & STEP_CURSOR_MASK) as usize;
                let frame = if resume >> STEP_CURSOR_BITS == 0 {
                    Frame::root(value, end)
                } else {
                    locate(steps, end, value, pc - 1, (resume >> STEP_CURSOR_BITS) - 1)
                };
                let slot = frame
                    .base
                    .add((*steps.add(pc - 1)).offset as usize)
                    .cast::<*mut Header>();
                slot.write_unaligned(parent);
            }
            parent = item;
            item = child;
            let child_layout = ctx.descriptor((*child).index());
            ctx.heap().set_state(child, STATE_FINALIZE_PENDING);
            save_cursor(child, 0, child_layout);
            ctx.work = item;
            fresh = true;
        }
    }
    state.root = ptr::null_mut();
    state.item = ptr::null_mut();
    state.parent = ptr::null_mut();
    state.status = status;
    state.kept_root = kept_root;
    state.complete = true;
    FinalizerPoll::Complete(Drained { status, kept_root })
}

/// Run one step that opens no frame on its field at `at`, advancing `pc` past a skipped tag
/// span. Returns the child a [`STEP_RELEASE`] killed, or null.
#[inline(always)]
unsafe fn run_step(
    ctx: &mut TaskCtx,
    step: &Step,
    at: *mut u8,
    pc: &mut usize,
    anchor: u64,
    operation_site: u64,
    status: &mut u8,
) -> *mut Header {
    // SAFETY: `at` addresses the step's field inside the value being dropped.
    unsafe {
        match step.kind {
            STEP_RELEASE | STEP_RELEASE_LAST => {
                let child = at.cast::<*mut Header>().read_unaligned();
                if consume_edge(ctx, child) {
                    return child;
                }
            }
            STEP_RELEASE_RECEIPT => {
                let receipt = at.cast::<*mut std::ffi::c_void>().read_unaligned();
                crate::sealed_region::receipt::abi::hirsel_rt_v2_receipt_release(receipt);
            }
            STEP_RELEASE_SEALED => {
                let owner = at.cast::<*mut std::ffi::c_void>().read_unaligned();
                crate::sealed_region::hirsel_rt_v2_sealed_release(ctx, owner);
            }
            STEP_RELEASE_ROOT => {
                let child = at.cast::<*mut Header>().read_unaligned();
                // A root's last release may defer to its owner's lifecycle.
                let _role = crate::tasks::role::Guard::new(ctx);
                if crate::roots::consume(ctx, child) {
                    return child;
                }
            }
            STEP_TAG | STEP_NONNULL | STEP_LIVE_BIT => {
                if variant_inactive(step, at) {
                    *pc += step.span as usize;
                }
            }
            STEP_FINALIZE => *status |= run_finalizer(ctx, step.layout, at, anchor, operation_site),
            STEP_END_FINALIZE => {
                if (*step.layout).flags & LAYOUT_IMPLEMENTATION_FINALIZER != 0 {
                    return ptr::null_mut();
                }
                if ctx.cleanup_rejections != 0 {
                    ctx.cleanup_rejections -= 1;
                } else {
                    let activation = ctx
                        .cleanup_invocations
                        .pop()
                        .unwrap_or_else(|| fault("finalizer retirement without admission"));
                    ctx.activations.retire(activation);
                }
            }
            // The last root died: the value is ordinary again before its steps release it.
            STEP_THAW => crate::shared::thaw_value(ctx, step.layout, at),
            STEP_CLOSE_HANDLE => {
                *status |= run_close(
                    ctx,
                    at.cast::<u64>().read_unaligned(),
                    anchor,
                    operation_site,
                );
            }
            _ => fault("a drop step has an unknown kind"),
        }
        ptr::null_mut()
    }
}

/// The general drop program walk, for every step kind, from step `pc` in `frame`, whose
/// enclosing loops' element indices are `nest`: loops and columns open frames, and leaving one
/// returns to its enclosing frame. Returns the child a step killed (or null when the program
/// ends) and the count word to resume from.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
unsafe fn walk_frames<const FINALIZERS: bool>(
    ctx: &mut TaskCtx,
    item: *mut Header,
    steps: *const Step,
    mut frame: Frame,
    mut pc: usize,
    mut nest: u64,
    mask: *const u64,
    anchor: u64,
    operation_site: u64,
    status: &mut u8,
) -> Walk {
    // SAFETY: as `drain`; `frame` is a frame of this item's program.
    unsafe {
        let value = value_of(item);
        let step_count = (*ctx.descriptor(index_of(item))).step_count as usize;
        loop {
            if pc == frame.end {
                if frame.depth == 0 {
                    return Walk::Complete;
                }
                let op = &*steps.add(frame.op);
                if op.kind == STEP_LOOP && frame.index > 0 {
                    // The next lower element.
                    frame.index -= 1;
                    nest -= 1;
                    frame.base = frame.parent.wrapping_add(
                        op.offset.wrapping_add(frame.index.wrapping_mul(op.stride)) as usize,
                    );
                    pc = frame.op + 1;
                    continue;
                }
                if op.kind == STEP_LOOP {
                    // Element 0 is done: drop this loop's digit. An inner loop's count is
                    // static; the outermost leaves no digits.
                    nest = if frame.depth == 1 { 0 } else { nest / op.count };
                }
                frame = if frame.depth == 1 {
                    Frame::root(value, step_count)
                } else {
                    locate(steps, step_count, value, frame.op, nest)
                };
                continue;
            }
            let step = &*steps.add(pc);
            pc += 1;
            // A partial root bypasses its own finalizer; child scopes still run.
            if step.kind == STEP_END_FINALIZE && step.variant == 1 && !mask.is_null() {
                continue;
            }
            if frame.depth == 0 && !field_live(mask, step.field) {
                if spans(step.kind) {
                    pc += step.span as usize;
                }
                continue;
            }
            match step.kind {
                STEP_LOOP => {
                    let count = if step.count_offset == NO_OFFSET {
                        step.count
                    } else {
                        frame
                            .base
                            .add(step.count_offset as usize)
                            .cast::<u64>()
                            .read_unaligned()
                    };
                    if count == 0 || step.span == 0 {
                        pc += step.span as usize;
                        continue;
                    }
                    if ctx.heap().diagnostic && step.count_offset != NO_OFFSET {
                        check_extent(item, &frame, step, count);
                    }
                    let Some(inner) = nest
                        .checked_mul(count)
                        .and_then(|n| n.checked_add(count - 1))
                    else {
                        fault("a drop loop nest has more elements than a dead header can count")
                    };
                    nest = inner;
                    frame = frame.enter(pc - 1, step, count - 1);
                }
                STEP_COLUMN => {
                    if ctx.heap().diagnostic {
                        let capacity = frame
                            .base
                            .add(step.count_offset as usize)
                            .cast::<u64>()
                            .read_unaligned();
                        check_extent(item, &frame, step, capacity);
                    }
                    frame = frame.enter(pc - 1, step, 0);
                }
                _ => {
                    let at = step_at(step, frame.base, frame.index);
                    if FINALIZERS && step.kind == STEP_FINALIZE {
                        return Walk::Finalizer(step.layout, at, suspended(pc, &frame, nest));
                    }
                    if step.kind == STEP_CLOSE_HANDLE {
                        let handle = at.cast::<u64>().read_unaligned();
                        return Walk::Close(handle, suspended(pc, &frame, nest));
                    }
                    let child = run_step(ctx, step, at, &mut pc, anchor, operation_site, status);
                    #[cfg(feature = "ownership-trace")]
                    if step.kind == STEP_END_FINALIZE
                        && (*step.layout).flags & LAYOUT_IMPLEMENTATION_FINALIZER == 0
                    {
                        crate::ownership_trace::retired(ctx, step.layout, at, value, step.field);
                    }
                    if !child.is_null() {
                        return Walk::Child(child, suspended(pc, &frame, nest));
                    }
                }
            }
        }
    }
}

/// T16 Free — a drained item's storage is returned (blocks) or its continuation ends.
#[inline]
unsafe fn finish(ctx: &mut TaskCtx, item: *mut Header) {
    // SAFETY: `item` is fully drained.
    unsafe {
        if is_cleanup(item) {
            (*cleanup(item)).phase = 0;
        } else if (*item).domain() == DOMAIN_HEAP {
            ctx.heap().free(item);
        } else {
            fault("an immortal block reached the release worklist");
        }
    }
}

/// T16 Free for compiled drop glue. The glue consumed the last credit of `block` (it read a
/// count of 1) and has already released every field in the layout's drop order; this returns
/// the storage. Shallow: it never scans the payload or runs a finalizer, so the glue calls it
/// only for layouts without one.
///
/// # Safety
/// `ctx` is live and `block` is a live heap block of this context whose only credit the
/// caller owns and whose fields hold nothing owned any more.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_free(ctx: *mut TaskCtx, block: *mut Header) {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    unsafe {
        if (*ctx).heap().diagnostic {
            return free_glue_checked(ctx, block);
        }
        let heap = &mut (*ctx).heap();
        let kind = (*block).descriptor & (3 << crate::alloc::KIND_SHIFT);
        if kind == 0 {
            heap.free_small(block);
        } else if kind == crate::alloc::KIND_LARGE << crate::alloc::KIND_SHIFT {
            heap.free_large(block);
        } else {
            fault("an immortal block reached a free");
        }
    }
}

/// Close a constructor-tail block (LANG-10, runtime ABI 70): its payload now holds a value,
/// so its header takes the box layout at `index` in place of the open stand-in it was
/// allocated at, which has the same size and alignment and releases nothing. Until it closes,
/// a release, which follows the header, frees the block's storage alone.
///
/// # Safety
/// `ctx` is live and `block` a unique heap block of this context, allocated at an open
/// stand-in, whose payload holds a value of the layout at `index`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_hole_close(
    ctx: *mut TaskCtx,
    block: *mut Header,
    index: u32,
) {
    // SAFETY: caller contract.
    unsafe {
        if (*ctx).heap().diagnostic {
            check_hole_close(&*ctx, block, index);
        }
        (*block).descriptor = index | ((*block).descriptor & !crate::alloc::INDEX_MASK);
    }
}

#[cold]
#[inline(never)]
unsafe fn check_hole_close(ctx: &TaskCtx, block: *mut Header, index: u32) {
    // SAFETY: caller contract.
    unsafe {
        check_live_block(ctx, block);
        if (*block).count != 1 || (*block).domain() != DOMAIN_HEAP {
            fault("a constructor-tail block closes shared or outside the heap");
        }
        let open = ctx.descriptor((*block).index());
        let closed = ctx.descriptor(index);
        check_layout(closed);
        if (*open).step_count != 0
            || (*open).finalizer.is_some()
            || (*open).size != (*closed).size
            || (*open).align != (*closed).align
        {
            fault("a constructor-tail block closes from no open stand-in of its layout");
        }
    }
}

#[cold]
#[inline(never)]
unsafe fn free_glue_checked(ctx: *mut TaskCtx, block: *mut Header) {
    unsafe {
        let ctx = &mut *ctx;
        check_live_block(ctx, block);
        if (*block).count != 1 {
            fault("a free of a block that still has other holders");
        }
        if (*ctx.descriptor((*block).index())).finalizer.is_some() {
            fault("a free of a block whose layout has a finalizer");
        }
        if (*block).domain() != DOMAIN_HEAP {
            fault("an immortal block reached a free");
        }
        ctx.heap().free(block);
    }
}

/// Validate a root release request, reached only in diagnostic mode.
#[cold]
#[inline(never)]
unsafe fn check_root(ctx: &TaskCtx, block: *mut Header, layout: *const Layout, mask: *const u64) {
    // SAFETY: caller passes a block reference and descriptor.
    unsafe {
        check_layout(layout);
        check_live_block(ctx, block);
        check_not_root(ctx, block);
        if ctx.descriptor((*block).index()) != layout {
            fault("header/layout mismatch: the block's header names a different layout");
        }
        if (*block).domain() == DOMAIN_HEAP && (*layout).size > ctx.heap_ref().capacity(block) {
            fault("header/layout mismatch: the layout is larger than the block's capacity");
        }
        if !mask.is_null() && (*block).count != 1 {
            fault("a partially initialized block has more than one holder");
        }
    }
}

/// Fault on an ordinary count operation reaching a `Shared` root, whose count only
/// [`crate::roots`] touches (diagnostic mode).
#[cold]
pub(crate) unsafe fn check_not_root(ctx: &TaskCtx, block: *mut Header) {
    // SAFETY: caller passes a live block.
    unsafe {
        if (*block).domain() == DOMAIN_HEAP && crate::shared::is_root(ctx, block) {
            fault("an ordinary count operation reached a Shared root");
        }
    }
}

/// Consume one holder credit of `block`, draining everything that dies.
pub(crate) unsafe fn release_block(
    ctx: &mut TaskCtx,
    block: *mut Header,
    mask: *const u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: caller owns one credit of a live block.
    unsafe {
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("count_decrements", 1);
            ctx.heap().measurement.count_decrements += 1;
        }
        let count = (*block).count;
        if count >= crate::count::COUNT_ARENA_SHARED {
            // T04 DropImmortal — immortal and tracked arena blocks ignore releases.
            return STATUS_OK;
        }
        if count > 1 {
            // T04 Drop — Finite(n) -> Finite(n - 1).
            (*block).count = count - 1;
            return STATUS_OK;
        }
        drop_last(ctx, block, mask, anchor, operation_site)
    }
}

/// T04 DropLast + T16 Free: the caller consumed the last credit of `block`; drain everything
/// that dies.
///
/// # Safety
/// `block` is a live block of `ctx` whose last credit the caller consumed.
pub(crate) unsafe fn drop_last(
    ctx: &mut TaskCtx,
    block: *mut Header,
    mask: *const u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        // A scalar/byte payload has no work, including when reached through
        // an erased String/Bytes owner, and neither has a partly moved block whose mask leaves
        // no step live. T04 DropLast + T16 Free need neither dead-header cursor setup nor a
        // drain invocation for an empty program.
        let layout = &*ctx.descriptor((*block).index());
        if layout.finalizer.is_none() && layout.step_count == 0 || masked_inert(layout, mask) {
            ctx.heap().free(block);
            return STATUS_OK;
        }
        // T04 DropLast — Finite(1) -> Dead, then drain.
        let base = ctx.work;
        push(ctx, block, mask.is_null());
        drain(ctx, base, block, mask, false, anchor, operation_site).status
    }
}

/// Release one holder credit of `block` (T04 Drop, T04 DropLast, T04 DropImmortal). Ordinary freeing
/// allocates nothing; user finalizers may. `live_mask` is null for a fully initialized value,
/// else one bit per top-level field for a partially initialized unique block, whose own
/// finalizer then does not run. Returns [`crate::STATUS_FAILURE`] after cleanup finishes if any
/// finalizer failed; the failures are in the run context.
///
/// # Safety
/// `ctx` is live, `block` a counted or static block whose header names `layout`, and the caller
/// owns one credit of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_release(
    ctx: *mut TaskCtx,
    block: *mut Header,
    index: u32,
    live_mask: *const u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if let Some(status) = unsafe { crate::tasks::free::tail::invoke_release(ctx, block) } {
        assert!(
            live_mask.is_null(),
            "a bound release has an unproved live mask"
        );
        return status;
    }
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        if ctx.heap().diagnostic {
            check_root(ctx, block, ctx.descriptor(index), live_mask);
        }
        release_block(ctx, block, live_mask, anchor, operation_site)
    }
}

/// Whether a masked root of `layout` runs nothing: it has no finalizer and every top-level step
/// belongs to a field `mask` leaves dead (`hirsel_lir::rt_program::masked_inert`, which compiled
/// code tests statically).
fn masked_inert(layout: &Layout, mask: *const u64) -> bool {
    if mask.is_null() || layout.finalizer.is_some() {
        return false;
    }
    let mut at = 0;
    while at < layout.step_count as usize {
        // SAFETY: a checked descriptor's steps; a live mask covers its fields.
        let step = unsafe { &*layout.steps.add(at) };
        if unsafe { field_live(mask, step.field) } {
            return false;
        }
        at += 1 + if spans(step.kind) {
            step.span as usize
        } else {
            0
        };
    }
    true
}

/// T12 Reset. Consume the donor holder. A unique donor drains its children and finalizers at this
/// anchor and becomes raw storage written to `out_token`; a shared or immortal donor is released
/// normally and the token is null (None). If any finalizer fails the storage is disposed, no
/// token escapes and the status is a failure.
///
/// # Safety
/// As `hirsel_rt_v2_release`; `out_token` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_reset(
    ctx: *mut TaskCtx,
    block: *mut Header,
    index: u32,
    live_mask: *const u64,
    anchor: u64,
    operation_site: u64,
    out_token: *mut *mut Header,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        if ctx.heap().diagnostic {
            check_root(ctx, block, ctx.descriptor(index), live_mask);
        }
        out_token.write(ptr::null_mut());
        if (*block).count != 1 || (*block).domain() != DOMAIN_HEAP {
            return release_block(ctx, block, live_mask, anchor, operation_site);
        }
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("count_decrements", 1);
            ctx.heap().measurement.count_decrements += 1;
        }
        // Every field with drop steps has moved out: no walk, the donor is the token.
        if masked_inert(&*ctx.descriptor(index), live_mask) {
            ctx.heap().set_state(block, STATE_RAW);
            #[cfg(feature = "memcheck")]
            crate::memcheck::noaccess(
                crate::alloc::payload(block),
                ctx.heap().capacity(block) as usize,
            );
            out_token.write(block);
            return STATUS_OK;
        }
        let base = ctx.work;
        push(ctx, block, live_mask.is_null());
        let drained = drain(ctx, base, block, live_mask, true, anchor, operation_site);
        if drained.kept_root {
            // The donor is gone: its payload is unaddressable until a reuse installs a value.
            #[cfg(feature = "memcheck")]
            crate::memcheck::noaccess(
                crate::alloc::payload(block),
                ctx.heap().capacity(block) as usize,
            );
            out_token.write(block);
        }
        drained.status
    }
}

#[cfg(test)]
mod cursor_tests;
