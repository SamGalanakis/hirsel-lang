//! Exact `Shared` root counts (MEM-11, S-4, S-5; lane R3): a root never saturates, its
//! count moves into and out of the overflow table at the count bound, a thaw's sole-holder
//! test answers by the exact count, and a dying root's walk releases its nested roots in
//! drop order.

use crate::support;

use hirsel_rt::context::CONFIG_DIAGNOSTIC;
use hirsel_rt::roots::{count as exact, hirsel_rt_v2_root_dup, hirsel_rt_v2_root_release};
use hirsel_rt::shared::hirsel_rt_v2_thaw_turn;
use hirsel_rt::{
    Config, Header, LAYOUT_FROZEN, LAYOUT_MAGIC, LAYOUT_VERSION, Layout, ROOT_OVERFLOW,
    STEP_RELEASE_ROOT, STEP_THAW, Step,
};
use support::*;

/// A diagnostic context whose count bound is `max` (`Config::count_max`).
fn bounded(max: u64) -> Ctx {
    Ctx::with(Config {
        flags: CONFIG_DIAGNOSTIC,
        count_max: max,
        ..Config::new()
    })
}

/// A root block layout (`LAYOUT_FROZEN`) with the drop program `steps`.
fn frozen(steps: Vec<Step>) -> &'static Layout {
    let steps: &'static [Step] = Box::leak(steps.into_boxed_slice());
    leak(Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: steps.as_ptr(),
        step_count: steps.len() as u64,
        field_count: 1,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: u64::MAX,
        flags: LAYOUT_FROZEN,
    })
}

/// A root block layout freezing a scalar: nothing to thaw or walk.
fn scalar_root() -> &'static Layout {
    frozen(Vec::new())
}

/// A root block layout freezing a value that holds one nested root at offset 0.
fn nesting_root() -> &'static Layout {
    let value = layout(8, 1, None, vec![step(STEP_RELEASE_ROOT, 0, 0)]);
    let thaw = Step {
        layout: value,
        ..step(STEP_THAW, 0, 0)
    };
    frozen(vec![thaw, step(STEP_RELEASE_ROOT, 0, 0)])
}

fn dup(ctx: &Ctx, root: *mut Header) {
    unsafe { hirsel_rt_v2_root_dup(ctx.0, root) };
}

fn release(ctx: &Ctx, root: *mut Header) {
    unsafe { hirsel_rt_v2_root_release(ctx.0, root) };
}

fn roots(ctx: &Ctx, root: *mut Header) -> u64 {
    unsafe { exact(&*ctx.0, root) }
}

/// `shared.thaw`'s sole-holder test outside free mode.
fn last(ctx: &Ctx, root: *mut Header) -> bool {
    let mut out = 0u8;
    assert_eq!(unsafe { hirsel_rt_v2_thaw_turn(ctx.0, &mut out, root) }, 0);
    out == 1
}

#[test]
fn a_root_count_stays_exact_through_the_overflow_table() {
    // A bound of 3 leaves finite counts 1 and 2, so the third root overflows.
    let ctx = bounded(3);
    let layout = scalar_root();
    let root = alloc_block(ctx.0, layout, 8);
    assert!(last(&ctx, root));
    dup(&ctx, root);
    assert_eq!((count(root), roots(&ctx, root)), (2, 2));
    dup(&ctx, root);
    assert_eq!(
        (count(root), roots(&ctx, root)),
        (ROOT_OVERFLOW, 3),
        "the root past the bound enters the overflow table"
    );
    for expected in 4..=1000 {
        dup(&ctx, root);
        assert_eq!(roots(&ctx, root), expected);
    }
    assert_eq!(ctx.stats().immortalized_blocks, 0, "a root never saturates");
    assert!(!last(&ctx, root), "an overflowed root is never the last");
    for expected in (2..1000).rev() {
        release(&ctx, root);
        assert_eq!(roots(&ctx, root), expected);
    }
    assert_eq!(
        count(root),
        2,
        "back at the bound, the count leaves the table for the header"
    );
    release(&ctx, root);
    assert!(last(&ctx, root), "the exact count reaches one again");
    assert_eq!(ctx.stats().live_blocks, 1);
    release(&ctx, root);
    assert_eq!(ctx.stats().live_blocks, 0, "the last root frees the block");
}

#[test]
fn entry_and_exit_alternate_at_the_bound() {
    let ctx = bounded(3);
    let root = alloc_block(ctx.0, scalar_root(), 8);
    dup(&ctx, root);
    for _ in 0..100 {
        dup(&ctx, root);
        assert_eq!((count(root), roots(&ctx, root)), (ROOT_OVERFLOW, 3));
        release(&ctx, root);
        assert_eq!((count(root), roots(&ctx, root)), (2, 2));
    }
    release(&ctx, root);
    release(&ctx, root);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_dying_root_releases_its_nested_roots_in_its_walk() {
    let ctx = bounded(3);
    let (outer, inner) = (nesting_root(), scalar_root());
    // The nested root has a holder of its own besides the outer value's.
    let held = alloc_block(ctx.0, inner, 8);
    dup(&ctx, held);
    let root = alloc_block(ctx.0, outer, 8);
    write_ref(root, 0, held);
    assert_eq!(ctx.stats().live_blocks, 2);
    release(&ctx, root);
    assert_eq!(
        (ctx.stats().live_blocks, roots(&ctx, held)),
        (1, 1),
        "the outer walk gave up the root its value held, and only that"
    );
    release(&ctx, held);
    assert_eq!(ctx.stats().live_blocks, 0);
    // A nested root whose last holder is the outer value dies in the same walk.
    let alone = alloc_block(ctx.0, inner, 8);
    let root = alloc_block(ctx.0, outer, 8);
    write_ref(root, 0, alone);
    release(&ctx, root);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_overflowed_nested_root_counts_down_in_the_walk() {
    let ctx = bounded(3);
    let (outer, inner) = (nesting_root(), scalar_root());
    let held = alloc_block(ctx.0, inner, 8);
    dup(&ctx, held);
    dup(&ctx, held);
    assert_eq!(count(held), ROOT_OVERFLOW);
    let root = alloc_block(ctx.0, outer, 8);
    write_ref(root, 0, held);
    release(&ctx, root);
    assert_eq!((count(held), roots(&ctx, held)), (2, 2));
    release(&ctx, held);
    release(&ctx, held);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_ordinary_release_of_a_root_is_refused_in_diagnostic_mode() {
    if in_child("an_ordinary_release_of_a_root_is_refused_in_diagnostic_mode") {
        let ctx = bounded(0);
        let layout = scalar_root();
        let root = alloc_block(ctx.0, layout, 8);
        unsafe {
            hirsel_rt::release::hirsel_rt_v2_release(
                ctx.0,
                root,
                index(ctx.0, layout),
                std::ptr::null(),
                0,
                0,
            )
        };
        return;
    }
    assert_child_aborts(
        "an_ordinary_release_of_a_root_is_refused_in_diagnostic_mode",
        "an ordinary count operation reached a Shared root",
    );
}
