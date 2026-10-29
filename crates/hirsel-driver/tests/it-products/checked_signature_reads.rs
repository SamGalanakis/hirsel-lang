use hirsel_driver::{COMPILER, requests::*};
use hirsel_engine::{CompileConfig, with_compiler_stack};
use std::sync::Arc;

#[test]
fn checked_dependency_readers_preserve_the_complete_generated_signature_sequence() {
    use hirsel_engine::{DefinitionQueries, runtime::ProcessBudget};
    with_compiler_stack(|| {
        let source = "type Seed {x:u32}\nfn body_text()->String{\"fn()->u32 uses {}{11u32}\"}\nfn make(s:TypeShape[Seed])->Result[Body[fn()->u32 uses {}],String]{Ok(codegen.body(body_text()))}\nfn chosen()->u32 uses {}=comptime make(shape(Seed))\nconst A:u32=comptime chosen()\nconst B:u32=comptime chosen()\n";
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("comptime.hir"), source).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"comptime.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let mut control = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = control.sources.add("comptime.hir", source.into()).unwrap();
        control
            .parse_file(file, &hirsel_engine::default_context())
            .unwrap();
        let mut path = hirsel_engine::entry_path(&hirsel_engine::default_context());
        for name in ["B", "A", "B"] {
            path.0.name = name.into();
            let result = control.materialize_value(&path);
            assert!(result.is_ok(), "{path}: {result:?}; phase={:?}", control.last_phase);
        }
        path.0.name = "chosen".into();
        let expected = control
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
                    .unwrap_or_else(|error| panic!("jobs={jobs} warm={warm}: {error:?}"));
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
