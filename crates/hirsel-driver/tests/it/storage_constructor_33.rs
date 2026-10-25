//! Constructor names are declarations, while constructed values have actual type arguments.
use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    CheckCompletion, QueryEngine, check_progress, default_context, default_environment,
    with_compiler_stack,
};

#[test]
fn generic_constructor_heads_do_not_enter_value_path_domains() {
    for (binding, placement) in [("let root", "a"), ("let root: Wrap[List[u64]]", "heap")] {
        with_compiler_stack(|| {
            let source = format!(
                "type Wrap[T] {{ value: T }}\n\
                 pub fn main() {{\n\
                   let mut sealed = arena.with(fn(a: Arena) -> Region[Wrap[List[u64]]] {{\n\
                     let xs: List[u64] in {placement} = [7u64]\n\
                     {binding} = Wrap(value: take xs)\n\
                     arena.seal(take a, take root)\n\
                   }})\n\
                   let score = region.with(mut sealed, fn(root: Wrap[List[u64]]) -> u64 {{\n\
                     root.value[0]\n\
                   }})\n\
                 }}\n"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("storage-constructor.hir", source)
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(checked.completion, CheckCompletion::Complete);
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let input = engine.export_native_input().unwrap();
            assert!(!input.storage_profiles.is_empty());
            let validated = engine.import_native_input(input).unwrap();
            assert!(!validated.storage_proof_reads().bodies.is_empty());
        });
    }
}
