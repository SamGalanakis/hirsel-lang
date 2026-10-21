//! Tracked arena Lists (MEM-6, runtime ABI 11): a List built in an arena counts
//! `COUNT_ARENA_UNIQUE` until a dup shares it, which no release undoes. A push charges its
//! element, and a shared List's count when it copies it within the arena; growth relocates
//! within the arena, uncharged, so after `n` pushes the List's storage is at most `4n + 4`
//! elements. A detach copies a tracked block within its arena, a deep copy into an arena
//! tracks the sequences, Maps and Sets it copies except a Map's index and anything below an
//! untracked-below block, and a reserve leaves a tracked block.

use crate::support;

use std::mem::MaybeUninit;
use std::ptr;

use hirsel_rt::alloc::{SEQUENCE_CAPACITY_OFFSET, hirsel_rt_v2_seq_grow};
use hirsel_rt::context::CONFIG_DIAGNOSTIC;
use hirsel_rt::copy::{hirsel_rt_v2_detach, hirsel_rt_v2_unique_copy};
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::region::{
    ALLOC_TRACKED, TRACKED_PREFIX_BYTES, hirsel_rt_v2_arena_alloc, hirsel_rt_v2_arena_bound,
    hirsel_rt_v2_arena_enter, hirsel_rt_v2_arena_push, hirsel_rt_v2_region_exit,
};
use hirsel_rt::release::hirsel_rt_v2_release;
use hirsel_rt::{
    COUNT_ARENA_SHARED, COUNT_ARENA_UNIQUE, COUNT_IMMORTAL, Config, Header, LAYOUT_DYNAMIC,
    LAYOUT_KEYED, LAYOUT_LOGICAL_PER_ELEMENT, LAYOUT_NO_EXTENT, LAYOUT_SEQUENCE,
    LAYOUT_UNTRACKED_BELOW, Layout, Region, STATUS_EXHAUSTED, WHOLE_VALUE,
};
use support::*;

/// The payload of a `List[u64]`: count, capacity, then 8-byte elements from offset 16,
/// charged 8 bytes each.
fn words() -> &'static Layout {
    cached_layout("arena-list-words", 0, || {
        leak(Layout {
            logical: 8,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: u64::MAX,
            flags: LAYOUT_DYNAMIC | LAYOUT_LOGICAL_PER_ELEMENT | LAYOUT_SEQUENCE,
            ..*leaf(16)
        })
    })
}

/// The payload of a `List[Box[u64]]`: as `words`, each element a counted reference.
fn boxes() -> &'static Layout {
    cached_layout("arena-list-boxes", 0, || {
        let program = layout(
            16,
            1,
            None,
            vec![elements_step(0, 0, 16, 8, 1), release_step(WHOLE_VALUE, 0)],
        );
        leak(Layout {
            logical: 8,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: u64::MAX,
            flags: program.flags | LAYOUT_DYNAMIC | LAYOUT_LOGICAL_PER_ELEMENT | LAYOUT_SEQUENCE,
            ..*program
        })
    })
}

fn contexts() -> [Ctx; 2] {
    [
        Ctx::production(),
        Ctx::with(Config {
            flags: CONFIG_DIAGNOSTIC,
            ..Config::new()
        }),
    ]
}

/// Bind the fixture descriptors to `ctx`, which replaces its run context: before any arena
/// opens.
fn bind(ctx: &Ctx, layouts: &[&'static Layout]) {
    for &layout in layouts {
        index(ctx.0, layout);
    }
}

/// An open arena, bounded to `cap` logical bytes unless it is None.
fn open(ctx: &Ctx, cap: Option<u64>) -> Box<MaybeUninit<Region>> {
    let mut arena = Box::new(MaybeUninit::<Region>::uninit());
    unsafe {
        hirsel_rt_v2_arena_enter(ctx.0, arena.as_mut_ptr());
        if let Some(cap) = cap {
            hirsel_rt_v2_arena_bound(ctx.0, arena.as_mut_ptr(), cap);
        }
    }
    arena
}

fn close(ctx: &Ctx, arena: &mut MaybeUninit<Region>) {
    assert_eq!(
        unsafe { hirsel_rt_v2_region_exit(ctx.0, arena.as_mut_ptr(), 0, 0) },
        0
    );
}

fn remaining(arena: &MaybeUninit<Region>) -> u64 {
    unsafe { (*arena.as_ptr()).remaining }
}

/// An empty tracked List of `layout` in `arena`, as an arena literal `[]` builds it.
fn empty(ctx: &Ctx, arena: &mut MaybeUninit<Region>, layout: &'static Layout) -> *mut Header {
    let block = unsafe {
        hirsel_rt_v2_arena_alloc(
            ctx.0,
            arena.as_mut_ptr(),
            index(ctx.0, layout),
            16,
            8,
            ALLOC_TRACKED,
        )
    };
    write_u64(block, 0, 0);
    write_u64(block, SEQUENCE_CAPACITY_OFFSET as usize, 0);
    block
}

fn capacity(block: *mut Header) -> u64 {
    read_u64(block, SEQUENCE_CAPACITY_OFFSET as usize)
}

/// `list.push(mut slot, word)`: the runtime's preparation, then the compiled append.
fn push(ctx: &Ctx, slot: &mut *mut Header, word: u64) -> u8 {
    let status = unsafe { hirsel_rt_v2_arena_push(ctx.0, *slot, 8, 8, slot) };
    if status == 0 {
        let len = read_u64(*slot, 0);
        assert!(len < capacity(*slot), "a prepared push has room");
        write_u64(*slot, 16 + 8 * len as usize, word);
        write_u64(*slot, 0, len + 1);
    }
    status
}

fn elements(block: *mut Header) -> Vec<u64> {
    (0..read_u64(block, 0) as usize)
        .map(|i| read_u64(block, 16 + 8 * i))
        .collect()
}

#[test]
fn a_tracked_block_is_unshared_until_a_dup_and_ignores_releases() {
    for ctx in contexts() {
        bind(&ctx, &[words()]);
        let mut arena = open(&ctx, None);
        let block = empty(&ctx, &mut arena, words());
        assert_eq!(count(block), COUNT_ARENA_UNIQUE);
        let base = block.cast::<u8>().wrapping_sub(24).cast::<*mut Region>();
        assert_eq!(
            unsafe { base.read() },
            arena.as_mut_ptr(),
            "its arena precedes it"
        );
        unsafe { hirsel_rt_v2_dup(ctx.0, block) };
        assert_eq!(count(block), COUNT_ARENA_SHARED);
        unsafe { hirsel_rt_v2_dup(ctx.0, block) };
        let status =
            unsafe { hirsel_rt_v2_release(ctx.0, block, (*block).index(), ptr::null(), 0, 0) };
        assert_eq!((status, count(block)), (0, COUNT_ARENA_SHARED));
        close(&ctx, &mut arena);
    }
}

/// The physical promise of MEM-6 for unaliased growth: after `n` pushes of a u64 to a List
/// built empty, the arena holds at most `4n` elements plus four, besides each block's prefix
/// (its region word, extent, count and capacity) of which there is one per doubling.
#[test]
fn unaliased_pushes_charge_each_element_and_store_at_most_four_times_it() {
    for ctx in contexts() {
        bind(&ctx, &[words()]);
        for n in [1u64, 3, 4, 5, 17, 100, 1000] {
            let mut arena = open(&ctx, Some(8 * n));
            let mut list = empty(&ctx, &mut arena, words());
            let segment = unsafe { (*arena.as_ptr()).segment };
            // The block's prefix precedes its header: its region word and extent.
            let start = list.addr() - (TRACKED_PREFIX_BYTES as usize - 8);
            let mut blocks = 1u64;
            for i in 0..n {
                let before = list;
                assert_eq!(push(&ctx, &mut list, i), 0);
                blocks += u64::from(before != list);
                assert_eq!(count(list), COUNT_ARENA_UNIQUE);
            }
            assert_eq!(elements(list), (0..n).collect::<Vec<_>>());
            assert_eq!(remaining(&arena), 0, "each push charges its element");
            assert_eq!(unsafe { (*arena.as_ptr()).segment }, segment);
            let used = unsafe { (*arena.as_ptr()).cursor }.addr() - start;
            let prefix = TRACKED_PREFIX_BYTES + 16;
            assert!(
                used as u64 <= 4 * n * 8 + 4 * 8 + blocks * prefix,
                "{n} pushes used {used} arena bytes in {blocks} blocks"
            );
            assert!(blocks <= 2 + n.ilog2() as u64, "growth doubles");
            let status = push(&ctx, &mut list, n);
            assert_eq!(status, STATUS_EXHAUSTED);
            assert_eq!(read_u64(list, 0), n, "an exhausted push changes nothing");
            close(&ctx, &mut arena);
        }
    }
}

/// A push to a shared List copies it within its arena, charged its count before the copy and
/// its element, leaving the original to its other holder.
#[test]
fn a_shared_push_copies_within_the_arena_charged_its_count() {
    for ctx in contexts() {
        bind(&ctx, &[words()]);
        for (cap, fits) in [(40, true), (39, false)] {
            let mut arena = open(&ctx, Some(cap));
            let mut list = empty(&ctx, &mut arena, words());
            assert_eq!(push(&ctx, &mut list, 1), 0);
            assert_eq!(push(&ctx, &mut list, 2), 0);
            let original = list;
            unsafe { hirsel_rt_v2_dup(ctx.0, list) };
            let status = push(&ctx, &mut list, 3);
            if fits {
                assert_eq!((status, remaining(&arena)), (0, 0));
                assert_ne!(list, original);
                assert_eq!(count(list), COUNT_ARENA_UNIQUE);
                assert_eq!(elements(list), [1, 2, 3]);
                assert_eq!(elements(original), [1, 2]);
                assert_eq!(count(original), COUNT_ARENA_SHARED);
            } else {
                assert_eq!((status, remaining(&arena)), (STATUS_EXHAUSTED, 23));
                assert_eq!(list, original);
            }
            close(&ctx, &mut arena);
        }
    }
}

/// Heap credits moved into a tracked List are released once when its arena closes, however
/// often it relocated or was copied: an abandoned block's count is zeroed, a copy's elements
/// gain holders, and each block is scheduled once.
#[test]
fn heap_elements_of_relocated_and_copied_lists_are_released_once() {
    for ctx in contexts() {
        let leaf = leaf(8);
        bind(&ctx, &[leaf, boxes()]);
        let mut arena = open(&ctx, None);
        let mut list = empty(&ctx, &mut arena, boxes());
        for i in 0..9 {
            let element = alloc_block(ctx.0, leaf, 8);
            write_u64(element, 0, i);
            assert_eq!(push(&ctx, &mut list, element.addr() as u64), 0);
        }
        unsafe { hirsel_rt_v2_dup(ctx.0, list) };
        let mut copy = list;
        let element = alloc_block(ctx.0, leaf, 8);
        assert_eq!(push(&ctx, &mut copy, element.addr() as u64), 0);
        assert_ne!(copy, list);
        assert_eq!(
            count(read_ref(list, 16)),
            2,
            "both lists hold the first element"
        );
        assert_eq!(ctx.stats().live_blocks, 10);
        close(&ctx, &mut arena);
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}

fn read_ref(block: *mut Header, offset: usize) -> *mut Header {
    read_u64(block, offset) as usize as *mut Header
}

/// A copy-on-write detach of a tracked block (an element store or `pop` of a shared List)
/// copies it within its arena, charged its count; an exhausted budget copies nothing.
#[test]
fn a_detach_copies_a_tracked_block_within_its_arena() {
    for ctx in contexts() {
        bind(&ctx, &[words()]);
        for (cap, fits) in [(32, true), (31, false)] {
            let mut arena = open(&ctx, Some(cap));
            let mut list = empty(&ctx, &mut arena, words());
            assert_eq!(push(&ctx, &mut list, 7), 0);
            assert_eq!(push(&ctx, &mut list, 8), 0);
            unsafe { hirsel_rt_v2_dup(ctx.0, list) };
            let mut out = ptr::null_mut();
            let status = unsafe { hirsel_rt_v2_detach(ctx.0, list, 0, &mut out) };
            if fits {
                assert_eq!((status, remaining(&arena)), (0, 0));
                assert_eq!(count(out), COUNT_ARENA_UNIQUE);
                assert_eq!(elements(out), [7, 8]);
                let base = out.cast::<u8>().wrapping_sub(24).cast::<*mut Region>();
                assert_eq!(unsafe { base.read() }, arena.as_mut_ptr());
            } else {
                assert_eq!((status, remaining(&arena)), (STATUS_EXHAUSTED, 15));
                assert!(out.is_null());
            }
            close(&ctx, &mut arena);
        }
    }
}

/// A push's preparation leaves a heap List alone, unique or not: heap growth is
/// `list.push`'s own.
#[test]
fn a_push_leaves_a_heap_list_alone() {
    for ctx in contexts() {
        bind(&ctx, &[words()]);
        let mut list = alloc_block(ctx.0, words(), 16);
        write_u64(list, 0, 0);
        write_u64(list, SEQUENCE_CAPACITY_OFFSET as usize, 0);
        let heap = list;
        assert_eq!(
            unsafe { hirsel_rt_v2_arena_push(ctx.0, list, 8, 8, &mut list) },
            0
        );
        unsafe { hirsel_rt_v2_dup(ctx.0, list) };
        assert_eq!(
            unsafe { hirsel_rt_v2_arena_push(ctx.0, list, 8, 8, &mut list) },
            0
        );
        assert_eq!((list, count(list)), (heap, 2));
        for _ in 0..2 {
            let status =
                unsafe { hirsel_rt_v2_release(ctx.0, list, (*list).index(), ptr::null(), 0, 0) };
            assert_eq!(status, 0);
        }
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}

/// `list.reserve` is only a hint for a tracked List: the block is returned unchanged.
#[test]
fn a_reserve_leaves_a_tracked_block() {
    for ctx in contexts() {
        bind(&ctx, &[words()]);
        let mut arena = open(&ctx, None);
        let mut list = empty(&ctx, &mut arena, words());
        assert_eq!(push(&ctx, &mut list, 1), 0);
        unsafe { hirsel_rt_v2_dup(ctx.0, list) };
        let grown = unsafe { hirsel_rt_v2_seq_grow(ctx.0, list, 8, 8, 1000) };
        assert_eq!(
            (grown, count(grown), capacity(grown)),
            (list, COUNT_ARENA_SHARED, 4)
        );
        close(&ctx, &mut arena);
    }
}

/// A deep copy into a bounded arena tracks the root when asked and every sequence below it,
/// except those an untracked-below block (a SortedMap's storage) holds, which stay immortal.
/// An unbounded arena, whose charges nothing observes, tracks nothing.
#[test]
fn a_deep_copy_tracks_sequences_except_below_an_untracked_block() {
    for ctx in contexts() {
        let holder = layout(8, 1, None, vec![release_step(0, 0)]);
        let map = with_flags(holder, LAYOUT_UNTRACKED_BELOW);
        bind(&ctx, &[words(), holder, map]);
        let inner = alloc_block(ctx.0, words(), 16);
        write_u64(inner, 0, 0);
        write_u64(inner, SEQUENCE_CAPACITY_OFFSET as usize, 0);
        unsafe { hirsel_rt_v2_dup(ctx.0, inner) };
        let boxed = alloc_block(ctx.0, holder, 8);
        write_ref(boxed, 0, inner);
        let mapped = alloc_block(ctx.0, map, 8);
        write_ref(mapped, 0, inner);
        let mut unbounded = open(&ctx, None);
        let mut out = ptr::null_mut();
        let status = unsafe {
            hirsel_rt_v2_unique_copy(ctx.0, boxed, unbounded.as_mut_ptr(), 0, 0, &mut out)
        };
        assert_eq!(status, 0);
        assert_eq!(count(read_ref(out, 0)), COUNT_IMMORTAL);
        close(&ctx, &mut unbounded);
        let mut arena = open(&ctx, Some(1 << 20));
        for (root, flags, root_count, child_count) in [
            (boxed, 0, COUNT_IMMORTAL, COUNT_ARENA_UNIQUE),
            (mapped, 0, COUNT_IMMORTAL, COUNT_IMMORTAL),
            (inner, ALLOC_TRACKED, COUNT_ARENA_UNIQUE, COUNT_ARENA_UNIQUE),
        ] {
            let mut out = ptr::null_mut();
            let status = unsafe {
                hirsel_rt_v2_unique_copy(ctx.0, root, arena.as_mut_ptr(), flags, 0, &mut out)
            };
            assert_eq!(status, 0);
            assert_eq!(count(out), root_count);
            if root != inner {
                assert_eq!(count(read_ref(out, 0)), child_count);
            }
        }
        close(&ctx, &mut arena);
        for block in [boxed, mapped] {
            let status =
                unsafe { hirsel_rt_v2_release(ctx.0, block, (*block).index(), ptr::null(), 0, 0) };
            assert_eq!(status, 0);
        }
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}

/// A Map's block (`LAYOUT_KEYED`, runtime ABI 12) charges nothing itself and its index nothing:
/// a deep copy into a bounded arena charges only the entries, tracks the block (a fixed
/// layout, so its prefix leaves the extent words unused) and its entries, and keeps the index
/// immortal. A detach of the shared block copies it within its arena uncharged, and the
/// entries it then shares are charged when written.
#[test]
fn a_map_block_tracks_its_entries_and_never_its_index() {
    for ctx in contexts() {
        let map = with_flags(
            layout(16, 2, None, vec![release_step(0, 0), release_step(1, 8)]),
            LAYOUT_KEYED | LAYOUT_NO_EXTENT,
        );
        bind(&ctx, &[words(), map]);
        let list = |len: u64| {
            let block = alloc_block(ctx.0, words(), 16 + 8 * len);
            write_u64(block, 0, len);
            write_u64(block, SEQUENCE_CAPACITY_OFFSET as usize, len);
            block
        };
        let (entries, index) = (list(2), list(3));
        let source = alloc_block(ctx.0, map, 16);
        write_ref(source, 0, entries);
        write_ref(source, 8, index);
        for (cap, status) in [(15, STATUS_EXHAUSTED), (16, 0)] {
            let mut arena = open(&ctx, Some(cap));
            let mut out = ptr::null_mut();
            let copied = unsafe {
                hirsel_rt_v2_unique_copy(
                    ctx.0,
                    source,
                    arena.as_mut_ptr(),
                    ALLOC_TRACKED,
                    0,
                    &mut out,
                )
            };
            assert_eq!(copied, status);
            if status == 0 {
                assert_eq!(
                    (count(out), count(read_ref(out, 0)), count(read_ref(out, 8))),
                    (COUNT_ARENA_UNIQUE, COUNT_ARENA_UNIQUE, COUNT_IMMORTAL)
                );
                unsafe { hirsel_rt_v2_dup(ctx.0, out) };
                let mut copy = ptr::null_mut();
                assert_eq!(unsafe { hirsel_rt_v2_detach(ctx.0, out, 0, &mut copy) }, 0);
                assert_ne!(copy, out);
                assert_eq!(
                    (count(copy), count(read_ref(copy, 0))),
                    (COUNT_ARENA_UNIQUE, COUNT_ARENA_SHARED)
                );
                // The budget is spent: the shared entries' copy does not fit.
                let mut entries = read_ref(copy, 0);
                assert_eq!(
                    unsafe { hirsel_rt_v2_detach(ctx.0, entries, 0, &mut entries) },
                    STATUS_EXHAUSTED
                );
            }
            close(&ctx, &mut arena);
        }
        let status =
            unsafe { hirsel_rt_v2_release(ctx.0, source, (*source).index(), ptr::null(), 0, 0) };
        assert_eq!(status, 0);
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}
