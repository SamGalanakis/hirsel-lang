//! Deep unique copy (`unique x`, a deep LIR `UniqueCopy`): every counted heap block reachable
//! from a root block, or from an inline value's holder slots, is copied once, so sharing
//! inside the copied graph is preserved. A copy-on-write
//! detach (a shallow `UniqueCopy`, OWN-4) copies the root block alone and gives each counted
//! child one more holder.
//!
//! Children are found through each block's own descriptor: its `RELEASE` steps are exactly
//! its counted holders (`TAG` and a niche's `NONNULL` select the active variant's, `LOOP` and `COLUMN` reach
//! elements).
//! A copy keeps the original's class capacity and bytes, then each counted slot is rewritten
//! to the child's copy, so every copy holds exactly the credits its slots name. Immortal and
//! static blocks are copied like any other: the copy is unique, so a `unique` parameter
//! writes it in place without a test (MODE-2), which a shared static block would not survive.
//! A `FINALIZE` or `CLOSE_HANDLE`
//! step is an invariant fault: resources and handles are noncopyable, which the validators
//! enforce. The walk keeps its state in runtime-owned scratch (a visited map and two explicit
//! stacks) and never recurses, so its native stack is constant whatever the graph's depth
//! (MEM-3).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;

use crate::alloc::{DOMAIN_HEAP, Header, payload};
use crate::context::RunCtx;
use crate::count::check_live_block;
use crate::release::{
    Layout, NO_OFFSET, STEP_CLOSE_HANDLE, STEP_COLUMN, STEP_FINALIZE, STEP_LIVE_BIT, STEP_LOOP,
    STEP_NONNULL, STEP_RELEASE, STEP_TAG, Step, variant_inactive,
};
use crate::{STATUS_OK, fault};

/// A new block of `original`'s layout, capacity and alignment holding its bytes, with one
/// holder.
unsafe fn clone_block(ctx: &mut RunCtx, original: *mut Header) -> *mut Header {
    // SAFETY: `original` is a live heap block; the copy is fresh storage of its capacity.
    unsafe {
        if (*ctx.descriptor((*original).index())).finalizer.is_some() {
            fault("a unique copy reached a resource, which is noncopyable");
        }
        let (capacity, align) = ctx.heap.usable(original);
        #[cfg(feature = "measurement")]
        {
            ctx.heap.measurement.record("copied_blocks", 1);
            ctx.heap.measurement.copied_blocks += 1;
            ctx.heap.measurement.record("copied_bytes", capacity);
            ctx.heap.measurement.copied_bytes += capacity;
        }
        let copy = crate::alloc::hirsel_rt_v2_alloc(
            ctx,
            (*original).index(),
            capacity,
            align,
            DOMAIN_HEAP,
            0,
        );
        ptr::copy_nonoverlapping(payload(original), payload(copy), capacity as usize);
        copy
    }
}

/// One stretch of a drop program to visit: steps `pc..end` over the value at `base`, inside
/// a loop element `index` (a column's element).
struct Span {
    pc: usize,
    end: usize,
    base: *mut u8,
    index: u64,
}

/// Visits every counted holder slot of the value at `value`, read through `layout`'s program.
unsafe fn holders(layout: *const Layout, value: *mut u8, visit: &mut dyn FnMut(*mut u8)) {
    // SAFETY: `layout` is the descriptor of the value at `value`.
    unsafe {
        let steps: *const Step = (*layout).steps;
        let count = (*layout).step_count as usize;
        // A pointer-free layout holds nothing; skip the work list's allocation.
        if count == 0 {
            return;
        }
        let mut work = vec![Span {
            pc: 0,
            end: count,
            base: value,
            index: 0,
        }];
        while let Some(Span {
            mut pc,
            end,
            base,
            index,
        }) = work.pop()
        {
            while pc < end {
                let step = &*steps.add(pc);
                pc += 1;
                let at = base.wrapping_add(step.offset as usize);
                match step.kind {
                    STEP_RELEASE => visit(at),
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
                        for element in 0..elements {
                            work.push(Span {
                                pc,
                                end: pc + step.span as usize,
                                base: at.wrapping_add(element.wrapping_mul(step.stride) as usize),
                                index: element,
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
                        });
                        pc += step.span as usize;
                    }
                    STEP_FINALIZE | STEP_CLOSE_HANDLE => {
                        fault("a unique copy reached a resource or handle, which is noncopyable");
                    }
                    _ => fault("a drop step has an unknown kind"),
                }
            }
        }
    }
}

/// The deep copy walk: each slot of `root` naming a counted block is rewritten to that
/// block's copy (made once), and then every copy's slots in turn. A block reached again gains
/// one holder instead of a second copy, so sharing inside the graph is kept.
unsafe fn copy_graph(raw: *mut RunCtx, root: impl FnOnce(&mut dyn FnMut(*mut u8))) {
    // SAFETY: caller contract of the entries below.
    unsafe {
        let ctx = &mut *raw;
        #[cfg(feature = "measurement")]
        {
            ctx.heap.measurement.record("deep_copies", 1);
            ctx.heap.measurement.deep_copies += 1;
        }
        let diagnostic = ctx.heap.diagnostic;
        let mut copies: HashMap<*mut Header, *mut Header> = HashMap::new();
        let pending: RefCell<Vec<*mut Header>> = RefCell::new(Vec::new());
        let mut redirect = |slot: *mut u8| {
            let child = slot.cast::<*mut Header>().read_unaligned();
            if diagnostic {
                check_live_block(ctx, child);
            }
            let target = match copies.get(&child) {
                Some(&target) => {
                    // Another holder of a block already copied.
                    crate::count::hirsel_rt_v2_dup(raw, target);
                    target
                }
                None => {
                    let target = clone_block(&mut *raw, child);
                    copies.insert(child, target);
                    pending.borrow_mut().push(target);
                    target
                }
            };
            slot.cast::<*mut Header>().write_unaligned(target);
        };
        root(&mut redirect);
        loop {
            let Some(copy) = pending.borrow_mut().pop() else {
                break;
            };
            holders(
                (*ctx).descriptor((*copy).index()),
                payload(copy),
                &mut redirect,
            );
        }
    }
}

/// `unique x`: a deep copy of the graph of counted heap blocks reachable from `block`, written
/// to `out_block` with one holder. Sharing inside the graph is preserved (each block is copied
/// once and every slot naming it names its copy), immortal and static blocks included.
/// Returns 0; host exhaustion terminates environmentally. `site` is reserved for future
/// bounded arena charging, as for `alloc`.
///
/// # Safety
/// `ctx` is live, `block` a live block the caller can read (it keeps its own credit), and
/// `out_block` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_unique_copy(
    ctx: *mut RunCtx,
    block: *mut Header,
    _site: u64,
    out_block: *mut *mut Header,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        if (*ctx).heap.diagnostic {
            check_live_block(&*ctx, block);
        }
        let mut root: *mut Header = ptr::null_mut();
        copy_graph(ctx, |redirect| {
            let slot = (&raw mut root).cast::<u8>();
            slot.cast::<*mut Header>().write_unaligned(block);
            redirect(slot);
        });
        out_block.write(root);
        STATUS_OK
    }
}

/// `unique x` of an inline value (an aggregate, variant or inline array holding blocks):
/// `value` is a bit copy of the source, whose holder slots (read through `layout`'s
/// program) still name the source's blocks without a credit; each is rewritten in place to a
/// deep copy, as `unique_copy` makes them, one visited map serving the whole value. Returns
/// 0; host exhaustion terminates environmentally.
///
/// # Safety
/// `ctx` is live, `layout` the descriptor of the value at `value`, which the caller owns and
/// may write, and whose slots name live blocks the source keeps its credits for.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_unique_copy_value(
    ctx: *mut RunCtx,
    index: u32,
    value: *mut u8,
    _site: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        let layout = (*ctx).descriptor(index);
        copy_graph(ctx, |redirect| holders(layout, value, redirect));
        STATUS_OK
    }
}

/// A copy-on-write detach (OWN-4) of the sequence `block` into a fresh heap block of at least
/// `extent` payload bytes at `align`, for a reserve (`hirsel_rt_v2_seq_grow`): its first `used`
/// bytes (the count and the elements) are copied, each counted child gains a holder, and the
/// caller's credit of `block`, which is not unique, is given back.
///
/// # Safety
/// `ctx` is live, `block` a live sequence block that is not unique, of which the caller holds
/// one credit, with `used` initialized payload bytes; `extent` is at least `used`.
pub(crate) unsafe fn detach_sized(
    ctx: *mut RunCtx,
    block: *mut Header,
    used: u64,
    extent: u64,
    align: u64,
) -> *mut Header {
    // SAFETY: caller contract.
    unsafe {
        let raw = ctx;
        let ctx = &mut *ctx;
        #[cfg(feature = "measurement")]
        {
            ctx.heap.measurement.record("cow_detaches", 1);
            ctx.heap.measurement.cow_detaches += 1;
            ctx.heap.measurement.record("copied_blocks", 1);
            ctx.heap.measurement.copied_blocks += 1;
            ctx.heap.measurement.record("copied_bytes", used);
            ctx.heap.measurement.copied_bytes += used;
        }
        if ctx.heap.diagnostic {
            check_live_block(&*ctx, block);
        }
        if (*ctx.descriptor((*block).index())).finalizer.is_some() {
            fault("a detach reached a resource, which is noncopyable");
        }
        let copy = ctx.heap.allocate((*block).index(), extent, align);
        if ctx.heap.diagnostic {
            ptr::write_bytes(
                payload(copy),
                crate::alloc::UNINIT_POISON,
                ctx.heap.capacity(copy) as usize,
            );
        }
        ptr::copy_nonoverlapping(payload(block), payload(copy), used as usize);
        let diagnostic = ctx.heap.diagnostic;
        holders(
            ctx.descriptor((*copy).index()),
            payload(copy),
            &mut |slot| {
                let child = slot.cast::<*mut Header>().read_unaligned();
                if diagnostic {
                    check_live_block(ctx, child);
                }
                crate::count::hirsel_rt_v2_dup(raw, child);
            },
        );
        // Not unique, so the credit never frees it.
        crate::release::release_block(ctx, block, ptr::null(), 0);
        copy
    }
}

/// A copy-on-write detach (OWN-4): a new heap block of `block`'s layout, capacity and bytes
/// with one holder, written to `out_block`; each counted child the copy holds gains one
/// holder, so the original and the copy share every child. A static or immortal block is
/// copied too, since the caller writes the copy. Returns 0; host exhaustion terminates
/// environmentally. `site` is reserved, as for `unique_copy`.
///
/// # Safety
/// `ctx` is live, `block` a live block the caller can read (it keeps its own credit), and
/// `out_block` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_detach(
    ctx: *mut RunCtx,
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
            ctx.heap.measurement.record("cow_detaches", 1);
            ctx.heap.measurement.cow_detaches += 1;
        }
        if ctx.heap.diagnostic {
            check_live_block(&*ctx, block);
        }
        let copy = clone_block(ctx, block);
        let diagnostic = ctx.heap.diagnostic;
        holders(
            ctx.descriptor((*copy).index()),
            payload(copy),
            &mut |slot| {
                let child = slot.cast::<*mut Header>().read_unaligned();
                if diagnostic {
                    check_live_block(ctx, child);
                }
                crate::count::hirsel_rt_v2_dup(raw, child);
            },
        );
        out_block.write(copy);
        STATUS_OK
    }
}
