//! Deep copy (`unique x`, `copy_to`, a deep LIR `UniqueCopy`): every headered block
//! reachable from a root block or an inline value's holder slots is copied per holder
//! into the explicit heap/arena destination. Every ordinary counted block in the
//! result has one holder, including repeated empty containers and byte intervals.
//! Source sharing is preserved in the source; nested Shared roots keep their roots.
//! A copy-on-write
//! detach (a shallow `UniqueCopy`, OWN-4) copies the root block alone and gives each counted
//! child one more holder.
//!
//! Prior art: Rust a7c37f9f0ee5, library/alloc/src/rcs/rc.rs:2223-2270
//! (`make_mut`) detaches shared storage before permitting a unique write;
//! :2676-2682 (`clone`) only adds a shallow credit. Hirsel's deep MODE-2
//! guarantee requires detaching every ordinary holder, while Shared roots
//! remain boundaries. The language cannot build an ordinary counted cycle.
//!
//! Children are found through each block's own descriptor: its `RELEASE` steps are exactly
//! its counted holders (`TAG` and a niche's `NONNULL` select the active variant's, `LOOP` and `COLUMN` reach
//! elements).
//! A whole-block copy keeps its class capacity and bytes. A String/Bytes interval
//! gets a fresh byte sequence holding only its viewed bytes; each holder gets its
//! own copy, even when two intervals are equal. Each counted slot is rewritten
//! to the child's copy, so every copy holds exactly the credits its slots name. Immortal and
//! static blocks are copied like any other: the copy is unique, so a `unique` parameter
//! writes it in place without a test (MODE-2), which a shared static block would not survive.
//! A `Shared` root is never copied: the copy holds one more root of the same graph (MEM-11).
//! A `FINALIZE` or `CLOSE_HANDLE`
//! step is an invariant fault: resources and handles are noncopyable, which the validators
//! enforce. The walk keeps its state in runtime-owned scratch (two explicit stacks) and never recurses, so its native stack is constant whatever the graph's depth
//! (MEM-3). The scratch allocates only for a block with counted children; a root whose layout holds no counted child (a String, Bytes or a List
//! of plain elements) is copied as one allocation and its bytes, with no walk.
//!
//! Into a bounded arena (MEM-6) the copy is charged before anything is copied: every block
//! of the tree once per reference to it, independently of sharing, at its descriptor's
//! `logical` size (per element: times its u64 count). A copy the budget does not cover
//! returns [`STATUS_EXHAUSTED`] and copies nothing; the caller records the failure.
//!
//! Into a bounded arena, every copied sequence block ([`LAYOUT_SEQUENCE`]) and Map, Set or SortedMap block
//! ([`LAYOUT_KEYED`]) is a tracked arena block, unshared, except a Map's or Set's index and
//! anything below Slots or an environment ([`LAYOUT_UNTRACKED_BELOW`]); the root is
//! tracked when the caller says so. A Map's or Set's index is never charged. A detach of a
//! tracked arena block copies it within its arena, charged its count.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;

use crate::alloc::{DOMAIN_HEAP, Header, payload};
use crate::context::TaskCtx;
use crate::count::check_live_block;
use crate::region::{ALLOC_TRACKED, Region, is_tracked, tracked_region};
use crate::release::{
    LAYOUT_COLUMNS, LAYOUT_KEYED, LAYOUT_LOGICAL_PER_ELEMENT, LAYOUT_SEQUENCE, LAYOUT_SEQUENTIAL,
    LAYOUT_UNTRACKED_BELOW, LOGICAL_UNSUPPORTED,
};
use crate::release::{
    Layout, NO_OFFSET, STEP_CLOSE_HANDLE, STEP_COLUMN, STEP_FINALIZE, STEP_LIVE_BIT, STEP_LOOP,
    STEP_NONNULL, STEP_RELEASE, STEP_RELEASE_LAST, STEP_RELEASE_ROOT, STEP_TAG, STEP_THAW, Step,
    step_at, variant_inactive,
};
use crate::{STATUS_EXHAUSTED, STATUS_OK, fault};

/// A new block of `original`'s layout, capacity and alignment holding its bytes, with one
/// holder: in `destination`, a tracked arena block when `tracked`.
unsafe fn clone_block(
    ctx: &mut TaskCtx,
    original: *mut Header,
    destination: *mut Region,
    tracked: bool,
) -> *mut Header {
    // SAFETY: `original` is a live headered block; the copy holds all its payload data.
    unsafe {
        if (*ctx.descriptor((*original).index())).finalizer.is_some() {
            fault("a unique copy reached a resource, which is noncopyable");
        }
        let (capacity, mut align) = ctx.heap().usable(original);
        // Fixed arena blocks omit extent metadata and use exactly the descriptor size.
        let layout = &*ctx.descriptor((*original).index());
        let capacity = if !destination.is_null() && layout.flags & crate::LAYOUT_NO_EXTENT != 0 {
            align = layout.align;
            layout.size
        } else {
            capacity
        };
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("copied_blocks", 1);
            ctx.heap().measurement.copied_blocks += 1;
            ctx.heap().measurement.record("copied_bytes", capacity);
            ctx.heap().measurement.copied_bytes += capacity;
        }
        let copy = if destination.is_null() {
            crate::alloc::hirsel_rt_v2_alloc(
                ctx,
                (*original).index(),
                capacity,
                align,
                DOMAIN_HEAP,
                0,
            )
        } else {
            crate::region::hirsel_rt_v2_arena_alloc(
                ctx,
                destination,
                (*original).index(),
                capacity,
                align,
                if tracked { ALLOC_TRACKED } else { 0 },
            )
        };
        ptr::copy_nonoverlapping(payload(original), payload(copy), capacity as usize);
        copy
    }
}

/// One stretch of a drop program to visit: steps `pc..end` over the value at `base`, inside
/// a loop element `index` (a column's element). A loop queues one cursor for all its
/// elements, highest index first, rather than one work item per element.
#[derive(Clone, Copy)]
struct Span {
    pc: usize,
    end: usize,
    base: *mut u8,
    index: u64,
    remaining: u64,
    stride: u64,
}

/// Visits every counted holder slot of the value at `value`, read through `layout`'s program.
/// The value's own steps run without allocating; only a loop or column queues spans.
pub(crate) unsafe fn holders(
    layout: *const Layout,
    value: *mut u8,
    visit: &mut dyn FnMut(*mut u8),
) {
    unsafe { holder_views(layout, value, &mut |slot, _| visit(slot)) }
}

unsafe fn holder_views(
    layout: *const Layout,
    value: *mut u8,
    visit: &mut dyn FnMut(*mut u8, bool),
) {
    // SAFETY: `layout` is the descriptor of the value at `value`.
    unsafe {
        let steps: *const Step = (*layout).steps;
        let count = (*layout).step_count as usize;
        let mut work: Vec<Span> = Vec::new();
        let mut span = Span {
            pc: 0,
            end: count,
            base: value,
            index: 0,
            remaining: 1,
            stride: 0,
        };
        loop {
            let Span {
                mut pc,
                end,
                base,
                index,
                ..
            } = span;
            while pc < end {
                let step = &*steps.add(pc);
                pc += 1;
                let at = step_at(step, base, index);
                match step.kind {
                    STEP_RELEASE | STEP_RELEASE_LAST => visit(at, step.stride == 24),
                    STEP_RELEASE_ROOT => visit(at, false),
                    STEP_TAG | STEP_NONNULL | STEP_LIVE_BIT => {
                        if variant_inactive(step, at) {
                            pc += step.span as usize;
                        }
                    }
                    STEP_LOOP => {
                        let elements = if step.count_offset == NO_OFFSET {
                            step.count
                        } else {
                            base.add(step.count_offset as usize)
                                .cast::<u64>()
                                .read_unaligned()
                        };
                        // Rust alloc/src/slice.rs:425-432 (a7c37f9f0ee5) clones
                        // elements directly, without an element-sized worklist.
                        // Our descriptor walk keeps one reverse cursor instead:
                        // nested loops/columns need scratch proportional to nesting.
                        if elements != 0 && step.span != 0 {
                            let element = elements - 1;
                            work.push(Span {
                                pc,
                                end: pc + step.span as usize,
                                base: at.wrapping_add(element.wrapping_mul(step.stride) as usize),
                                index: element,
                                remaining: elements,
                                stride: step.stride,
                            });
                        }
                        pc += step.span as usize;
                    }
                    STEP_COLUMN => {
                        let capacity = base
                            .add(step.count_offset as usize)
                            .cast::<u64>()
                            .read_unaligned();
                        let offset = step
                            .offset
                            .wrapping_add(capacity.wrapping_mul(step.count))
                            .wrapping_add(index.wrapping_mul(step.stride));
                        work.push(Span {
                            pc,
                            end: pc + step.span as usize,
                            base: base.wrapping_add(offset as usize),
                            index,
                            remaining: 1,
                            stride: 0,
                        });
                        pc += step.span as usize;
                    }
                    STEP_FINALIZE | STEP_CLOSE_HANDLE => {
                        fault("a unique copy reached a resource or handle, which is noncopyable");
                    }
                    STEP_THAW => fault("a holder walk entered a Shared root's own program"),
                    _ => fault("a drop step has an unknown kind"),
                }
            }
            match work.last_mut() {
                Some(next) => {
                    span = *next;
                    if next.remaining > 1 {
                        next.remaining -= 1;
                        next.index -= 1;
                        next.base = next.base.wrapping_sub(next.stride as usize);
                    } else {
                        work.pop();
                    }
                }
                None => return,
            }
        }
    }
}

/// Whether `slot`, a holder slot of a block whose descriptor is `layout` and whose payload is
/// at `payload`, is a Map's or Set's index (`LAYOUT_KEYED`): every holder but the entries at
/// offset 0.
///
/// # Safety
/// `layout` is a live descriptor.
unsafe fn keyed_index(layout: *const Layout, payload: *mut u8, slot: *mut u8) -> bool {
    // SAFETY: caller contract.
    unsafe { (*layout).flags & LAYOUT_KEYED != 0 && slot != payload }
}

/// A source block and, for String/Bytes, its compact interval. The charge walk
/// memoizes subtree sizes; the copy walk intentionally copies every holder.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
struct CopyKey {
    block: usize,
    view: Option<(u64, u64)>,
}

/// An interval needing compact storage, or None for a whole byte sequence. Flat
/// owners also become ordinary byte sequences: unrelated fields never follow a view.
unsafe fn view_key(ctx: &TaskCtx, slot: *mut u8, slice: bool) -> CopyKey {
    unsafe {
        let block = slot.cast::<*mut Header>().read_unaligned();
        let view = if slice {
            let view = slot.cast::<crate::views::Slice>().read_unaligned();
            let layout = &*ctx.descriptor((*block).index());
            let whole = layout.flags & LAYOUT_SEQUENCE != 0
                && layout.step_count == 0
                && layout.logical == 1
                && view.start == 8
                && view.length == payload(block).cast::<u64>().read_unaligned();
            (!whole).then_some((view.start, view.length))
        } else {
            None
        };
        CopyKey {
            block: block.expose_provenance(),
            view,
        }
    }
}

// Rust alloc/src/slice.rs:446-459 (a7c37f9f0ee5): allocate for the slice length,
// then copy that interval. Hirsel additionally writes its sequence header and uses
// the explicit heap/arena destination.
unsafe fn clone_view(
    ctx: &mut TaskCtx,
    block: *mut Header,
    start: u64,
    length: u64,
    destination: *mut Region,
    tracked: bool,
) -> *mut Header {
    unsafe {
        let size = length
            .checked_add(16)
            .unwrap_or_else(|| fault("a slice copy extent overflow"));
        let copy = if destination.is_null() {
            crate::alloc::hirsel_rt_v2_alloc(ctx, 0, size, 8, DOMAIN_HEAP, 0)
        } else {
            crate::region::hirsel_rt_v2_arena_alloc(
                ctx,
                destination,
                0,
                size,
                8,
                if tracked { ALLOC_TRACKED } else { 0 },
            )
        };
        payload(copy).cast::<u64>().write_unaligned(length);
        payload(copy).add(8).cast::<u64>().write_unaligned(length);
        ptr::copy_nonoverlapping(
            payload(block).add(8 + start as usize),
            payload(copy).add(16),
            length as usize,
        );
        #[cfg(feature = "measurement")]
        {
            let measurement = &mut ctx.heap().measurement;
            measurement.record("copied_blocks", 1);
            measurement.copied_blocks += 1;
            measurement.record("copied_bytes", size);
            measurement.copied_bytes += size;
        }
        copy
    }
}

/// The deep copy walk: each slot of `root` naming a counted block is rewritten to that
/// fresh copy, and then every copy's slots in turn. Each incoming holder gets its
/// own copy (MODE-2), so a diamond becomes a tree without changing the source.
/// Ordinary shallow copies and nested Shared roots still acquire holder credits. Into a bounded
/// arena, the first block copied is tracked when `first` says so; every other sequence, Map
/// or Set block is unless it is a Map's or Set's index or an untracked-below block holds it,
/// directly or not, or `track` is false for the root's. A tracked copy whose layout has a drop
/// program is scheduled for the arena's exit, since pushes may move heap credits into it.
unsafe fn copy_graph(
    raw: *mut TaskCtx,
    destination: *mut Region,
    first: Option<bool>,
    track: bool,
    root: impl FnOnce(&mut dyn FnMut(*mut u8, bool)),
) {
    // SAFETY: caller contract of the entries below.
    unsafe {
        let ctx = &mut *raw;
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("deep_copies", 1);
            ctx.heap().measurement.deep_copies += 1;
        }
        let diagnostic = ctx.heap().diagnostic;
        // Each copy with holders still to walk, and whether the sequences below it are tracked.
        let pending: RefCell<Vec<(*mut Header, bool)>> = RefCell::new(Vec::new());
        // The copy whose holder slots are being walked, if any: a Map's or Set's index is not
        // tracked.
        let walked: std::cell::Cell<*mut Header> = std::cell::Cell::new(ptr::null_mut());
        // Only a bounded arena's charges observe tracking (MEM-6).
        let arena = !destination.is_null() && (*destination).bound != 0;
        let below = std::cell::Cell::new(track && arena);
        let mut first = first;
        let mut redirect = |slot: *mut u8, slice: bool| {
            let child = slot.cast::<*mut Header>().read_unaligned();
            if diagnostic {
                check_live_block(ctx, child);
            }
            // A Shared root is never copied: the copy holds one more root (MEM-11).
            if crate::shared::is_root(&*raw, child) {
                crate::roots::hirsel_rt_v2_root_dup(raw, child);
                return;
            }
            let key = view_key(&*raw, slot, slice);
            let flags = if key.view.is_some() {
                LAYOUT_SEQUENCE
            } else {
                (*(*raw).descriptor((*child).index())).flags
            };
            let parent = walked.get();
            let index = !parent.is_null()
                && keyed_index((*raw).descriptor((*parent).index()), payload(parent), slot);
            let below = below.get() && !index;
            let tracked = match first.take() {
                Some(root) => root && arena,
                None => {
                    below
                        && flags
                            & (LAYOUT_SEQUENCE | LAYOUT_KEYED | LAYOUT_COLUMNS | LAYOUT_SEQUENTIAL)
                            != 0
                }
            };
            let target = match key.view {
                Some((start, length)) => {
                    clone_view(&mut *raw, child, start, length, destination, tracked)
                }
                None => clone_block(&mut *raw, child, destination, tracked),
            };
            if tracked && (*(*raw).descriptor((*target).index())).step_count != 0 {
                crate::region::hirsel_rt_v2_region_schedule(
                    raw,
                    destination,
                    (*target).index(),
                    payload(target),
                );
            }
            // A copy whose layout holds nothing has no holders to walk.
            if (*(*raw).descriptor((*target).index())).step_count != 0 {
                let tracks = below && flags & LAYOUT_UNTRACKED_BELOW == 0;
                pending.borrow_mut().push((target, tracks));
            }
            slot.cast::<*mut Header>().write_unaligned(target);
            if key.view.is_some() {
                slot.add(8).cast::<u64>().write_unaligned(8);
            }
        };
        root(&mut redirect);
        loop {
            let Some((copy, tracks)) = pending.borrow_mut().pop() else {
                break;
            };
            below.set(tracks);
            walked.set(copy);
            holder_views(
                (*ctx).descriptor((*copy).index()),
                payload(copy),
                &mut redirect,
            );
        }
    }
}

/// Read a private live count independently of any map operation's cached count.
unsafe fn logical_count(layout: &Layout, at: *const u8) -> u64 {
    unsafe {
        let count = at.cast::<u64>().read_unaligned();
        if layout.logical_count.is_null() {
            return count;
        }
        let program = std::slice::from_raw_parts(layout.logical_count, 5);
        let [first, stride, tag, form, bit] = *<&[u64; 5]>::try_from(program).unwrap();
        let mut live = 0;
        for i in 0..count {
            let at = at.add((first + i * stride + tag) as usize);
            let some = match form {
                0 => at.cast::<u32>().read_unaligned() == 1,
                1 => at.cast::<u64>().read_unaligned() & (1 << bit) != 0,
                2 => !at.cast::<*const u8>().read_unaligned().is_null(),
                _ => fault("a logical-count tag form"),
            };
            live += u64::from(some);
        }
        live
    }
}

/// The MEM-6 charge of one block: its descriptor's logical size, per element times its count.
pub(crate) unsafe fn block_charge(ctx: &TaskCtx, block: *mut Header) -> u64 {
    // SAFETY: `block` is a live block of a descriptor of `ctx`.
    unsafe {
        let layout = &*ctx.descriptor((*block).index());
        if layout.logical == LOGICAL_UNSUPPORTED {
            fault("a bounded-arena copy reached a block with no logical size");
        }
        if layout.flags & LAYOUT_LOGICAL_PER_ELEMENT != 0 {
            let count = logical_count(layout, payload(block));
            layout.logical.saturating_mul(count)
        } else {
            layout.logical
        }
    }
}

/// The MEM-6 charge of the tree whose holder slots `root` visits: every block once per
/// reference to it, each block's subtree charged once and memoized, without recursion.
unsafe fn tree_charge(ctx: &TaskCtx, root: impl FnOnce(&mut dyn FnMut(*mut u8, bool))) -> u64 {
    // SAFETY: caller contract of the entries below.
    unsafe {
        let child = |slot: *mut u8, slice: bool| view_key(ctx, slot, slice);
        // A Map's or Set's index is not charged (`LAYOUT_KEYED`).
        let children = |block: CopyKey| {
            let mut out = Vec::new();
            if block.view.is_some() {
                return out;
            }
            let owner = ptr::with_exposed_provenance_mut::<Header>(block.block);
            let layout = ctx.descriptor((*owner).index());
            holder_views(layout, payload(owner), &mut |slot, slice| {
                if !keyed_index(layout, payload(owner), slot) {
                    out.push(child(slot, slice));
                }
            });
            out
        };
        let mut roots = Vec::new();
        root(&mut |slot, slice| roots.push(child(slot, slice)));
        let mut charges: HashMap<CopyKey, u64> = HashMap::new();
        let mut work: Vec<(CopyKey, bool)> = roots.iter().map(|&b| (b, false)).collect();
        while let Some((block, expanded)) = work.pop() {
            if charges.contains_key(&block) {
                continue;
            }
            let below = children(block);
            if !expanded {
                work.push((block, true));
                work.extend(
                    below
                        .into_iter()
                        .filter(|c| !charges.contains_key(c))
                        .map(|c| (c, false)),
                );
                continue;
            }
            let charge = match block.view {
                Some((_, length)) => length,
                None => block_charge(ctx, ptr::with_exposed_provenance_mut(block.block)),
            };
            let total = below
                .iter()
                .fold(charge, |total, c| total.saturating_add(charges[c]));
            charges.insert(block, total);
        }
        roots
            .iter()
            .fold(0u64, |total, r| total.saturating_add(charges[r]))
    }
}

/// Take a deep copy's charge from a bounded destination; false when it exceeds the budget.
unsafe fn charge_copy(
    ctx: &TaskCtx,
    destination: *mut Region,
    root: impl FnOnce(&mut dyn FnMut(*mut u8, bool)),
) -> bool {
    // SAFETY: `destination` is null or an active arena record.
    unsafe {
        if destination.is_null() || (*destination).bound == 0 {
            return true;
        }
        let charge = tree_charge(ctx, root);
        match (*destination).remaining.checked_sub(charge) {
            Some(remaining) => {
                (*destination).remaining = remaining;
                true
            }
            None => false,
        }
    }
}

/// `unique x`: a deep copy of the graph of counted heap blocks reachable from `block`, written
/// to `out_block` with one holder per copied block, immortal and static blocks included.
/// Repeated ordinary holders are copied independently; nested Shared roots stay shared.
/// A null destination selects the heap; otherwise every copied block belongs to
/// that active arena. Returns 0, or [`STATUS_EXHAUSTED`] when a bounded destination's budget
/// does not cover the copy; host exhaustion terminates environmentally. `site` is reserved,
/// as for `alloc`.
///
/// # Safety
/// `ctx` is live, `block` a live block the caller can read (it keeps its own credit), and
/// `out_block` writable; `destination` is null or an active arena record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_unique_copy(
    ctx: *mut TaskCtx,
    block: *mut Header,
    destination: *mut Region,
    flags: u32,
    _site: u64,
    out_block: *mut *mut Header,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        if (*ctx).heap().diagnostic {
            check_live_block(&*ctx, block);
        }
        let charged = charge_copy(&*ctx, destination, |visit| {
            let mut slot = block;
            visit((&raw mut slot).cast::<u8>(), false);
        });
        if !charged {
            return STATUS_EXHAUSTED;
        }
        if flags & !ALLOC_TRACKED != 0 {
            fault("a unique copy has unknown flags");
        }
        // The root is tracked as the caller says; the sequences below it always are.
        let root_tracked = flags & ALLOC_TRACKED != 0;
        // A block whose layout holds no counted child is the whole graph: one allocation and
        // its bytes, with no holder walk and no visited map. A Shared root is never copied.
        if (*(*ctx).descriptor((*block).index())).step_count == 0
            && !crate::shared::is_root(&*ctx, block)
        {
            #[cfg(feature = "measurement")]
            {
                let measurement = &mut (*ctx).heap().measurement;
                measurement.record("deep_copies", 1);
                measurement.deep_copies += 1;
            }
            let arena = !destination.is_null() && (*destination).bound != 0;
            out_block.write(clone_block(
                &mut *ctx,
                block,
                destination,
                root_tracked && arena,
            ));
            return STATUS_OK;
        }
        let mut root: *mut Header = ptr::null_mut();
        let walk = |redirect: &mut dyn FnMut(*mut u8, bool)| {
            let slot = (&raw mut root).cast::<u8>();
            slot.cast::<*mut Header>().write_unaligned(block);
            redirect(slot, false);
        };
        copy_graph(ctx, destination, Some(root_tracked), true, walk);
        out_block.write(root);
        STATUS_OK
    }
}

/// `unique x` of an inline value (an aggregate, variant or inline array holding blocks):
/// `value` is a bit copy of the source, whose holder slots (read through `layout`'s
/// program) still name the source's blocks without a credit; each is rewritten in place to a
/// deep copy at `destination`, as `unique_copy` makes them, independently per holder. Returns
/// 0, or [`STATUS_EXHAUSTED`] as `unique_copy` does; host exhaustion terminates environmentally.
///
/// # Safety
/// `ctx` is live, `layout` the descriptor of the value at `value`, which the caller owns and
/// may write, and whose slots name live blocks the source keeps its credits for.
/// `destination` is null (heap) or an active arena record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_unique_copy_value(
    ctx: *mut TaskCtx,
    index: u32,
    value: *mut u8,
    destination: *mut Region,
    _site: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        let layout = (*ctx).descriptor(index);
        if !charge_copy(&*ctx, destination, |visit| {
            holder_views(layout, value, visit)
        }) {
            return STATUS_EXHAUSTED;
        }
        let below = (*layout).flags & LAYOUT_UNTRACKED_BELOW == 0;
        // A single String/Bytes holder is already the entire byte-copy graph.
        // Rust alloc/src/slice.rs:446-459 (a7c37f9f0ee5) likewise allocates
        // once and copies the interval. Keep the same precharge above and the
        // existing whole-block/compact-view allocation semantics below.
        if (*layout).size == 24 && (*layout).step_count == 1 {
            let step = &*(*layout).steps;
            if matches!(step.kind, STEP_RELEASE | STEP_RELEASE_LAST)
                && step.offset == 0
                && step.stride == 24
            {
                let child = value.cast::<*mut Header>().read_unaligned();
                if (*ctx).heap().diagnostic {
                    check_live_block(&*ctx, child);
                }
                if !crate::shared::is_root(&*ctx, child) {
                    let key = view_key(&*ctx, value, true);
                    // A compact view has only bytes; a whole slice's owner
                    // was certified childless by view_key. No walk or memo
                    // table is needed in either case.
                    let tracked = below && !destination.is_null() && (*destination).bound != 0;
                    #[cfg(feature = "measurement")]
                    {
                        let measurement = &mut (*ctx).heap().measurement;
                        measurement.record("deep_copies", 1);
                        measurement.deep_copies += 1;
                    }
                    let copy = match key.view {
                        Some((start, length)) => {
                            let copy =
                                clone_view(&mut *ctx, child, start, length, destination, tracked);
                            value.add(8).cast::<u64>().write_unaligned(8);
                            copy
                        }
                        None => clone_block(&mut *ctx, child, destination, tracked),
                    };
                    value.cast::<*mut Header>().write_unaligned(copy);
                    return STATUS_OK;
                }
            }
        }
        copy_graph(ctx, destination, None, below, |redirect| {
            holder_views(layout, value, redirect)
        });
        STATUS_OK
    }
}

/// Retain the child holders of a freshly copied block using its drop program.
///
/// # Safety
/// `copy` has initialized holder slots borrowed from a live source. The caller
/// owns the task role and adds exactly one credit for each copied slot.
pub(crate) unsafe fn retain_children(ctx: *mut TaskCtx, copy: *mut Header) {
    unsafe {
        let diagnostic = (*ctx).heap_ref().diagnostic;
        holders(
            (*ctx).descriptor((*copy).index()),
            payload(copy),
            &mut |slot| {
                let child = slot.cast::<*mut Header>().read_unaligned();
                if diagnostic {
                    check_live_block(&*ctx, child);
                }
                crate::roots::dup_holder(ctx, child);
            },
        );
    }
}

/// A copy-on-write detach (OWN-4) of the sequence `block` into a fresh heap block of at least
/// `extent` payload bytes at `align`, for a reserve (`hirsel_rt_v2_seq_grow`): its first `used`
/// bytes (the count and the elements) are copied, each counted child gains a holder, and the
/// caller's credit of `block`, which is not unique, is given back. Returns the fresh block
/// and its exact usable bytes from the allocation's chosen class.
///
/// # Safety
/// `ctx` is live, `block` a live sequence block that is not unique, of which the caller holds
/// one credit, with `used` initialized payload bytes; `extent` is at least `used`.
pub(crate) unsafe fn detach_sized(
    ctx: *mut TaskCtx,
    block: *mut Header,
    used: u64,
    extent: u64,
    align: u64,
) -> (*mut Header, u64) {
    // SAFETY: caller contract.
    unsafe {
        let raw = ctx;
        let ctx = &mut *ctx;
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("cow_detaches", 1);
            ctx.heap().measurement.cow_detaches += 1;
            ctx.heap().measurement.record("copied_blocks", 1);
            ctx.heap().measurement.copied_blocks += 1;
            ctx.heap().measurement.record("copied_bytes", used);
            ctx.heap().measurement.copied_bytes += used;
        }
        if ctx.heap().diagnostic {
            check_live_block(&*ctx, block);
        }
        if (*ctx.descriptor((*block).index())).finalizer.is_some() {
            fault("a detach reached a resource, which is noncopyable");
        }
        let (copy, capacity) = crate::alloc::allocate_heap(raw, (*block).index(), extent, align);
        if ctx.heap().diagnostic {
            ptr::write_bytes(
                payload(copy),
                crate::alloc::UNINIT_POISON,
                capacity as usize,
            );
        }
        ptr::copy_nonoverlapping(payload(block), payload(copy), used as usize);
        retain_children(raw, copy);
        // Not unique, so the credit never frees it.
        crate::release::release_block(ctx, block, ptr::null(), 0, 0);
        (copy, capacity)
    }
}

/// A copy-on-write detach (OWN-4): a new heap block of `block`'s layout, capacity and bytes
/// with one holder, written to `out_block`; each counted child the copy holds gains one
/// holder, so the original and the copy share every child. A static or immortal block is
/// copied too, since the caller writes the copy. A tracked arena block is copied within its
/// arena instead, as an unshared tracked block charged its count (MEM-6), scheduled for the
/// arena's exit when its layout has a drop program; [`STATUS_EXHAUSTED`], with nothing
/// copied, when the charge exceeds the arena's budget. Otherwise returns 0; host exhaustion
/// terminates environmentally. `site` is reserved, as for `unique_copy`.
///
/// # Safety
/// `ctx` is live, `block` a live block the caller can read (it keeps its own credit), and
/// `out_block` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_detach(
    ctx: *mut TaskCtx,
    block: *mut Header,
    _site: u64,
    out_block: *mut *mut Header,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        let raw = ctx;
        let ctx = &mut *ctx;
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("cow_detaches", 1);
            ctx.heap().measurement.cow_detaches += 1;
        }
        if ctx.heap().diagnostic {
            check_live_block(&*ctx, block);
        }
        let tracked = is_tracked(block);
        let region = if tracked {
            tracked_region(block)
        } else {
            ptr::null_mut()
        };
        if tracked && !crate::region::charge_arena(region, block_charge(ctx, block)) {
            return STATUS_EXHAUSTED;
        }
        out_block.write(copy_held(raw, block, region, tracked));
        STATUS_OK
    }
}

/// A copy of `block` at its capacity, in the arena `region` and tracked when `tracked`, else
/// on the heap, whose children each gain a holder; uncharged.
///
/// # Safety
/// As [`hirsel_rt_v2_detach`]; `region` is `block`'s tracked arena when `tracked`.
pub(crate) unsafe fn copy_held(
    raw: *mut TaskCtx,
    block: *mut Header,
    region: *mut Region,
    tracked: bool,
) -> *mut Header {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *raw;
        let copy = clone_block(ctx, block, region, tracked);
        retain_children(raw, copy);
        let index = (*copy).index();
        if tracked && (*ctx.descriptor(index)).step_count != 0 {
            crate::region::hirsel_rt_v2_region_schedule(raw, region, index, payload(copy));
        }
        copy
    }
}
