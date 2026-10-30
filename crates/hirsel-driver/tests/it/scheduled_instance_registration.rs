//! Explicit CT-5 registration rounds, independent of the host bootstrap.
use hirsel_driver::COMPILER;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

const SOURCE: &str = r#"
type Point { x: u8 }
type Box3 { x: u16 }
fn make[T](s: TypeShape[T]) -> Result[Body[fn(T)->u64 uses {}], String] uses {} {
  let info = type_shape.info(s)
  if info.name == "instance_front@1/Point" { Ok(codegen.body("fn(v:Point)->u64 uses {} {7u64}")) }
  else { Ok(codegen.body("fn(v:Box3)->u64 uses {} {7u64}")) }
}
fn generated[T](v: T)->u64 uses {} = comptime make(shape(T))
pub fn main()->u64 uses {} { generated(Point(x:1u8)) + generated(Box3(x:2u16)) + generated(Point(x:3u8)) }
"#;
fn fixture(source: &str, jobs: usize, run: impl FnOnce(&mut LoadedProgram, &Arc<ProcessBudget>)) {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("hirsel.json");
    std::fs::write(directory.path().join("main.hir"), source).unwrap();
    std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"instance_front","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    let compiler = COMPILER
        .with_config(CompileConfig {
            store: Arc::new(MemStore::default()),
            jobs: hirsel_intern::jobs::Jobs::new(jobs),
            hold_bytes: 0,
            ..Default::default()
        })
        .with_owned_engines();
    let mut program = compiler.load_program(&manifest).unwrap();
    let budget = ProcessBudget::new(1 << 30);
    run(&mut program, &budget);
}
fn weight() -> MemoryWeight {
    MemoryWeight {
        retained: 2 << 20,
        scratch: 8 << 20,
        stack: 0,
    }
}
fn ordinary(program: &mut LoadedProgram) -> Vec<DefPath> {
    program
        .engine
        .front_source_roots()
        .into_iter()
        .filter(|p| {
            p.0.name != "generated" && p.0.name != "inner" && !p.0.name.contains("$instance$")
        })
        .collect()
}
fn front(
    program: &mut LoadedProgram,
    budget: &Arc<ProcessBudget>,
    jobs: usize,
    accepted: &[Arc<AcceptedFrontRegistration>],
) -> Arc<FrontBodyQueries> {
    let mut paths = ordinary(program);
    paths.extend(accepted.iter().map(|a| a.definition().clone()));
    let front = program
        .engine
        .front_body_queries_with_registrations(
            &paths,
            accepted,
            weight(),
            budget.reserve(32 << 20).unwrap(),
            crate::scheduled_front::errors(),
        )
        .unwrap();
    let mut runtime = AdmittedRuntime::new(
        hirsel_intern::jobs::Jobs::new(jobs),
        budget.reserve(256 << 20).unwrap(),
        crate::scheduled_front::errors(),
    )
    .unwrap();
    runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
    let queries: Vec<_> = paths.iter().map(|p| front.query(p).unwrap()).collect();
    let request = runtime.request(&queries);
    if let Err(error) = runtime.wait(request) {
        let failure = front.outcome(Err(error));
        panic!(
            "front {error:?}: {:?}; resource={}, protocol={}, work={:?}, accepted={:?}",
            failure.map(|o| o.outcome.result),
            error == crate::scheduled_front::errors().resource,
            error == crate::scheduled_front::errors().protocol,
            front.work_snapshot(),
            accepted.iter().map(|a| a.definition()).collect::<Vec<_>>()
        );
    }
    runtime.shutdown();
    front
}
fn execute(
    program: &mut LoadedProgram,
    budget: &Arc<ProcessBudget>,
    jobs: usize,
    front: Arc<FrontBodyQueries>,
    paths: &[DefPath],
    run: impl FnOnce(&mut LoadedProgram, Vec<FrontRegistrationOutput>),
) {
    let lower_paths = ordinary(program);
    let errors = crate::scheduled_front::errors();
    let lower = program
        .engine
        .front_lower_queries(
            &lower_paths,
            front.clone(),
            weight(),
            32,
            budget.reserve(32 << 20).unwrap(),
            errors,
        )
        .unwrap();
    let registration = program
        .engine
        .front_registration_queries(
            paths,
            lower.clone(),
            weight(),
            budget.reserve(32 << 20).unwrap(),
            errors,
        )
        .unwrap();
    let mut runtime = AdmittedRuntime::new(
        hirsel_intern::jobs::Jobs::new(jobs),
        budget.reserve(256 << 20).unwrap(),
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
    let queries: Vec<_> = paths
        .iter()
        .map(|p| registration.query(p).unwrap())
        .collect();
    let request = runtime.request(&queries);
    let products = runtime.wait(request).unwrap_or_else(|r| {
        panic!(
            "generator {:?}",
            registration.failed(r).map(|o| o.text().map(str::to_owned))
        )
    });
    // A caller cannot turn arbitrary product bytes into private execution
    // acceptance: only this producer's exact completed product is transportable.
    let mut forged = products[0].clone();
    forged.action = hirsel_artifact_store::Action(hirsel_syntax::ContentHash::default());
    assert!(registration.outcome(forged).is_none());
    let outputs: Vec<_> = products
        .into_iter()
        .map(|p| registration.outcome(p).unwrap())
        .collect();
    assert!(outputs.iter().all(|o| o.has_execution_acceptance()));
    runtime.shutdown();
    run(program, outputs);
}
fn discovery(
    program: &mut LoadedProgram,
    budget: &Arc<ProcessBudget>,
    fronts: &[Arc<FrontBodyQueries>],
) -> FrontInstanceDiscovery {
    let roots = [program.entry.clone()];
    program
        .engine
        .discover_front_instances(&roots, fronts, budget.reserve(16 << 20).unwrap())
        .unwrap()
}
fn register(
    program: &mut LoadedProgram,
    budget: &Arc<ProcessBudget>,
    round: &FrontInstanceDiscovery,
) -> Vec<DefPath> {
    round
        .requests()
        .iter()
        .map(|r| {
            let input = program
                .engine
                .register_front_instance(r, budget.reserve(16 << 20).unwrap())
                .unwrap();
            assert!(
                program
                    .engine
                    .generated_source(input.definition())
                    .is_none(),
                "registration cannot execute"
            );
            input.definition().clone()
        })
        .collect()
}
#[test]
fn closed_instances_accept_two_types_deduplicate_and_select_nested_instances_before_publication() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let source = SOURCE.replace("{7u64}", "{ inner(v) }")
                + "\nfn inner[T](v:T)->u64 uses {} = comptime inner_make(shape(T))\n"
                + &SOURCE[SOURCE.find("fn make").unwrap()..SOURCE.find("fn generated").unwrap()]
                    .replace("make[T]", "inner_make[T]");
            fixture(&source, jobs, |program, budget| {
                drop(
                    program
                        .engine
                        .prepare_private_inference(budget.reserve(256 << 20).unwrap())
                        .unwrap(),
                );
                let first = front(program, budget, jobs, &[]);
                let round = discovery(program, budget, &[first]);
                assert_eq!(round.requests().len(), 2);
                let paths = register(program, budget, &round);
                assert_eq!(paths.len(), 2);
                let mut accepted = vec![];
                let before_execution = front(program, budget, jobs, &[]);
                execute(
                    program,
                    budget,
                    jobs,
                    before_execution,
                    &paths,
                    |program, outputs| {
                        for output in outputs {
                            accepted.push(
                                program
                                    .engine
                                    .accept_front_registration(
                                        output,
                                        budget.reserve(16 << 20).unwrap(),
                                    )
                                    .unwrap(),
                            );
                        }
                    },
                );
                let second = front(program, budget, jobs, &accepted);
                let round = discovery(program, budget, &[second.clone()]);
                assert_eq!(round.requests().len(), 4);
                let nested: Vec<_> = round
                    .requests()
                    .iter()
                    .filter(|r| r.declaration.0.name == "inner")
                    .map(|r| {
                        program
                            .engine
                            .register_front_instance(r, budget.reserve(16 << 20).unwrap())
                            .unwrap()
                            .definition()
                            .clone()
                    })
                    .collect();
                assert_eq!(nested.len(), 2);
                // New declaration heads change V's universe. Keep its original
                // refusal, then refresh all private execution receipts in the
                // final universe before publishing any accepted front.
                assert!(
                    accepted
                        .iter()
                        .all(|a| a.validate_in_engine(&mut program.engine).is_err())
                );
                accepted.clear();
                let mut final_paths = paths.clone();
                final_paths.extend(nested);
                let before_execution = front(program, budget, jobs, &[]);
                execute(
                    program,
                    budget,
                    jobs,
                    before_execution,
                    &final_paths,
                    |program, outputs| {
                        for output in outputs {
                            accepted.push(
                                program
                                    .engine
                                    .accept_front_registration(
                                        output,
                                        budget.reserve(16 << 20).unwrap(),
                                    )
                                    .unwrap(),
                            );
                        }
                    },
                );
                let final_front = front(program, budget, jobs, &accepted);
                let done = discovery(program, budget, &[final_front]);
                assert!(
                    done.pending_bodies().is_empty(),
                    "{:?}",
                    done.pending_bodies()
                );
                assert_eq!(done.requests().len(), 4);
                assert!(
                    accepted
                        .iter()
                        .all(|a| a.validate_in_engine(&mut program.engine).is_ok())
                );
                assert_eq!(
                    program
                        .engine
                        .recompute_count(&program.entry, QueryKind::Check),
                    0
                );
            });
        }
    });
}
#[test]
fn closed_instance_registration_and_acceptance_enforce_exact_grants() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let mut required_syntax = 0;
            let mut required_accepted = 0;
            for phase in 0..3 {
                fixture(SOURCE, jobs, |program, budget| {
                    let first = front(program, budget, jobs, &[]);
                    let round = discovery(program, budget, &[first]);
                    let request = &round.requests()[0];
                    if phase == 1 {
                        let before = program.engine.sources.retained_payload_bytes();
                        assert!(
                            program
                                .engine
                                .register_front_instance(
                                    request,
                                    budget.reserve(required_syntax - 1).unwrap()
                                )
                                .is_err()
                        );
                        assert_eq!(program.engine.sources.retained_payload_bytes(), before);
                    }
                    let input = program
                        .engine
                        .register_front_instance(
                            request,
                            budget
                                .reserve(if phase == 0 {
                                    16 << 20
                                } else {
                                    required_syntax
                                })
                                .unwrap(),
                        )
                        .unwrap();
                    required_syntax = input.retained_bytes();
                    let path = input.definition().clone();
                    let repeated = program
                        .engine
                        .register_front_instance(request, budget.reserve(16 << 20).unwrap())
                        .unwrap();
                    assert_eq!(input.definition(), repeated.definition());
                    let next = front(program, budget, jobs, &[]);
                    execute(
                        program,
                        budget,
                        jobs,
                        next,
                        &[path],
                        |program, mut outputs| {
                            let output = outputs.pop().unwrap();
                            if phase == 1 {
                                let before = program.engine.sources.retained_payload_bytes();
                                assert!(
                                    program
                                        .engine
                                        .accept_front_registration(
                                            output.clone(),
                                            budget.reserve(required_accepted - 1).unwrap()
                                        )
                                        .is_err()
                                );
                                assert_eq!(program.engine.sources.retained_payload_bytes(), before);
                            }
                            let accepted = program
                                .engine
                                .accept_front_registration(
                                    output,
                                    budget
                                        .reserve(if phase == 0 {
                                            16 << 20
                                        } else {
                                            required_accepted
                                        })
                                        .unwrap(),
                                )
                                .unwrap();
                            required_accepted = accepted.retained_bytes();
                        },
                    );
                });
            }
        }
    });
}
#[test]
fn closed_instance_views_reject_stale_generator_helper_and_declaring_header_reads() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            for edit in [0, 1, 2] {
                fixture(SOURCE, jobs, |program, budget| {
                    let first = front(program, budget, jobs, &[]);
                    let round = discovery(program, budget, &[first]);
                    let paths = register(program, budget, &round);
                    let before = front(program, budget, jobs, &[]);
                    execute(program, budget, jobs, before, &paths, |program, outputs| {
                        let accepted: Vec<_> = outputs
                            .iter()
                            .map(|output| {
                                program
                                    .engine
                                    .accept_front_registration(
                                        output.clone(),
                                        budget.reserve(16 << 20).unwrap(),
                                    )
                                    .unwrap()
                            })
                            .collect();
                        for view in &accepted {
                            assert!(
                                view.validate_current(Verify::On, |_, key| view.reads().get(key))
                                    .is_ok()
                            );
                            assert!(
                                view.validate_current(Verify::On, |_, key| {
                                    if matches!(key, ReadKey::ValidationUniverse(_)) {
                                        None
                                    } else {
                                        view.reads().get(key)
                                    }
                                })
                                .is_err()
                            );
                            assert!(
                                view.validate_current(Verify::On, |_, key| {
                                    if matches!(key, ReadKey::ValidationUniverse(_)) {
                                        Some(hirsel_syntax::ContentHash::default())
                                    } else {
                                        view.reads().get(key)
                                    }
                                })
                                .is_err()
                            );
                            assert!(view.reads().iter().any(|(k, _)| matches!(k, ReadKey::ExecutedBody(d) if d.path().0.name == "generated")));
                        }
                        let source = match edit {
                            0 => SOURCE
                                .replace("comptime make(shape(T))", "comptime {make(shape(T))}"),
                            1 => SOURCE.replace("{7u64}", "{8u64}"),
                            _ => {
                                SOURCE.replace("fn generated[T](v: T)", "fn generated[T](value: T)")
                            }
                        };
                        let path = program
                            .engine
                            .sources
                            .file(program.primary)
                            .unwrap()
                            .path
                            .clone();
                        assert!(program.replace_source(&path, source).unwrap());
                        program.parse_sources().unwrap();
                        for view in accepted {
                            assert!(view.validate_in_engine(&mut program.engine).is_err());
                        }
                        for output in outputs {
                            assert!(
                                program
                                    .engine
                                    .accept_front_registration(
                                        output,
                                        budget.reserve(16 << 20).unwrap()
                                    )
                                    .is_err()
                            );
                        }
                    });
                });
            }
        }
    });
}
#[test]
fn closed_instance_discovery_refuses_stale_summaries_and_exact_output_budget() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            fixture(SOURCE, jobs, |program, budget| {
                let first = front(program, budget, jobs, &[]);
                let round = discovery(program, budget, &[first.clone()]);
                let bytes = round.retained_bytes();
                let roots = [program.entry.clone()];
                assert!(
                    program
                        .engine
                        .discover_front_instances(
                            &roots,
                            &[first.clone()],
                            budget.reserve(bytes - 1).unwrap()
                        )
                        .is_err()
                );
                let exact = program
                    .engine
                    .discover_front_instances(
                        &roots,
                        &[first.clone()],
                        budget.reserve(bytes).unwrap(),
                    )
                    .unwrap();
                assert_eq!(exact.retained_bytes(), bytes);
                let path = program
                    .engine
                    .sources
                    .file(program.primary)
                    .unwrap()
                    .path
                    .clone();
                assert!(
                    program
                        .replace_source(
                            &path,
                            SOURCE.replace("generated(Box3(x:2u16))", "generated(Point(x:2u8))")
                        )
                        .unwrap()
                );
                program.parse_sources().unwrap();
                assert!(
                    program
                        .engine
                        .discover_front_instances(
                            &roots,
                            &[first],
                            budget.reserve(16 << 20).unwrap()
                        )
                        .is_err()
                );
            });
        }
    });
}
#[test]
fn closed_instance_registration_refuses_open_wrong_kind_and_257th_selection_with_owned_spans() {
    use hirsel_types::{
        Actuals, GenericArgument, Size, Type, TypeKind, TypeParamId, TypeParameterRef,
    };
    with_compiler_stack(|| {
        // CT-5's existing size fixture gives 257 distinct closed keys without
        // executing or growing a body. Keep the original 256-instance bound.
        let source = "fn generated[n:size](v:u8)->u64 uses {} = comptime make()\nfn make()->Result[Body[fn(u8)->u64 uses {}], String] uses {} {Ok(codegen.body(\"fn(v:u8)->u64 uses {} {7u64}\"))}\npub fn main()->u64 uses {} {0u64}\n";
        fixture(source, 1, |program, budget| {
            let declaration = program
                .engine
                .front_source_roots()
                .into_iter()
                .find(|p| p.0.name == "generated")
                .unwrap();
            let site =
                hirsel_span::Span::new(program.engine.sources.file_anchor(program.primary), 0, 1)
                    .unwrap();
            let open = program
                .engine
                .types()
                .intern(Type::with_kind(TypeKind::Parameter(
                    TypeParameterRef::bound(TypeParamId(0)),
                )));
            let wrong = FrontInstanceRequest {
                declaration: declaration.clone(),
                actuals: Actuals {
                    arguments: vec![GenericArgument::Type(open)],
                },
                site,
            };
            assert!(
                program
                    .engine
                    .register_front_instance(&wrong, budget.reserve(16 << 20).unwrap())
                    .is_err()
            );
            let open = FrontInstanceRequest {
                declaration: declaration.clone(),
                actuals: Actuals {
                    arguments: vec![GenericArgument::Size(Size::Parameter(
                        hirsel_types::SizeParameterRef::bound(hirsel_types::SizeParamId(0)),
                    ))],
                },
                site,
            };
            assert!(
                program
                    .engine
                    .register_front_instance(&open, budget.reserve(16 << 20).unwrap())
                    .is_err()
            );
            for n in 0..256 {
                let request = FrontInstanceRequest {
                    declaration: declaration.clone(),
                    actuals: Actuals {
                        arguments: vec![GenericArgument::Size(Size::Constant(n))],
                    },
                    site,
                };
                program
                    .engine
                    .register_front_instance(&request, budget.reserve(512 << 10).unwrap())
                    .unwrap();
            }
            let request = FrontInstanceRequest {
                declaration,
                actuals: Actuals {
                    arguments: vec![GenericArgument::Size(Size::Constant(256))],
                },
                site,
            };
            let failure = program
                .engine
                .register_front_instance(&request, budget.reserve(16 << 20).unwrap())
                .err()
                .expect("257 refused");
            let QueryError::Diagnostics(ds) = &failure.error else {
                panic!("{failure:?}")
            };
            assert!(
                ds.iter()
                    .any(|d| d.rule.as_str() == "CT-1" && d.message.contains("256"))
            );
            for d in ds {
                assert!(failure.sources.slice(d.primary).is_some());
                for label in &d.secondary {
                    assert!(failure.sources.slice(label.span).is_some());
                }
            }
            let detached = failure.sources.clone();
            drop(failure);
            assert!(detached.slice(site).is_some());
        });
    });
}

#[test]
fn bounded_instance_bootstrap_reuses_one_grant_and_refreshes_nested_final_universe_views() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let source = SOURCE.replace("{7u64}", "{ inner(v) }")
                + "\nfn inner[T](v:T)->u64 uses {} = comptime inner_make(shape(T))\n"
                + &SOURCE[SOURCE.find("fn make").unwrap()..SOURCE.find("fn generated").unwrap()]
                    .replace("make[T]", "inner_make[T]");
            let mut retained = None;
            fixture(&source, jobs, |program, budget| {
                let definitions = program.engine.front_source_roots();
                let generators = program.engine.front_generator_roots(&definitions).unwrap();
                assert_eq!(generators.len(), 2);
                let admission = FrontInstanceBootstrapAdmission {
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    weight: weight(),
                    max_reads: 64,
                    runtime: budget.reserve(256 << 20).unwrap(),
                    front: budget.reserve(32 << 20).unwrap(),
                    lower: budget.reserve(32 << 20).unwrap(),
                    text: budget.reserve(32 << 20).unwrap(),
                    syntax: budget.reserve(32 << 20).unwrap(),
                    discovery: budget.reserve(16 << 20).unwrap(),
                    metadata: budget.reserve(4 << 20).unwrap(),
                };
                let reserved = budget.reserved();
                assert_eq!(reserved, 404 << 20);
                let bootstrap = program
                    .engine
                    .bootstrap_front_instances(
                        &definitions,
                        &[program.entry.clone()],
                        &generators,
                        admission,
                        crate::scheduled_front::errors(),
                    )
                    .unwrap();
                assert_eq!(
                    bootstrap.registrations().len(),
                    4,
                    "two types at outer and nested declarations"
                );
                assert_eq!(bootstrap.rounds(), 3);
                assert_eq!(bootstrap.stats().active, 0);
                assert_eq!(bootstrap.stats().charged_bytes, 0);
                assert!(bootstrap.stats().claims > 0);
                for view in bootstrap.registrations() {
                    view.validate_in_engine(&mut program.engine).unwrap();
                    assert!(bootstrap.definitions().contains(view.definition()));
                }
                for path in bootstrap.definitions() {
                    for kind in [
                        QueryKind::Resolve,
                        QueryKind::Check,
                        QueryKind::Modes,
                        QueryKind::Lower,
                    ] {
                        assert_eq!(
                            program.engine.recompute_count(path, kind),
                            0,
                            "only S5 workers perform {kind:?}: {path}"
                        );
                    }
                }
                assert!(
                    budget.reserved() < 16 << 20,
                    "one runtime and all completed stage grants release; accepted payloads stay charged"
                );
                let final_front = program
                    .engine
                    .front_body_queries_with_registrations(
                        bootstrap.definitions(),
                        bootstrap.registrations(),
                        weight(),
                        budget.reserve(32 << 20).unwrap(),
                        crate::scheduled_front::errors(),
                    )
                    .unwrap();
                let mut runtime = AdmittedRuntime::new(
                    hirsel_intern::jobs::Jobs::new(jobs),
                    budget.reserve(256 << 20).unwrap(),
                    crate::scheduled_front::errors(),
                )
                .unwrap();
                runtime
                    .register(FRONT_BODY_KIND, final_front.factory())
                    .unwrap();
                let roots: Vec<_> = bootstrap
                    .definitions()
                    .iter()
                    .map(|p| final_front.query(p).unwrap())
                    .collect();
                let request = runtime.request(&roots);
                runtime.wait(request).unwrap();
                let complete = program
                    .engine
                    .discover_front_instances(
                        &[program.entry.clone()],
                        &[final_front.clone()],
                        budget.reserve(16 << 20).unwrap(),
                    )
                    .unwrap();
                assert!(complete.pending_bodies().is_empty());
                assert_eq!(complete.requests().len(), 4);
                runtime.shutdown();
                drop(runtime);
                drop(complete);
                drop(final_front);
                drop(bootstrap);
                retained = Some(budget.clone());
            });
            assert_eq!(retained.unwrap().reserved(), 0);
        }
    });
}

#[test]
fn bounded_instance_bootstrap_refuses_short_metadata_before_body_work_or_registration() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            fixture(SOURCE, jobs, |program, budget| {
                let definitions = program.engine.front_source_roots();
                let generators = program.engine.front_generator_roots(&definitions).unwrap();
                let required =
                    FrontInstanceBootstrap::input_bytes(&definitions, &generators).unwrap();
                let before = program.engine.work_stats();
                let error = program
                    .engine
                    .bootstrap_front_instances(
                        &definitions,
                        &[program.entry.clone()],
                        &generators,
                        FrontInstanceBootstrapAdmission {
                            jobs: hirsel_intern::jobs::Jobs::new(jobs),
                            weight: weight(),
                            max_reads: 64,
                            runtime: budget.reserve(256 << 20).unwrap(),
                            front: budget.reserve(32 << 20).unwrap(),
                            lower: budget.reserve(32 << 20).unwrap(),
                            text: budget.reserve(32 << 20).unwrap(),
                            syntax: budget.reserve(32 << 20).unwrap(),
                            discovery: budget.reserve(16 << 20).unwrap(),
                            metadata: budget.reserve(required - 1).unwrap(),
                        },
                        crate::scheduled_front::errors(),
                    )
                    .err()
                    .expect("short cursor grant");
                let FrontInstanceBootstrapError::Bootstrap(
                    FrontRegistrationBootstrapError::Admission(QueryError::Operational(error)),
                ) = error
                else {
                    panic!("{error:?}")
                };
                assert_eq!(error.operation, "generated instance bootstrap admission");
                assert_eq!(program.engine.work_stats(), before);
                assert_eq!(budget.reserved(), 0);
                assert!(program.engine.generated_sources().next().is_none());
            });
        }
    });
}

#[test]
fn bounded_instance_bootstrap_keeps_generator_error_sources_and_never_accepts_failed_text() {
    instance_bootstrap_failure(false);
}

#[test]
fn bounded_instance_bootstrap_preserves_zero_depth_refusal_without_private_execution() {
    instance_bootstrap_failure(true);
}

fn instance_bootstrap_failure(zero_depth: bool) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"), "type Point{x:u8}\nfn make()->Result[Body[fn(Point)->u64 uses {}],String] uses {} {Error(\"generic bootstrap stop\")}\nfn generated[T](v:T)->u64 uses {} = comptime make()\npub fn main()->u64 uses {} {generated(Point(x:1u8))}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"instance_failure","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
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
            let generators = program.engine.front_generator_roots(&definitions).unwrap();
            let budget = ProcessBudget::new(1 << 30);
            let error = program
                .engine
                .bootstrap_front_instances(
                    &definitions,
                    &[program.entry.clone()],
                    &generators,
                    FrontInstanceBootstrapAdmission {
                        jobs: hirsel_intern::jobs::Jobs::new(jobs),
                        weight: weight(),
                        max_reads: 64,
                        runtime: budget.reserve(256 << 20).unwrap(),
                        front: budget.reserve(32 << 20).unwrap(),
                        lower: budget.reserve(32 << 20).unwrap(),
                        text: budget.reserve(32 << 20).unwrap(),
                        syntax: budget.reserve(32 << 20).unwrap(),
                        discovery: budget.reserve(16 << 20).unwrap(),
                        metadata: budget.reserve(4 << 20).unwrap(),
                    },
                    crate::scheduled_front::errors(),
                )
                .err()
                .expect("generator refusal");
            let FrontInstanceBootstrapError::Bootstrap(FrontRegistrationBootstrapError::Runtime {
                reference,
                error: Some(failure),
                sources,
                owner,
            }) = &error
            else {
                panic!("{error:?}")
            };
            let output = owner
                .failed(*reference)
                .expect("failed private text still owns its product");
            assert!(!output.has_execution_acceptance());
            match failure {
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
                            .any(|note| note == "generic bootstrap stop")
                    );
                    assert!(sources.slice(diagnostics[0].primary).is_some());
                }
                other => panic!("{other:?}"),
            }
            assert!(program.engine.generated_sources().next().is_none());
            assert_eq!(
                program
                    .engine
                    .recompute_count(&program.entry, QueryKind::Check),
                0
            );
            assert!(
                budget.reserved() < 256 << 20,
                "runtime partition released on failure"
            );
            drop(output);
            drop(program);
            assert!(
                budget.reserved() > 0,
                "detached error owns failed payload/sources"
            );
            drop(error);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn bounded_instance_bootstrap_front_failure_keeps_its_admitted_source_owner() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let source = SOURCE.replace(
                "generated(Point(x:1u8)) + generated(Box3(x:2u16)) + generated(Point(x:3u8))",
                "missing()",
            );
            let mut detached = None;
            let mut retained = None;
            fixture(&source, jobs, |program, budget| {
                let definitions = program.engine.front_source_roots();
                let generators = program.engine.front_generator_roots(&definitions).unwrap();
                let error = program
                    .engine
                    .bootstrap_front_instances(
                        &definitions,
                        &[program.entry.clone()],
                        &generators,
                        FrontInstanceBootstrapAdmission {
                            jobs: hirsel_intern::jobs::Jobs::new(jobs),
                            weight: weight(),
                            max_reads: 64,
                            runtime: budget.reserve(256 << 20).unwrap(),
                            front: budget.reserve(32 << 20).unwrap(),
                            lower: budget.reserve(32 << 20).unwrap(),
                            text: budget.reserve(32 << 20).unwrap(),
                            syntax: budget.reserve(32 << 20).unwrap(),
                            discovery: budget.reserve(16 << 20).unwrap(),
                            metadata: budget.reserve(4 << 20).unwrap(),
                        },
                        crate::scheduled_front::errors(),
                    )
                    .err()
                    .expect("unresolved ordinary body");
                let FrontInstanceBootstrapError::Front {
                    error: Some(QueryError::Diagnostics(diagnostics)),
                    sources,
                    owner,
                    ..
                } = &error
                else {
                    panic!("{error:?}")
                };
                assert!(!diagnostics.is_empty());
                assert!(
                    diagnostics
                        .iter()
                        .all(|d| sources.slice(d.primary).is_some())
                );
                assert!(owner.query(&program.entry).is_some());
                assert!(program.engine.generated_sources().next().is_none());
                assert!(budget.reserved() > 0 && budget.reserved() < 256 << 20);
                detached = Some(error);
                retained = Some(budget.clone());
            });
            let budget = retained.unwrap();
            assert!(
                budget.reserved() > 0,
                "detached diagnostics own their admitted front after host drop"
            );
            drop(detached);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn source_requests_install_nested_instance_views_for_check_run_and_native_ownership() {
    use hirsel_driver::{native, requests::*, resident::ObservedSession};
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let directory = tempfile::tempdir().unwrap();
            let manifest = directory.path().join("hirsel.json");
            let source = SOURCE.replace("{7u64}", "{ inner(v) }")
                + "\nfn inner[T](v:T)->u64 uses {} = comptime inner_make(shape(T))\n"
                + &SOURCE[SOURCE.find("fn make").unwrap()..SOURCE.find("fn generated").unwrap()]
                    .replace("make[T]", "inner_make[T]");
            let source = source + "\nfn zzz_unrelated()->u8{1u8}\nfn aaa_unrelated()->u8{2u8}\n";
            std::fs::write(directory.path().join("main.hir"), source).unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"instance_front","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let config = CompileConfig {
                store: Arc::new(MemStore::default()),
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                hold_bytes: 0,
                ..Default::default()
            };
            let compiler = COMPILER.with_config(config.clone()).with_owned_engines();
            let budget = ProcessBudget::new(1 << 30);
            let selector = CompilerRequests::new(
                compiler.clone(),
                config.clone(),
                budget.clone(),
                RequestLimits {
                    runtime_bytes: 512 << 20,
                    snapshot_bytes: 128 << 20,
                    transport_bytes: 32 << 20,
                },
            );
            let limits = selector.input_limits();
            let mut scalar = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    ..config.clone()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let entry = scalar.entry.clone();
            let expected_revision = scalar.engine.semantic_program_revision(&entry).unwrap();
            drop(scalar);
            let mut session = ObservedSession::new(manifest, compiler, config, selector, limits, 3);
            {
                let checked = session
                    .request(CompileOperation::Check {
                        lowering: LoweringPolicy::Reachable,
                    })
                    .unwrap();
                let CompileResult::Check(checked) = &checked.result else {
                    panic!("check result")
                };
                assert_eq!(checked.completion, CheckCompletion::Complete);
                assert!(checked.diagnostics.is_empty());
            }
            {
                let run = session
                    .request(CompileOperation::Tir { definition: None })
                    .unwrap();
                let CompileResult::Tir(run) = &run.result else {
                    panic!("run result")
                };
                assert_eq!(run.revision, expected_revision);
                use hirsel_tir::TirProvider;
                let mut instances = std::collections::BTreeSet::new();
                for name in ["Point", "Box3"] {
                    let mut path = run.entry.clone();
                    path.0.name = name.into();
                    let actual = run.runtime.types.intern(hirsel_types::Type::with_kind(
                        hirsel_types::TypeKind::Named {
                            definition: hirsel_syntax::DefId::intern(&path),
                            arguments: vec![].into(),
                        },
                    ));
                    for name in ["generated", "inner"] {
                        path.0.name = name.into();
                        let instance = run
                            .provider()
                            .instance_body(&path, &[hirsel_types::GenericArgument::Type(actual)])
                            .unwrap()
                            .expect("the closed generated instance is installed");
                        run.provider().definition(&instance).unwrap();
                        instances.insert(instance);
                    }
                }
                assert_eq!(
                    instances.len(),
                    4,
                    "both outer and inner types are installed"
                );
                for checked in [false, true] {
                    let mut runtime = (*run.runtime).clone();
                    runtime.checked = checked.then(|| (*run.checked_runtime).clone());
                    let execution = hirsel_interp::run_with_runtime(
                        &mut run.provider(),
                        &run.sources,
                        &hirsel_tir::RunConfiguration {
                            entry: run.entry.clone(),
                            hash_seed: 0,
                            inputs: vec![],
                        },
                        vec![],
                        hirsel_interp::Limits {
                            steps: Some(10000),
                            call_depth: 256,
                            source: None,
                        },
                        &mut hirsel_interp::NoOperations,
                        &runtime,
                    )
                    .unwrap();
                    assert!(
                        matches!(
                            &execution.outcome,
                            hirsel_interp::Outcome::Returned {
                                value: hirsel_interp::Value::Integer(value),
                                ..
                            } if value.bits() == 21
                        ),
                        "{checked}: {:?}",
                        execution.outcome
                    );
                }
            }
            let ownership_output = session
                .request(CompileOperation::NativeOwnership(OwnershipRequest {
                    selection: hirsel_lir::ownership_trace::Selection::Resources,
                    definition: None,
                    target: native::TargetSpec::x86_64_unknown_linux_gnu(),
                    validation: native::Validation::EveryStage,
                    launch: Default::default(),
                    fuel: false,
                }))
                .unwrap();
            let CompileResult::Ownership(ownership) = &ownership_output.result else {
                panic!("ownership result")
            };
            assert!(!ownership.units().is_empty());
            drop(session);
            assert!(budget.reserved() > 0);
            drop(ownership_output);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
