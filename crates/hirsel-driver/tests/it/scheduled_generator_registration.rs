//! CT-5 source registration must settle before freezing an admitted front.
use hirsel_driver::COMPILER;
use hirsel_engine::with_compiler_stack;

#[test]
fn admitted_generator_text_waits_on_lower_with_one_worker_and_keeps_detached_evidence() {
    use hirsel_engine::{runtime::*, store::MemStore, *};
    use std::sync::Arc;
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"),
            "fn make()->Result[Body[fn()->u8 uses {}],String]{Ok(codegen.body(\"fn()->u8 uses {} {7u8}\"))}\npub fn main()->u8 uses {} = comptime make()\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"generated_front","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            let compiler = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines();
            let mut scalar = compiler.load_program(&manifest).unwrap();
            assert!(
                scalar
                    .check(LoweringPolicy::Comptime)
                    .unwrap()
                    .diagnostics
                    .is_empty()
            );
            let expected = scalar
                .engine
                .generated_source(&scalar.entry)
                .unwrap()
                .text
                .clone();
            drop(scalar);
            let mut program = compiler.load_program(&manifest).unwrap();
            let paths = program.engine.front_source_roots();
            let ordinary: Vec<_> = paths.into_iter().filter(|p| p != &program.entry).collect();
            let generators = vec![program.entry.clone()];
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
                .front_body_queries(&ordinary, weight, budget.reserve(16 << 20).unwrap(), errors)
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &ordinary,
                    front.clone(),
                    weight,
                    32,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let required = FrontRegistrationQueries::input_bytes(&generators);
            assert!(
                program
                    .engine
                    .front_registration_queries(
                        &generators,
                        lower.clone(),
                        weight,
                        budget.reserve(required - 1).unwrap(),
                        errors
                    )
                    .is_err()
            );
            let registration = program
                .engine
                .front_registration_queries(
                    &generators,
                    lower.clone(),
                    weight,
                    budget.reserve(16 << 20).unwrap(),
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
            runtime
                .register_local(FRONT_REGISTRATION_KIND, registration.factory())
                .unwrap();
            let query = registration.query(&program.entry).unwrap();
            let request = runtime.request(&[query]);
            let products = runtime.wait(request).unwrap_or_else(|reference| {
                panic!(
                    "generator: {:?}",
                    registration
                        .failed(reference)
                        .map(|o| o.text().map(str::to_owned))
                )
            });
            let output = registration.outcome(products[0]).unwrap();
            assert_eq!(output.text().unwrap(), expected);
            assert!(output.has_execution_acceptance());
            assert!(output.reads().iter().any(
                |(key, _)| matches!(key, ReadKey::ExecutedBody(id) if id.path().0.name == "make")
            ));
            assert!(
                program.engine.generated_source(&program.entry).is_none(),
                "private text never publishes a namespace"
            );
            runtime.shutdown();
            drop(runtime);
            drop(registration);
            drop(lower);
            drop(front);
            assert!(
                budget.reserved() > 0,
                "detached text keeps its original owners"
            );
            assert_eq!(output.text().unwrap(), expected);
            drop(output);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn admitted_generator_text_preserves_typed_errors_and_leaves_unrequested_siblings_idle() {
    generator_refusal_control(false);
}

#[test]
fn admitted_generator_text_refuses_zero_depth_before_lowering_or_execution() {
    generator_refusal_control(true);
}

fn generator_refusal_control(zero_depth: bool) {
    use hirsel_engine::{runtime::*, store::MemStore, *};
    use std::sync::Arc;
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"),
            "fn fail()->Result[Body[fn()->u8 uses {}],String]{Error(\"generator stop\")}\nfn untouched()->u8{9u8}\npub fn main()->u8 uses {} = comptime fail()\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"generated_refusal","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            let compiler = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_comptime_limits(ComptimeLimits {
                    call_depth: if zero_depth { 0 } else { usize::MAX },
                    ..Default::default()
                })
                .with_owned_engines();
            let mut program = compiler.load_program(&manifest).unwrap();
            let ordinary: Vec<_> = program
                .engine
                .front_source_roots()
                .into_iter()
                .filter(|p| p != &program.entry)
                .collect();
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
                .front_body_queries(&ordinary, weight, budget.reserve(16 << 20).unwrap(), errors)
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &ordinary,
                    front.clone(),
                    weight,
                    32,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let registration = program
                .engine
                .front_registration_queries(
                    &[program.entry.clone()],
                    lower.clone(),
                    weight,
                    budget.reserve(16 << 20).unwrap(),
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
            runtime
                .register_local(FRONT_REGISTRATION_KIND, registration.factory())
                .unwrap();
            let request = runtime.request(&[registration.query(&program.entry).unwrap()]);
            let failed = runtime
                .wait(request)
                .expect_err("refusal must not publish successful text");
            let output = registration.failed(failed).unwrap();
            match output.text().unwrap_err() {
                QueryError::Operational(error) if zero_depth => {
                    assert_eq!(error.operation, "compile-time tooling limit");
                    assert!(error.message.contains("CallDepth"));
                }
                QueryError::Diagnostics(diagnostics) if !zero_depth => {
                    assert_eq!(diagnostics.len(), 1);
                    assert_eq!(diagnostics[0].rule.as_str(), "CT-1");
                    assert!(
                        diagnostics[0]
                            .notes
                            .iter()
                            .any(|note| note == "generator stop")
                    );
                }
                error => panic!("unexpected generator refusal: {error:?}"),
            }
            assert!(!output.has_execution_acceptance());
            assert!(program.engine.generated_source(&program.entry).is_none());
            let mut untouched = program.entry.clone();
            untouched.0.name = "untouched".into();
            assert_eq!(front.recompute_count(&untouched, QueryKind::Check), 0);
            if zero_depth {
                for path in &ordinary {
                    assert_eq!(front.recompute_count(path, QueryKind::Check), 0);
                    assert_eq!(lower.work(path).lowerings, 0);
                }
            }
            runtime.shutdown();
        }
    });
}
