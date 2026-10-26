use hirsel_driver::COMPILER;
use hirsel_engine::{store::DirStore, *};
use std::sync::Arc;

#[test]
fn repeated_execution_check_reuses_every_lower_product_including_unused_roots() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"),
            "fn child()->u64{7u64}\nfn unused()->u64{9u64}\npub fn main()->Result[(),()]{let _=child();Ok(())}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"warm","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            let compiler = COMPILER.clone().with_config(CompileConfig {
                store: Arc::new(DirStore::new(
                    directory.path().join(format!("products-{jobs}")),
                )),
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                hold_bytes: 0,
                ..CompileConfig::for_memory_cap(None)
            });
            let mut first = compiler.load_program(&manifest).unwrap();
            first.check_for_execution().unwrap();
            let paths = first.engine.front_source_roots();
            assert_eq!(paths.len(), 3);
            assert!(
                paths
                    .iter()
                    .any(|path| first.engine.lower_adapter_calls(path) > 0)
            );
            drop(first);
            let mut warm = compiler.load_program(&manifest).unwrap();
            warm.check_for_execution().unwrap();
            for path in &paths {
                assert_eq!(
                    warm.engine.lower_adapter_calls(path),
                    0,
                    "{path}, jobs={jobs}: unchanged execution check must reuse its original Lower product"
                );
            }
        }
    });
}

#[test]
fn generated_function_and_its_runtime_caller_reuse_their_ordinary_lower_products() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"),
            "fn generator()->Result[Body[fn()->u32 uses {}],String]{Ok(codegen.body(\"fn()->u32 uses {} { 7u32 }\"))}\npub fn generated()->u32 uses {}=comptime generator()\npub fn main()->u32{generated()}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"warm_generated","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let compiler = COMPILER.clone().with_config(CompileConfig {
            store: Arc::new(DirStore::new(directory.path().join("products"))),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            hold_bytes: 0,
            ..CompileConfig::for_memory_cap(None)
        });
        for warm in [false, true] {
            let mut program = compiler.load_program(&manifest).unwrap();
            program.check_for_execution().unwrap();
            let entry = program.entry.clone();
            let mut generated = entry.clone();
            generated.0.name = "generated".into();
            let value = program.engine.lower_to_tir(&generated).unwrap();
            assert_eq!(value.value.path.path(), &generated);
            program.engine.lower_to_tir(&entry).unwrap();
            if warm {
                for path in [&entry, &generated] {
                    assert_eq!(
                        program.engine.lower_adapter_calls(path),
                        0,
                        "{path}: unchanged generated body must reopen after accepted generation"
                    );
                }
            }
        }
    });
}

#[test]
fn check_only_constant_roots_reuse_their_ordinary_lower_products() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            directory.path().join("main.hir"),
            "const HELP:String=\"help\"\nconst LIMIT:u64=3u64\npub fn main()->u64{LIMIT}\n",
        )
        .unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"warm_constants","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let compiler = COMPILER.clone().with_config(CompileConfig {
            store: Arc::new(DirStore::new(directory.path().join("products"))),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            hold_bytes: 0,
            ..CompileConfig::for_memory_cap(None)
        });
        let mut first = compiler.load_program(&manifest).unwrap();
        first.check_for_execution().unwrap();
        let roots = first.engine.front_source_roots();
        assert_eq!(roots.len(), 3);
        drop(first);
        let mut warm = compiler.load_program(&manifest).unwrap();
        warm.check_for_execution().unwrap();
        for path in &roots {
            assert_eq!(
                warm.engine.lower_adapter_calls(path),
                0,
                "{path}: check-only root must reopen its ordinary Lower product"
            );
        }
    });
}

#[test]
fn worker_lower_products_match_scalar_records_without_publishing_a_failed_suffix() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"), "const GOOD:u64=3u64\nconst BAD:u64=comptime {let zero=0u64;1u64/zero}\nconst LATER:u64=5u64\npub fn main()->u64{GOOD}\n").unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"warm_prefix","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let compiler = COMPILER.clone().with_config(CompileConfig {
            store: Arc::new(DirStore::new(directory.path().join("products"))),
            jobs: hirsel_intern::jobs::Jobs::new(2),
            hold_bytes: 0,
            ..CompileConfig::for_memory_cap(None)
        });
        let mut seed = compiler.load_program(&manifest).unwrap();
        let named = |name: &str| {
            let mut path = seed.entry.clone();
            path.0.name = name.into();
            path
        };
        let (good, bad, later) = (named("GOOD"), named("BAD"), named("LATER"));
        let outcomes = seed.engine.check_lower_definition_prefix(
            vec![good.clone(), bad, later.clone()],
            hirsel_intern::jobs::Jobs::new(2),
        );
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes[0].result.is_ok());
        assert!(outcomes[1].result.is_err());
        drop(seed);
        let mut warm = compiler.load_program(&manifest).unwrap();
        warm.engine.adapters.lowering = None;
        warm.engine.lower_to_tir(&good).unwrap();
        assert_eq!(warm.engine.lower_adapter_calls(&good), 0);
        assert!(
            warm.engine.lower_to_tir(&later).is_err(),
            "speculative suffix has no published Lower association"
        );
        drop(warm);
        let mut oracle = compiler.load_program(&manifest).unwrap();
        oracle.engine = oracle
            .engine
            .with_verify_store(store::VerifyStore::Recompute);
        oracle.engine.lower_to_tir(&good).unwrap();
        assert_eq!(
            oracle.engine.lower_adapter_calls(&good),
            1,
            "canonical oracle actually recomputes through scalar Lower"
        );
    });
}
