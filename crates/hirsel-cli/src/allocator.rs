//! The binary's global allocator: mimalloc, which keeps no live-byte statistics
//! in a release build, so `--mem-usage` counts them here. Counting is off until
//! `count_live` turns it on; until then each call costs one relaxed load.
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering::Relaxed};

pub struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicI64 = AtomicI64::new(0);

fn count(bytes: i64) {
    if COUNTING.load(Relaxed) {
        LIVE.fetch_add(bytes, Relaxed);
    }
}

/// Count from now on the bytes allocations request and frees return; the
/// returned meter reads their net total. A block allocated before counting
/// started subtracts when it is freed, so only differences between two
/// readings mean anything, which is how `hirsel_intern::memory` uses them.
pub fn count_live() -> fn() -> i64 {
    COUNTING.store(true, Relaxed);
    || LIVE.load(Relaxed)
}

// SAFETY: every call forwards to mimalloc with its own arguments; counting
// only reads the layout.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let block = unsafe { mimalloc::MiMalloc.alloc(layout) };
        if !block.is_null() {
            count(layout.size() as i64);
        }
        block
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let block = unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) };
        if !block.is_null() {
            count(layout.size() as i64);
        }
        block
    }
    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        unsafe { mimalloc::MiMalloc.dealloc(block, layout) };
        count(-(layout.size() as i64));
    }
    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { mimalloc::MiMalloc.realloc(block, layout, size) };
        if !moved.is_null() {
            count(size as i64 - layout.size() as i64);
        }
        moved
    }
}
