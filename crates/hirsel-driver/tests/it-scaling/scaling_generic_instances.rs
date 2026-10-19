use hirsel_driver::*;
use hirsel_engine::*;
/// A body selecting many flat instances of one size-generic declaration once
/// expanded each argument's crossing receipts into a clone of the body's
/// store, so the first expansion copied every body row: quadratic in the
/// number of calls. Expansions now go to proof-local scratch.
#[test]
fn generic_instance_calls_copy_no_body_rows_per_call() {
    with_compiler_stack(|| {
        let work = |count: usize| {
            let calls = (0..count)
                .map(|k| format!("let t{k}: Tag[{k}] = Tag(a: {k})\nlet _ = describe(t{k})\n"))
                .collect::<String>();
            let source = format!(
                "type Tag[n: size] {{ a: u64 }}\nfn describe[T](v: T) -> u64 {{ let _ = v\n1 }}\nfn f() -> u64 {{ {calls}0 }}\n"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("generic-instances.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows);
            let output = engine
                .mode_definition(&parsed.value.items[2].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows) - before
        };
        let (half, full) = (work(64), work(128));
        assert!(
            full <= 3 * half + 1024,
            "{half} copied type rows for 64 calls, {full} for 128"
        );
    });
}
