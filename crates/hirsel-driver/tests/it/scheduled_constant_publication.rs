use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;
#[test]
fn ordinary_and_opaque_constants_publish_original_lower_and_evaluate_products() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&path, "pub opaque type ExitCode: plain + inert + noalloc_drop uses {} { code:u8, }\npub const SUCCESS:ExitCode=ExitCode(code:0u8)\nconst N:u8=7u8\npub fn main()->u8{N}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"constants","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let definitions = program.engine.front_source_roots();
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let weight = MemoryWeight {
                retained: 2 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors,
            )
            .unwrap();
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    weight,
                    32,
                    budget.reserve(32 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            for name in ["N", "SUCCESS"] {
                let mut path = program.entry.clone();
                path.0.name = name.into();
                let request = runtime.request(&[lower.query(&path).unwrap()]);
                let products = runtime
                    .wait(request)
                    .unwrap_or_else(|r| panic!("Lower {path}: {:?}", lower.failure(r)));
                let actual = lower.outcome((products[0]).clone()).unwrap();
                let expected = program.engine.definition(&path).unwrap();
                let records = NativeRecords::new(program.engine.types().clone(), store.clone());
                assert_eq!(
                    records
                        .encode(&program.engine, &actual.output.value)
                        .unwrap(),
                    records.encode(&program.engine, &expected).unwrap(),
                    "original Lower {path}, jobs={jobs}"
                );
                let scalar = program.engine.materialize_value(&path).unwrap();
                let actual = lower.settled_constant(&path, &[]).unwrap();
                assert_eq!(
                    records.encode(&program.engine, &*actual).unwrap(),
                    records.encode(&program.engine, &scalar).unwrap(),
                    "Evaluate {path}, jobs={jobs}"
                );
            }
            runtime.shutdown();
        }
    });
}

#[test]
fn std_opaque_constant_publishes_original_lower_and_evaluate_products() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main(){}\n").unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let definitions = program
                .engine
                .front_source_roots()
                .into_iter()
                .filter(|p| {
                    p.0.package.name == "std" && p.0.modules == ["process"] && p.0.name == "SUCCESS"
                })
                .collect::<Vec<_>>();
            assert_eq!(definitions.len(), 1);
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let weight = MemoryWeight {
                retained: 2 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors,
            )
            .unwrap();
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    weight,
                    32,
                    budget.reserve(32 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            for path in definitions.clone() {
                let request = runtime.request(&[lower.query(&path).unwrap()]);
                let products = runtime
                    .wait(request)
                    .unwrap_or_else(|r| panic!("Lower {path}: {:?}", lower.failure(r)));
                let actual = lower.outcome((products[0]).clone()).unwrap();
                let expected = program.engine.definition(&path).unwrap();
                let records = NativeRecords::new(program.engine.types().clone(), store.clone());
                assert_eq!(
                    records
                        .encode(&program.engine, &actual.output.value)
                        .unwrap(),
                    records.encode(&program.engine, &expected).unwrap(),
                    "original Lower {path}, jobs={jobs}"
                );
                let scalar = program.engine.materialize_value(&path).unwrap();
                let actual = lower.settled_constant(&path, &[]).unwrap();
                assert_eq!(
                    records.encode(&program.engine, &*actual).unwrap(),
                    records.encode(&program.engine, &scalar).unwrap(),
                    "Evaluate {path}, jobs={jobs}"
                );
            }
            runtime.shutdown();
        }
    });
}

#[test]
fn std_constant_publishes_after_full_namespace_capture_and_body_eviction() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main(){}\n").unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let definitions = program.engine.front_source_roots();
            let selected = definitions
                .iter()
                .filter(|p| {
                    p.0.package.name == "std" && p.0.modules == ["process"] && p.0.name == "SUCCESS"
                })
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(selected.len(), 1);
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let weight = MemoryWeight {
                retained: 2 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors,
            )
            .unwrap();
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    weight,
                    32,
                    budget.reserve(32 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            for path in selected {
                let request = runtime.request(&[lower.query(&path).unwrap()]);
                let products = runtime
                    .wait(request)
                    .unwrap_or_else(|r| panic!("Lower {path}: {:?}", lower.failure(r)));
                let actual = lower.outcome((products[0]).clone()).unwrap();
                let expected = program.engine.definition(&path).unwrap();
                let records = NativeRecords::new(program.engine.types().clone(), store.clone());
                assert_eq!(
                    records
                        .encode(&program.engine, &actual.output.value)
                        .unwrap(),
                    records.encode(&program.engine, &expected).unwrap(),
                    "original Lower {path}, jobs={jobs}"
                );
                let scalar = program.engine.materialize_value(&path).unwrap();
                let actual = lower.settled_constant(&path, &[]).unwrap();
                assert_eq!(
                    records.encode(&program.engine, &*actual).unwrap(),
                    records.encode(&program.engine, &scalar).unwrap(),
                    "Evaluate {path}, jobs={jobs}"
                );
            }
            runtime.shutdown();
        }
    });
}

#[test]
fn immutable_check_loader_std_constant_keeps_lower_and_evaluate_bindings() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main(){}\n").unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let compiler = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines();
            let capture = hirsel_engine::loader_inputs::CaptureInputs::new(
                hirsel_engine::loader_inputs::InputLimits {
                    bytes: 256 << 20,
                    operations: 100000,
                },
            );
            drop(
                compiler
                    .load_check_package_inputs(&path, &capture, false)
                    .unwrap(),
            );
            let inputs = capture.freeze();
            let mut program = compiler
                .load_check_package_inputs(&path, &inputs, true)
                .unwrap();
            let definitions = program
                .engine
                .indexed_definitions()
                .into_iter()
                .chain(program.engine.front_source_roots())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            program.engine.front_input_footprint(&definitions).unwrap();
            let selected = definitions
                .iter()
                .filter(|p| {
                    p.0.package.name == "std" && p.0.modules == ["process"] && p.0.name == "SUCCESS"
                })
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(selected.len(), 1);
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let weight = MemoryWeight {
                retained: 2 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors,
            )
            .unwrap();
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    weight,
                    32,
                    budget.reserve(32 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            for path in selected {
                let request = runtime.request(&[lower.query(&path).unwrap()]);
                let products = runtime
                    .wait(request)
                    .unwrap_or_else(|r| panic!("Lower {path}: {:?}", lower.failure(r)));
                let actual = lower.outcome((products[0]).clone()).unwrap();
                let expected = program.engine.definition(&path).unwrap();
                let records = NativeRecords::new(program.engine.types().clone(), store.clone());
                assert_eq!(
                    records
                        .encode(&program.engine, &actual.output.value)
                        .unwrap(),
                    records.encode(&program.engine, &expected).unwrap(),
                    "original Lower {path}, jobs={jobs}"
                );
                let scalar = program.engine.materialize_value(&path).unwrap();
                let actual = lower.settled_constant(&path, &[]).unwrap();
                assert_eq!(
                    records.encode(&program.engine, &*actual).unwrap(),
                    records.encode(&program.engine, &scalar).unwrap(),
                    "Evaluate {path}, jobs={jobs}"
                );
            }
            runtime.shutdown();
        }
    });
}
