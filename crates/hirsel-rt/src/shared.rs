//! `Shared[T]` roots (MEM-11).
//!
//! A root is a heap block whose payload descriptor carries [`LAYOUT_FROZEN`]: the frozen value
//! lies inline at offset 0, and the block's header count is the root count: exact and atomic,
//! touched only through [`crate::roots`] (`hirsel_rt_v2_root_dup`, `hirsel_rt_v2_root_release`
//! and the walks' `STEP_RELEASE_ROOT`). While a root lives, every counted block the value reaches is immortal
//! ([`COUNT_IMMORTAL`]): a read that copies a part writes no count, and copy on write detaches
//! instead of writing in place. Freezing stops at static and region blocks, which are never
//! written, and at nested roots, which keep their own counts.
//!
//! The frozen graph is a tree: `shared.freeze` takes a `unique` value, whose counted blocks
//! have one holder each. Thawing the last root, or releasing it ([`STEP_THAW`]), restores every
//! one of them to [`COUNT_UNIQUE`](crate::count) before the value is moved out or released. The
//! walks keep their stack in the run context and never recurse, so the native stack is constant
//! whatever the graph's depth (MEM-3).

use crate::alloc::{DOMAIN_HEAP, Header, payload};
use crate::context::TaskCtx;
use crate::copy::holders;
use crate::count::{COUNT_IMMORTAL, check_live_block};
use crate::fault;
use crate::release::{LAYOUT_FROZEN, Layout, STEP_THAW};

/// Whether `block` is a `Shared` root block.
///
/// # Safety
/// `block` is a live headered block of `ctx`.
pub(crate) unsafe fn is_root(ctx: &TaskCtx, block: *mut Header) -> bool {
    // SAFETY: caller contract.
    unsafe { (*ctx.descriptor((*block).index())).flags & LAYOUT_FROZEN != 0 }
}

/// The layout of the value a root block freezes: its program's leading `STEP_THAW` names it.
/// A root whose value holds no counted block has no such step and nothing to walk.
unsafe fn frozen_value(ctx: &TaskCtx, root: *mut Header) -> Option<*const Layout> {
    // SAFETY: `root` is a live root block.
    unsafe {
        let layout = &*ctx.descriptor((*root).index());
        if layout.flags & LAYOUT_FROZEN == 0 {
            fault("a freeze or thaw of a block that is not a Shared root");
        }
        (layout.step_count > 0 && (*layout.steps).kind == STEP_THAW).then(|| (*layout.steps).layout)
    }
}

/// Visit each counted heap block of the frozen graph of the value at `value` of `layout`
/// (static and region blocks and nested roots excluded) once, parents first, setting its count
/// from `from` to `to`. A block whose count is not `from` breaks the tree invariant.
unsafe fn recount(ctx: &mut TaskCtx, layout: *const Layout, value: *mut u8, from: u32, to: u32) {
    // SAFETY: `value` is a live value of `layout` whose counted slots name live blocks.
    unsafe {
        let mut stack = std::mem::take(&mut ctx.frozen);
        let shared: &TaskCtx = ctx;
        let diagnostic = shared.heap_ref().diagnostic;
        let visit = |stack: &mut Vec<*mut Header>, slot: *mut u8| {
            let child = slot.cast::<*mut Header>().read_unaligned();
            if diagnostic {
                check_live_block(shared, child);
            }
            if (*child).domain() != DOMAIN_HEAP || is_root(shared, child) {
                return;
            }
            if (*child).count != from {
                fault(if from == COUNT_IMMORTAL {
                    "a thaw reached a block that is not frozen"
                } else {
                    "a freeze reached a block with more than one holder"
                });
            }
            (*child).count = to;
            stack.push(child);
        };
        holders(layout, value, &mut |slot| visit(&mut stack, slot));
        while let Some(block) = stack.pop() {
            let layout = shared.descriptor((*block).index());
            holders(layout, payload(block), &mut |slot| visit(&mut stack, slot));
        }
        ctx.frozen = stack;
    }
}

/// `STEP_THAW` of a dying root's program: restore ordinary unique counts in the frozen value
/// at `value` of `layout` before the following steps release it. Never fails or allocates
/// beyond the context's retained stack.
///
/// # Safety
/// `value` is the frozen value of a root block whose last credit was just consumed.
pub(crate) unsafe fn thaw_value(ctx: &mut TaskCtx, layout: *const Layout, value: *mut u8) {
    // SAFETY: caller contract.
    unsafe {
        recount(
            ctx,
            layout,
            value,
            COUNT_IMMORTAL,
            crate::count::COUNT_UNIQUE,
        )
    }
}

/// `shared.freeze`: `block` is a fresh root block, one root, holding the value moved into it.
/// Every counted heap block the value reaches becomes immortal.
///
/// # Safety
/// `ctx` is live and `block` a live root block of it whose value the caller owned uniquely.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_freeze(ctx: *mut TaskCtx, block: *mut Header) {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        if ctx.heap().diagnostic {
            check_live_block(ctx, block);
            if (*block).count != 1 {
                fault("a freeze of a root block with more than one root");
            }
        }
        if let Some(layout) = frozen_value(ctx, block) {
            recount(
                ctx,
                layout,
                payload(block),
                crate::count::COUNT_UNIQUE,
                COUNT_IMMORTAL,
            );
        }
        #[cfg(feature = "diagnostic")]
        crate::root_trace::frozen(block);
    }
}

/// `shared.thaw` of the last root: every frozen block of the value in `block` has one holder
/// again. The caller then moves the value out and frees the root block shallowly.
///
/// # Safety
/// `ctx` is live and `block` a live root block of it with exactly one root, the caller's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_thaw(ctx: *mut TaskCtx, block: *mut Header) {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        if ctx.heap().diagnostic {
            check_live_block(ctx, block);
        }
        if !crate::roots::sole(block) {
            fault("a thaw of a root that is not the last");
        }
        if let Some(layout) = frozen_value(ctx, block) {
            thaw_value(ctx, layout, payload(block));
        }
        #[cfg(feature = "diagnostic")]
        crate::root_trace::retired(block);
    }
}

/// `$intrinsic.SharedThawTurn(root) -> bool`: whether the borrowed root is the last, which
/// `shared.thaw`'s compiled body tests before it thaws or gives the root back. Outside free
/// mode it is the exact count test alone and not a yield. In free mode it is an op whose
/// entered phase applies the task's pending releases first: its answer is recorded live and
/// replayed (a recorded `Ok` waits until the root is the last), and it yields.
///
/// # Safety
/// `ctx` is live, `out` writable and `root` a live root block of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_thaw_turn(
    ctx: *mut TaskCtx,
    out: *mut u8,
    root: *mut Header,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    // SAFETY: caller contract; the borrowed root stays live through any wait. The sole-holder
    // test acquires every other holder's release before the thaw restores interior counts; an
    // overflowed root is never the last.
    if unsafe { (*ctx).run_ref().tasks.is_some() } && crate::tasks::ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(ctx, crate::tasks::ordered::drops::thaw(ctx, root, out))
        };
    }
    let last = || unsafe { crate::roots::sole(root) };
    let answer = if crate::free::enter(ctx, hirsel_free::phase::Phase::Done) {
        let answer = crate::free::thaw(ctx, last);
        crate::free::done(ctx);
        // SAFETY: the context is live.
        unsafe { crate::tasks::yield_now(&mut *ctx) };
        answer
    } else {
        last()
    };
    // SAFETY: caller contract.
    unsafe { out.write(u8::from(answer)) };
    crate::STATUS_OK
}
