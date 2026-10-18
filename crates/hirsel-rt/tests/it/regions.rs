//! Compiler-local regions (MEM-10): bump storage, unpublished builders, cleanup at exit, the
//! 8 MiB idle pool cap and full return at task exit.

use crate::support;

use std::mem::MaybeUninit;
use std::ptr;

use hirsel_rt::alloc::hirsel_rt_v2_alloc;
use hirsel_rt::context::hirsel_rt_v2_task_exit;
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::failure::{hirsel_rt_v2_failure_cause_count, hirsel_rt_v2_failure_primary};
use hirsel_rt::region::{
    hirsel_rt_v2_builder_begin, hirsel_rt_v2_builder_grow, hirsel_rt_v2_builder_publish,
    hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit, hirsel_rt_v2_region_hold,
    hirsel_rt_v2_region_reserve, hirsel_rt_v2_region_schedule,
};
use hirsel_rt::{
    Audit, Builder, DOMAIN_HEAP, Header, Layout, REGION_POOL_IDLE_CAP, Region, RunCtx, SEGMENT_SIZE,
};
use support::*;

fn enter(ctx: &Ctx) -> Box<Region> {
    let mut region = Box::new(MaybeUninit::<Region>::uninit());
    unsafe { hirsel_rt_v2_region_enter(ctx.0, region.as_mut_ptr()) };
    unsafe { Box::from_raw(Box::into_raw(region).cast()) }
}

fn reserve(ctx: &Ctx, region: &mut Region, extent: u64, align: u64) -> *mut u8 {
    let mut out = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_region_reserve(ctx.0, region, extent, align, &mut out) },
        0
    );
    out
}

fn exit(ctx: &Ctx, region: &mut Region) -> u8 {
    unsafe { hirsel_rt_v2_region_exit(ctx.0, region, 0, 0) }
}

fn node(ctx: *mut RunCtx, layout: &Layout, id: u64) -> *mut Header {
    let block =
        unsafe { hirsel_rt_v2_alloc(ctx, index(ctx, layout), layout.size, 8, DOMAIN_HEAP, 0) };
    write_u64(block, 0, id);
    block
}

fn ids(events: Vec<(u64, u64)>) -> Vec<u64> {
    events.into_iter().map(|(id, _)| id).collect()
}

#[test]
fn reservations_are_aligned_bumps_that_span_segments() {
    let ctx = Ctx::diagnostic();
    let mut region = enter(&ctx);
    let mut previous = 0usize;
    for index in 0..10_000u64 {
        let align = 1u64 << (index % 7);
        let extent = 1 + index % 100;
        let address = reserve(&ctx, &mut region, extent, align);
        assert_eq!(address as usize % align as usize, 0);
        unsafe { ptr::write_bytes(address, 0x11, extent as usize) };
        assert_ne!(address as usize, previous);
        previous = address as usize;
    }
    let big = reserve(&ctx, &mut region, 3 * SEGMENT_SIZE as u64, 64);
    assert_eq!(big as usize % 64, 0);
    unsafe { ptr::write_bytes(big, 0x22, 3 * SEGMENT_SIZE) };
    assert!(ctx.stats().region_active_bytes >= 4 * SEGMENT_SIZE as u64);
    assert_eq!(ctx.stats().live_blocks, 0, "region storage is uncounted");
    assert_eq!(exit(&ctx, &mut region), 0);
    assert_eq!(ctx.stats().region_active_bytes, 0);
    assert_eq!(ctx.stats().active_regions, 0);
}

#[test]
fn an_immortal_owner_in_region_storage_accepts_diagnostic_count_operations() {
    use hirsel_rt::release::hirsel_rt_v2_release;
    use hirsel_rt::{COUNT_IMMORTAL, HEADER_SIZE};

    let ctx = Ctx::diagnostic();
    let layout = leaf(16);
    let layout_index = index(ctx.0, layout);
    let mut region = enter(&ctx);
    let storage = reserve(&ctx, &mut region, (16 + HEADER_SIZE + 16) as u64, 8);
    let storage = unsafe { storage.add(16) };
    let owner = storage.cast::<Header>();
    unsafe {
        hirsel_rt::region::hirsel_rt_v2_region_owner(
            ctx.0,
            &mut *region,
            owner,
            layout_index,
            16,
            8,
        );
        storage.add(HEADER_SIZE).cast::<u64>().write(2);
        ptr::copy_nonoverlapping(b"ab".as_ptr(), storage.add(HEADER_SIZE + 8), 2);
        for _ in 0..3 {
            hirsel_rt_v2_dup(ctx.0, owner);
            assert_ne!((*owner).count, 1);
            assert_eq!(
                hirsel_rt_v2_release(ctx.0, owner, index(ctx.0, layout), ptr::null(), 0, 0),
                0
            );
            assert_eq!((*owner).count, COUNT_IMMORTAL);
        }
        assert_eq!(
            std::slice::from_raw_parts(storage.add(HEADER_SIZE + 8), 2),
            b"ab"
        );
    }
    assert_eq!(ctx.stats().live_blocks, 0);
    assert_eq!(ctx.stats().immortalized_blocks, 0);
    assert_eq!(exit(&ctx, &mut region), 0);
    assert_eq!(ctx.stats().active_regions, 0);
    assert_eq!(ctx.stats().region_active_bytes, 0);
}

#[test]
fn an_unpublished_builder_grows_in_place_then_relocates_with_its_contents() {
    let ctx = Ctx::diagnostic();
    let mut region = enter(&ctx);
    let mut builder = MaybeUninit::<Builder>::uninit();
    unsafe { hirsel_rt_v2_builder_begin(ctx.0, &mut *region, 16, 8, builder.as_mut_ptr()) };
    let builder = unsafe { builder.assume_init_mut() };
    unsafe { ptr::write_bytes(builder.data, 7, 16) };
    let start = builder.data;
    assert_eq!(
        unsafe { hirsel_rt_v2_builder_grow(ctx.0, builder, 16, 64) },
        0
    );
    assert_eq!(builder.data, start, "the newest reservation grows in place");
    assert_eq!(builder.capacity, 64);
    unsafe { ptr::write_bytes(builder.data.add(16), 8, 48) };
    // Another reservation lands after the builder, so the next growth relocates.
    let after = reserve(&ctx, &mut region, 8, 8);
    assert_eq!(
        unsafe { hirsel_rt_v2_builder_grow(ctx.0, builder, 64, 100) },
        0
    );
    assert_ne!(builder.data, start);
    assert_eq!(builder.capacity, 128);
    let contents = unsafe { std::slice::from_raw_parts(builder.data, 64) };
    assert!(
        contents[..16].iter().all(|&byte| byte == 7)
            && contents[16..].iter().all(|&byte| byte == 8)
    );
    assert!(
        unsafe { std::slice::from_raw_parts(start, 64) }
            .iter()
            .all(|&byte| byte == 0xdf),
        "abandoned bytes are poisoned"
    );
    assert_ne!(after, builder.data);
    // Growth across a segment boundary also preserves the contents.
    assert_eq!(
        unsafe { hirsel_rt_v2_builder_grow(ctx.0, builder, 64, 2 * SEGMENT_SIZE as u64) },
        0
    );
    assert!(
        unsafe { std::slice::from_raw_parts(builder.data, 16) }
            .iter()
            .all(|&byte| byte == 7)
    );
    unsafe { hirsel_rt_v2_builder_publish(ctx.0, builder, 64) };
    assert_eq!(builder.published, 1);
    assert_eq!(builder.capacity, 64, "publication returns the unused tail");
    let next = reserve(&ctx, &mut region, 8, 8);
    assert_eq!(next, unsafe { builder.data.add(64) });
    assert_eq!(exit(&ctx, &mut region), 0);
}

#[test]
fn a_published_builder_never_grows() {
    if in_child("a_published_builder_never_grows") {
        let ctx = Ctx::production();
        let mut region = enter(&ctx);
        let mut builder = MaybeUninit::<Builder>::uninit();
        unsafe { hirsel_rt_v2_builder_begin(ctx.0, &mut *region, 16, 8, builder.as_mut_ptr()) };
        let builder = unsafe { builder.assume_init_mut() };
        unsafe { hirsel_rt_v2_builder_publish(ctx.0, builder, 16) };
        unsafe { hirsel_rt_v2_builder_grow(ctx.0, builder, 16, 32) };
        return;
    }
    assert_child_aborts(
        "a_published_builder_never_grows",
        "published builder was grown",
    );
}

#[test]
fn exit_releases_held_heap_credits_and_finalizes_scheduled_resources_newest_first() {
    let ctx = Ctx::diagnostic();
    let leaf = layout(8, 1, Some(log_drop), Vec::new());
    let plain = layout(8, 1, None, Vec::new());
    let regional = layout(16, 2, Some(log_drop), vec![release_step(1, 8)]);
    index(ctx.0, leaf);
    let mut region = enter(&ctx);
    let unique = node(ctx.0, leaf, 1);
    // Resources are noncopyable, so the shared block is a plain one.
    let shared = node(ctx.0, plain, 2);
    unsafe { hirsel_rt_v2_dup(ctx.0, shared) };
    unsafe { hirsel_rt_v2_region_hold(ctx.0, &mut *region, unique) };
    // A regional resource `{ id, child: ref }` holding a heap edge in its field.
    let value = reserve(&ctx, &mut region, 16, 8);
    unsafe {
        value.cast::<u64>().write(3);
        value
            .add(8)
            .cast::<*mut Header>()
            .write(node(ctx.0, leaf, 4));
        hirsel_rt_v2_region_schedule(ctx.0, &mut *region, index(ctx.0, regional), value);
        hirsel_rt_v2_region_hold(ctx.0, &mut *region, shared);
    }
    take_events();
    assert_eq!(exit(&ctx, &mut region), 0);
    assert_eq!(ids(take_events()), [3, 4, 1]);
    assert_eq!(
        count(shared),
        1,
        "the region released exactly its own credit"
    );
    assert_eq!(ctx.stats().live_blocks, 1);
}

#[test]
fn a_failing_exit_still_releases_every_heap_edge_and_meets_the_pool_cap() {
    let ctx = Ctx::diagnostic();
    let failing = layout(8, 1, Some(failing_drop), Vec::new());
    let logged = layout(8, 1, Some(log_drop), Vec::new());
    index(ctx.0, failing);
    let mut region = enter(&ctx);
    // 20 MiB of active region storage: more than the idle cap, which only bounds idle storage.
    for _ in 0..320 {
        reserve(&ctx, &mut region, SEGMENT_SIZE as u64 - 64, 16);
    }
    assert!(ctx.stats().region_active_bytes > 2 * REGION_POOL_IDLE_CAP);
    for id in 1..=6u64 {
        let layout = if id % 2 == 0 { failing } else { logged };
        unsafe { hirsel_rt_v2_region_hold(ctx.0, &mut *region, node(ctx.0, layout, id)) };
    }
    take_events();
    assert_eq!(exit(&ctx, &mut region), 1);
    assert_eq!(
        ids(take_events()),
        [6, 5, 4, 3, 2, 1],
        "every edge released once, newest first"
    );
    let mut primary = MaybeUninit::uninit();
    assert_eq!(
        unsafe { hirsel_rt_v2_failure_primary(ctx.0, primary.as_mut_ptr()) },
        1
    );
    assert_eq!(unsafe { primary.assume_init() }.rule, 6);
    assert_eq!(unsafe { hirsel_rt_v2_failure_cause_count(ctx.0) }, 2);
    let stats = ctx.stats();
    assert_eq!(stats.live_blocks, 0);
    assert_eq!(stats.region_active_bytes, 0);
    assert!(stats.region_pool_bytes <= REGION_POOL_IDLE_CAP, "{stats:?}");
    assert_eq!(stats.region_pool_bytes, REGION_POOL_IDLE_CAP);
}

#[test]
fn pooled_segments_are_reused_by_later_regions() {
    let ctx = Ctx::production();
    for _ in 0..100 {
        let mut region = enter(&ctx);
        for _ in 0..40 {
            reserve(&ctx, &mut region, 4000, 8);
        }
        exit(&ctx, &mut region);
    }
    let stats = ctx.stats();
    assert!(stats.backing_allocations <= 4, "{stats:?}");
}

/// An arena enters warm once the pool holds a standard segment: `arena_enter` seeds its
/// cursor and inline limit (null in diagnostic mode), so its first block is the bump a
/// compiled construction makes inline, from the segment the pool already had.
#[test]
fn an_arena_enters_warm_from_a_pooled_segment() {
    use hirsel_rt::region::{hirsel_rt_v2_arena_alloc, hirsel_rt_v2_arena_enter};
    for (ctx, diagnostic) in [(Ctx::production(), false), (Ctx::diagnostic(), true)] {
        let layout = leaf(16);
        let layout_index = index(ctx.0, layout);
        let arena = |ctx: &Ctx| {
            let mut region = Box::new(MaybeUninit::<Region>::uninit());
            unsafe { hirsel_rt_v2_arena_enter(ctx.0, region.as_mut_ptr()) };
            unsafe { Box::<Region>::from_raw(Box::into_raw(region).cast()) }
        };
        // The pool is empty: nothing to seed, so the first block takes a segment.
        let mut cold = arena(&ctx);
        assert!(cold.segment.is_null() && cold.cursor.is_null() && cold.end.is_null());
        unsafe { hirsel_rt_v2_arena_alloc(ctx.0, &mut *cold, layout_index, 16, 8, 0) };
        assert_eq!(exit(&ctx, &mut cold), 0);
        let backing = ctx.stats().backing_allocations;
        let mut warm = arena(&ctx);
        assert!(!warm.segment.is_null() && !warm.cursor.is_null());
        assert_eq!(warm.end.is_null(), diagnostic);
        assert_eq!(warm.cursor as usize % 8, 0);
        let seeded = warm.cursor;
        let block = unsafe { hirsel_rt_v2_arena_alloc(ctx.0, &mut *warm, layout_index, 16, 8, 0) };
        // An extent record (16 bytes) precedes the header of a layout without LAYOUT_NO_EXTENT.
        assert_eq!(block as usize, seeded as usize + 16);
        assert_eq!(ctx.stats().region_active_bytes, SEGMENT_SIZE as u64);
        assert_eq!(exit(&ctx, &mut warm), 0);
        assert_eq!(ctx.stats().backing_allocations, backing);
        assert_eq!(ctx.stats().region_active_bytes, 0);
        assert_eq!(ctx.stats().active_regions, 0);
    }
}

#[test]
fn nested_regions_exit_innermost_first() {
    let ctx = Ctx::diagnostic();
    let mut outer = enter(&ctx);
    let mut inner = enter(&ctx);
    let outer_value = reserve(&ctx, &mut outer, 8, 8);
    reserve(&ctx, &mut inner, 8, 8);
    assert_eq!(ctx.stats().active_regions, 2);
    assert_eq!(exit(&ctx, &mut inner), 0);
    unsafe { outer_value.cast::<u64>().write(5) };
    assert_eq!(exit(&ctx, &mut outer), 0);
    assert_eq!(ctx.stats().active_regions, 0);
}

#[test]
fn an_outer_region_cannot_exit_before_an_inner_one() {
    if in_child("an_outer_region_cannot_exit_before_an_inner_one") {
        let ctx = Ctx::production();
        let mut outer = enter(&ctx);
        let _inner = enter(&ctx);
        exit(&ctx, &mut outer);
        return;
    }
    assert_child_aborts(
        "an_outer_region_cannot_exit_before_an_inner_one",
        "region exits before",
    );
}

#[test]
fn a_reservation_in_an_exited_region_is_rejected() {
    if in_child("a_reservation_in_an_exited_region_is_rejected") {
        let ctx = Ctx::production();
        let mut region = enter(&ctx);
        exit(&ctx, &mut region);
        reserve(&ctx, &mut region, 8, 8);
        return;
    }
    assert_child_aborts(
        "a_reservation_in_an_exited_region_is_rejected",
        "not active",
    );
}

#[test]
fn task_exit_returns_active_and_pooled_region_storage() {
    let before = host_outstanding();
    let ctx = Ctx::production();
    let mut done = enter(&ctx);
    reserve(&ctx, &mut done, 1 << 20, 8);
    exit(&ctx, &mut done);
    let mut open = enter(&ctx);
    reserve(&ctx, &mut open, 8, 8);
    assert!(ctx.stats().region_pool_bytes > 0 && ctx.stats().region_active_bytes > 0);
    let mut audit = Audit::default();
    unsafe { hirsel_rt_v2_task_exit(ctx.0, &mut audit) };
    assert_eq!(audit.active_regions, 1);
    assert_eq!(audit.retained_bytes, 0);
    drop((ctx, open, done));
    assert_eq!(host_outstanding(), before);
}

#[test]
fn an_expired_regional_owner_is_rejected_before_its_poisoned_header_is_read() {
    if in_child("an_expired_regional_owner_is_rejected_before_its_poisoned_header_is_read") {
        let ctx = Ctx::diagnostic();
        let layout = leaf(16);
        let index = index(ctx.0, layout);
        let mut region = enter(&ctx);
        let storage = reserve(&ctx, &mut region, 40, 8);
        let owner = unsafe { storage.add(16).cast::<Header>() };
        unsafe {
            hirsel_rt::region::hirsel_rt_v2_region_owner(ctx.0, &mut *region, owner, index, 16, 8)
        };
        assert_eq!(exit(&ctx, &mut region), 0);
        unsafe { hirsel_rt_v2_dup(ctx.0, owner) };
        return;
    }
    assert_child_aborts(
        "an_expired_regional_owner_is_rejected_before_its_poisoned_header_is_read",
        "regional owner expired",
    );
    // The live owner is exercised with dup/release/the uniqueness count test in the immortal-owner twin above.
}
