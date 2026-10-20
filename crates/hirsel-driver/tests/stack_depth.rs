//! Measure live debug stacks at allocation sites in each real recursive phase.
//! It keeps its own test binary: its global allocator observes every allocation.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {
    static BASE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}
fn observe() {
    let marker = 0u8;
    let address = &marker as *const u8 as usize;
    let _ = BASE.try_with(|base| {
        if base.get() != 0 {
            let _ = PEAK.try_with(|peak| peak.set(peak.get().max(base.get().abs_diff(address))));
        }
    });
}
struct MeteredAllocator;
// Delegates unchanged allocation semantics to System; only stack addresses are observed.
unsafe impl GlobalAlloc for MeteredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        observe();
        unsafe { System.realloc(ptr, layout, size) }
    }
}
hirsel_guard::install!(MeteredAllocator);
fn measured<T>(phase: &str, work: impl FnOnce() -> T) -> T {
    let marker = 0u8;
    BASE.set(&marker as *const u8 as usize);
    PEAK.set(0);
    let result = work();
    BASE.set(0);
    let bytes = PEAK.get();
    eprintln!("{phase}: {bytes} measured stack bytes");
    assert!(
        bytes * 4 <= COMPILER_STACK_BYTES,
        "{phase}: {bytes} bytes needs a larger fourfold stack margin"
    );
    result
}
#[test]
fn deepest_source_phases_fit_with_fourfold_stack_margin() {
    with_compiler_stack(|| {
        let n = hirsel_parser::MAX_PARSE_DEPTH - 2;
        let source = format!(
            "fn f(a: u8) -> u8 {{ {}a{} }}",
            "{".repeat(n),
            "}".repeat(n)
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("stack.hir", source).unwrap();
        let parsed = measured("parser", || {
            engine.parse_file(file, &default_context()).unwrap()
        });
        let path = parsed.value.items[0].item.definition.clone();
        measured("printer", || {
            format_snapshot(&mut engine, file, &default_context()).unwrap()
        });
        measured("resolver", || engine.resolve_definition(&path).unwrap());
        measured("checker", || engine.check_definition(&path).unwrap());
        measured("modes", || engine.mode_definition(&path).unwrap());
        // Fail only at the deepest leaf. This measures the full recursive lower
        // stack without making this safety test a large-body scaling benchmark.
        // The scaling tests separately exercise successful 1,024-level lowering.
        let mut analyzed = engine.mode_definition(&path).unwrap().value;
        let signature = engine.item_signature(&path).unwrap().value;
        analyzed
            .typed
            .checked
            .expressions
            .retain(|row| row.expression != hirsel_syntax::ExprId(0));
        let comptime = engine.prepare_comptime(&analyzed, &signature).unwrap();
        let error = measured("lowering", || {
            LowerAdapter
                .lower(
                    engine.types(),
                    engine.type_memo(),
                    &analyzed,
                    &comptime,
                    &signature,
                )
                .unwrap_err()
        });
        assert!(
            matches!(error, QueryError::Operational(ref failure) if failure.message.contains("missing expression decision"))
        );
    });
}
