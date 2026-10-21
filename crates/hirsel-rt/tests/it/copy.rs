//! Deep unique copy (`hirsel_rt_v2_unique_copy`): sharing inside the copied graph is kept,
//! immortal blocks are copied like any other, the native stack stays constant over a deep chain,
//! and a resource is an invariant fault; an inline value's holders are copied in place. A
//! copy-on-write detach (`hirsel_rt_v2_detach`)
//! copies the block alone and shares its children.

use crate::support;

use std::ptr;

use hirsel_rt::context::CONFIG_DIAGNOSTIC;
use hirsel_rt::copy::{
    hirsel_rt_v2_detach, hirsel_rt_v2_unique_copy, hirsel_rt_v2_unique_copy_value,
};
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::release::hirsel_rt_v2_release;
use hirsel_rt::{COUNT_IMMORTAL, Config, Header, Layout, Step, TaskCtx, WHOLE_VALUE};
use support::*;

fn copy(ctx: *mut TaskCtx, block: *mut Header) -> *mut Header {
    let mut out = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_unique_copy(ctx, block, ptr::null_mut(), 0, 0, &mut out) },
        0
    );
    out
}

fn release(ctx: *mut TaskCtx, block: *mut Header) {
    let status = unsafe { hirsel_rt_v2_release(ctx, block, (*block).index(), ptr::null(), 0, 0) };
    assert_eq!(status, 0);
}

fn layout_of(block: *mut Header) -> u32 {
    unsafe { (*block).index() }
}

fn child(block: *mut Header, offset: usize) -> *mut Header {
    unsafe {
        payload(block)
            .add(offset)
            .cast::<*mut Header>()
            .read_unaligned()
    }
}

/// `pair { left, right, tag }` of two references; `leaf { n }`.
fn pair() -> &'static Layout {
    cached_layout("copy-pair", 0, || {
        layout(24, 3, None, vec![release_step(1, 8), release_step(0, 0)])
    })
}

#[test]
fn a_diamond_keeps_its_sharing_and_every_copy_is_fresh() {
    let ctx = Ctx::diagnostic();
    let leaf = leaf(8);
    let pair = pair();
    let shared = alloc_block(ctx.0, leaf, 8);
    write_u64(shared, 0, 42);
    unsafe { hirsel_rt_v2_dup(ctx.0, shared) };
    let root = alloc_block(ctx.0, pair, 24);
    write_ref(root, 0, shared);
    write_ref(root, 8, shared);
    write_u64(root, 16, 7);
    let live = ctx.stats().live_blocks;

    let copied = copy(ctx.0, root);
    assert_ne!(copied, root);
    assert_eq!(layout_of(copied), layout_of(root));
    assert_eq!(read_u64(copied, 16), 7);
    let (left, right) = (child(copied, 0), child(copied, 8));
    assert_eq!(left, right, "both slots name one copy of the shared leaf");
    assert_ne!(left, shared);
    assert_eq!(read_u64(left, 0), 42);
    assert_eq!((count(copied), count(left)), (1, 2));
    assert_eq!(count(shared), 2, "the original keeps its holders");
    assert_eq!(ctx.stats().live_blocks, live + 2);

    release(ctx.0, copied);
    assert_eq!(ctx.stats().live_blocks, live);
    release(ctx.0, root);
    assert_eq!(ctx.stats().live_blocks, 0);
}

/// The visited map keeps its first blocks inline; a block first copied after it has spilled
/// is still copied once, whichever store finds it.
#[test]
fn sharing_found_past_the_inline_visited_map_is_kept() {
    let ctx = Ctx::diagnostic();
    let (leaf, pair) = (leaf(8), pair());
    let shared = alloc_block(ctx.0, leaf, 8);
    write_u64(shared, 0, 9);
    for _ in 0..2 {
        unsafe { hirsel_rt_v2_dup(ctx.0, shared) };
    }
    // p0 .. p11: each holds the next on the left and its own leaf on the right, except that
    // p10 and p11 hold `shared` on the right and p11 also on the left.
    let links = 12;
    let mut next = shared;
    for i in (0..links).rev() {
        let node = alloc_block(ctx.0, pair, 24);
        let right = if i >= links - 2 {
            shared
        } else {
            let own = alloc_block(ctx.0, leaf, 8);
            write_u64(own, 0, i as u64);
            own
        };
        write_ref(node, 0, next);
        write_ref(node, 8, right);
        next = node;
    }
    let root = next;
    let live = ctx.stats().live_blocks;
    let copied = copy(ctx.0, root);
    let mut node = copied;
    for _ in 0..links - 2 {
        node = child(node, 0);
    }
    let (p10, p11) = (node, child(node, 0));
    let copy_of_shared = child(p10, 8);
    assert_ne!(copy_of_shared, shared);
    assert_eq!(read_u64(copy_of_shared, 0), 9);
    assert_eq!(child(p11, 8), copy_of_shared);
    assert_eq!(child(p11, 0), copy_of_shared);
    assert_eq!(count(copy_of_shared), 3);
    // The pairs, their own leaves and one shared leaf.
    assert_eq!(ctx.stats().live_blocks, live + 2 * links as u64 - 1);
    release(ctx.0, copied);
    release(ctx.0, root);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn immortal_blocks_are_copied_so_the_copy_is_unique() {
    pair();
    let ctx = Ctx::with(Config {
        flags: CONFIG_DIAGNOSTIC,
        count_max: 3,
        ..Config::new()
    });
    let leaf = leaf(8);
    let immortal = alloc_block(ctx.0, leaf, 8);
    write_u64(immortal, 0, 5);
    for _ in 0..3 {
        unsafe { hirsel_rt_v2_dup(ctx.0, immortal) };
    }
    assert_eq!(count(immortal), COUNT_IMMORTAL);
    let fresh = alloc_block(ctx.0, leaf, 8);
    let root = alloc_block(ctx.0, pair(), 24);
    write_ref(root, 0, immortal);
    write_ref(root, 8, fresh);
    let copied = copy(ctx.0, root);
    let (left, right) = (child(copied, 0), child(copied, 8));
    assert_ne!(left, immortal, "a unique copy may be written in place");
    assert_eq!((read_u64(left, 0), count(left)), (5, 1));
    assert_ne!(right, fresh);
    let alone = copy(ctx.0, immortal);
    assert_ne!(alone, immortal, "an immortal root is copied too");
    assert_eq!(count(alone), 1);
    release(ctx.0, alone);
    release(ctx.0, copied);
    release(ctx.0, root);
    assert_eq!(ctx.stats().live_blocks, 1, "only the immortal leaf remains");
}

#[test]
fn a_chain_a_million_deep_copies_on_a_constant_native_stack() {
    // The test thread's stack is far smaller than a million frames of any recursion.
    std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(|| {
            let ctx = Ctx::production();
            let node = layout(16, 2, None, vec![release_step(0, 0)]);
            let end = leaf(16);
            let mut head = alloc_block(ctx.0, end, 16);
            for i in 0..1_000_000u64 {
                let next = alloc_block(ctx.0, node, 16);
                write_ref(next, 0, head);
                write_u64(next, 8, i);
                head = next;
            }
            let copied = copy(ctx.0, head);
            let (mut a, mut b, mut depth) = (head, copied, 0u64);
            while layout_of(a) == index(ctx.0, node) {
                assert_ne!(a, b);
                assert_eq!(read_u64(a, 8), read_u64(b, 8));
                assert_eq!(count(b), 1);
                (a, b, depth) = (child(a, 0), child(b, 0), depth + 1);
            }
            assert_eq!(depth, 1_000_000);
            release(ctx.0, copied);
            release(ctx.0, head);
            assert_eq!(ctx.stats().live_blocks, 0);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn tags_and_element_loops_select_the_holders_copied() {
    let ctx = Ctx::diagnostic();
    let leaf = leaf(8);
    // A variant { tag: u32 at 0, Some(ref at 8) } and a list { count, refs... }.
    let option = layout(16, 1, None, vec![tag_step(0, 1, 1), release_step(0, 8)]);
    let list = layout(
        8,
        1,
        None,
        vec![elements_step(0, 0, 8, 8, 1), release_step(WHOLE_VALUE, 0)],
    );
    let some = alloc_block(ctx.0, option, 16);
    unsafe { payload(some).cast::<u32>().write(1) };
    write_ref(some, 8, alloc_block(ctx.0, leaf, 8));
    let none = alloc_block(ctx.0, option, 16);
    unsafe { payload(none).cast::<u32>().write(0) };
    write_u64(none, 8, 0xdead);
    let items = alloc_block(ctx.0, list, 8 + 3 * 8);
    write_u64(items, 0, 3);
    write_ref(items, 8, some);
    write_ref(items, 16, none);
    write_ref(items, 24, some);
    unsafe { hirsel_rt_v2_dup(ctx.0, some) };
    let copied = copy(ctx.0, items);
    let (a, b, c) = (child(copied, 8), child(copied, 16), child(copied, 24));
    assert_eq!(a, c);
    assert_ne!(a, some);
    assert_ne!(child(a, 8), child(some, 8));
    assert_eq!(
        read_u64(b, 8),
        0xdead,
        "an inactive variant's bytes are not a holder"
    );
    release(ctx.0, copied);
    release(ctx.0, items);
    assert_eq!(ctx.stats().live_blocks, 0);
}

unsafe extern "C" fn no_drop(_: *mut TaskCtx, _: *mut u8, _: u64, _: u64) -> u8 {
    0
}

#[test]
fn a_resource_in_the_graph_is_an_invariant_fault() {
    if in_child("a_resource_in_the_graph_is_an_invariant_fault") {
        let ctx = Ctx::diagnostic();
        let resource = layout(8, 0, Some(no_drop), Vec::new());
        let plain = leaf(8);
        let root = alloc_block(ctx.0, pair(), 24);
        write_ref(root, 0, alloc_block(ctx.0, resource, 8));
        write_ref(root, 8, alloc_block(ctx.0, plain, 8));
        copy(ctx.0, root);
        return;
    }
    assert_child_aborts(
        "a_resource_in_the_graph_is_an_invariant_fault",
        "a unique copy reached a resource",
    );
    // Accepted twin: the same graph without the resource.
    let ctx = Ctx::diagnostic();
    let plain = leaf(8);
    let root = alloc_block(ctx.0, pair(), 24);
    write_ref(root, 0, alloc_block(ctx.0, plain, 8));
    write_ref(root, 8, alloc_block(ctx.0, plain, 8));
    let copied = copy(ctx.0, root);
    release(ctx.0, copied);
    release(ctx.0, root);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_null_pointer_niche_selects_the_dataful_holder() {
    let ctx = Ctx::diagnostic();
    let leaf = leaf(8);
    // Option<Box<..>> as a niche: the dataful variant's reference at 0, null for None.
    let option = layout(
        8,
        1,
        None,
        vec![
            Step {
                span: 1,
                ..step(hirsel_rt::STEP_NONNULL, WHOLE_VALUE, 0)
            },
            release_step(0, 0),
        ],
    );
    let list = layout(
        8,
        1,
        None,
        vec![elements_step(0, 0, 8, 8, 1), release_step(WHOLE_VALUE, 0)],
    );
    let some = alloc_block(ctx.0, option, 8);
    write_ref(some, 0, alloc_block(ctx.0, leaf, 8));
    let none = alloc_block(ctx.0, option, 8);
    write_u64(none, 0, 0);
    let items = alloc_block(ctx.0, list, 8 + 2 * 8);
    write_u64(items, 0, 2);
    write_ref(items, 8, some);
    write_ref(items, 16, none);
    let live = ctx.stats().live_blocks;
    let copied = copy(ctx.0, items);
    let (a, b) = (child(copied, 8), child(copied, 16));
    assert_ne!(
        child(a, 0),
        child(some, 0),
        "the dataful reference is copied"
    );
    assert_eq!(read_u64(b, 0), 0, "None stays null");
    assert_eq!(ctx.stats().live_blocks, 2 * live);
    release(ctx.0, copied);
    release(ctx.0, items);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_detach_copies_the_block_alone_and_shares_each_active_child() {
    let ctx = Ctx::diagnostic();
    let leaf = leaf(8);
    let option = layout(16, 1, None, vec![tag_step(0, 1, 1), release_step(0, 8)]);
    let list = layout(
        8,
        1,
        None,
        vec![elements_step(0, 0, 8, 8, 1), release_step(WHOLE_VALUE, 0)],
    );
    let some = alloc_block(ctx.0, option, 16);
    unsafe { payload(some).cast::<u32>().write(1) };
    write_ref(some, 8, alloc_block(ctx.0, leaf, 8));
    let none = alloc_block(ctx.0, option, 16);
    unsafe { payload(none).cast::<u32>().write(0) };
    let items = alloc_block(ctx.0, list, 8 + 2 * 8);
    write_u64(items, 0, 2);
    write_ref(items, 8, some);
    write_ref(items, 16, none);
    let live = ctx.stats().live_blocks;

    let mut detached = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_detach(ctx.0, items, 0, &mut detached) },
        0
    );
    assert_ne!(detached, items);
    assert_eq!(layout_of(detached), layout_of(items));
    assert_eq!(read_u64(detached, 0), 2);
    assert_eq!((child(detached, 8), child(detached, 16)), (some, none));
    assert_eq!((count(detached), count(some), count(none)), (1, 2, 2));
    assert_eq!(count(child(some, 8)), 1, "a grandchild is not touched");
    assert_eq!(ctx.stats().live_blocks, live + 1, "one block allocated");

    release(ctx.0, detached);
    assert_eq!((count(some), count(none)), (1, 1));
    release(ctx.0, items);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_inline_value_s_holders_are_copied_in_place_keeping_their_sharing() {
    pair();
    let ctx = Ctx::diagnostic();
    let leaf = leaf(8);
    let shared = alloc_block(ctx.0, leaf, 8);
    write_u64(shared, 0, 42);
    unsafe { hirsel_rt_v2_dup(ctx.0, shared) };
    // `(left, right, n)` held inline: the bit copy names the source's blocks.
    let mut value = [shared as u64, shared as u64, 7u64];
    let status = unsafe {
        hirsel_rt_v2_unique_copy_value(
            ctx.0,
            index(ctx.0, pair()),
            value.as_mut_ptr().cast::<u8>(),
            ptr::null_mut(),
            0,
        )
    };
    assert_eq!(status, 0);
    let (left, right) = (value[0] as *mut Header, value[1] as *mut Header);
    assert_eq!(left, right, "both slots name one copy of the shared leaf");
    assert_ne!(left, shared);
    assert_eq!((read_u64(left, 0), count(left), value[2]), (42, 2, 7));
    assert_eq!(count(shared), 2, "the source keeps its holders");
    release(ctx.0, left);
    release(ctx.0, right);
    release(ctx.0, shared);
    release(ctx.0, shared);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn detach_preserves_sequence_length_and_returns_one_holder_in_every_storage_case() {
    let ctx = Ctx::with(Config {
        flags: CONFIG_DIAGNOSTIC,
        count_max: 3,
        ..Config::new()
    });
    let sequence = leaf(32);
    for length in [0, 3] {
        for holders in [1, 2, 4] {
            let source = alloc_block(ctx.0, sequence, 32);
            write_u64(source, 0, length);
            for _ in 1..holders {
                unsafe { hirsel_rt_v2_dup(ctx.0, source) };
            }
            let before = count(source);
            let mut result = ptr::null_mut();
            assert_eq!(
                unsafe { hirsel_rt_v2_detach(ctx.0, source, 0, &mut result) },
                0
            );
            assert_ne!(result, source);
            assert_eq!((count(result), read_u64(result, 0)), (1, length));
            assert_eq!((count(source), read_u64(source, 0)), (before, length));
            release(ctx.0, result);
            if before != COUNT_IMMORTAL {
                for _ in 0..holders {
                    release(ctx.0, source);
                }
            }
        }
        // A real static owner includes immutable extent metadata before its
        // immortal header, exactly as the compiler's static objects do.
        #[repr(C)]
        struct StaticSequence {
            extent: hirsel_rt::alloc::Extent,
            header: Header,
            length: u64,
            elements: [u64; 3],
        }
        let mut source = StaticSequence {
            extent: hirsel_rt::alloc::Extent {
                capacity: 32,
                align: 8,
            },
            header: Header {
                count: COUNT_IMMORTAL,
                descriptor: index(ctx.0, sequence) | (hirsel_rt::alloc::KIND_STATIC << 29),
            },
            length,
            elements: [11, 22, 33],
        };
        let mut result = ptr::null_mut();
        assert_eq!(
            unsafe { hirsel_rt_v2_detach(ctx.0, &mut source.header, 0, &mut result) },
            0
        );
        assert_ne!(result, &raw mut source.header);
        assert_eq!((count(result), read_u64(result, 0)), (1, length));
        assert_eq!(
            (source.header.count, source.length),
            (COUNT_IMMORTAL, length)
        );
        release(ctx.0, result);
    }
}

/// Source values cannot observe allocator ownership or graph sharing. Check those
/// independently here, including reads after both source arenas have closed.
#[test]
fn graph_copies_use_the_destination_and_outlive_source_arenas() {
    use hirsel_rt::Region;
    use hirsel_rt::region::{hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit};
    use std::mem::MaybeUninit;

    for (flags, fixed) in [
        (0, false),
        (CONFIG_DIAGNOSTIC, false),
        (0, true),
        (CONFIG_DIAGNOSTIC, true),
    ] {
        let parent = if fixed {
            with_flags(pair(), hirsel_rt::LAYOUT_NO_EXTENT)
        } else {
            pair()
        };
        let child_layout = if fixed {
            with_flags(leaf(8), hirsel_rt::LAYOUT_NO_EXTENT)
        } else {
            leaf(8)
        };
        let ctx = Ctx::with(Config {
            flags,
            ..Config::new()
        });
        let shared = alloc_block(ctx.0, child_layout, 8);
        write_u64(shared, 0, 42);
        unsafe { hirsel_rt_v2_dup(ctx.0, shared) };
        let root = alloc_block(ctx.0, parent, 24);
        write_ref(root, 0, shared);
        write_ref(root, 8, shared);
        write_u64(root, 16, 7);
        let mut outer = MaybeUninit::<Region>::uninit();
        let mut inner = MaybeUninit::<Region>::uninit();
        unsafe {
            hirsel_rt_v2_region_enter(ctx.0, outer.as_mut_ptr());
            hirsel_rt_v2_region_enter(ctx.0, inner.as_mut_ptr());
            let mut first = ptr::null_mut();
            assert_eq!(
                hirsel_rt_v2_unique_copy(ctx.0, root, outer.as_mut_ptr(), 0, 0, &mut first),
                0
            );
            let mut second = ptr::null_mut();
            assert_eq!(
                hirsel_rt_v2_unique_copy(ctx.0, first, inner.as_mut_ptr(), 0, 0, &mut second),
                0
            );
            assert_eq!(count(first), COUNT_IMMORTAL);
            assert_eq!(count(child(second, 0)), COUNT_IMMORTAL);
            assert_eq!(child(second, 0), child(second, 8));
            assert_ne!(child(first, 0), child(second, 0));
            assert_eq!(
                ctx.stats().live_blocks,
                2,
                "arena copies allocated no heap blocks"
            );
            release(ctx.0, root);
            let escaped = copy(ctx.0, second);
            assert_eq!(hirsel_rt_v2_region_exit(ctx.0, inner.as_mut_ptr(), 0, 0), 0);
            assert_eq!(hirsel_rt_v2_region_exit(ctx.0, outer.as_mut_ptr(), 0, 0), 0);
            assert_eq!(read_u64(escaped, 16), 7);
            assert_eq!(read_u64(child(escaped, 0), 0), 42);
            assert_eq!(child(escaped, 0), child(escaped, 8));
            assert_eq!(count(child(escaped, 0)), 2);
            release(ctx.0, escaped);
            assert_eq!(ctx.stats().live_blocks, 0);
            assert_eq!(ctx.stats().active_regions, 0);
        }
    }
}

/// A deep copy into a bounded arena (MEM-6) is charged before anything is copied: every
/// block once per reference, so a diamond's shared leaf counts twice. A budget one byte short
/// returns `STATUS_EXHAUSTED`, copies nothing and leaves the budget; the exact budget copies.
#[test]
fn a_bounded_copy_charges_every_reference_before_copying() {
    use hirsel_rt::Region;
    use hirsel_rt::region::{
        hirsel_rt_v2_arena_bound, hirsel_rt_v2_arena_enter, hirsel_rt_v2_region_exit,
    };
    use std::mem::MaybeUninit;

    let ctx = Ctx::diagnostic();
    let leaf = leak(Layout {
        logical: 8,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: u64::MAX,
        ..*leaf(8)
    });
    let pair = leak(Layout {
        logical: 24,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: u64::MAX,
        ..*pair()
    });
    let shared = alloc_block(ctx.0, leaf, 8);
    write_u64(shared, 0, 42);
    unsafe { hirsel_rt_v2_dup(ctx.0, shared) };
    let root = alloc_block(ctx.0, pair, 24);
    write_ref(root, 0, shared);
    write_ref(root, 8, shared);
    write_u64(root, 16, 7);
    for (cap, copied) in [(39, false), (40, true)] {
        let mut arena = MaybeUninit::<Region>::uninit();
        unsafe {
            hirsel_rt_v2_arena_enter(ctx.0, arena.as_mut_ptr());
            hirsel_rt_v2_arena_bound(ctx.0, arena.as_mut_ptr(), cap);
            let mut out = ptr::null_mut();
            let status = hirsel_rt_v2_unique_copy(ctx.0, root, arena.as_mut_ptr(), 0, 0, &mut out);
            let remaining = (*arena.as_ptr()).remaining;
            if copied {
                assert_eq!((status, remaining), (0, 0));
                assert_eq!(child(out, 0), child(out, 8), "the copy keeps its sharing");
            } else {
                assert_eq!((status, remaining), (hirsel_rt::STATUS_EXHAUSTED, 39));
                assert!(out.is_null());
            }
            assert_eq!(hirsel_rt_v2_region_exit(ctx.0, arena.as_mut_ptr(), 0, 0), 0);
        }
    }
    release(ctx.0, root);
    assert_eq!(ctx.stats().live_blocks, 0);
}

/// `arena_recover` forgets exactly the bounded arena's own `OutOfSpace`, the one record since
/// its bound: not another arena's, not one recorded before the bound, and not one a cleanup
/// failure followed.
#[test]
fn a_bounded_arena_recovers_only_its_own_sole_out_of_space() {
    use hirsel_rt::Region;
    use hirsel_rt::failure::{
        hirsel_rt_v2_failure_active, hirsel_rt_v2_failure_begin, hirsel_rt_v2_failure_cause_count,
        hirsel_rt_v2_failure_clear,
    };
    use hirsel_rt::region::{
        hirsel_rt_v2_arena_bound, hirsel_rt_v2_arena_enter, hirsel_rt_v2_arena_exhausted,
        hirsel_rt_v2_arena_recover, hirsel_rt_v2_region_exit,
    };
    use std::mem::MaybeUninit;

    let ctx = Ctx::diagnostic();
    let mut outer = MaybeUninit::<Region>::uninit();
    let mut inner = MaybeUninit::<Region>::uninit();
    let (outer, inner) = (outer.as_mut_ptr(), inner.as_mut_ptr());
    unsafe {
        hirsel_rt_v2_arena_enter(ctx.0, outer);
        hirsel_rt_v2_arena_bound(ctx.0, outer, 10);
        hirsel_rt_v2_arena_enter(ctx.0, inner);
        hirsel_rt_v2_arena_bound(ctx.0, inner, 10);
        // The inner callback exhausts the outer arena: only the outer recovers it.
        assert_eq!(hirsel_rt_v2_arena_exhausted(ctx.0, outer, 1, 6, 0, 0), 1);
        assert_eq!(hirsel_rt_v2_arena_recover(ctx.0, inner), 0);
        assert_eq!(hirsel_rt_v2_arena_recover(ctx.0, outer), 1);
        assert_eq!(hirsel_rt_v2_failure_active(ctx.0), 0);
        // A cleanup failure after the arena's own keeps both.
        assert_eq!(hirsel_rt_v2_arena_exhausted(ctx.0, inner, 1, 6, 0, 0), 1);
        assert_eq!(hirsel_rt_v2_failure_begin(ctx.0, 1, 2, 0, 0), 1);
        assert_eq!(hirsel_rt_v2_arena_recover(ctx.0, inner), 0);
        assert_eq!(hirsel_rt_v2_failure_cause_count(ctx.0), 1);
        hirsel_rt_v2_failure_clear(ctx.0);
        assert_eq!(hirsel_rt_v2_region_exit(ctx.0, inner, 0, 0), 0);
        // Bound while a failure is active: its own record is a cause, recovered alone.
        assert_eq!(hirsel_rt_v2_failure_begin(ctx.0, 1, 2, 0, 0), 1);
        hirsel_rt_v2_arena_enter(ctx.0, inner);
        hirsel_rt_v2_arena_bound(ctx.0, inner, 10);
        assert_eq!(hirsel_rt_v2_arena_exhausted(ctx.0, inner, 1, 6, 0, 0), 1);
        assert_eq!(hirsel_rt_v2_arena_recover(ctx.0, inner), 1);
        assert_eq!(
            (
                hirsel_rt_v2_failure_active(ctx.0),
                hirsel_rt_v2_failure_cause_count(ctx.0)
            ),
            (1, 0)
        );
        hirsel_rt_v2_failure_clear(ctx.0);
        assert_eq!(hirsel_rt_v2_region_exit(ctx.0, inner, 0, 0), 0);
        assert_eq!(hirsel_rt_v2_region_exit(ctx.0, outer, 0, 0), 0);
    }
}
