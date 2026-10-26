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
