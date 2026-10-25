//! MEM-3 release: the intrusive dead-header worklist, drop order, masks, variants, reference
//! arrays, raw reuse tokens and descriptor checks.

use crate::support;

use std::ptr;

use hirsel_rt::alloc::{hirsel_rt_v2_alloc, hirsel_rt_v2_dispose, hirsel_rt_v2_reuse};
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::failure::{hirsel_rt_v2_failure_active, hirsel_rt_v2_failure_clear};
use hirsel_rt::release::{hirsel_rt_v2_release, hirsel_rt_v2_reset};
use hirsel_rt::{
    COUNT_IMMORTAL, DOMAIN_HEAP, Header, Layout, STATE_LIVE, STATE_RAW, STEP_NONNULL,
    STEP_RELEASE_LAST, Step, TaskCtx, WHOLE_VALUE,
};
use support::*;

fn alloc(ctx: *mut TaskCtx, layout: &Layout, extent: u64) -> *mut Header {
    let block = unsafe { hirsel_rt_v2_alloc(ctx, index(ctx, layout), extent, 8, DOMAIN_HEAP, 0) };
    block
}

fn release(ctx: *mut TaskCtx, block: *mut Header, mask: Option<&[u64]>) -> u8 {
    let layout = unsafe { (*block).index() };
    let mask = mask.map_or(ptr::null(), <[u64]>::as_ptr);
    unsafe { hirsel_rt_v2_release(ctx, block, layout, mask, 77, 77) }
}

/// A logged resource with no fields: payload `id`.
fn logged_leaf() -> &'static Layout {
    cached_layout("release-leaf", 0, || {
        layout(8, 1, Some(log_drop), Vec::new())
    })
}

/// A logged node `{ id, a: ref, b: ref }`; fields drop b then a.
fn logged_pair() -> &'static Layout {
    layout(
        24,
        3,
        Some(log_drop),
        vec![release_step(2, 16), release_step(1, 8)],
    )
}

fn node(ctx: *mut TaskCtx, layout: &Layout, id: u64) -> *mut Header {
    let block = alloc(ctx, layout, layout.size);
    write_u64(block, 0, id);
    block
}

struct SendCtx(*mut TaskCtx, *mut Header);
unsafe impl Send for SendCtx {}

#[test]
fn a_million_node_chain_releases_on_a_small_stack_without_allocating() {
    let ctx = Ctx::production();
    // `{ tag: u32, next: ref }`: Cons (tag 1) owns `next`; Nil (tag 0) owns nothing.
    let cell = layout(16, 1, None, vec![tag_step(0, 1, 1), release_step(0, 8)]);
    let mut head = alloc(ctx.0, cell, 16);
    write_u64(head, 0, 0);
    for _ in 1..1_000_000 {
        let cons = alloc(ctx.0, cell, 16);
        write_u64(cons, 0, 1);
        write_ref(cons, 8, head);
        head = cons;
    }
    assert_eq!(ctx.stats().live_blocks, 1_000_000);
    let job = SendCtx(ctx.0, head);
    let (status, calls) = std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            let job = job;
            let before = host_calls();
            let status = release(job.0, job.1, None);
            (status, host_calls() - before)
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(status, 0);
    assert_eq!(calls, 0, "freeing work made no host allocation");
    assert_eq!(ctx.stats().live_blocks, 0);
    #[cfg(not(feature = "system-allocator"))]
    assert_eq!(
        ctx.stats().backing_allocations,
        ctx.stats().slab_bytes / hirsel_rt::SLAB_SIZE as u64
    );
}

/// A tail release (`STEP_RELEASE_LAST`) frees its link before the next one drains, so a list
/// built tail first, as a constructor builds it, is freed head first and LIFO reuse replays
/// its build order; with a resource in every link the finalizers still run head first, each
/// before the rest of the chain.
#[cfg(not(feature = "system-allocator"))]
#[test]
fn a_tail_release_frees_each_link_before_the_rest_of_the_chain() {
    let ctx = Ctx::production();
    // `{ id: u64, next: ref }`: `next` is null at the end.
    let link = layout(
        16,
        2,
        None,
        vec![
            Step {
                span: 1,
                ..step(STEP_NONNULL, WHOLE_VALUE, 8)
            },
            step(STEP_RELEASE_LAST, 1, 8),
        ],
    );
    let build = |order: &mut Vec<*mut Header>, recycled: Option<*mut Header>| {
        let mut next = ptr::null_mut();
        for i in 0..64u64 {
            let block = match (i, recycled) {
                (0, Some(block)) => block,
                _ => alloc(ctx.0, link, 16),
            };
            order.push(block);
            write_u64(block, 0, i);
            write_ref(block, 8, next);
            next = block;
        }
        next
    };
    let mut first = Vec::new();
    let head = build(&mut first, None);
    assert_eq!(release(ctx.0, head, None), 0);
    assert_eq!(ctx.stats().live_blocks, 0);
    let mut fresh = Vec::new();
    let recycled = loop {
        let block = alloc(ctx.0, link, 16);
        if block == first[0] {
            break block;
        }
        write_u64(block, 0, 0);
        write_ref(block, 8, ptr::null_mut());
        fresh.push(block);
        assert!(fresh.len() < hirsel_rt::SLAB_SIZE / 16);
    };
    let mut second = Vec::new();
    let head = build(&mut second, Some(recycled));
    assert_eq!(
        first, second,
        "each link is freed before the rest of its chain, so LIFO reuse replays the build order"
    );
    assert_eq!(release(ctx.0, head, None), 0);
    for block in fresh {
        assert_eq!(release(ctx.0, block, None), 0);
    }
    assert_eq!(ctx.stats().live_blocks, 0);

    // `{ resource: logged leaf, next: ref }`: the leaf's finalizer runs before the link's tail.
    let ctx = Ctx::production();
    let logged = logged_leaf();
    let link = layout(
        16,
        2,
        None,
        vec![
            finalize_step(0, 0, logged),
            end_finalize_step(0, 0, logged),
            Step {
                span: 1,
                ..step(STEP_NONNULL, WHOLE_VALUE, 8)
            },
            step(STEP_RELEASE_LAST, 1, 8),
        ],
    );
    let mut next = ptr::null_mut();
    for i in 0..4u64 {
        let block = alloc(ctx.0, link, 16);
        write_u64(block, 0, i);
        write_ref(block, 8, next);
        next = block;
    }
    take_events();
    assert_eq!(release(ctx.0, next, None), 0);
    let order: Vec<u64> = take_events().into_iter().map(|(id, _)| id).collect();
    assert_eq!(order, [3, 2, 1, 0]);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_deep_tree_of_resources_drops_depth_first_in_reverse_field_order() {
    let ctx = Ctx::diagnostic();
    let pair = logged_pair();
    let leaf = logged_leaf();
    let c = node(ctx.0, leaf, 4);
    let d = node(ctx.0, leaf, 5);
    let a = node(ctx.0, pair, 2);
    write_ref(a, 8, c);
    write_ref(a, 16, d);
    let b = node(ctx.0, leaf, 3);
    let root = node(ctx.0, pair, 1);
    write_ref(root, 8, a);
    write_ref(root, 16, b);
    take_events();
    assert_eq!(release(ctx.0, root, None), 0);
    let order: Vec<u64> = take_events().into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        order,
        [1, 3, 2, 5, 4],
        "parent before fields, fields high to low, each subtree finished first"
    );
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_shared_child_is_released_only_when_its_last_parent_dies() {
    let ctx = Ctx::diagnostic();
    let one = layout(16, 2, Some(log_drop), vec![release_step(1, 8)]);
    // Resources are noncopyable: the shared child is a plain box `{ id, inner: ref }` whose
    // unique resource field reports its death.
    let boxed = layout(16, 2, None, vec![release_step(1, 8)]);
    logged_leaf();
    let shared = node(ctx.0, boxed, 0);
    write_ref(shared, 8, node(ctx.0, logged_leaf(), 9));
    unsafe { hirsel_rt_v2_dup(ctx.0, shared) };
    let p = node(ctx.0, one, 1);
    let q = node(ctx.0, one, 2);
    write_ref(p, 8, shared);
    write_ref(q, 8, shared);
    take_events();
    release(ctx.0, p, None);
    assert_eq!(take_events(), [(1, 77)]);
    assert_eq!(count(shared), 1);
    release(ctx.0, q, None);
    assert_eq!(take_events(), [(2, 77), (9, 77)]);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_immortal_child_survives_its_parents_release() {
    let ctx = Ctx::diagnostic();
    let one = layout(16, 2, None, vec![release_step(1, 8)]);
    let child = node(ctx.0, logged_leaf(), 9);
    unsafe { (*child).count = COUNT_IMMORTAL };
    let parent = node(ctx.0, one, 1);
    write_ref(parent, 8, child);
    take_events();
    release(ctx.0, parent, None);
    assert_eq!(take_events(), []);
    assert_eq!(state(ctx.0, child), STATE_LIVE);
    assert_eq!(ctx.stats().live_blocks, 1);
}

#[test]
fn a_partial_live_mask_skips_dead_fields_and_the_own_finalizer() {
    let ctx = Ctx::diagnostic();
    let triple = layout(
        32,
        4,
        Some(log_drop),
        vec![release_step(3, 24), release_step(2, 16), release_step(1, 8)],
    );
    let leaf = logged_leaf();
    let (a, b, c) = (
        node(ctx.0, leaf, 10),
        node(ctx.0, leaf, 20),
        node(ctx.0, leaf, 30),
    );
    let root = node(ctx.0, triple, 1);
    write_ref(root, 8, a);
    write_ref(root, 16, ptr::dangling_mut::<Header>()); // never initialized: must not be touched
    write_ref(root, 24, c);
    take_events();
    // Fields 0 (id), 1 (a) and 3 (c) are initialized; 2 (b) is not.
    assert_eq!(release(ctx.0, root, Some(&[0b1011])), 0);
    assert_eq!(
        take_events(),
        [(30, 77), (10, 77)],
        "no own finalizer for a partially built value"
    );
    assert_eq!(count(b), 1);
    release(ctx.0, b, None);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn variant_steps_release_only_the_active_payload() {
    let ctx = Ctx::diagnostic();
    let either = layout(
        24,
        2,
        None,
        vec![
            tag_step(0, 2, 1),
            release_step(1, 16),
            tag_step(0, 1, 1),
            release_step(0, 8),
        ],
    );
    let leaf = logged_leaf();
    let value = alloc(ctx.0, either, 24);
    write_u64(value, 0, 2);
    write_ref(value, 8, ptr::dangling_mut::<Header>());
    write_ref(value, 16, node(ctx.0, leaf, 5));
    take_events();
    release(ctx.0, value, None);
    assert_eq!(take_events(), [(5, 77)]);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_element_loop_releases_its_elements_highest_index_first() {
    let ctx = Ctx::diagnostic();
    let list = layout(
        16,
        2,
        None,
        vec![elements_step(1, 8, 16, 8, 1), release_step(UNUSED, 0)],
    );
    let leaf = logged_leaf();
    let length = 1000u64;
    let block = alloc(ctx.0, list, 16 + 8 * length);
    write_u64(block, 8, length);
    for index in 0..length {
        write_ref(block, 16 + 8 * index as usize, node(ctx.0, leaf, index));
    }
    take_events();
    release(ctx.0, block, None);
    let order: Vec<u64> = take_events().into_iter().map(|(id, _)| id).collect();
    assert_eq!(order, (0..length).rev().collect::<Vec<_>>());
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_empty_element_loop_releases_nothing() {
    let ctx = Ctx::diagnostic();
    let list = layout(
        16,
        2,
        None,
        vec![elements_step(1, 8, 16, 8, 1), release_step(UNUSED, 0)],
    );
    let block = alloc(ctx.0, list, 16);
    write_u64(block, 8, 0);
    release(ctx.0, block, None);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_inline_resource_field_is_finalized_before_its_own_fields() {
    let ctx = Ctx::diagnostic();
    let leaf = logged_leaf();
    let inner = layout(16, 2, Some(log_drop), Vec::new());
    // `{ id, inner: Inner { id, child: ref }, c: ref }` flattened in drop order.
    let outer = layout(
        32,
        3,
        Some(log_drop),
        vec![
            release_step(2, 24),
            finalize_step(1, 8, inner),
            release_step(1, 16),
            end_finalize_step(1, 8, inner),
        ],
    );
    let block = node(ctx.0, outer, 1);
    write_u64(block, 8, 2);
    write_ref(block, 16, node(ctx.0, leaf, 3));
    write_ref(block, 24, node(ctx.0, leaf, 4));
    take_events();
    release(ctx.0, block, None);
    let order: Vec<u64> = take_events().into_iter().map(|(id, _)| id).collect();
    assert_eq!(order, [1, 4, 2, 3]);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn reset_of_a_unique_donor_drains_it_into_a_raw_token_for_reuse() {
    let ctx = Ctx::diagnostic();
    let pair = logged_pair();
    let leaf = logged_leaf();
    let donor = node(ctx.0, pair, 1);
    write_ref(donor, 8, node(ctx.0, leaf, 2));
    write_ref(donor, 16, node(ctx.0, leaf, 3));
    take_events();
    let mut token = ptr::null_mut();
    assert_eq!(
        unsafe {
            hirsel_rt_v2_reset(
                ctx.0,
                donor,
                index(ctx.0, pair),
                ptr::null(),
                5,
                5,
                &mut token,
            )
        },
        0
    );
    assert_eq!(token, donor);
    assert_eq!(state(ctx.0, token), STATE_RAW);
    assert_eq!(
        take_events(),
        [(1, 5), (3, 5), (2, 5)],
        "the donor's cleanup runs at the reset anchor"
    );
    assert_eq!(
        ctx.stats().live_blocks,
        1,
        "the raw token still holds its storage"
    );
    // Same class (24 and 32 bytes both serve from the 32-byte class): exact key match.
    let other = leaf;
    let mut reused = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_reuse(ctx.0, token, index(ctx.0, other), 32, 8, &mut reused) },
        0
    );
    assert_eq!(reused, donor);
    assert_eq!(count(reused), 1);
    assert_eq!(unsafe { (*reused).index() }, index(ctx.0, other));
    write_u64(reused, 0, 4);
    release(ctx.0, reused, None);
    assert_eq!(take_events(), [(4, 77)]);
    assert_eq!(ctx.stats().live_blocks, 0);
}

/// A field moved out of the donor (cleared in the reset's live mask) keeps its bits through
/// the reset and the reuse, in diagnostic mode too: a reuse may skip storing it again.
#[test]
fn a_moved_field_keeps_its_bits_through_reset_and_reuse() {
    let ctx = Ctx::diagnostic();
    let pair = layout(24, 3, None, vec![release_step(2, 16), release_step(1, 8)]);
    let leaf = logged_leaf();
    let donor = node(ctx.0, pair, 1);
    let (kept, dropped) = (node(ctx.0, leaf, 2), node(ctx.0, leaf, 3));
    write_ref(donor, 8, kept);
    write_ref(donor, 16, dropped);
    take_events();
    // Field 1 was moved out: live fields are 0 and 2.
    let live = [0b101u64];
    let mut token = ptr::null_mut();
    assert_eq!(
        unsafe {
            hirsel_rt_v2_reset(
                ctx.0,
                donor,
                index(ctx.0, pair),
                live.as_ptr(),
                5,
                5,
                &mut token,
            )
        },
        0
    );
    assert_eq!(token, donor);
    assert_eq!(take_events(), [(3, 5)], "only the live child is released");
    let mut reused = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_reuse(ctx.0, token, index(ctx.0, pair), 24, 8, &mut reused) },
        0
    );
    let word = |offset: usize| unsafe {
        support::payload(reused)
            .add(offset)
            .cast::<*mut Header>()
            .read_unaligned()
    };
    assert_eq!(word(8), kept, "the moved field's bits are still in place");
    // The new value stores only its other field.
    write_ref(reused, 16, node(ctx.0, leaf, 4));
    release(ctx.0, reused, None);
    assert_eq!(ctx.stats().live_blocks, 0);
}

/// A donor whose live mask leaves no step live (every counted field moved out) is the token
/// with no walk, in both modes, and a masked release of one is freed at once: the moved
/// children keep their credits, and nothing is finalized.
#[test]
fn a_donor_with_every_counted_field_moved_resets_and_releases_without_a_walk() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        let pair = layout(24, 3, None, vec![release_step(2, 16), release_step(1, 8)]);
        let leaf = logged_leaf();
        let moved = [node(ctx.0, leaf, 2), node(ctx.0, leaf, 3)];
        let donor = node(ctx.0, pair, 1);
        write_ref(donor, 8, moved[0]);
        write_ref(donor, 16, moved[1]);
        take_events();
        // Fields 1 and 2 were moved out: only the scalar field 0 is live.
        let live = [0b001u64];
        let mut token = ptr::null_mut();
        assert_eq!(
            unsafe {
                hirsel_rt_v2_reset(
                    ctx.0,
                    donor,
                    index(ctx.0, pair),
                    live.as_ptr(),
                    5,
                    5,
                    &mut token,
                )
            },
            0
        );
        assert_eq!(token, donor);
        assert_eq!(take_events(), []);
        assert_eq!(ctx.stats().live_blocks, 3);
        let mut reused = ptr::null_mut();
        assert_eq!(
            unsafe { hirsel_rt_v2_reuse(ctx.0, token, index(ctx.0, pair), 24, 8, &mut reused) },
            0
        );
        assert_eq!(reused, donor);
        assert_eq!(release(ctx.0, reused, Some(&live[..])), 0);
        assert_eq!(take_events(), []);
        assert_eq!(ctx.stats().live_blocks, 2);
        for child in moved {
            assert_eq!(unsafe { (*child).count }, 1);
            assert_eq!(release(ctx.0, child, None), 0);
        }
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}

#[test]
fn reset_of_a_shared_donor_releases_one_credit_and_yields_none() {
    let ctx = Ctx::diagnostic();
    let leaf = leaf(8);
    let donor = node(ctx.0, leaf, 1);
    unsafe { hirsel_rt_v2_dup(ctx.0, donor) };
    let mut token = ptr::dangling_mut::<Header>();
    assert_eq!(
        unsafe {
            hirsel_rt_v2_reset(
                ctx.0,
                donor,
                index(ctx.0, leaf),
                ptr::null(),
                0,
                0,
                &mut token,
            )
        },
        0
    );
    assert!(token.is_null());
    assert_eq!(count(donor), 1);
    // None reuses by allocating.
    let mut fresh = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_reuse(ctx.0, ptr::null_mut(), index(ctx.0, leaf), 8, 8, &mut fresh) },
        0
    );
    assert!(!fresh.is_null() && fresh != donor);
}

#[test]
fn a_failing_finalizer_during_reset_disposes_the_storage_and_yields_none() {
    let ctx = Ctx::diagnostic();
    let failing = layout(8, 1, Some(failing_drop), Vec::new());
    let donor = node(ctx.0, failing, 6);
    let mut token = ptr::null_mut();
    assert_eq!(
        unsafe {
            hirsel_rt_v2_reset(
                ctx.0,
                donor,
                index(ctx.0, failing),
                ptr::null(),
                0,
                0,
                &mut token,
            )
        },
        1
    );
    assert!(token.is_null());
    assert_eq!(ctx.stats().live_blocks, 0);
    assert_eq!(unsafe { hirsel_rt_v2_failure_active(ctx.0) }, 1);
    unsafe { hirsel_rt_v2_failure_clear(ctx.0) };
}

#[test]
fn dispose_shallow_frees_a_token_without_running_finalizers() {
    let ctx = Ctx::diagnostic();
    let pair = logged_pair();
    logged_leaf();
    let donor = node(ctx.0, pair, 1);
    write_ref(donor, 8, node(ctx.0, logged_leaf(), 2));
    write_ref(donor, 16, node(ctx.0, logged_leaf(), 3));
    let mut token = ptr::null_mut();
    unsafe {
        hirsel_rt_v2_reset(
            ctx.0,
            donor,
            index(ctx.0, pair),
            ptr::null(),
            0,
            0,
            &mut token,
        )
    };
    take_events();
    unsafe { hirsel_rt_v2_dispose(ctx.0, token) };
    assert_eq!(take_events(), []);
    assert_eq!(ctx.stats().live_blocks, 0);
    unsafe { hirsel_rt_v2_dispose(ctx.0, ptr::null_mut()) };
}

#[test]
fn a_reuse_token_with_another_key_is_rejected() {
    if in_child("a_reuse_token_with_another_key_is_rejected") {
        let ctx = Ctx::production();
        let leaf = leaf(8);
        let donor = alloc(ctx.0, leaf, 8);
        let mut token = ptr::null_mut();
        unsafe {
            hirsel_rt_v2_reset(
                ctx.0,
                donor,
                index(ctx.0, leaf),
                ptr::null(),
                0,
                0,
                &mut token,
            )
        };
        let mut block = ptr::null_mut();
        unsafe { hirsel_rt_v2_reuse(ctx.0, token, index(ctx.0, leaf), 100, 8, &mut block) };
        return;
    }
    assert_child_aborts(
        "a_reuse_token_with_another_key_is_rejected",
        "key does not match",
    );
}

#[test]
fn a_header_layout_mismatch_is_rejected_in_diagnostic_mode() {
    if in_child("a_header_layout_mismatch_is_rejected_in_diagnostic_mode") {
        let ctx = Ctx::diagnostic();
        let own = leaf(8);
        let other = leaf(8);
        let block = alloc(ctx.0, own, 8);
        unsafe { hirsel_rt_v2_release(ctx.0, block, index(ctx.0, other), ptr::null(), 0, 0) };
        return;
    }
    assert_child_aborts(
        "a_header_layout_mismatch_is_rejected_in_diagnostic_mode",
        "header/layout mismatch",
    );
    // Accepted twin: the header's own layout.
    let ctx = Ctx::diagnostic();
    let own = leaf(8);
    let block = alloc(ctx.0, own, 8);
    assert_eq!(
        unsafe { hirsel_rt_v2_release(ctx.0, block, index(ctx.0, own), ptr::null(), 0, 0) },
        0
    );
}

#[test]
fn a_layout_larger_than_the_block_is_rejected_in_diagnostic_mode() {
    if in_child("a_layout_larger_than_the_block_is_rejected_in_diagnostic_mode") {
        let ctx = Ctx::diagnostic();
        let small = leaf(8);
        let big = leaf(64);
        let block = alloc(ctx.0, small, 8);
        unsafe { (*block).descriptor = index(ctx.0, big) };
        unsafe { hirsel_rt_v2_release(ctx.0, block, index(ctx.0, big), ptr::null(), 0, 0) };
        return;
    }
    assert_child_aborts(
        "a_layout_larger_than_the_block_is_rejected_in_diagnostic_mode",
        "larger than the block",
    );
}

#[test]
fn a_malformed_descriptor_is_rejected_in_diagnostic_mode() {
    if in_child("a_malformed_descriptor_is_rejected_in_diagnostic_mode") {
        let ctx = Ctx::diagnostic();
        // A reference slot that lies outside the 8-byte payload.
        let bad = layout(8, 1, None, vec![release_step(0, 8)]);
        alloc(ctx.0, bad, 8);
        return;
    }
    assert_child_aborts(
        "a_malformed_descriptor_is_rejected_in_diagnostic_mode",
        "outside its layout",
    );
}

#[test]
fn a_double_release_is_rejected_in_diagnostic_mode() {
    if in_child("a_double_release_is_rejected_in_diagnostic_mode") {
        let ctx = Ctx::diagnostic();
        let leaf = leaf(8);
        let block = alloc(ctx.0, leaf, 8);
        release(ctx.0, block, None);
        release(ctx.0, block, None);
        return;
    }
    assert_child_aborts(
        "a_double_release_is_rejected_in_diagnostic_mode",
        "not live",
    );
}

#[test]
fn element_loops_in_nested_blocks_resume_their_own_cursors() {
    let ctx = Ctx::diagnostic();
    let leaf = logged_leaf();
    // `{ id, len, xs: [ref; len], ys: [ref; len] }`: two columns share one length; ys drops
    // first, then xs, then the trailing reference.
    let inner = layout(
        16,
        3,
        Some(log_drop),
        vec![
            elements_step(2, 8, 16 + 8 * 3, 8, 1),
            release_step(UNUSED, 0),
            elements_step(1, 8, 16, 8, 1),
            release_step(UNUSED, 0),
        ],
    );
    let outer = layout(
        24,
        3,
        None,
        vec![
            elements_step(1, 8, 24, 8, 1),
            release_step(UNUSED, 0),
            release_step(2, 16),
        ],
    );
    let block = alloc(ctx.0, outer, 24 + 8 * 2);
    write_u64(block, 8, 2);
    write_ref(block, 16, node(ctx.0, leaf, 99));
    for row in 0..2u64 {
        let columns = alloc(ctx.0, inner, 16 + 8 * 6);
        write_u64(columns, 0, 10 * (row + 1));
        write_u64(columns, 8, 3);
        for index in 0..6 {
            let id = 10 * (row + 1) + 1 + index;
            write_ref(columns, 16 + 8 * index as usize, node(ctx.0, leaf, id));
        }
        write_ref(block, 24 + 8 * row as usize, columns);
    }
    take_events();
    assert_eq!(release(ctx.0, block, None), 0);
    let order: Vec<u64> = take_events().into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        order,
        [20, 26, 25, 24, 23, 22, 21, 10, 16, 15, 14, 13, 12, 11, 99]
    );
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_tag_inside_one_arm_may_share_a_sibling_arms_reference_word() {
    // `X(ref) | Y(Inner, ref)`, Inner's tag at 8, where X's reference lies.
    let arms = || {
        vec![
            tag_step(0, 0, 1),
            release_step(UNUSED, 8),
            tag_step(0, 1, 3),
            tag_step(8, 0, 1),
            release_step(UNUSED, 16),
            release_step(UNUSED, 24),
        ]
    };
    if in_child("a_tag_inside_one_arm_may_share_a_sibling_arms_reference_word") {
        // The control: the same inner tag outside the exclusive arm.
        let mut steps = arms();
        steps.remove(2);
        let ctx = Ctx::diagnostic();
        alloc(ctx.0, layout(32, 0, None, steps), 32);
        return;
    }
    assert_child_aborts(
        "a_tag_inside_one_arm_may_share_a_sibling_arms_reference_word",
        "control word aliases",
    );
    let ctx = Ctx::diagnostic();
    let leaf = logged_leaf();
    let pair = layout(32, 0, None, arms());
    for (tag, inner) in [(0u32, 0u32), (1, 0), (1, 1)] {
        let block = alloc(ctx.0, pair, 32);
        unsafe { payload(block).cast::<u32>().write_unaligned(tag) };
        if tag == 0 {
            write_ref(block, 8, node(ctx.0, leaf, 1));
        } else {
            unsafe { payload(block).add(8).cast::<u32>().write_unaligned(inner) };
            if inner == 0 {
                write_ref(block, 16, node(ctx.0, leaf, 2));
            }
            write_ref(block, 24, node(ctx.0, leaf, 3));
        }
        take_events();
        assert_eq!(release(ctx.0, block, None), 0);
        let order: Vec<u64> = take_events().into_iter().map(|(id, _)| id).collect();
        let want: &[u64] = match (tag, inner) {
            (0, _) => &[1],
            (_, 0) => &[2, 3],
            _ => &[3],
        };
        assert_eq!(order, want);
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}

// ----- the final schemas: layouts derived by hirsel-lir, lowered by rt_program ------

mod schema_release {
    use super::*;
    use hirsel_layout::layout::Owners;
    use hirsel_lir::layout_types::LayoutId;
    use hirsel_lir::rt_program::StepKind;
    use hirsel_types::Primitive::{F64, String as Str, U32, U64};
    use support::schema::*;

    /// Records, nested variants, resources with handles, lists, nested Inline repeats, a
    /// structure-of-arrays list, a resource enum and a null-pointer niche whose word follows a
    /// resource, all derived and validated by hirsel-lir.
    fn corpus() -> (Derived, Vec<(&'static str, LayoutId)>) {
        let mut owners = Owners::default();
        let socket = handle(&mut owners, "Socket", true);
        let finalized = Nominal {
            resource: true,
            finalizer: true,
            ..Nominal::default()
        };
        let res = nominal(&mut owners, "Res", finalized, |t| {
            let (u, f) = (t.p(U64), t.p(F64));
            let a = t.boxed(u);
            let s = t.named(&socket, Vec::new());
            let b = t.boxed(f);
            t.record(&[a, s, b])
        });
        let rec = nominal(&mut owners, "Rec", Nominal::default(), |t| {
            let u = t.p(U64);
            let a = t.boxed(u);
            let r = t.named(&res, Vec::new());
            let three = t.inline(a, 3);
            let opt = t.variant(&[&[], &[a, r], &[three]]);
            let name = t.p(Str);
            t.record(&[a, r, opt, name, a])
        });
        let pair = nominal(&mut owners, "Pair", Nominal::default(), |t| {
            let u = t.p(U64);
            let a = t.boxed(u);
            let two = t.inline(a, 2);
            let r = t.named(&res, Vec::new());
            t.record(&[two, r])
        });
        let particle = nominal(
            &mut owners,
            "Particle",
            Nominal {
                soa: true,
                ..Nominal::default()
            },
            |t| {
                let (id, name, f, u) = (t.p(U32), t.p(Str), t.p(F64), t.p(U64));
                let bf = t.boxed(f);
                let bu = t.boxed(u);
                let two = t.inline(bu, 2);
                t.record(&[id, name, f, bf, two])
            },
        );
        // An @soa variant: a u8 tag column and one payload column, dropped by arm.
        let sample = nominal(
            &mut owners,
            "Sample",
            Nominal {
                soa: true,
                ..Nominal::default()
            },
            |t| {
                let (name, f, u) = (t.p(Str), t.p(F64), t.p(U64));
                let bu = t.boxed(u);
                let two = t.inline(bu, 2);
                t.variant(&[&[bu], &[], &[name, f], &[two, name]])
            },
        );
        let res_enum = nominal(&mut owners, "ResEnum", finalized, |t| {
            let u = t.p(U64);
            let a = t.boxed(u);
            let r = t.named(&res, Vec::new());
            t.variant(&[&[a], &[r], &[]])
        });
        let mut t = support::schema::ty();
        let (u, r, rc, pr, pa, re) = (
            t.p(U64),
            t.named(&res, Vec::new()),
            t.named(&rec, Vec::new()),
            t.named(&pair, Vec::new()),
            t.named(&particle, Vec::new()),
            t.named(&res_enum, Vec::new()),
        );
        let sm = t.named(&sample, Vec::new());
        let leaf = t.boxed(u);
        let grid = t.inline(r, 2);
        let grid = t.inline(grid, 3);
        let gridded = t.record(&[grid, leaf]);
        let pairs = t.inline(pr, 2);
        let wide = t.inline(leaf, 64);
        let wide = t.inline(wide, 32);
        // `Node(Res, Box) | Leaf`: the niche word is the Box, after the resource.
        let niche = t.variant(&[&[r, leaf], &[]]);
        // A tag inside one arm shares bytes with a sibling arm's reference: exclusive arms.
        let inner_tagged = t.variant(&[&[leaf], &[], &[u]]);
        let overlapping = t.variant(&[&[leaf], &[inner_tagged, leaf]]);
        let roots = [
            ("Box[Res]", t.boxed(r)),
            ("Box[Rec]", t.boxed(rc)),
            ("List[Rec]", t.list(rc)),
            ("Box[{Inline[Inline[Res,2],3], Box}]", t.boxed(gridded)),
            ("List[Inline[Pair,2]]", t.list(pairs)),
            ("@soa List[Particle]", t.list(pa)),
            ("@soa List[Sample]", t.list(sm)),
            ("Box[ResEnum]", t.boxed(re)),
            ("List[ResEnum]", t.list(re)),
            ("List[Inline[Inline[Box,64],32]]", t.list(wide)),
            ("Box[Node(Res, Box) | Leaf]", t.boxed(niche)),
            ("List[Node(Res, Box) | Leaf]", t.list(niche)),
            ("List[X(Box) | Y(Tagged, Box)]", t.list(overlapping)),
        ];
        let types: Vec<_> = roots.iter().map(|(_, ty)| *ty).collect();
        let (derived, ids) = derive(&owners, &t.store, &types);
        let payloads = roots
            .iter()
            .zip(ids)
            .map(|((name, _), id)| (*name, derived.payload(id)))
            .collect();
        (derived, payloads)
    }

    fn run(diagnostic: bool) {
        let (d, cases) = corpus();
        for kind in [
            StepKind::Release,
            StepKind::ReleaseLast,
            StepKind::Finalize,
            StepKind::CloseHandle,
            StepKind::Tag,
            StepKind::NonNull,
            StepKind::Loop,
            StepKind::Column,
        ] {
            assert!(d.uses(kind), "the corpus exercises {kind:?}");
        }
        let ctx = if diagnostic {
            context()
        } else {
            Ctx::with(hirsel_rt::Config {
                close_handle: Some(log_close),
                ..hirsel_rt::Config::new()
            })
        };
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        for (name, payload) in cases {
            for round in 0..12u64 {
                let count = round % 5;
                let block = d.build(&ctx, payload, count, &mut rng);
                let mut want = Vec::new();
                d.expected(payload, support::payload(block), &mut want);
                reset_log();
                let before = host_calls();
                let status = release(ctx.0, block, None);
                let calls = host_calls() - before;
                assert_eq!(status, 0, "{name}");
                assert_eq!(take_log(), want, "{name}, round {round}");
                assert_eq!(calls, 0, "{name}: release allocated");
                assert_eq!(ctx.stats().live_blocks, 0, "{name}: every block is freed");
            }
        }
    }

    #[test]
    fn release_follows_the_nested_drop_program_of_derived_layouts() {
        run(true);
    }

    #[test]
    fn production_release_follows_the_nested_drop_program_of_derived_layouts() {
        run(false);
    }

    #[test]
    fn a_partial_live_mask_names_the_derived_records_fields() {
        let (d, cases) = corpus();
        let (_, payload) = cases.into_iter().find(|(n, _)| *n == "Box[Rec]").unwrap();
        // Rec { a: Box, r: Res, opt: Opt, name: String, b: Box }: the steps of each field name
        // its ordinal, highest field first.
        let fields: Vec<u32> = d.descriptors[&payload]
            .steps
            .iter()
            .map(|s| s.field)
            .collect();
        assert_eq!(fields.first(), Some(&4));
        assert_eq!(fields.last(), Some(&0));
        assert!(fields.windows(2).all(|w| w[0] >= w[1]));
        let ctx = context();
        let mut rng = 3;
        let block = d.build(&ctx, payload, 0, &mut rng);
        let value = support::payload(block);
        let leaf_of = |offset: u64| unsafe {
            let child = value
                .add(offset as usize)
                .cast::<*mut Header>()
                .read_unaligned();
            Event::Finalize(support::payload(child) as u64)
        };
        let record = d.table.layout(payload).unwrap();
        let offset = |ordinal: u32| record.fields[ordinal as usize].offset;
        let live = d.stats_live(&ctx);
        let want = [leaf_of(offset(4)), leaf_of(offset(0))];
        // Fields 1 (r), 2 (opt) and 3 (name) were moved out: only b and a release.
        reset_log();
        assert_eq!(release(ctx.0, block, Some(&[0b1_0001])), 0);
        assert_eq!(take_log(), want);
        assert_eq!(d.stats_live(&ctx), live - 3);
    }

    #[test]
    fn a_million_node_chain_of_a_derived_recursive_variant_releases_on_a_small_stack() {
        let mut owners = Owners::default();
        let chain = support::schema::path("Chain");
        nominal(&mut owners, "Chain", Nominal::default(), |t| {
            let next = t.named(&chain, Vec::new());
            let next = t.boxed(next);
            t.variant(&[&[], &[next]])
        });
        let mut t = support::schema::ty();
        let named = t.named(&chain, Vec::new());
        let boxed = t.boxed(named);
        let (d, ids) = derive(&owners, &t.store, &[boxed]);
        let payload = d.payload(ids[0]);
        let layout = d.descriptor(payload);
        // A null-pointer niche: `Nil | Cons(Box[Chain])` is the one word of `next`, null for Nil.
        assert_eq!(layout.size, 8);
        let ctx = Ctx::production();
        let mut head = alloc_block(ctx.0, layout, layout.size);
        write_u64(head, 0, 0);
        for _ in 1..1_000_000 {
            let cons = alloc_block(ctx.0, layout, layout.size);
            write_ref(cons, 0, head);
            head = cons;
        }
        let job = SendCtx(ctx.0, head);
        let (status, calls) = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                let job = job;
                let before = host_calls();
                let status = release(job.0, job.1, None);
                (status, host_calls() - before)
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!((status, calls), (0, 0));
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}

#[test]
fn a_control_word_cannot_alias_the_slot_used_by_pointer_reversal() {
    if in_child("a_control_word_cannot_alias_the_slot_used_by_pointer_reversal") {
        let ctx = Ctx::diagnostic();
        let bad = layout(
            16,
            1,
            None,
            vec![
                hirsel_rt::Step {
                    variant: 0,
                    span: 1,
                    ..step(hirsel_rt::STEP_LIVE_BIT, hirsel_rt::WHOLE_VALUE, 0)
                },
                release_step(0, 0),
            ],
        );
        alloc(ctx.0, bad, 16);
        return;
    }
    assert_child_aborts(
        "a_control_word_cannot_alias_the_slot_used_by_pointer_reversal",
        "control word aliases",
    );
    let ctx = Ctx::diagnostic();
    let good = layout(
        16,
        1,
        None,
        vec![
            hirsel_rt::Step {
                variant: 0,
                span: 1,
                ..step(hirsel_rt::STEP_LIVE_BIT, hirsel_rt::WHOLE_VALUE, 8)
            },
            release_step(0, 0),
        ],
    );
    let child_layout = leaf(8);
    let block = alloc(ctx.0, good, 16);
    let child = alloc(ctx.0, child_layout, 8);
    write_ref(block, 0, child);
    write_u64(block, 8, 1);
    assert_eq!(release(ctx.0, block, None), 0);
    assert_eq!(ctx.stats().live_blocks, 0);
}
