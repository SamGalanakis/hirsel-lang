//! Deep unique copy (`hirsel_rt_v2_unique_copy`): every copied ordinary block has one holder,
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
fn a_source_diamond_becomes_an_exclusive_tree_and_every_copy_is_fresh() {
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
    assert_ne!(
        left, right,
        "each copied holder must own its block exclusively"
    );
    assert_eq!((read_u64(right, 0), count(right)), (42, 1));
    assert_ne!(left, shared);
    assert_eq!(read_u64(left, 0), 42);
    assert_eq!((count(copied), count(left)), (1, 1));
    assert_eq!(count(shared), 2, "the original keeps its holders");
    assert_eq!(ctx.stats().live_blocks, live + 3);

    release(ctx.0, copied);
    assert_eq!(ctx.stats().live_blocks, live);
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
    assert_ne!(a, c);
    assert_ne!(a, some);
    assert_ne!(child(a, 8), child(c, 8));
    assert_eq!((count(a), count(c), count(some)), (1, 1, 2));
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
fn a_wide_holder_loop_does_not_allocate_per_element_work_items() {
    let ctx = Ctx::production();
    let leaf_layout = leaf(8);
    let list = layout(
        8,
        1,
        None,
        vec![elements_step(0, 0, 8, 8, 1), release_step(WHOLE_VALUE, 0)],
    );
    let leaf = alloc_block(ctx.0, leaf_layout, 8);
    write_u64(leaf, 0, 42);
    let length = 4096;
    let items = alloc_block(ctx.0, list, 8 + length * 8);
    write_u64(items, 0, length);
    for i in 0..length {
        if i != 0 {
            unsafe { hirsel_rt_v2_dup(ctx.0, leaf) };
        }
        write_ref(items, (8 + i * 8) as usize, leaf);
    }

    let before = host_calls();
    let mut detached = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_detach(ctx.0, items, 0, &mut detached) },
        0
    );
    let calls = host_calls() - before;
    assert!(
        calls <= 4,
        "one copied block and bounded walk scratch: {calls}"
    );
    for i in 0..length {
        assert_eq!(child(detached, (8 + i * 8) as usize), leaf);
    }
    assert_eq!(count(leaf), 2 * length as u32);
    release(ctx.0, detached);
    assert_eq!(count(leaf), length as u32);
    release(ctx.0, items);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_inline_value_s_holders_are_copied_independently() {
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
    assert_ne!(
        left, right,
        "each copied holder must own its block exclusively"
    );
    assert_eq!((read_u64(right, 0), count(right)), (42, 1));
    assert_ne!(left, shared);
    assert_eq!((read_u64(left, 0), count(left), value[2]), (42, 1, 7));
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
            assert_ne!(child(second, 0), child(second, 8));
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
            assert_ne!(child(escaped, 0), child(escaped, 8));
            assert_eq!(count(child(escaped, 0)), 1);
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
                assert_ne!(
                    child(out, 0),
                    child(out, 8),
                    "the unique copy detaches every holder"
                );
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

#[test]
fn copied_views_distinguish_intervals_and_detach_equal_intervals() {
    let steps = (0..4)
        .rev()
        .map(|i| {
            let mut step = release_step(i, u64::from(i) * 24);
            step.stride = 24;
            step
        })
        .collect();
    let views = layout(96, 4, None, steps);
    let ctx = Ctx::diagnostic();
    index(ctx.0, views);
    let data: Vec<u8> = (0..200).collect();
    let source = unsafe { hirsel_rt::text::allocate_bytes(ctx.0, &data) };
    let left = hirsel_rt::views::Slice {
        start: source.start + 7,
        length: 13,
        ..source
    };
    let right = hirsel_rt::views::Slice {
        start: source.start + 12,
        length: 13,
        ..source
    };
    let empty = hirsel_rt::views::Slice {
        start: source.start + 200,
        length: 0,
        ..source
    };
    let mut values = [left, right, left, empty];
    assert_eq!(
        unsafe {
            hirsel_rt_v2_unique_copy_value(
                ctx.0,
                index(ctx.0, views),
                values.as_mut_ptr().cast(),
                ptr::null_mut(),
                0,
            )
        },
        0
    );
    assert_ne!(values[0].owner, values[2].owner);
    assert_ne!(values[0].owner, values[1].owner);
    assert_eq!(count(values[0].owner), 1);
    for (view, expected) in
        values
            .iter()
            .zip([&data[7..20], &data[12..25], &data[7..20], &data[200..]])
    {
        assert_eq!(view.start, 8);
        assert_eq!(read_u64(view.owner, 0), expected.len() as u64);
        assert_eq!(read_u64(view.owner, 8), expected.len() as u64);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(payload(view.owner).add(16), expected.len()) },
            expected
        );
    }
    assert_eq!(count(source.owner), 1);
    for view in values {
        release(ctx.0, view.owner);
    }
    release(ctx.0, source.owner);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn ten_thousand_short_views_use_less_than_twice_the_copied_byte_storage() {
    let mut step = release_step(0, 0);
    step.stride = 24;
    let view = layout(24, 1, None, vec![step]);
    let ctx = Ctx::diagnostic();
    index(ctx.0, view);
    let source = unsafe { hirsel_rt::text::allocate_bytes(ctx.0, &vec![b'x'; 1024 * 1024]) };
    let before = ctx.stats().live_bytes;
    let mut copies = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        let mut slice = hirsel_rt::views::Slice {
            start: source.start + i * 64,
            length: 64,
            ..source
        };
        assert_eq!(
            unsafe {
                hirsel_rt_v2_unique_copy_value(
                    ctx.0,
                    index(ctx.0, view),
                    (&raw mut slice).cast(),
                    ptr::null_mut(),
                    0,
                )
            },
            0
        );
        copies.push(slice.owner);
    }
    let copied = ctx.stats().live_bytes - before;
    assert!(copied <= 2 * 10_000 * 64, "copy storage: {copied}");
    for owner in copies {
        release(ctx.0, owner);
    }
    release(ctx.0, source.owner);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn single_utf8_view_copies_keep_aliases_and_precharge_bounded_arenas() {
    use hirsel_rt::region::{
        hirsel_rt_v2_arena_bound, hirsel_rt_v2_arena_enter, hirsel_rt_v2_region_exit,
    };
    use std::mem::MaybeUninit;

    let mut step = release_step(0, 0);
    step.stride = 24;
    let layout = layout(24, 1, None, vec![step]);
    for diagnostic in [false, true] {
        for sliced in [false, true] {
            let ctx = Ctx::with(Config {
                flags: if diagnostic { CONFIG_DIAGNOSTIC } else { 0 },
                ..Config::new()
            });
            let view_index = index(ctx.0, layout);
            let source = unsafe { hirsel_rt::text::allocate_bytes(ctx.0, "xéλy".as_bytes()) };
            let original = if sliced {
                hirsel_rt::views::Slice {
                    start: source.start + 1,
                    length: 4,
                    ..source
                }
            } else {
                source
            };
            let mut arena = MaybeUninit::<hirsel_rt::Region>::uninit();
            unsafe {
                let arena = arena.as_mut_ptr();
                hirsel_rt_v2_arena_enter(ctx.0, arena);
                hirsel_rt_v2_arena_bound(ctx.0, arena, original.length - 1);
                let mut copied = original;
                let before = ctx.stats().live_blocks;
                assert_eq!(
                    hirsel_rt_v2_unique_copy_value(
                        ctx.0,
                        view_index,
                        (&raw mut copied).cast(),
                        arena,
                        0
                    ),
                    hirsel_rt::STATUS_EXHAUSTED
                );
                assert_eq!(
                    (copied.owner, copied.start, copied.length),
                    (original.owner, original.start, original.length)
                );
                assert_eq!(ctx.stats().live_blocks, before);
                assert_eq!((*arena).remaining, original.length - 1);
                hirsel_rt_v2_arena_bound(ctx.0, arena, original.length);
                assert_eq!(
                    hirsel_rt_v2_unique_copy_value(
                        ctx.0,
                        view_index,
                        (&raw mut copied).cast(),
                        arena,
                        0
                    ),
                    0
                );
                assert_ne!(copied.owner, source.owner);
                assert_eq!((*arena).remaining, 0);
                assert_eq!((copied.start, copied.length), (8, original.length));
                let expected = if sliced { "éλ" } else { "xéλy" }.as_bytes();
                assert_eq!(
                    std::slice::from_raw_parts(payload(copied.owner).add(16), expected.len()),
                    expected
                );
                assert_eq!(count(source.owner), 1);
                assert_eq!(hirsel_rt_v2_region_exit(ctx.0, arena, 0, 0), 0);
            }
            release(ctx.0, source.owner);
            assert_eq!(ctx.stats().live_blocks, 0);
        }
    }
}

#[test]
fn a_unique_copy_preserves_nested_shared_root_boundaries() {
    let root_layout = with_flags(leaf(8), hirsel_rt::LAYOUT_FROZEN);
    let pair = layout(
        16,
        2,
        None,
        vec![
            step(hirsel_rt::STEP_RELEASE_ROOT, 1, 8),
            step(hirsel_rt::STEP_RELEASE_ROOT, 0, 0),
        ],
    );
    let ctx = Ctx::diagnostic();
    let nested = alloc_block(ctx.0, root_layout, 8);
    write_u64(nested, 0, 42);
    unsafe { hirsel_rt::roots::hirsel_rt_v2_root_dup(ctx.0, nested) };
    let source = alloc_block(ctx.0, pair, 16);
    write_ref(source, 0, nested);
    write_ref(source, 8, nested);
    let copied = copy(ctx.0, source);
    assert_ne!(copied, source);
    assert_eq!((child(copied, 0), child(copied, 8)), (nested, nested));
    assert_eq!(unsafe { hirsel_rt::roots::count(&*ctx.0, nested) }, 4);
    release(ctx.0, copied);
    assert_eq!(unsafe { hirsel_rt::roots::count(&*ctx.0, nested) }, 2);
    release(ctx.0, source);
    assert_eq!(ctx.stats().live_blocks, 0);
}
