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

#[test]
fn precomputed_dependency_readers_preserve_order_admission_and_owner_lifetime() {
    use hirsel_engine::{QueryKind, runtime::ProcessBudget};
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            directory.path().join("library.hir"),
            "pub fn twice(n:u64)->u64{n+n}\npub fn answer()->u64{twice(21u64)}\n",
        )
        .unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"dependency_reader","version":"1"},"sources":[{"path":"library.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let config = CompileConfig {
            store: Arc::new(DirStore::new(directory.path().join("products"))),
            jobs: hirsel_intern::jobs::Jobs::ONE,
            hold_bytes: 0,
            ..Default::default()
        };
        let cold = {
            let mut requests =
                CompilerRequests::for_host(COMPILER.with_config(config.clone()), config.clone());
            let output = requests
                .execute(&manifest, CompileOperation::Precompute)
                .unwrap();
            let CompileResult::Precomputed { sources, .. } = &output.result else {
                panic!("expected cold source products")
            };
            let path = sources
                .definitions()
                .find(|path| path.0.name == "answer")
                .unwrap()
                .clone();
            let bound = sources.dependency_decode_bytes(&path).unwrap();
            let budget = ProcessBudget::new(bound);
            let dependencies = sources
                .dependencies(&path, budget.reserve(bound).unwrap())
                .unwrap();
            (path, dependencies.dependencies().to_vec())
        };
        let (path, expected) = cold;
        assert!(!expected.is_empty());
        assert!(
            expected.windows(2).any(|rows| rows[0] == rows[1]),
            "repeated ordered reads are part of the oracle"
        );
        let mut requests = CompilerRequests::for_host(COMPILER.with_config(config.clone()), config);
        let output = requests
            .execute(&manifest, CompileOperation::Precompute)
            .unwrap();
        let CompileResult::Precomputed { sources, .. } = &output.result else {
            panic!("expected restored source products")
        };
        let sources = sources.clone();
        let queries = [QueryKind::Check, QueryKind::Modes, QueryKind::Lower]
            .map(|kind| (path.clone(), kind))
            .to_vec();
        let inspect = |requests: &mut CompilerRequests| {
            let output = requests
                .execute(
                    &manifest,
                    CompileOperation::RecomputeCounts {
                        queries: queries.clone(),
                    },
                )
                .unwrap();
            let CompileResult::RecomputeCounts(counts) = &output.result else {
                panic!("expected work counters")
            };
            counts.clone()
        };
        let before = inspect(&mut requests);
        let bound = sources.dependency_decode_bytes(&path).unwrap();
        let budget = ProcessBudget::new(bound);
        assert!(matches!(
            sources.dependencies(&path, budget.reserve(bound - 1).unwrap()),
            Err(QueryError::Operational(_))
        ));
        let held = sources
            .dependencies(&path, budget.reserve(bound).unwrap())
            .unwrap();
        assert_eq!(held.dependencies(), expected);
        assert_eq!(
            inspect(&mut requests),
            before,
            "reading must not run a compiler query"
        );
        drop(output);
        drop(requests);
        drop(sources);
        assert_eq!(
            held.dependencies(),
            expected,
            "the read owns its source and admission"
        );
        assert!(budget.reserve(bound).is_err());
        drop(held);
        assert!(budget.reserve(bound).is_ok());
    });
}

#[test]
fn checked_dependency_readers_preserve_the_complete_generated_signature_sequence() {
    use hirsel_engine::runtime::ProcessBudget;
    with_compiler_stack(|| {
        let source = "type Seed {x:u32}\nfn body_text()->String{\"fn()->u32 uses {}{11u32}\"}\nfn make(s:TypeShape[Seed])->Result[Body[fn()->u32 uses {}],String]{Ok(codegen.body(body_text()))}\nfn chosen()->u32 uses {}=comptime make(shape(Seed))\nconst A:u32=comptime chosen()\nconst B:u32=comptime chosen()\n";
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("comptime.hir"), source).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"comptime.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let mut control = COMPILER
            .with_owned_engines()
            .load_check_package(&manifest)
            .unwrap();
        let mut path = control.entry.clone();
        for name in ["B", "A", "B"] {
            path.0.name = name.into();
            control.engine.materialize_value(&path).unwrap();
        }
        path.0.name = "chosen".into();
        let expected = control
            .engine
            .check_definition(&path)
            .unwrap()
            .dependencies
            .clone();
        assert!(
            expected
                .iter()
                .any(|read| read.definition.path().0.name == "make")
        );
        assert!(
            expected
                .iter()
                .enumerate()
                .any(|(i, read)| expected[..i].contains(read)),
            "original repeated signature reads are present"
        );
        for jobs in [1, 2, 8] {
            let config = CompileConfig {
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                store: Arc::new(hirsel_engine::store::MemStore::default()),
                hold_bytes: 0,
                ..Default::default()
            };
            for warm in [false, true] {
                let mut requests = CompilerRequests::for_host(
                    COMPILER.with_config(config.clone()),
                    config.clone(),
                );
                let output = requests
                    .execute(&manifest, CompileOperation::Precompute)
                    .unwrap_or_else(|error| panic!("jobs={jobs} warm={warm}: {error}"));
                let CompileResult::Precomputed { sources, .. } = &output.result else {
                    panic!("expected source products");
                };
                let sources = sources.clone();
                drop(output);
                drop(requests);
                let bytes = sources.checked_dependency_decode_bytes(&path).unwrap();
                let budget = ProcessBudget::new(bytes);
                assert!(
                    sources
                        .checked_dependencies(&path, budget.reserve(bytes - 1).unwrap())
                        .is_err()
                );
                assert_eq!(budget.reserved(), 0);
                let reads = sources
                    .checked_dependencies(&path, budget.reserve(bytes).unwrap())
                    .unwrap();
                drop(sources);
                assert_eq!(
                    reads.dependencies(),
                    expected.as_slice(),
                    "jobs={jobs} warm={warm}"
                );
                assert_eq!(budget.reserved(), bytes);
                drop(reads);
                assert_eq!(budget.reserved(), 0);
            }
        }
    });
}
