//! Test-only System allocation meter, ported from proof-export admission tests.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
#[derive(Clone, Copy, Default)]
pub(crate) struct Count {
    pub(crate) live: isize,
    pub(crate) peak: isize,
    pub(crate) allocations: usize,
}
thread_local! {
    static COUNT: Cell<Option<Count>> = const { Cell::new(None) };
}
fn count(added: usize, removed: usize) {
    let _ = COUNT.try_with(|slot| {
        if let Some(mut count) = slot.get() {
            count.live += added as isize - removed as isize;
            count.peak = count.peak.max(count.live);
            count.allocations += usize::from(added != 0);
            slot.set(Some(count));
        }
    });
}
// Same System delegation as driver/tests/validate_memory.rs; thread-local
// counters exclude concurrent test harness work from this control's peak.
pub(crate) struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            count(layout.size(), 0);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            count(layout.size(), 0);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        count(0, layout.size());
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, size) };
        if !moved.is_null() {
            count(size, layout.size());
        }
        moved
    }
}
pub(crate) fn measured<T>(run: impl FnOnce() -> T) -> (T, Count) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            COUNT.set(None);
        }
    }
    COUNT.set(Some(Count::default()));
    let reset = Reset;
    let result = run();
    let count = COUNT.get().unwrap();
    drop(reset);
    (result, count)
}
/// Nest one metadata inspection while preserving its outer physical live count.
pub(crate) fn measured_independent<T>(run: impl FnOnce() -> T) -> (T, Count) {
    let previous = COUNT.with(|slot| slot.replace(Some(Count::default())));
    let result = run();
    let count = COUNT.with(|slot| slot.replace(previous).unwrap());
    (result, count)
}
