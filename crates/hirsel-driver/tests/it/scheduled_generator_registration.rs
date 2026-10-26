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

#[test]
fn accepted_generator_view_checks_and_lowers_the_literal_without_changing_raw_source() {
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
            let raw = program.engine.definition_items(&program.entry).unwrap()[0].clone();
            let accepted = program
                .engine
                .accept_front_registration(output.clone(), budget.reserve(16 << 20).unwrap())
                .unwrap();
            assert_eq!(
                program.engine.definition_items(&program.entry).unwrap()[0].text_hash,
                raw.text_hash
            );
            let all = program.engine.front_source_roots();
            assert!(
                program
                    .engine
                    .front_body_queries(&all, weight, budget.reserve(16 << 20).unwrap(), errors)
                    .is_err()
            );
            let accepted_front = program
                .engine
                .front_body_queries_with_registrations(
                    &all,
                    &[accepted],
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let accepted_lower = program
                .engine
                .front_lower_queries(
                    &all,
                    accepted_front.clone(),
                    weight,
                    64,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let mut checked_runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(256 << 20).unwrap(),
                errors,
            )
            .unwrap();
            checked_runtime
                .register(FRONT_BODY_KIND, accepted_front.factory())
                .unwrap();
            checked_runtime
                .register_local(FRONT_LOWER_KIND, accepted_lower.factory())
                .unwrap();
            checked_runtime
                .register_local(FRONT_EVALUATE_KIND, accepted_lower.factory())
                .unwrap();
            let lowered = accepted_lower.query(&program.entry).unwrap();
            let request = checked_runtime.request(&[lowered]);
            let result = checked_runtime
                .wait(request)
                .unwrap_or_else(|failure| panic!("accepted Lower failed: {failure:?}, resource={}, protocol={}, typed={:?}, work={:?}, runtime={:?}", failure == errors.resource, failure == errors.protocol, accepted_lower.failure(failure), accepted_front.work_snapshot(), checked_runtime.stats()));
            let actual = accepted_lower.outcome(result[0]).unwrap();
            assert_eq!(actual.output.value.path.path(), &program.entry);
            assert!(actual.execution.is_some());
            checked_runtime.shutdown();
            drop(checked_runtime);
            drop(actual);
            drop(accepted_lower);
            drop(accepted_front);

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
            drop(program);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

const ACCEPTED_SOURCE: &str = "fn make()->Result[Body[fn()->u8 uses {}],String]{Ok(codegen.body(\"fn()->u8 uses {} {7u8}\"))}\npub fn main()->u8 uses {} = comptime make()\n";

fn accepted_output_control(
    jobs: usize,
    run: impl FnOnce(
        &mut hirsel_engine::LoadedProgram,
        &std::sync::Arc<hirsel_engine::runtime::ProcessBudget>,
        hirsel_engine::FrontRegistrationOutput,
    ),
) {
    accepted_output_control_with(jobs, hirsel_engine::Verify::On, ACCEPTED_SOURCE, run);
}

fn accepted_output_control_with(
    jobs: usize,
    verify: hirsel_engine::Verify,
    source: &str,
    run: impl FnOnce(
        &mut hirsel_engine::LoadedProgram,
        &std::sync::Arc<hirsel_engine::runtime::ProcessBudget>,
        hirsel_engine::FrontRegistrationOutput,
    ),
) {
    use hirsel_engine::{runtime::*, store::MemStore, *};
    use std::sync::Arc;
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("hirsel.json");
    std::fs::write(directory.path().join("main.hir"), source).unwrap();
    std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"accepted_front","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    let compiler = COMPILER
        .with_config(CompileConfig {
            store: Arc::new(MemStore::default()),
            jobs: hirsel_intern::jobs::Jobs::new(jobs),
            hold_bytes: 0,
            verify,
            ..Default::default()
        })
        .with_owned_engines();
    let mut program = compiler.load_program(&manifest).unwrap();
    let ordinary: Vec<_> = program
        .engine
        .front_source_roots()
        .into_iter()
        .filter(|path| path != &program.entry)
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
    let query = registration.query(&program.entry).unwrap();
    let request = runtime.request(&[query]);
    let complete = runtime.wait(request).unwrap();
    let output = registration.outcome(complete[0]).unwrap();
    runtime.shutdown();
    drop(runtime);
    drop(registration);
    drop(lower);
    drop(front);
    run(&mut program, &budget, output);
}

#[test]
fn accepted_generator_view_refuses_short_grants_without_publishing_sources_and_keeps_last_source_owner()
 {
    use hirsel_engine::{runtime::*, *};
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            accepted_output_control(jobs, |program, budget, output| {
                let before = program.engine.sources.retained_payload_bytes();
                let reserved = budget.reserved();
                assert!(
                    program
                        .engine
                        .accept_front_registration(output.clone(), budget.reserve(0).unwrap())
                        .is_err()
                );
                assert_eq!(budget.reserved(), reserved);
                assert_eq!(program.engine.sources.retained_payload_bytes(), before);
                assert!(program.engine.generated_source(&program.entry).is_none());
                let accepted = program
                    .engine
                    .accept_front_registration(output.clone(), budget.reserve(16 << 20).unwrap())
                    .unwrap();
                assert!(accepted.retained_bytes() > 0);
                assert!(accepted.retained_bytes() <= 16 << 20);
                let retained = accepted.retained_bytes();
                let detached = program.engine.sources.clone();
                drop(accepted);
                drop(output);
                assert!(budget.reserved() > 0);
                // The engine and this detached source both retain the new source grant.
                drop(detached);
                assert_eq!(budget.reserved(), retained);
            });
        }
    });
}

#[test]
fn accepted_generator_view_refuses_changed_executed_helper_reads() {
    accepted_edit_control(false);
}

#[test]
fn accepted_generator_view_refuses_changed_original_declarations() {
    accepted_edit_control(true);
}

fn accepted_edit_control(own: bool) {
    use hirsel_engine::{runtime::*, *};
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            accepted_output_control(jobs, |program, budget, output| {
                let accepted = program
                    .engine
                    .accept_front_registration(output.clone(), budget.reserve(16 << 20).unwrap())
                    .unwrap();
                let primary = program
                    .engine
                    .sources
                    .file(program.primary)
                    .unwrap()
                    .path
                    .clone();
                let edited = if own {
                    ACCEPTED_SOURCE.replace("comptime make()", "comptime {make()}")
                } else {
                    ACCEPTED_SOURCE.replace("{7u8}", "{8u8}")
                };
                assert!(program.replace_source(&primary, edited).unwrap());
                program.parse_sources().unwrap();
                let paths = program.engine.front_source_roots();
                let weight = MemoryWeight {
                    retained: 2 << 20,
                    scratch: 8 << 20,
                    stack: 0,
                };
                let error = program
                    .engine
                    .front_body_queries_with_registrations(
                        &paths,
                        &[accepted],
                        weight,
                        budget.reserve(16 << 20).unwrap(),
                        crate::scheduled_front::errors(),
                    )
                    .err()
                    .expect("stale view must refuse");
                let QueryError::Operational(failure) = error else {
                    panic!("unexpected rejection: {error:?}")
                };
                assert_eq!(failure.operation, "generated syntax validation");
                if own {
                    assert!(failure.message.contains("original declaration changed"));
                } else {
                    assert!(
                        failure.message.contains("read changed"),
                        "{}",
                        failure.message
                    );
                }
                assert!(
                    program
                        .engine
                        .accept_front_registration(output, budget.reserve(16 << 20).unwrap())
                        .is_err()
                );
            });
        }
    });
}

#[test]
fn accepted_generator_view_accepts_the_exact_grant_and_refuses_n_minus_one() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let mut required = 0;
            accepted_output_control(jobs, |program, budget, output| {
                let accepted = program
                    .engine
                    .accept_front_registration(output, budget.reserve(16 << 20).unwrap())
                    .unwrap();
                required = accepted.retained_bytes();
            });
            accepted_output_control(jobs, |program, budget, output| {
                let before = program.engine.sources.retained_payload_bytes();
                assert!(
                    program
                        .engine
                        .accept_front_registration(output, budget.reserve(required - 1).unwrap())
                        .is_err()
                );
                assert_eq!(program.engine.sources.retained_payload_bytes(), before);
                assert!(program.engine.generated_source(&program.entry).is_none());
            });
            accepted_output_control(jobs, |program, budget, output| {
                let accepted = program
                    .engine
                    .accept_front_registration(output, budget.reserve(required).unwrap())
                    .unwrap();
                assert_eq!(accepted.retained_bytes(), required);
            });
        }
    });
}

#[test]
fn accepted_generator_view_rejects_verify_off_text_when_validation_is_required() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            accepted_output_control_with(
                jobs,
                hirsel_engine::Verify::Off,
                ACCEPTED_SOURCE,
                |program, budget, output| {
                    assert!(!output.has_execution_acceptance());
                    program.engine.verify = hirsel_engine::Verify::On;
                    let before = program.engine.sources.retained_payload_bytes();
                    assert!(
                        program
                            .engine
                            .accept_front_registration(output, budget.reserve(16 << 20).unwrap())
                            .is_err()
                    );
                    assert_eq!(program.engine.sources.retained_payload_bytes(), before);
                    assert!(program.engine.generated_source(&program.entry).is_none());
                },
            );
        }
    });
}

#[test]
fn accepted_generator_view_rejects_a_literal_header_that_differs_from_its_declaration() {
    with_compiler_stack(|| {
        let source = ACCEPTED_SOURCE.replace("fn()->u8 uses {} {7u8}", "fn()->u16 uses {} {7u16}");
        for jobs in [1, 2] {
            accepted_output_control_with(
                jobs,
                hirsel_engine::Verify::On,
                &source,
                |program, budget, output| {
                    assert!(output.has_execution_acceptance());
                    let before = program.engine.sources.retained_payload_bytes();
                    let error = program
                        .engine
                        .accept_front_registration(output, budget.reserve(16 << 20).unwrap())
                        .err()
                        .expect("literal type differs");
                    let hirsel_engine::QueryError::Diagnostics(diagnostics) = error.error else {
                        panic!("{error:?}")
                    };
                    assert!(
                        diagnostics
                            .iter()
                            .any(|diagnostic| diagnostic.rule.as_str() == "LANG-7"),
                        "{diagnostics:?}"
                    );
                    assert_eq!(program.engine.sources.retained_payload_bytes(), before);
                    assert!(program.engine.generated_source(&program.entry).is_none());
                },
            );
        }
    });
}

#[test]
fn rejected_generator_header_retains_diagnostic_sources_without_namespace_publication() {
    with_compiler_stack(|| {
        let source = ACCEPTED_SOURCE.replace("fn()->u8 uses {} {7u8}", "fn()->u16 uses {} {7u16}");
        for jobs in [1, 2] {
            accepted_output_control_with(
                jobs,
                hirsel_engine::Verify::On,
                &source,
                |program, budget, output| {
                    let before = program.engine.sources.retained_payload_bytes();
                    let failure = program
                        .engine
                        .accept_front_registration(output, budget.reserve(16 << 20).unwrap())
                        .err()
                        .expect("header rejection");
                    let hirsel_engine::QueryError::Diagnostics(diagnostics) = &failure.error else {
                        panic!("{failure:?}")
                    };
                    assert!(diagnostics.iter().any(|d| d.rule.as_str() == "LANG-7"));
                    for diagnostic in diagnostics {
                        assert!(failure.sources.slice(diagnostic.primary).is_some());
                        assert!(
                            diagnostic
                                .secondary
                                .iter()
                                .any(|label| failure.sources.slice(label.span).is_some())
                        );
                    }
                    assert_eq!(program.engine.sources.retained_payload_bytes(), before);
                    assert!(program.engine.generated_source(&program.entry).is_none());
                    let detached = failure.sources.clone();
                    drop(failure);
                    assert_eq!(budget.reserved(), 16 << 20);
                    drop(detached);
                    assert_eq!(budget.reserved(), 0);
                },
            );
        }
    });
}

#[test]
fn accepted_generator_view_releases_unused_syntax_grant_and_keeps_clone_owned_bytes() {
    use hirsel_engine::{runtime::*, *};
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let mut retained_budget = None;
            accepted_output_control(jobs, |program, budget, output| {
                let before = budget.reserved();
                let accepted = program
                    .engine
                    .accept_front_registration(output, budget.reserve(16 << 20).unwrap())
                    .unwrap();
                let retained = accepted.retained_bytes();
                assert!(retained > 0 && retained < 16 << 20);
                assert_eq!(
                    budget.reserved(),
                    before + retained,
                    "acceptance releases every unused syntax byte immediately"
                );
                let cloned = accepted.clone();
                let source = program.engine.sources.clone();
                drop(accepted);
                assert_eq!(budget.reserved(), before + retained);
                drop(cloned);
                assert_eq!(
                    budget.reserved(),
                    retained,
                    "only detached source ownership survives producer destruction"
                );
                drop(source);
                assert_eq!(
                    budget.reserved(),
                    retained,
                    "engine source still owns accepted bytes"
                );
                retained_budget = Some(budget.clone());
            });
            assert_eq!(
                retained_budget.unwrap().reserved(),
                0,
                "last engine source releases exact retained grant"
            );
        }
    });
}

#[test]
fn generator_bootstrap_finishes_before_final_admitted_check_and_preserves_its_scalar_result() {
    use hirsel_engine::{runtime::*, store::MemStore, *};
    use std::sync::Arc;
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"), ACCEPTED_SOURCE).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"bootstrap_front","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            let compiler = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines();
            let expected = compiler
                .load_program(&manifest)
                .unwrap()
                .check(LoweringPolicy::Comptime)
                .unwrap();
            let mut program = compiler.load_program(&manifest).unwrap();
            let definitions = program.engine.front_source_roots();
            let generators = program.engine.front_generator_roots(&definitions).unwrap();
            assert_eq!(generators, vec![program.entry.clone()]);
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let weight = MemoryWeight {
                retained: 2 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let required =
                FrontRegistrationBootstrap::input_bytes(&definitions, generators.len()).unwrap();
            let admission = |metadata| FrontRegistrationBootstrapAdmission {
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                weight,
                max_reads: 64,
                runtime: budget.reserve(512 << 20).unwrap(),
                front: budget.reserve(16 << 20).unwrap(),
                lower: budget.reserve(16 << 20).unwrap(),
                text: budget.reserve(16 << 20).unwrap(),
                syntax: vec![budget.reserve(16 << 20).unwrap()],
                metadata: budget.reserve(metadata).unwrap(),
            };
            let before = program.engine.work_stats();
            assert!(
                program
                    .engine
                    .bootstrap_front_registrations(
                        &definitions,
                        &generators,
                        admission(required - 1),
                        errors
                    )
                    .is_err()
            );
            assert_eq!(program.engine.work_stats(), before);
            assert_eq!(budget.reserved(), 0);
            let bootstrap = program
                .engine
                .bootstrap_front_registrations(
                    &definitions,
                    &generators,
                    admission(required),
                    errors,
                )
                .unwrap();
            assert_eq!(bootstrap.stats().active, 0);
            assert_eq!(bootstrap.registrations().len(), 1);
            assert_eq!(
                bootstrap.query_capacity(),
                FrontRegistrationBootstrap::required_queries(
                    definitions.len() - generators.len(),
                    generators.len()
                )
                .unwrap()
            );
            assert!(
                budget.reserved() < 512 << 20,
                "bootstrap runtime grant releases before final construction"
            );
            let front = program
                .engine
                .front_body_queries_with_registrations(
                    &definitions,
                    bootstrap.registrations(),
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
                    64,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let growth = program
                .engine
                .front_growth_queries(
                    &program.entry,
                    front.clone(),
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let check = program
                .engine
                .front_check_queries(
                    FrontCheckPlan {
                        files: program
                            .files
                            .iter()
                            .map(|(file, context)| {
                                (*file, context.clone(), LoweringPolicy::Comptime)
                            })
                            .collect(),
                        front: front.clone(),
                        lower: lower.clone(),
                        growth: growth.clone(),
                        unsafe_ffi: program.unsafe_ffi.clone(),
                        lint_package: None,
                    },
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let capacity = definitions.len() * 4 + 8;
            let mut runtime = AdmittedRuntime::with_query_capacity(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors,
                capacity,
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
                .register(FRONT_GROWTH_KIND, growth.factory())
                .unwrap();
            runtime.register(FRONT_CHECK_KIND, check.factory()).unwrap();
            let request = runtime.request(&[check.query()]);
            let complete = runtime
                .wait(request)
                .unwrap_or_else(|reference| panic!("final check {:?}", check.failure(reference)));
            let output = check.outcome(complete[0]).unwrap();
            assert_eq!(output.checked.completion, expected.completion);
            assert_eq!(output.checked.diagnostics, expected.diagnostics);
            runtime.shutdown();
            drop(runtime);
            drop(output);
            drop(check);
            drop(growth);
            drop(lower);
            drop(front);
            drop(bootstrap);
            drop(program);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn generator_bootstrap_preserves_typed_failure_and_only_the_accepted_source_prefix() {
    bootstrap_refusal_control(false, false);
}

#[test]
fn generator_bootstrap_zero_depth_refuses_before_any_source_publication() {
    bootstrap_refusal_control(true, false);
}

#[test]
fn generator_bootstrap_counts_spare_syntax_grants_before_engine_work() {
    bootstrap_refusal_control(false, true);
}

fn bootstrap_refusal_control(zero_depth: bool, spare_grants: bool) {
    use hirsel_engine::{runtime::*, store::MemStore, *};
    use std::sync::Arc;
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"),
            "fn good()->Result[Body[fn()->u8 uses {}],String]{Ok(codegen.body(\"fn()->u8 uses {} {7u8}\"))}\nfn fail()->Result[Body[fn()->u8 uses {}],String]{Error(\"bootstrap stop\")}\nfn later()->Result[Body[fn()->u8 uses {}],String]{Error(\"must stay idle\")}\npub fn a_ok()->u8 uses {} = comptime good()\npub fn b_fail()->u8 uses {} = comptime fail()\npub fn c_later()->u8 uses {} = comptime later()\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"bootstrap_refusal","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"a_ok"}}"#).unwrap();
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
            let definitions = program.engine.front_source_roots();
            let mut generators = program.engine.front_generator_roots(&definitions).unwrap();
            generators.sort_by(|a, b| a.0.name.cmp(&b.0.name));
            assert_eq!(
                generators
                    .iter()
                    .map(|p| p.0.name.as_str())
                    .collect::<Vec<_>>(),
                ["a_ok", "b_fail", "c_later"]
            );
            let budget = ProcessBudget::new(1 << 30);
            let metadata =
                FrontRegistrationBootstrap::input_bytes(&definitions, generators.len()).unwrap();
            let mut syntax = Vec::with_capacity(if spare_grants { 32 } else { generators.len() });
            syntax.extend(generators.iter().map(|_| budget.reserve(16 << 20).unwrap()));
            let before = program.engine.work_stats();
            let failure = program
                .engine
                .bootstrap_front_registrations(
                    &definitions,
                    &generators,
                    FrontRegistrationBootstrapAdmission {
                        jobs: hirsel_intern::jobs::Jobs::new(jobs),
                        weight: MemoryWeight {
                            retained: 2 << 20,
                            scratch: 8 << 20,
                            stack: 0,
                        },
                        max_reads: 64,
                        runtime: budget.reserve(512 << 20).unwrap(),
                        front: budget.reserve(16 << 20).unwrap(),
                        lower: budget.reserve(16 << 20).unwrap(),
                        text: budget.reserve(16 << 20).unwrap(),
                        syntax,
                        metadata: budget.reserve(metadata).unwrap(),
                    },
                    crate::scheduled_front::errors(),
                )
                .err()
                .expect("bootstrap refusal");
            if spare_grants {
                assert!(
                    matches!(&failure, FrontRegistrationBootstrapError::Admission(QueryError::Operational(error)) if error.operation == "generator bootstrap admission")
                );
                assert_eq!(program.engine.work_stats(), before);
                assert_eq!(budget.reserved(), 0);
            } else {
                let FrontRegistrationBootstrapError::Runtime {
                    reference,
                    error: Some(error),
                    owner,
                    sources,
                } = &failure
                else {
                    panic!("{failure:?}")
                };
                let output = owner
                    .failed(*reference)
                    .expect("typed failure product survives runtime shutdown");
                assert_eq!(
                    output.definition(),
                    &generators[if zero_depth { 0 } else { 1 }]
                );
                assert!(!output.has_execution_acceptance());
                match error {
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
                                .any(|note| note == "bootstrap stop")
                        );
                        assert!(sources.slice(diagnostics[0].primary).is_some());
                    }
                    error => panic!("unexpected bootstrap failure: {error:?}"),
                }
                assert!(
                    budget.reserved() < 512 << 20,
                    "failed bootstrap releases its runtime partition too"
                );
            }
            assert_eq!(
                program.engine.generated_source(&generators[0]).is_some(),
                !zero_depth && !spare_grants
            );
            assert!(program.engine.generated_source(&generators[1]).is_none());
            assert!(program.engine.generated_source(&generators[2]).is_none());
            drop(program);
            if !spare_grants {
                assert!(budget.reserved() > 0, "detached failure owns evidence");
            }
            drop(failure);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
