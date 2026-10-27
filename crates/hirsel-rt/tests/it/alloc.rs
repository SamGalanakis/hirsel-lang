//! The size-class allocator: class table, alignment paths, LIFO reuse, poisoning, the idle cap
//! and full return at task exit.

use crate::support;

use std::ptr;

#[cfg(not(feature = "system-allocator"))]
use hirsel_rt::alloc::{FREE_POISON, UNINIT_POISON};
use hirsel_rt::alloc::{
    hirsel_rt_v2_alloc, hirsel_rt_v2_block_grow, hirsel_rt_v2_reuse, hirsel_rt_v2_seq_grow,
};
use hirsel_rt::context::hirsel_rt_v2_task_exit;
use hirsel_rt::release::{hirsel_rt_v2_release, hirsel_rt_v2_reset};
use hirsel_rt::{
    Audit, CLASS_SIZES, DOMAIN_HEAP, Header, LARGE_CLASS, Layout, SMALL_MAX, class_of,
};
#[cfg(not(feature = "system-allocator"))]
use hirsel_rt::{Config, SLAB_SIZE};
use support::*;

fn alloc(ctx: &Ctx, layout: &Layout, extent: u64, align: u64) -> *mut Header {
    let block =
        unsafe { hirsel_rt_v2_alloc(ctx.0, index(ctx.0, layout), extent, align, DOMAIN_HEAP, 0) };
    block
}

fn release(ctx: &Ctx, block: *mut Header, layout: &Layout) {
    assert_eq!(
        unsafe { hirsel_rt_v2_release(ctx.0, block, index(ctx.0, layout), ptr::null(), 0, 0) },
        0
    );
}

fn capacity(block: *mut Header) -> u64 {
    unsafe { (*block).capacity() }
}

fn class(block: *mut Header) -> u8 {
    unsafe {
        if (!cfg!(feature = "system-allocator") && (*block).kind() == hirsel_rt::alloc::KIND_LARGE)
            || (*block).capacity() > SMALL_MAX
            || (*block).align() > 16
        {
            LARGE_CLASS
        } else {
            class_of((*block).capacity()) as u8
        }
    }
}

#[test]
fn the_class_function_picks_the_smallest_class_that_fits() {
    for extent in 0..=SMALL_MAX {
        let class = class_of(extent);
        let size = CLASS_SIZES[class] as u64;
        assert!(size >= extent.max(1), "{extent}");
        assert!(
            class == 0 || (CLASS_SIZES[class - 1] as u64) < extent,
            "{extent}"
        );
    }
}

#[test]
fn class_alignment_up_to_16_and_larger_on_the_large_path() {
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    for align in [1u64, 2, 4, 8, 16] {
        for extent in [1u64, 24, 256, 1000, 4096] {
            let block = alloc(&ctx, empty, extent, align);
            assert_ne!(class(block), LARGE_CLASS);
            assert_eq!(payload(block) as usize % align as usize, 0);
            assert_eq!(
                payload(block) as usize % align.max(8) as usize,
                0,
                "class payloads preserve physical alignment"
            );
            release(&ctx, block, empty);
        }
    }
    for align in [32u64, 64, 256, 4096, 65536] {
        for extent in [1u64, 100, 5000] {
            let block = alloc(&ctx, empty, extent, align);
            assert_eq!(class(block), LARGE_CLASS, "align {align}");
            assert_eq!(payload(block) as usize % align as usize, 0, "align {align}");
            assert_eq!(unsafe { (*block).align() }, align);
            unsafe { ptr::write_bytes(payload(block), 7, extent as usize) };
            release(&ctx, block, empty);
        }
    }
    let huge = alloc(&ctx, empty, 3 << 20, 8);
    assert_eq!(class(huge), LARGE_CLASS);
    assert!(ctx.stats().large_bytes as usize >= 3 << 20);
    release(&ctx, huge, empty);
    assert_eq!(ctx.stats().large_bytes, 0);
    assert_eq!(ctx.stats().live_blocks, 0);
}

// mimalloc page invariant: freeing defers reuse until the allocation list and
// fresh page capacity have been consumed. Exercise the public runtime path.

#[cfg(not(feature = "system-allocator"))]
#[test]
fn allocation_stays_in_one_slab_whatever_order_blocks_were_freed_in() {
    let ctx = Ctx::production();
    let empty = leaf(0);
    let slab = |block: *mut Header| block as usize & !(SLAB_SIZE - 1);
    let blocks: Vec<_> = (0..3000).map(|_| alloc(&ctx, empty, 48, 8)).collect();
    let mut slabs: Vec<usize> = blocks.iter().map(|&block| slab(block)).collect();
    slabs.dedup();
    assert!(slabs.len() >= 3, "the class spans several slabs");
    // Free round-robin across the slabs, so consecutive frees never share a slab.
    let by_slab: Vec<Vec<_>> = slabs
        .iter()
        .map(|&base| {
            blocks
                .iter()
                .copied()
                .filter(|&b| slab(b) == base)
                .collect()
        })
        .collect();
    for index in 0..by_slab.iter().map(Vec::len).max().unwrap() {
        for list in &by_slab {
            if let Some(&block) = list.get(index) {
                release(&ctx, block, empty);
            }
        }
    }
    let again: Vec<_> = (0..3000).map(|_| slab(alloc(&ctx, empty, 48, 8))).collect();
    let switches = again.windows(2).filter(|pair| pair[0] != pair[1]).count();
    assert_eq!(
        switches,
        slabs.len() - 1,
        "each slab serves all its blocks before the next"
    );
    assert_eq!(ctx.stats().slab_bytes, (slabs.len() * SLAB_SIZE) as u64);
}

#[cfg(not(feature = "system-allocator"))]
#[test]
fn diagnostic_mode_poisons_freed_and_fresh_payloads() {
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    let block = alloc(&ctx, empty, 64, 8);
    let bytes =
        |block: *mut Header| unsafe { std::slice::from_raw_parts(payload(block), 64).to_vec() };
    assert!(bytes(block).iter().all(|&byte| byte == UNINIT_POISON));
    unsafe { ptr::write_bytes(payload(block), 1, 64) };
    release(&ctx, block, empty);
    // The slab still backs the freed block, so its poison is observable.
    assert!(bytes(block).iter().all(|&byte| byte == FREE_POISON));
    let mut fresh = Vec::new();
    loop {
        let next = alloc(&ctx, empty, 64, 8);
        assert!(bytes(next).iter().all(|&byte| byte == UNINIT_POISON));
        if next == block {
            break;
        }
        fresh.push(next);
        assert!(fresh.len() < SLAB_SIZE / 64);
    }
    for next in fresh.into_iter().chain([block]) {
        release(&ctx, next, empty);
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[cfg(not(feature = "system-allocator"))]
#[test]
fn diagnostic_mode_rejects_a_write_after_free() {
    if in_child("diagnostic_mode_rejects_a_write_after_free") {
        let ctx = Ctx::diagnostic();
        let empty = leaf(0);
        let block = alloc(&ctx, empty, 64, 8);
        release(&ctx, block, empty);
        unsafe { *payload(block).add(9) = 0 };
        // Reuse is deferred until fresh capacity is exhausted; the bad byte must
        // still be caught when this page's local frees are collected.
        for _ in 0..SLAB_SIZE / 64 {
            alloc(&ctx, empty, 64, 8);
        }
        return;
    }
    assert_child_aborts(
        "diagnostic_mode_rejects_a_write_after_free",
        "written after free",
    );
}

#[cfg(not(feature = "system-allocator"))]
#[test]
fn idle_slabs_above_the_cap_are_returned_after_churn() {
    let cap = 1 << 20;
    let ctx = Ctx::with(Config {
        heap_idle_cap: cap,
        ..Config::new()
    });
    let empty = leaf(0);
    let mut kept = Vec::new();
    for round in 0..4u64 {
        let mut blocks = Vec::new();
        for index in 0..40_000u64 {
            let extent = [16, 72, 300, 1500][(index % 4) as usize] + round;
            blocks.push(alloc(&ctx, empty, extent, 8));
        }
        let peak = ctx.stats().slab_bytes;
        assert!(peak > 4 * cap, "the churn spans many slabs");
        // Keep every thousandth block alive so some slabs stay partly used.
        for (index, block) in blocks.into_iter().enumerate() {
            if index % 1000 == 0 {
                kept.push(block)
            } else {
                release(&ctx, block, empty)
            }
        }
        unsafe { hirsel_rt::context::hirsel_rt_v2_trim(ctx.0) };
        let stats = ctx.stats();
        assert!(stats.idle_slab_bytes <= cap, "round {round}: {stats:?}");
        assert!(stats.slab_bytes < peak);
        assert_eq!(stats.live_blocks, kept.len() as u64);
    }
    // The trimmed free lists still serve every class correctly.
    let again: Vec<_> = (0..20_000u64)
        .map(|index| alloc(&ctx, empty, 16 + index % 4000, 8))
        .collect();
    for &block in &again {
        unsafe { ptr::write_bytes(payload(block), 3, capacity(block) as usize) };
    }
    for block in again.into_iter().chain(kept) {
        release(&ctx, block, empty);
    }
    unsafe { hirsel_rt::context::hirsel_rt_v2_trim(ctx.0) };
    assert!(ctx.stats().slab_bytes <= cap);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[cfg(not(feature = "system-allocator"))]
#[test]
fn worker_collection_reclaims_idle_slabs_without_a_region_exit() {
    let ctx = Ctx::production();
    let empty = leaf(0);
    let blocks: Vec<_> = (0..12_000).map(|_| alloc(&ctx, empty, 1024, 8)).collect();
    for block in blocks {
        release(&ctx, block, empty);
    }
    assert!(ctx.stats().idle_slab_bytes > 8 << 20);
    unsafe { hirsel_rt::context::hirsel_rt_v2_worker_collect(ctx.0) };
    let stats = ctx.stats();
    assert!(stats.idle_slab_bytes <= 1 << 20, "{stats:?}");
    assert_eq!(stats.live_blocks, 0);
    let again = alloc(&ctx, empty, 1024, 8);
    unsafe { ptr::write_bytes(payload(again), 7, capacity(again) as usize) };
    release(&ctx, again, empty);
}

#[cfg(not(feature = "system-allocator"))]
#[cfg(target_os = "linux")]
#[test]
fn large_buffers_return_pages_even_when_libc_would_retain_them() {
    const NAME: &str = "large_buffers_return_pages_even_when_libc_would_retain_them";
    if !in_child(NAME) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                format!("alloc::{NAME}"),
                "--exact".into(),
                "--nocapture".into(),
            ])
            .env("HIRSEL_RT_CHILD", NAME)
            // Reproduce the arena policy after glibc has seen a large transient buffer,
            // independently of this process's other tests and their allocation order.
            .env("MALLOC_MMAP_THRESHOLD_", (32 << 20).to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let ctx = Ctx::production();
    let empty = leaf(0);
    let bytes = 512 * 1024;
    let buffer = alloc(&ctx, empty, bytes, 16);
    let guard = alloc(&ctx, empty, bytes, 16);
    unsafe {
        ptr::write_bytes(payload(buffer), 0x39, bytes as usize);
        ptr::write_bytes(payload(guard), 0x71, bytes as usize);
    }
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    let base = (buffer.addr() & !(page - 1)) as *mut libc::c_void;
    release(&ctx, buffer, empty);
    let mut residency = vec![0u8; bytes as usize / page];
    let result = unsafe { libc::mincore(base, bytes as usize, residency.as_mut_ptr()) };
    if result == 0 {
        assert!(
            residency
                .iter()
                .skip(1)
                .take(residency.len() - 2)
                .all(|b| b & 1 == 0),
            "released large buffer is still resident: {residency:?}"
        );
    } else {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOMEM)
        );
    }
    assert_eq!(unsafe { *payload(guard) }, 0x71);
    assert_eq!(unsafe { *payload(guard).add(bytes as usize - 1) }, 0x71);
    assert_eq!(ctx.stats().live_blocks, 1);
    release(&ctx, guard, empty);
}

// Both sides of the original backing threshold, including prefix/padding and
// alignments that keep System's existing over-aligned allocation path.
#[test]
fn large_threshold_bursts_preserve_alignment_and_live_neighbors() {
    let ctx = Ctx::production();
    let empty = leaf(0);
    let guard = alloc(&ctx, empty, 512 * 1024, 16);
    unsafe { ptr::write_bytes(payload(guard), 0x71, 512 * 1024) };
    for _ in 0..8 {
        for bytes in [128 * 1024 - 128, 128 * 1024 - 1, 128 * 1024, 128 * 1024 + 1] {
            for align in [8, 16, 64, 4096, 8192] {
                let block = alloc(&ctx, empty, bytes, align);
                assert_eq!(payload(block).addr() % align as usize, 0);
                unsafe { ptr::write_bytes(payload(block), 0x39, bytes as usize) };
                release(&ctx, block, empty);
                assert_eq!(unsafe { *payload(guard) }, 0x71);
                assert_eq!(unsafe { *payload(guard).add(512 * 1024 - 1) }, 0x71);
            }
        }
        unsafe { hirsel_rt::context::hirsel_rt_v2_worker_collect(ctx.0) };
        assert_eq!(ctx.stats().live_blocks, 1);
    }
    release(&ctx, guard, empty);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[cfg(not(feature = "system-allocator"))]
#[cfg(target_os = "linux")]
#[test]
fn retiring_slabs_discards_their_resident_pages_and_preserves_live_ones() {
    let ctx = Ctx::with(Config {
        heap_idle_cap: 1,
        ..Config::new()
    });
    let empty = leaf(0);
    let blocks: Vec<_> = (0..2000).map(|_| alloc(&ctx, empty, 1024, 8)).collect();
    let slab = |block: *mut Header| (block.addr() & !(SLAB_SIZE - 1)) as *mut libc::c_void;
    let first = blocks[0];
    let live_slab = slab(first);
    let retired_slab = slab(*blocks.last().unwrap());
    assert_ne!(live_slab, retired_slab);
    for &block in &blocks {
        unsafe { ptr::write_bytes(payload(block), 0x39, 1024) };
    }
    for block in blocks.into_iter().skip(1) {
        release(&ctx, block, empty);
    }
    unsafe { hirsel_rt::context::hirsel_rt_v2_worker_collect(ctx.0) };
    assert_eq!(ctx.stats().live_blocks, 1);
    assert_eq!(unsafe { *payload(first) }, 0x39);
    // mincore may return ENOMEM if libc returned the slab's mapping outright.
    // Otherwise all but allocator metadata at its edges must be nonresident.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    let mut residency = vec![0u8; SLAB_SIZE / page];
    let result = unsafe { libc::mincore(retired_slab, SLAB_SIZE, residency.as_mut_ptr()) };
    if result == 0 {
        assert!(
            residency
                .iter()
                .skip(1)
                .take(residency.len().saturating_sub(2))
                .all(|b| b & 1 == 0),
            "{residency:?}"
        );
    } else {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOMEM)
        );
    }
    release(&ctx, first, empty);
}

#[test]
fn task_exit_returns_every_byte_even_with_live_blocks() {
    let empty = leaf(0);
    let before = host_outstanding();
    let ctx = Ctx::diagnostic();
    let mut live = 0;
    for index in 0..5000u64 {
        let block = alloc(
            &ctx,
            empty,
            index % 6000,
            if index % 7 == 0 { 64 } else { 8 },
        );
        if index % 3 == 0 {
            release(&ctx, block, empty)
        } else {
            live += 1
        }
    }
    let mut region = std::mem::MaybeUninit::uninit();
    unsafe { hirsel_rt::region::hirsel_rt_v2_region_enter(ctx.0, region.as_mut_ptr()) };
    let mut storage = ptr::null_mut();
    unsafe {
        hirsel_rt::region::hirsel_rt_v2_region_reserve(
            ctx.0,
            region.as_mut_ptr(),
            100_000,
            8,
            &mut storage,
        )
    };
    assert!(ctx.stats().backing_bytes > 0);
    let mut audit = Audit::default();
    unsafe { hirsel_rt_v2_task_exit(ctx.0, &mut audit) };
    assert_eq!(
        audit.live_blocks, live,
        "the audit reports what was still held"
    );
    assert_eq!(audit.active_regions, 1);
    assert_eq!(audit.retained_bytes, 0);
    assert_eq!(ctx.stats().backing_bytes, 0);
    drop(ctx);
    assert_eq!(
        host_outstanding(),
        before,
        "every host byte the task took came back"
    );
}

#[test]
fn an_extent_smaller_than_its_layout_is_rejected_in_diagnostic_mode() {
    if in_child("an_extent_smaller_than_its_layout_is_rejected_in_diagnostic_mode") {
        let ctx = Ctx::diagnostic();
        alloc(&ctx, leaf(32), 16, 8);
        return;
    }
    assert_child_aborts(
        "an_extent_smaller_than_its_layout_is_rejected_in_diagnostic_mode",
        "smaller than its layout size",
    );
    // Accepted twin: an extent at least the layout size.
    let ctx = Ctx::diagnostic();
    let layout = leaf(32);
    release(&ctx, alloc(&ctx, layout, 32, 8), layout);
}

#[test]
fn an_alignment_that_is_not_a_power_of_two_is_an_invariant_fault() {
    if in_child("an_alignment_that_is_not_a_power_of_two_is_an_invariant_fault") {
        let ctx = Ctx::production();
        alloc(&ctx, leaf(0), 16, 24);
        return;
    }
    assert_child_aborts(
        "an_alignment_that_is_not_a_power_of_two_is_an_invariant_fault",
        "not a power of two",
    );
}

#[test]
fn an_extent_beyond_the_host_terminates_environmentally() {
    if in_child("an_extent_beyond_the_host_terminates_environmentally") {
        let ctx = Ctx::production();
        alloc(&ctx, leaf(0), u64::MAX - 8, 8);
        return;
    }
    assert_child_aborts(
        "an_extent_beyond_the_host_terminates_environmentally",
        "environmental",
    );
}

/// A byte sequence block holding `[1, 2, 3]` as a construction writes it (runtime ABI 5): its
/// u64 count, its u64 capacity (the count), then the bytes.
fn bytes_block(ctx: &Ctx, layout: &Layout) -> *mut Header {
    let block = alloc(ctx, layout, 19, 8);
    unsafe {
        let p = payload(block);
        p.cast::<u64>().write(3);
        p.add(8).cast::<u64>().write(3);
        ptr::copy_nonoverlapping([1u8, 2, 3].as_ptr(), p.add(16), 3);
    }
    block
}

/// The capacity word of the sequence block `block`.
fn capacity_word(block: *mut Header) -> u64 {
    unsafe { payload(block).add(8).cast::<u64>().read() }
}

fn reserve(ctx: &Ctx, block: &mut *mut Header, size: u64, align: u64, capacity: u64) {
    *block = unsafe { hirsel_rt_v2_seq_grow(ctx.0, *block, size, align, capacity) };
}

#[test]
fn seq_reserve_amortizes_incremental_growth() {
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    let mut block = bytes_block(&ctx, empty);
    let limit = SMALL_MAX + 4096;
    let mut relocations = 0;
    for wanted in 4..=limit {
        let before = block;
        reserve(&ctx, &mut block, 1, 1, wanted);
        relocations += u32::from(block != before);
        assert!(
            relocations <= limit.ilog2() + 2,
            "incremental reserve must amortize growth: {relocations} relocations at {wanted} bytes"
        );
        unsafe {
            let p = payload(block);
            assert_eq!(p.cast::<u64>().read(), wanted - 1);
            p.add(16 + wanted as usize - 1).write((wanted % 251) as u8);
            p.cast::<u64>().write(wanted);
        }
    }
    unsafe {
        let bytes = std::slice::from_raw_parts(payload(block).add(16), limit as usize);
        assert_eq!(&bytes[..3], &[1, 2, 3]);
        for (i, byte) in bytes.iter().enumerate().skip(3) {
            assert_eq!(*byte, ((i + 1) % 251) as u8);
        }
    }
    release(&ctx, block, empty);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn seq_reserve_lands_on_every_class_boundary_and_keeps_the_elements() {
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    // The 16-byte class holds no byte past the count and capacity words.
    for (index, &size) in CLASS_SIZES.iter().enumerate().skip(1) {
        let size = size as u64;
        // A byte capacity filling the class exactly after the count and capacity, and one
        // byte more.
        for (wanted, class_index) in [
            (size - 16, index as u8),
            (
                size - 15,
                if index + 1 < CLASS_SIZES.len() {
                    index as u8 + 1
                } else {
                    LARGE_CLASS
                },
            ),
        ] {
            let mut block = bytes_block(&ctx, empty);
            reserve(&ctx, &mut block, 1, 1, wanted);
            assert_eq!(class(block), class_index, "{wanted} bytes");
            assert!(capacity(block) - 16 >= wanted, "{wanted} bytes");
            // The capacity word is exact: every slot of the class.
            assert_eq!(capacity_word(block), capacity(block) - 16, "{wanted} bytes");
            unsafe {
                let p = payload(block);
                assert_eq!(p.cast::<u64>().read(), 3);
                assert_eq!(std::slice::from_raw_parts(p.add(16), 3), [1, 2, 3]);
                assert_eq!((*block).count, 1);
            }
            release(&ctx, block, empty);
        }
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn seq_reserve_leaves_a_block_that_already_holds_the_capacity() {
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    let mut block = bytes_block(&ctx, empty);
    let before = block;
    // The 32-byte class holds sixteen bytes after the count and capacity. Leaving the block
    // alone still makes its capacity word, written as the count, exact.
    assert_eq!(capacity_word(block), 3);
    reserve(&ctx, &mut block, 1, 1, 16);
    assert_eq!(block, before);
    assert_eq!(capacity_word(block), 16);
    reserve(&ctx, &mut block, 1, 1, 17);
    assert_ne!(block, before);
    assert_eq!(capacity_word(block), capacity(block) - 16);
    // Zero-sized elements hold every capacity.
    let moved = block;
    reserve(&ctx, &mut block, 0, 1, u64::MAX);
    assert_eq!(block, moved);
    assert_eq!(capacity_word(block), u64::MAX);
    release(&ctx, block, empty);
}

#[test]
fn seq_reserve_gives_a_static_or_shared_block_unique_storage_at_the_capacity() {
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    // The static empty sequence an empty list literal reads: extent, immortal header, count
    // and capacity.
    #[repr(C)]
    struct StaticEmpty {
        extent: hirsel_rt::alloc::Extent,
        header: Header,
        length: u64,
        capacity: u64,
    }
    let mut source = StaticEmpty {
        extent: hirsel_rt::alloc::Extent {
            capacity: 16,
            align: 8,
        },
        header: Header {
            count: hirsel_rt::COUNT_IMMORTAL,
            descriptor: index(ctx.0, empty) | (hirsel_rt::alloc::KIND_STATIC << 29),
        },
        length: 0,
        capacity: 0,
    };
    let mut block = &raw mut source.header;
    reserve(&ctx, &mut block, 8, 8, 64);
    assert_ne!(block, &raw mut source.header);
    assert!(capacity(block) - 16 >= 64 * 8);
    assert_eq!(capacity_word(block), (capacity(block) - 16) / 8);
    unsafe {
        assert_eq!(((*block).count, (*block).domain()), (1, DOMAIN_HEAP));
        assert_eq!(payload(block).cast::<u64>().read(), 0);
    }
    assert_eq!(
        (source.header.count, source.length),
        (hirsel_rt::COUNT_IMMORTAL, 0)
    );
    release(&ctx, block, empty);
    // A shared heap block is copied once at the capacity; the caller's credit goes back.
    let shared = bytes_block(&ctx, empty);
    unsafe { (*shared).count = 2 };
    let mut block = shared;
    reserve(&ctx, &mut block, 1, 1, 100);
    assert_ne!(block, shared);
    assert!(capacity(block) - 16 >= 100);
    assert_eq!(capacity_word(block), capacity(block) - 16);
    assert_eq!(capacity_word(shared), 3);
    unsafe {
        let p = payload(block);
        assert_eq!(p.cast::<u64>().read(), 3);
        assert_eq!(std::slice::from_raw_parts(p.add(16), 3), [1, 2, 3]);
        assert_eq!(((*block).count, (*shared).count), (1, 1));
    }
    release(&ctx, block, empty);
    release(&ctx, shared, empty);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn seq_reserve_offsets_wide_elements_by_their_alignment() {
    // Elements aligned to 32 start at offset 32: an empty sequence of them is a 32-byte
    // extent, which its reserve grows by whole elements.
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    let mut block = alloc(&ctx, empty, 32, 32);
    reserve(&ctx, &mut block, 32, 32, 0);
    unsafe { payload(block).cast::<u64>().write(0) };
    reserve(&ctx, &mut block, 32, 32, 1);
    assert!(capacity(block) >= 64);
    assert_eq!(unsafe { payload(block).cast::<u64>().read() }, 0);
    release(&ctx, block, empty);
}

#[test]
fn block_grow_keeps_the_leading_bytes_or_leaves_a_block_that_holds_the_extent() {
    // A sequential Packed sequence (MEM-13) grows by bytes: its count, used-bytes word and
    // elements are the leading bytes kept.
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    let mut block = bytes_block(&ctx, empty);
    let before = block;
    // The 32-byte class already holds 32 payload bytes.
    block = unsafe { hirsel_rt_v2_block_grow(ctx.0, block, 19, 32, 8) };
    assert_eq!(block, before);
    block = unsafe { hirsel_rt_v2_block_grow(ctx.0, block, 19, 200, 8) };
    assert_ne!(block, before);
    assert!(capacity(block) >= 200);
    unsafe {
        let p = payload(block);
        assert_eq!(p.cast::<u64>().read(), 3);
        assert_eq!(std::slice::from_raw_parts(p.add(16), 3), [1, 2, 3]);
        assert_eq!((*block).count, 1);
    }
    release(&ctx, block, empty);
    assert_eq!(ctx.stats().live_blocks, 0);
}

// Port the single-owner part of mimalloc test-stress.c: deterministic varying
// sizes, initialized payloads checked before free, and surviving allocations
// across rounds. Hirsel task heaps intentionally have no cross-thread transfer.
#[test]
fn interleaved_page_exhaustion_preserves_live_payloads_in_every_class() {
    use std::collections::BTreeSet;
    for diagnostic in [false, true] {
        let ctx = if diagnostic {
            Ctx::diagnostic()
        } else {
            Ctx::production()
        };
        let empty = leaf(0);
        let mut live = Vec::new();
        let mut addresses = BTreeSet::new();
        for round in 0..4 {
            for i in 0..4096 {
                let extent = CLASS_SIZES[(i * 17 + round) % CLASS_SIZES.len()] as u64;
                let block = alloc(&ctx, empty, extent, if i % 2 == 0 { 8 } else { 16 });
                assert!(
                    addresses.insert(block as usize),
                    "allocated an already live block"
                );
                let byte = (i % 251 + 1) as u8;
                unsafe {
                    payload(block).write_bytes(byte, extent as usize);
                }
                live.push((block, extent, byte));
            }
            let mut survivors = Vec::new();
            for (i, (block, extent, byte)) in live.drain(..).enumerate() {
                assert!(
                    unsafe { std::slice::from_raw_parts(payload(block), extent as usize) }
                        .iter()
                        .all(|b| *b == byte),
                    "allocation corrupted a live payload"
                );
                if i % 7 == round {
                    survivors.push((block, extent, byte));
                } else {
                    release(&ctx, block, empty);
                    addresses.remove(&(block as usize));
                }
            }
            live = survivors;
            unsafe {
                hirsel_rt::context::hirsel_rt_v2_trim(ctx.0);
            }
        }
        for (block, _, _) in live {
            release(&ctx, block, empty);
        }
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}

/// Compiled code's half of the fast path contract (runtime ABI 4, `hirsel_rt_v2.h`), done
/// through the published context and page offsets exactly as `hirsel-llvm` emits it. `None`
/// or `false` is where compiled code would call the runtime entry instead.
#[cfg(not(any(feature = "system-allocator", feature = "measurement")))]
mod compiled {
    use hirsel_rt::{
        CTX_DUP_MAX_OFFSET, CTX_FREE_INLINE_OFFSET, CTX_IDLE_SLABS_OFFSET, CTX_PAGES_OFFSET,
        Header, PAGE_FREE_OFFSET, PAGE_LIVE_OFFSET, PAGE_LOCAL_FREE_OFFSET, SLAB_SIZE, TaskCtx,
    };

    unsafe fn at<T>(base: *mut u8, offset: usize) -> *mut T {
        unsafe { base.add(offset).cast() }
    }

    pub fn dup_max(ctx: *mut TaskCtx) -> u32 {
        unsafe { *at::<u32>(ctx.cast(), CTX_DUP_MAX_OFFSET) }
    }

    pub fn free_inline(ctx: *mut TaskCtx) -> u32 {
        unsafe { *at::<u32>(ctx.cast(), CTX_FREE_INLINE_OFFSET) }
    }

    pub fn idle(ctx: *mut TaskCtx) -> u64 {
        unsafe { *at::<u64>(ctx.cast(), CTX_IDLE_SLABS_OFFSET) }
    }

    pub fn page(ctx: *mut TaskCtx, pool: usize) -> *mut u8 {
        unsafe { *at::<*mut u8>(ctx.cast(), CTX_PAGES_OFFSET + 8 * pool) }
    }

    pub fn alloc(ctx: *mut TaskCtx, pool: usize, index: u32) -> Option<*mut Header> {
        unsafe {
            let page = page(ctx, pool);
            if page.is_null() {
                return None;
            }
            let block = *at::<*mut Header>(page, PAGE_FREE_OFFSET);
            if block.is_null() {
                return None;
            }
            *at::<*mut Header>(page, PAGE_FREE_OFFSET) = *block.cast::<*mut Header>();
            let live = at::<u32>(page, PAGE_LIVE_OFFSET);
            if *live == 0 {
                *at::<u64>(ctx.cast(), CTX_IDLE_SLABS_OFFSET) -= 1;
            }
            *live += 1;
            block.write(Header {
                count: 1,
                descriptor: index,
            });
            Some(block)
        }
    }

    pub fn dup(ctx: *mut TaskCtx, block: *mut Header) -> bool {
        unsafe {
            let below = (*block).count < dup_max(ctx);
            if below {
                (*block).count += 1;
            }
            below
        }
    }

    pub fn free(ctx: *mut TaskCtx, block: *mut Header) -> bool {
        unsafe {
            if free_inline(ctx) == 0 || (*block).descriptor >> 29 & 3 != 0 {
                return false;
            }
            let page = (block as usize & !(SLAB_SIZE - 1)) as *mut u8;
            let head = *at::<*mut Header>(page, PAGE_LOCAL_FREE_OFFSET);
            if head.is_null() {
                return false;
            }
            block.cast::<*mut Header>().write(head);
            *at::<*mut Header>(page, PAGE_LOCAL_FREE_OFFSET) = block;
            let live = at::<u32>(page, PAGE_LIVE_OFFSET);
            *live -= 1;
            if *live == 0 {
                *at::<u64>(ctx.cast(), CTX_IDLE_SLABS_OFFSET) += 1;
            }
            true
        }
    }
}

/// Blocks allocated, duplicated and freed inline interleave with the runtime's own entries on
/// the same pages: headers, live counts, the idle slab count and trim stay exact, and once a
/// page is current nearly every operation stays inline, single-block churn included.
#[cfg(not(any(feature = "system-allocator", feature = "measurement")))]
#[test]
fn compiled_fast_paths_share_the_page_lists_with_the_runtime() {
    use hirsel_rt::finite_count_max;
    use hirsel_rt::release::hirsel_rt_v2_free;
    let ctx = Ctx::production();
    let layout = leaf(16);
    let index = support::index(ctx.0, layout);
    let pool = class_of(16);
    assert_eq!(
        hirsel_lir::runtime_abi::small_pool(16, 8),
        Some(pool as u64)
    );
    assert_eq!(compiled::dup_max(ctx.0), finite_count_max(0));
    assert_eq!(compiled::free_inline(ctx.0), 1);
    assert!(compiled::page(ctx.0, pool).is_null());
    assert_eq!(compiled::alloc(ctx.0, pool, index), None);
    let (mut blocks, mut inline) = (vec![alloc(&ctx, layout, 16, 8)], 0);
    let slab = |block: *mut Header| (block as usize & !(SLAB_SIZE as usize - 1)) as *mut u8;
    assert_eq!(compiled::page(ctx.0, pool), slab(blocks[0]));
    for _ in 0..20_000 {
        let block = match compiled::alloc(ctx.0, pool, index) {
            Some(block) => {
                inline += 1;
                block
            }
            None => alloc(&ctx, layout, 16, 8),
        };
        assert_eq!(unsafe { ((*block).count, (*block).descriptor) }, (1, index));
        blocks.push(block);
    }
    // Refills and fresh slabs stay with the runtime: about one call per 8 KiB batch.
    assert!(inline >= 19_800, "{inline}");
    assert_eq!(ctx.stats().live_blocks, 20_001);
    for (i, &block) in blocks.iter().enumerate() {
        assert!(compiled::dup(ctx.0, block));
        assert_eq!(unsafe { (*block).count }, 2);
        // The other holder goes back through the runtime or the glue's inline edge.
        if i % 2 == 0 {
            release(&ctx, block, layout);
        } else {
            unsafe { (*block).count -= 1 };
        }
    }
    let mut freed_inline = 0;
    for (i, block) in blocks.drain(..).enumerate() {
        if i % 3 == 0 {
            release(&ctx, block, layout);
        } else if compiled::free(ctx.0, block) {
            freed_inline += 1;
        } else {
            unsafe { hirsel_rt_v2_free(ctx.0, block) };
        }
    }
    assert!(freed_inline > 13_000 - 100, "{freed_inline}");
    let stats = ctx.stats();
    assert_eq!(stats.live_blocks, 0);
    assert_eq!(stats.idle_slab_bytes, stats.slab_bytes, "{stats:?}");
    assert_eq!(compiled::idle(ctx.0) * SLAB_SIZE as u64, stats.slab_bytes);
    // One block at a time: an idle page's first allocation and last free stay inline too.
    let (mut churned, mut calls) = (0, 0);
    for _ in 0..10_000 {
        let block = compiled::alloc(ctx.0, pool, index).unwrap_or_else(|| {
            calls += 1;
            alloc(&ctx, layout, 16, 8)
        });
        if compiled::free(ctx.0, block) {
            churned += 1;
        } else {
            calls += 1;
            unsafe { hirsel_rt_v2_free(ctx.0, block) };
        }
    }
    assert!(churned > 9_900 && calls < 100, "{churned} {calls}");
    let stats = ctx.stats();
    assert_eq!(
        (stats.live_blocks, stats.idle_slab_bytes),
        (0, stats.slab_bytes)
    );
    unsafe { hirsel_rt::context::hirsel_rt_v2_trim(ctx.0) };
    assert_eq!(
        compiled::idle(ctx.0) * SLAB_SIZE as u64,
        ctx.stats().slab_bytes
    );
    // A saturated or immortal count is the runtime's.
    let block = alloc(&ctx, layout, 16, 8);
    unsafe { (*block).count = finite_count_max(0) };
    assert!(!compiled::dup(ctx.0, block));
    unsafe { (*block).count = 1 };
    release(&ctx, block, layout);
}

/// Diagnostic mode keeps every compiled fast path on its runtime entry.
#[cfg(not(any(feature = "system-allocator", feature = "measurement")))]
#[test]
fn diagnostic_mode_sends_every_compiled_fast_path_to_the_runtime() {
    let ctx = Ctx::diagnostic();
    let layout = leaf(16);
    let block = alloc(&ctx, layout, 16, 8);
    let pool = class_of(16);
    assert_eq!(
        (compiled::dup_max(ctx.0), compiled::free_inline(ctx.0)),
        (0, 0)
    );
    assert!(compiled::page(ctx.0, pool).is_null());
    assert_eq!(compiled::alloc(ctx.0, pool, 0), None);
    assert!(!compiled::dup(ctx.0, block));
    assert!(!compiled::free(ctx.0, block));
    release(&ctx, block, layout);
}

/// The compiler's pool for a constant small request is the runtime's.
#[test]
fn the_compiled_pool_of_every_small_request_is_the_runtimes() {
    for extent in 0..=SMALL_MAX + 1 {
        for align in [1, 2, 4, 8, 16, 32] {
            let runtime = (extent <= SMALL_MAX && align <= 16)
                .then(|| (class_of(extent) + if align > 8 { 32 } else { 0 }) as u64);
            assert_eq!(
                hirsel_lir::runtime_abi::small_pool(extent, align),
                runtime,
                "{extent} {align}"
            );
        }
    }
}

/// C keys a reuse token by `runtime_abi::size_class` (`hirsel_lir::validate`'s token
/// check); the runtime keys it by the capacity and alignment its allocator gives the
/// request. For every extent through the classes and well onto the large path, at every
/// alignment, the block allocated for a request has C's key, and a token reset from it is
/// reused by the same request, which faults on any other key.
#[test]
fn the_compiled_token_key_of_every_request_is_the_runtimes() {
    use hirsel_lir::runtime_abi::{SizeClass, size_class};
    let ctx = Ctx::diagnostic();
    let empty = leaf(0);
    let large = [
        SMALL_MAX * 4,
        (1 << 16) - 1,
        1 << 16,
        (1 << 16) + 1,
        (1 << 20) + 7,
        3 << 20,
    ];
    for extent in (0..=2 * SMALL_MAX + 32).chain(large) {
        for align in [1u64, 2, 4, 8, 16, 32, 64] {
            let key = match size_class(extent, align) {
                SizeClass::Small { capacity, .. } | SizeClass::Large { capacity } => {
                    (capacity, align.max(8))
                }
            };
            let block = alloc(&ctx, empty, extent, align);
            assert_eq!(
                (capacity(block), unsafe { (*block).align() }),
                key,
                "{extent} {align}"
            );
            let mut token = ptr::null_mut();
            let mut reused = ptr::null_mut();
            unsafe {
                assert_eq!(
                    hirsel_rt_v2_reset(
                        ctx.0,
                        block,
                        index(ctx.0, empty),
                        ptr::null(),
                        0,
                        0,
                        &mut token
                    ),
                    0
                );
                assert!(!token.is_null(), "{extent} {align}");
                assert_eq!(
                    hirsel_rt_v2_reuse(
                        ctx.0,
                        token,
                        index(ctx.0, empty),
                        extent,
                        align,
                        &mut reused
                    ),
                    0
                );
            }
            assert_eq!(reused, block, "{extent} {align}");
            release(&ctx, reused, empty);
        }
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}

/// The compiler bounds a reservation by the extent the runtime admits.
#[test]
fn the_compiled_extent_bound_is_the_runtimes() {
    assert_eq!(
        hirsel_lir::runtime_abi::EXTENT_MAX,
        hirsel_rt::alloc::EXTENT_MAX
    );
}

/// The runtime's first-element offset, capacity offset and whole-sequence slice start are the
/// layout's own: past the u64 count and capacity, aligned to the element, for every element
/// alignment.
#[test]
fn sequence_elements_start_where_the_layout_places_them() {
    use hirsel_layout::layout::{LayoutBuilder, Owners};

    use hirsel_lir::layout_types::TargetSpec;
    use hirsel_types::Primitive;
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut ty = support::schema::ty();
    let byte = ty.p(Primitive::U8);
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let id = builder.layout(&ty.store, byte).unwrap();
    let mut element = builder.finish().layout(id).unwrap().clone();
    for align in (0..=6).map(|shift| 1u32 << shift) {
        element.align = align;
        assert_eq!(
            hirsel_rt::alloc::sequence_elements_offset(u64::from(align)),
            hirsel_lir::layout::sequence_elements_offset(&element).unwrap(),
            "element alignment {align}"
        );
        assert_eq!(
            hirsel_rt::alloc::sequence_slice_start(u64::from(align)),
            hirsel_lir::layout::sequence_slice_start(&element).unwrap(),
            "element alignment {align}"
        );
    }
    assert_eq!(
        hirsel_rt::alloc::SEQUENCE_CAPACITY_OFFSET,
        hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET
    );
}

#[test]
#[cfg(not(feature = "system-allocator"))]
fn exit_collects_remote_returns_but_still_reports_live_owners() {
    use hirsel_rt::context::{hirsel_rt_v2_worker_create, hirsel_rt_v2_worker_retire};

    let layout = leaf(0);
    for extent in [16, 8192] {
        for owner_is_home in [true, false] {
            for leak in [false, true] {
                let root = Ctx::diagnostic();
                let descriptor = index(root.0, layout);
                // SAFETY: these contexts share one run and are used serially;
                // the retiring worker owns no running task or release work.
                unsafe {
                    let other = hirsel_rt_v2_worker_create(root.0);
                    let owner = if owner_is_home { root.0 } else { other };
                    let block = hirsel_rt_v2_alloc(owner, descriptor, extent, 8, DOMAIN_HEAP, 0);
                    if leak {
                        let _live =
                            hirsel_rt_v2_alloc(owner, descriptor, extent, 8, DOMAIN_HEAP, 0);
                    }
                    if owner_is_home {
                        assert_eq!(
                            hirsel_rt_v2_release(other, block, descriptor, ptr::null(), 0, 0),
                            0
                        );
                        hirsel_rt_v2_worker_retire(other);
                    } else {
                        hirsel_rt_v2_worker_retire(other);
                        assert_eq!(
                            hirsel_rt_v2_release(root.0, block, descriptor, ptr::null(), 0, 0),
                            0
                        );
                    }
                    let mut audit = Audit::default();
                    hirsel_rt_v2_task_exit(root.0, &mut audit);
                    assert_eq!(
                        audit.live_blocks,
                        u64::from(leak),
                        "extent={extent} owner_is_home={owner_is_home} leak={leak}"
                    );
                }
            }
        }
    }
}
