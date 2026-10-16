use hirsel_driver::*;
use hirsel_engine::*;

fn source(depth: usize) -> String {
    let mut source = "type R0 { x:u8 }\n".to_string();
    for i in 1..depth {
        source += &format!("type R{i} {{ x:R{} }}\n", i - 1);
    }
    source += &format!("fn f(a:R{})->u8 {{ a{} }}", depth - 1, ".x".repeat(depth));
    source
}

#[test]
fn signature_queries_visit_shared_dependency_nodes_once() {
    with_compiler_stack(|| {
        for depth in [4, 8, 16, 32] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("dependencies.hir", source(depth))
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            assert!(parsed.diagnostics.is_empty());
            let path =
                &parsed.value.tree.items[parsed.value.roots.last().unwrap().0 as usize].definition;
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::CacheValidationRows);
            let signature = engine.item_signature(path).unwrap();
            assert!(signature.diagnostics.is_empty());
            let inspected =
                hirsel_intern::work::read(hirsel_intern::work::Counter::CacheValidationRows)
                    - before;
            assert!(
                inspected <= 8 * (depth as u64 + 1),
                "{depth} nominal levels inspected {inspected} cached dependency nodes"
            );
        }
    });
}

#[test]
fn changing_a_transitive_nominal_dependency_matches_a_fresh_session() {
    with_compiler_stack(|| {
        let original = source(8);
        let edited = original.replace("x:u8", "x:u16");
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let mut fingerprints = vec![];
        for source in [&original, &edited, &original] {
            let file = engine
                .sources
                .add("dependencies.hir", source.clone())
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let path =
                &parsed.value.tree.items[parsed.value.roots.last().unwrap().0 as usize].definition;
            let actual = engine.item_signature(path).unwrap();
            let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
            let file = fresh
                .sources
                .add("dependencies.hir", source.clone())
                .unwrap();
            fresh.parse_file(file, &default_context()).unwrap();
            let expected = fresh.item_signature(path).unwrap();
            assert_eq!(actual.value.fingerprint, expected.value.fingerprint);
            fingerprints.push(actual.value.fingerprint);
        }
        assert_ne!(fingerprints[0], fingerprints[1]);
        assert_eq!(fingerprints[0], fingerprints[2]);
    });
}
