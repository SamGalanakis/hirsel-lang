//! Original Value plans across graph-owned source, call transport and execution.
use hirsel_driver::{COMPILER, native, requests::*, resident::ObservedSession};
use hirsel_engine::{CompileConfig, runtime::ProcessBudget, store::MemStore};
use hirsel_run_schema::ownership_trace::{Action, Trace};
use std::sync::Arc;

pub(super) fn source_calls(source: &str, created: u64) {
    source_calls_exit(source, created, 0);
}

pub(super) fn source_calls_exit(source: &str, created: u64, expected_exit: i32) {
    source_calls_granted(source, created, expected_exit, "{}");
}

pub(super) fn source_calls_granted(source: &str, created: u64, expected_exit: i32, grants: &str) {
    source_calls_granted_with_log(source, created, expected_exit, grants, |_| {});
}

pub(super) fn source_calls_granted_with_log(
    source: &str,
    created: u64,
    expected_exit: i32,
    grants: &str,
    observe: impl Fn(&hirsel_run_schema::ownership_trace::native::Log) + Send,
) {
    source_calls_selected(
        source,
        created,
        expected_exit,
        grants,
        hirsel_lir::ownership_trace::Selection::Resources,
        observe,
    );
}

pub(super) fn source_owned_leaves(source: &str, created: u64) {
    source_calls_selected(
        source,
        created,
        0,
        "{}",
        hirsel_lir::ownership_trace::Selection::Owned,
        |_| {},
    );
}

fn source_calls_selected(
    source: &str,
    created: u64,
    expected_exit: i32,
    grants: &str,
    selection: hirsel_lir::ownership_trace::Selection,
    observe: impl Fn(&hirsel_run_schema::ownership_trace::native::Log) + Send,
) {
    hirsel_engine::with_compiler_stack(move || {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.hir"), source).unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"ownership","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let mut traces = Vec::new();
        let mut outcomes = Vec::new();
        for checked in [false, true] {
            let run_host = if selection == hirsel_lir::ownership_trace::Selection::Owned {
                hirsel_driver::ownership_trace::run_owned_on_host
            } else {
                hirsel_driver::ownership_trace::run_resources_on_host
            };
            let host = run_host(
                &manifest,
                hirsel_driver::HostLaunch::Live {
                    config: hirsel_driver::LaunchConfig::Grants {
                        input: std::ffi::OsStr::new(grants),
                        arguments: None,
                    },
                    record: None,
                    revocations: None,
                },
                Some(100_000),
                hirsel_engine::Verify::On,
                checked,
                1000,
            )
            .unwrap();
            assert_eq!(host.outcome.exit_code(), expected_exit);
            if expected_exit == hirsel_run_schema::exit::FAILED {
                let hirsel_run_schema::RunOutcome::Failed { primary, causes } = &host.outcome
                else {
                    panic!("expected actual language failure")
                };
                assert_eq!(primary.rule, "SEM-1");
                assert!(causes.is_empty());
            }
            outcomes.push(host.outcome);
            traces.push(host.trace.unwrap());
        }
        assert_eq!(traces[0], traces[1]);
        assert_eq!(outcomes[0], outcomes[1]);
        let config = CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::ONE,
            store: Arc::new(MemStore::default()),
            hold_bytes: 0,
            ..Default::default()
        };
        let budget = ProcessBudget::new(1 << 30);
        let compiler = COMPILER.with_config(config.clone()).with_owned_engines();
        let selector = CompilerRequests::new(
            compiler.clone(),
            config.clone(),
            budget.clone(),
            RequestLimits {
                runtime_bytes: 256 << 20,
                snapshot_bytes: 64 << 20,
                transport_bytes: 32 << 20,
            },
        );
        let limits = selector.input_limits();
        let mut host = ObservedSession::new(manifest, compiler, config, selector, limits, 3);
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let output = host
            .request(CompileOperation::NativeOwnership(OwnershipRequest {
                selection,
                definition: None,
                target: target.clone(),
                validation: native::Validation::EveryStage,
                launch: Default::default(),
                fuel: false,
            }))
            .unwrap();
        let CompileResult::Ownership(execution) = &output.result else {
            panic!("missing ownership output");
        };
        let execution = execution.clone();
        for product in execution.units() {
            execution
                .with_unit((*product).clone(), |unit| {
                    let native = hirsel_lir::ownership_trace::native::LeafPlan::from_relocated(
                        &unit.unit,
                        &unit.layouts,
                        &unit.plan,
                    )?;
                    for observation in &unit.plan.observations {
                        if observation.marker.site.source == u32::MAX {
                            assert!(
                                native
                                    .implicit_parameter_cleanup_source(
                                        unit.unit.unit(),
                                        observation.marker.id
                                    )
                                    .is_some(),
                                "missing exact parameter cleanup relation: {observation:?}"
                            );
                        }
                    }
                    assert_eq!(unit.recipes.len(), 10);
                    assert_eq!(unit.recipes[0].before, unit.initial.unit);
                    assert_eq!(unit.recipes.last().unwrap().after, unit.plan.unit);
                    Ok(())
                })
                .unwrap();
        }
        drop(output);
        drop(host);
        let runtime = native::RuntimeBuild {
            ownership_trace: true,
            ..Default::default()
        };
        let tools = native::Toolchain::in_tree_with_runtime(
            &directory.path().join("tools"),
            &[&target],
            runtime,
        )
        .unwrap();
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let objects = execution
                .emit(backend)
                .unwrap_or_else(|error| panic!("{backend:?} original plan emission: {error}"));
            let run = execution
                .launch_with(
                    &objects,
                    &tools,
                    &directory.path().join(backend.name()),
                    1000,
                    hirsel_driver::HostLaunch::Live {
                        config: hirsel_driver::LaunchConfig::Grants {
                            input: std::ffi::OsStr::new(grants),
                            arguments: None,
                        },
                        record: None,
                        revocations: None,
                    },
                )
                .unwrap();
            assert_eq!(
                run.execution.exit,
                Some(expected_exit),
                "{backend:?}: {:#?}",
                run.execution
            );
            let outcome = execution.observe_run(&objects, &run.execution).unwrap();
            assert_eq!(
                outcome, outcomes[0],
                "{backend:?} ordinary source failure/outcome"
            );
            assert!(run.execution.stdout.is_empty());
            assert!(
                run.execution.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&run.execution.stderr)
            );
            let raw = std::fs::read(
                directory
                    .path()
                    .join(backend.name())
                    .join("ownership.rows.json"),
            )
            .unwrap();
            observe(&serde_json::from_slice(&raw).unwrap());
            let trace = run
                .trace
                .unwrap_or_else(|error| panic!("{backend:?} original plan trace: {error}"));
            let analysis = trace.analyze(run.program, 1000).unwrap();
            assert_eq!(
                (analysis.created, analysis.dropped, analysis.live.len()),
                (created, created, 0)
            );
            traces.push(trace);
        }
        assert_eq!(traces[2], traces[3]);
        let lifecycle = |trace: &Trace| {
            trace
                .events
                .iter()
                .filter_map(|event| match &event.action {
                    Action::Create {
                        token, value_type, ..
                    } => Some((
                        0,
                        token.clone(),
                        (selection == hirsel_lir::ownership_trace::Selection::Resources)
                            .then(|| value_type.clone()),
                    )),
                    Action::Drop { token, .. } => Some((1, token.clone(), None)),
                    Action::Move { .. } => None,
                })
                .collect::<Vec<_>>()
        };
        for trace in &traces[1..] {
            assert_eq!(lifecycle(&traces[0]), lifecycle(trace));
        }
        drop(execution);
        assert_eq!(budget.reserved(), 0);
    });
}
