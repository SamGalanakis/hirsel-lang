use hirsel_driver::{COMPILER, requests::*};
use hirsel_engine::{
    CompileConfig, LoweringPolicy, QueryKind, runtime::ProcessBudget, store::DirStore,
    with_compiler_stack,
};
use std::sync::Arc;

#[test]
fn settled_precompute_graph_preserves_scalar_revisions_and_detached_ownership() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("library.json");
        std::fs::write(dir.path().join("library.hir"),
            "const VALUE:u64=comptime 42u64\npub fn leaf()->u64{VALUE}\npub fn middle()->u64{leaf()}\npub fn orphan()->u64{9u64}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"graph","version":"1"},"sources":[{"path":"library.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let mut scalar = COMPILER
            .with_owned_engines()
            .load_check_package(&manifest)
            .unwrap();
        let checked = scalar.check(LoweringPolicy::Reachable).unwrap();
        assert!(checked.diagnostics.is_empty());
        let expected = scalar
            .engine
            .indexed_definitions()
            .into_iter()
            .map(|path| {
                let revision = scalar.engine.semantic_program_revision(&path).unwrap();
                (path, revision)
            })
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), 4);
        drop(scalar);
        let config = CompileConfig {
            store: Arc::new(DirStore::new(dir.path().join("products"))),
            jobs: hirsel_intern::jobs::Jobs::ONE,
            hold_bytes: 0,
            ..Default::default()
        };
        for restored in [false, true] {
            let mut requests =
                CompilerRequests::for_host(COMPILER.with_config(config.clone()), config.clone());
            let output = requests
                .execute(&manifest, CompileOperation::Precompute)
                .unwrap();
            let CompileResult::Precomputed {
                definitions,
                sources,
                ..
            } = &output.result
            else {
                panic!("expected precomputed sources")
            };
            assert_eq!(
                *definitions, 4,
                "core semantic products are not source declarations"
            );
            let queries = expected
                .iter()
                .flat_map(|(path, _)| {
                    [QueryKind::Check, QueryKind::Modes, QueryKind::Lower]
                        .map(|kind| (path.clone(), kind))
                })
                .collect::<Vec<_>>();
            let before = requests.recompute_counts(&queries).unwrap();
            let bound = sources.semantic_graph_decode_bytes().unwrap();
            assert!(bound > 0);
            let budget = ProcessBudget::new(bound);
            assert!(
                sources
                    .semantic_graph(budget.reserve(bound - 1).unwrap())
                    .is_err()
            );
            assert_eq!(budget.reserved(), 0);
            let graph = sources
                .semantic_graph(budget.reserve(bound).unwrap())
                .unwrap();
            assert_eq!(requests.recompute_counts(&queries).unwrap(), before);
            if restored {
                for (path, kind, count) in before {
                    if matches!(kind, QueryKind::Check | QueryKind::Modes) {
                        assert_eq!(count, 0, "{path}: {kind:?}");
                    }
                }
            }
            drop(output);
            drop(requests);
            assert_eq!(budget.reserved(), bound);
            for _ in 0..2 {
                for (path, revision) in &expected {
                    assert_eq!(graph.revision(path).unwrap(), *revision, "{path}");
                }
            }
            let mut absent = expected[0].0.clone();
            absent.0.name = "absent".into();
            assert!(graph.revision(&absent).is_err());
            drop(graph);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
