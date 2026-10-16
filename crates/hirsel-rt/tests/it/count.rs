//! Plain task-local counts and sticky overflow immortality (MEM-1, OWN-6, OWN-7).

use crate::support;

use std::ptr;

use hirsel_rt::alloc::hirsel_rt_v2_alloc;
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::release::hirsel_rt_v2_release;
use hirsel_rt::{COUNT_IMMORTAL, Config, DOMAIN_HEAP, Header, Layout, finite_count_max};
use support::*;

fn alloc(ctx: &Ctx, layout: &Layout) -> *mut Header {
    let block =
        unsafe { hirsel_rt_v2_alloc(ctx.0, index(ctx.0, layout), layout.size, 8, DOMAIN_HEAP, 0) };
    block
}

fn dup(ctx: &Ctx, block: *mut Header) {
    unsafe { hirsel_rt_v2_dup(ctx.0, block) };
}

fn release(ctx: &Ctx, block: *mut Header, layout: &Layout) -> u8 {
    unsafe { hirsel_rt_v2_release(ctx.0, block, index(ctx.0, layout), ptr::null(), 0) }
}

fn unique(block: *mut Header) -> bool {
    count(block) == 1
}

#[test]
fn counts_go_up_and_down_and_free_at_zero() {
    let ctx = Ctx::diagnostic();
    let layout = leaf(8);
    let block = alloc(&ctx, layout);
    assert_eq!(count(block), 1);
    assert!(unique(block));
    dup(&ctx, block);
    dup(&ctx, block);
    assert_eq!(count(block), 3);
    assert!(!unique(block));
    release(&ctx, block, layout);
    release(&ctx, block, layout);
    assert!(
        unique(block),
        "count one again means the block may be mutated in place"
    );
    assert_eq!(ctx.stats().live_blocks, 1);
    release(&ctx, block, layout);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_tiny_count_bound_saturates_without_wrapping_or_later_uniqueness() {
    let ctx = Ctx::with(Config {
        count_max: 7,
        ..Config::new()
    });
    let max = finite_count_max(7);
    assert_eq!(max, 6);
    let layout = leaf(8);
    let block = alloc(&ctx, layout);
    for expected in 2..=max {
        dup(&ctx, block);
        assert_eq!(count(block), expected);
    }
    assert_eq!(ctx.stats().immortalized_blocks, 0);
    dup(&ctx, block);
    assert_eq!(
        count(block),
        COUNT_IMMORTAL,
        "the dup past the finite maximum makes the block immortal"
    );
    assert_eq!(ctx.stats().immortalized_blocks, 1);
    for _ in 0..100 {
        dup(&ctx, block);
        assert_eq!(
            count(block),
            COUNT_IMMORTAL,
            "immortality is sticky; the count never wraps"
        );
    }
    for _ in 0..1000 {
        assert_eq!(release(&ctx, block, layout), 0);
        assert_eq!(count(block), COUNT_IMMORTAL);
        assert!(!unique(block), "an immortal block never tests unique");
    }
    assert_eq!(ctx.stats().live_blocks, 1);
    assert_eq!(
        ctx.stats().live_blocks,
        1,
        "an immortal block is a permitted retained allocation"
    );
    assert_eq!(ctx.stats().immortalized_blocks, 1);
}

#[test]
fn static_blocks_ignore_dup_and_release() {
    let ctx = Ctx::diagnostic();
    let layout = leaf(8);
    let mut block = Header {
        count: COUNT_IMMORTAL,
        descriptor: index(ctx.0, layout) | (2 << 29),
    };
    dup(&ctx, &mut block);
    assert_eq!(release(&ctx, &mut block, layout), 0);
    assert_eq!(block.count, COUNT_IMMORTAL);
    assert!(!unique(&mut block));
    assert_eq!(
        ctx.stats().immortalized_blocks,
        0,
        "static immortality is not overflow"
    );
}

#[test]
fn a_dup_of_a_released_block_is_rejected_in_diagnostic_mode() {
    if in_child("a_dup_of_a_released_block_is_rejected_in_diagnostic_mode") {
        let ctx = Ctx::diagnostic();
        let layout = leaf(8);
        let block = alloc(&ctx, layout);
        release(&ctx, block, layout);
        dup(&ctx, block);
        return;
    }
    assert_child_aborts(
        "a_dup_of_a_released_block_is_rejected_in_diagnostic_mode",
        "not live",
    );
}

#[test]
fn a_count_operation_on_uncounted_storage_is_rejected_in_diagnostic_mode() {
    if in_child("a_count_operation_on_uncounted_storage_is_rejected_in_diagnostic_mode") {
        let ctx = Ctx::diagnostic();
        // Region-shaped storage: bytes with no counted header.
        let mut bytes = [0u64; 6];
        dup(&ctx, bytes.as_mut_ptr().cast());
        return;
    }
    assert_child_aborts(
        "a_count_operation_on_uncounted_storage_is_rejected_in_diagnostic_mode",
        "does not belong to the context",
    );
}

#[test]
fn diagnostic_large_free_is_checked_without_reading_released_backing() {
    if in_child("diagnostic_large_free_is_checked_without_reading_released_backing") {
        let ctx = Ctx::diagnostic();
        let layout = leaf(5000);
        let block = alloc_block(ctx.0, layout, 5000);
        release(&ctx, block, layout);
        dup(&ctx, block);
        return;
    }
    assert_child_aborts(
        "diagnostic_large_free_is_checked_without_reading_released_backing",
        "not live",
    );
    let ctx = Ctx::diagnostic();
    let layout = leaf(5000);
    let block = alloc_block(ctx.0, layout, 5000);
    dup(&ctx, block);
    assert_eq!(count(block), 2);
    release(&ctx, block, layout);
    release(&ctx, block, layout);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[cfg(not(feature = "system-allocator"))]
#[test]
fn diagnostic_trim_keeps_cold_tombstones_for_returned_slabs() {
    if in_child("diagnostic_trim_keeps_cold_tombstones_for_returned_slabs") {
        let ctx = Ctx::with(Config {
            flags: hirsel_rt::context::CONFIG_DIAGNOSTIC,
            heap_idle_cap: 1,
            ..Config::new()
        });
        let layout = leaf(8);
        let block = alloc(&ctx, layout);
        release(&ctx, block, layout);
        unsafe { hirsel_rt::context::hirsel_rt_v2_trim(ctx.0) };
        assert_eq!(ctx.stats().slab_bytes, 0);
        dup(&ctx, block);
        return;
    }
    assert_child_aborts(
        "diagnostic_trim_keeps_cold_tombstones_for_returned_slabs",
        "not live",
    );
    let ctx = Ctx::with(Config {
        flags: hirsel_rt::context::CONFIG_DIAGNOSTIC,
        heap_idle_cap: 1,
        ..Config::new()
    });
    let layout = leaf(8);
    let block = alloc(&ctx, layout);
    unsafe { hirsel_rt::context::hirsel_rt_v2_trim(ctx.0) };
    assert!(ctx.stats().slab_bytes > 0);
    assert!(unique(block));
    release(&ctx, block, layout);
}
