//! V's flow state scales with live facts, not with snapshot copies: the two
//! largest std bodies validate within a fixed heap budget.
//! It keeps its own test binary: its global allocator meters every allocation.
#[path = "support/image_fixture.rs"]
mod image_fixture;

use hirsel_engine::*;
use image_fixture::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicIsize, Ordering::Relaxed},
};

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
fn count(bytes: isize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}
struct MeteredAllocator;
// Delegates unchanged allocation semantics to System; only sizes are counted.
unsafe impl GlobalAlloc for MeteredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let block = unsafe { System.alloc(layout) };
        if !block.is_null() {
            count(layout.size() as isize);
        }
        block
    }
    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        unsafe { System.dealloc(block, layout) };
        count(-(layout.size() as isize));
    }
    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(block, layout, size) };
        if !moved.is_null() {
            count(size as isize - layout.size() as isize);
        }
        moved
    }
}
hirsel_guard::install!(MeteredAllocator);

/// Before shared rows, these bodies held 760 MiB and 590 MiB of flow state.
const BUDGET: isize = 64 << 20;

#[test]
fn largest_std_bodies_validate_within_a_fixed_heap_budget() {
    let f = Fixture::new();
    let image = std_image();
    let program = f.program(
        "aaa",
        "pub fn main() -> () uses {} {
  let html = markdown.to_html(\"# a *b*\", markdown.default_options())
  let time = time.parse_with(\"2024-01-02\", \"%Y-%m-%d\")
  ()
}
",
        &std_manifest(),
        "std",
    );
    with_compiler_stack(|| {
        let mut p = load(&program, &StdImages::of(vec![image]));
        assert!(p.engine.imaged(&package("std")), "{:?}", p.image_notes);
        let input = p.engine.export_native_input().unwrap();
        // The signature input V checks std's own bodies against.
        let snapshot = p.engine.signature_snapshot();
        let signatures = snapshot.validation(&package("std"));
        for name in ["std@1/markdown/parse_inlines", "std@1/time/parse_with"] {
            let definition = input
                .definitions
                .iter()
                .find_map(|(path, definition)| (path.to_string() == name).then_some(definition))
                .unwrap_or_else(|| panic!("{name} is not in the program's closure"));
            let base = LIVE.load(Relaxed);
            PEAK.store(base, Relaxed);
            let validation =
                hirsel_validate::validate(definition, &input.types, signatures, &mut vec![]);
            let peak = PEAK.load(Relaxed) - base;
            assert!(
                validation.violations.is_empty(),
                "{name}: {:?}",
                validation.violations
            );
            eprintln!("{name}: V peak {} KiB", peak >> 10);
            assert!(peak <= BUDGET, "{name}: V peak {} MiB", peak >> 20);
        }
    });
}
