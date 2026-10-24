use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn admitted_lower_retains_actual_metadata_and_refuses_an_insufficient_output_grant() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            &path,
            "fn left()->u8{1u8}\nfn right()->u8{left()}\npub fn main()->u8{comptime right()}\n",
        )
        .unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"debit","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            for output_bytes in [1, 64 << 10] {
                let mut program = COMPILER
                    .with_config(CompileConfig {
                        store: Arc::new(MemStore::default()),
                        ..Default::default()
                    })
                    .with_owned_engines()
                    .load_program(&manifest)
                    .unwrap();
                let definitions = program.engine.front_source_roots();
                let budget = ProcessBudget::new(1 << 30);
                let errors = crate::scheduled_front::errors();
                let weight = MemoryWeight {
                    retained: 1 << 20,
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
                        budget.reserve(8 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let input_bytes = FrontLowerQueries::input_bytes(&definitions).unwrap();
                let lower = program
                    .engine
                    .front_lower_queries(
                        &definitions,
                        front.clone(),
                        weight,
                        3 * definitions.len(),
                        budget.reserve(input_bytes + output_bytes).unwrap(),
                        errors,
                    )
                    .unwrap();
                assert_eq!(lower.retained_bytes(), input_bytes);
                runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                runtime
                    .register_local(FRONT_LOWER_KIND, lower.factory())
                    .unwrap();
                runtime
                    .register_local(FRONT_EVALUATE_KIND, lower.factory())
                    .unwrap();
                let request = runtime.request(&[lower.query(&program.entry).unwrap()]);
                let result = runtime.wait(request);
                assert!(
                    lower
                        .work_snapshot()
                        .iter()
                        .any(|(_, work)| work.lowerings > 0),
                    "actual Lower must execute before its settled metadata debit"
                );
                let held = if output_bytes == 1 {
                    assert_eq!(result.unwrap_err(), errors.resource);
                    None
                } else {
                    let products = result
                        .unwrap_or_else(|r| panic!("Lower refusal {r:?}: {:?}", lower.failure(r)));
                    let output = lower.outcome(products[0]).unwrap();
                    assert!(output.verified.is_some());
                    assert!(lower.retained_bytes() > input_bytes);
                    assert!(lower.retained_bytes() <= input_bytes + output_bytes);
                    assert!(
                        lower.retained_bytes() < FrontLowerQueries::output_bytes(weight).unwrap(),
                        "settled metadata does not retain the unused maximum future/body weight"
                    );
                    Some(output)
                };
                runtime.shutdown();
                drop(program);
                drop(front);
                drop(lower);
                if held.is_some() {
                    assert!(budget.reserved() > 0)
                }
                drop(held);
                assert_eq!(budget.reserved(), 0);
            }
        }
    });
}

#[test]
fn admitted_lower_consumes_many_constant_dependencies_with_original_mailbox_bound() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main()->u64{0u64}\n").unwrap();
        for jobs in [1, 2] {
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let definitions = program.engine.front_source_roots();
            let selected = definitions
                .iter()
                .find(|path| {
                    path.to_string()
                        .ends_with("/internal/curve25519_table/position")
                })
                .unwrap()
                .clone();
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let weight = MemoryWeight {
                retained: 64 << 10,
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
                    budget
                        .reserve((weight.retained + 8192) * definitions.len() as u64)
                        .unwrap(),
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
            let request = runtime.request(&[lower.query(&selected).unwrap()]);
            let products = runtime.wait(request).unwrap_or_else(|reference| {
                panic!(
                    "Lower {selected}: {reference:?}, failure={:?}, work={:?}",
                    lower.failure(reference),
                    lower.work_snapshot()
                )
            });
            let output = lower.outcome(products[0]).unwrap();
            assert!(!output.output.value.bodies.is_empty());
            let work = lower.work_snapshot();
            for index in 0..32 {
                let name = format!("BASE_{index}");
                let (_, observed) = work
                    .iter()
                    .find(|(path, _)| path.0.modules == selected.0.modules && path.0.name == name)
                    .unwrap();
                assert_eq!(
                    observed.initializers, 1,
                    "{name} initializer must not replay"
                );
            }
            assert_eq!(
                work.iter()
                    .find(|(path, _)| *path == selected)
                    .unwrap()
                    .1
                    .lowerings,
                1
            );
            assert_eq!(front.live_body_count(), 0);
            assert!(lower.retained_bytes() <= 32 << 20);
            let held = output.clone();
            runtime.shutdown();
            assert!(runtime.stats().peak_bytes <= 512 << 20);
            drop(output);
            drop(lower);
            drop(front);
            drop(program);
            assert!(
                budget.reserved() > 0,
                "copied product retains its original admission"
            );
            drop(held);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
