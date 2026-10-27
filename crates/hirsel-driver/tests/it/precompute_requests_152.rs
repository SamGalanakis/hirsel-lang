use hirsel_driver::{COMPILER, CompilerRequestsExt, requests::*};
use hirsel_engine::{CompileConfig, QueryError, ToolError, store::DirStore, with_compiler_stack};
use std::sync::Arc;

#[test]
fn precompute_libraries_on_fresh_request_graphs_preserves_complete_source_products() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        let source = directory.path().join("library.hir");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"precompute_library","version":"1"},"sources":[{"path":"library.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let valid = "pub fn square(n:u64)->u64{n*n}\nconst ANSWER:u64=comptime square(7u64)\npub fn answer()->u64{ANSWER}\n";
        std::fs::write(&source, valid).unwrap();
        let config = CompileConfig {
            store: Arc::new(DirStore::new(directory.path().join("products"))),
            jobs: hirsel_intern::jobs::Jobs::ONE,
            hold_bytes: 0,
            ..Default::default()
        };
        let mut expected = None;
        for _ in 0..2 {
            let mut requests =
                CompilerRequests::for_host(COMPILER.with_config(config.clone()), config.clone());
            let output = requests
                .execute(&manifest, CompileOperation::Precompute)
                .unwrap();
            let CompileResult::Precomputed {
                definitions,
                digest,
                sources,
            } = &output.result
            else {
                panic!("expected precomputed source products");
            };
            assert_eq!(
                *definitions, 3,
                "every definition of this fixture, without a launch entry"
            );
            let names = sources
                .definitions()
                .map(|path| path.0.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, ["ANSWER", "answer", "square"]);
            let answer = sources
                .definitions()
                .find(|path| path.0.name == "ANSWER")
                .unwrap();
            let bound = sources.source_product_bytes(answer).unwrap().unwrap();
            let budget = hirsel_engine::runtime::ProcessBudget::new(bound);
            let products = sources
                .source_products(answer, budget.reserve(bound).unwrap())
                .unwrap()
                .unwrap();
            assert!(matches!(
                products.constant().unwrap().root.kind,
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(49))
            ));
            let node = sources.semantic_node(answer).unwrap();
            assert_eq!(node.identity, answer.to_string());
            if let Some(expected) = expected {
                assert_eq!(*digest, expected);
            } else {
                expected = Some(*digest);
            }
        }
        std::fs::write(&source, format!("{valid}pub fn unrelated()->u64{{true}}\n")).unwrap();
        let mut requests = CompilerRequests::for_host(COMPILER.with_config(config.clone()), config);
        let Err(ToolError::Query {
            error: QueryError::Diagnostics(diagnostics),
            sources,
            ..
        }) = requests.execute(&manifest, CompileOperation::Precompute)
        else {
            panic!("unselected invalid source must reject precompute");
        };
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| sources.slice(diagnostic.primary) == Some("true"))
        );
    });
}

#[test]
fn precompute_rejects_disabled_verification_before_package_loading() {
    let directory = tempfile::tempdir().unwrap();
    let error = COMPILER
        .with_verify(hirsel_engine::Verify::Off)
        .precompute_package(&directory.path().join("absent.json"), directory.path());
    assert!(
        matches!(error, Err(ToolError::Runtime(message)) if message == "package precomputation requires V")
    );
}
