//! Child-local resources bind to actual native task paths, with no payload identity.
use hirsel_driver::{COMPILER, native, requests::*, resident::ObservedSession};
use hirsel_engine::{CompileConfig, runtime::ProcessBudget, store::MemStore};
use std::sync::Arc;

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_child_local_resources_keep_equal_tokens_in_distinct_tasks() {
    child_local_source(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=task.async(s,fn(){let token=Token()})
  let second=task.async(s,fn(){let token=Token()})
  let _=task.await(s,take first)
  let _=task.await(s,take second)
 })
 Ok(())
}
"#,
        2,
        false,
    );
}

fn child_local_source(source_text: &str, expected_tasks: usize, require_spawn_binding: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        std::fs::write(&source, source_text).unwrap();
        let path = directory.path().join("hirsel.json");
        std::fs::write(&path, r#"{"schema":1,"package":{"name":"ownership","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let grants = std::ffi::OsStr::new(r#"{"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#);
        let mut reference_traces = Vec::new();
        for checked in [false, true] {
            let run = hirsel_driver::ownership_trace::run_resources_on_host(
                &path,
                hirsel_driver::HostLaunch::Live {
                    config: hirsel_driver::LaunchConfig::Grants {
                        input: grants,
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
            assert_eq!(run.outcome.exit_code(), 0);
            let trace = run.trace.unwrap();
            let analysis = trace.analyze(run.program, 1000).unwrap();
            assert_eq!(
                (analysis.created, analysis.dropped, analysis.live.len()),
                (2, 2, 0)
            );
            reference_traces.push(trace);
        }
        assert_eq!(reference_traces[0], reference_traces[1]);
        let expected_paths = reference_traces[0]
            .events
            .iter()
            .map(|event| event.task.clone())
            .collect::<std::collections::BTreeSet<_>>();
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
        let mut session = ObservedSession::new(path, compiler, config, selector, limits, 3);
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let output = session
            .request(CompileOperation::NativeOwnership(OwnershipRequest {
                definition: None,
                target: target.clone(),
                validation: native::Validation::EveryStage,
                launch: Default::default(),
                fuel: false,
            }))
            .unwrap();
        let CompileResult::Ownership(execution) = &output.result else {
            panic!("missing diagnostic output")
        };
        let execution = execution.clone();
        drop(output);
        drop(session);
        let tools = native::Toolchain::in_tree_with_runtime(
            &directory.path().join("tools"),
            &[&target],
            native::RuntimeBuild {
                ownership_trace: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut traces = vec![];
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let objects = execution.emit(backend).unwrap();
            if require_spawn_binding {
                assert!(
                    objects
                        .metadata()
                        .values()
                        .flat_map(|unit| unit.values())
                        .any(|marker| marker.action == 5
                            && marker.empty_environment
                            && marker.callee.is_some()),
                    "missing exact empty-callback source binding"
                );
            }
            let run = execution
                .launch_with(
                    &objects,
                    &tools,
                    &directory.path().join(backend.name()),
                    1000,
                    hirsel_driver::HostLaunch::Live {
                        config: hirsel_driver::LaunchConfig::Grants {
                            input: std::ffi::OsStr::new(
                                r#"{"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#,
                            ),
                            arguments: None,
                        },
                        record: None,
                        revocations: None,
                    },
                )
                .unwrap();
            if require_spawn_binding {
                use hirsel_run_schema::ownership_trace::native::{Log, Row};
                let rows: Log = serde_json::from_slice(
                    &std::fs::read(
                        directory
                            .path()
                            .join(backend.name())
                            .join("ownership.rows.json"),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(
                    rows.rows
                        .iter()
                        .filter(|row| matches!(row, Row::Spawn { .. }))
                        .count(),
                    1,
                    "actual spawn commitment is absent"
                );
                assert_eq!(
                    rows.rows
                        .iter()
                        .filter(|row| matches!(row, Row::Event { action: 5, .. }))
                        .count(),
                    1,
                    "checked empty-callback intent is absent"
                );
            }
            assert_eq!(run.execution.exit, Some(0));
            assert_eq!(
                execution
                    .observe_run(&objects, &run.execution)
                    .unwrap()
                    .exit_code(),
                0
            );
            assert!(run.execution.stdout.is_empty());
            assert!(
                run.execution.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&run.execution.stderr)
            );
            let trace = run.trace.unwrap();
            let analysis = trace.analyze(run.program, 1000).unwrap();
            assert_eq!(
                (analysis.created, analysis.dropped, analysis.live.len()),
                (2, 2, 0)
            );
            let paths = trace
                .events
                .iter()
                .map(|event| event.task.clone())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(paths.len(), expected_tasks);
            assert_eq!(paths, expected_paths);
            assert!(paths.iter().all(|path| !path.0.is_empty()));
            traces.push(trace);
        }
        assert_eq!(traces[0], traces[1]);
        let lifecycle = |trace: &hirsel_run_schema::ownership_trace::Trace| {
            trace
                .events
                .iter()
                .filter_map(|event| match &event.action {
                    hirsel_run_schema::ownership_trace::Action::Create {
                        token,
                        value_type,
                        ..
                    } => Some((0, token.clone(), Some(value_type.clone()))),
                    hirsel_run_schema::ownership_trace::Action::Drop { token, .. } => {
                        Some((1, token.clone(), None))
                    }
                    hirsel_run_schema::ownership_trace::Action::Move { .. } => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(lifecycle(&reference_traces[0]), lifecycle(&traces[0]));
        drop(execution);
        assert_eq!(budget.reserved(), 0);
    });
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_empty_callback_binding_keeps_actual_spawn_and_child_local_resource_lifetimes() {
    child_local_source(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let child=task.async(s,fn(){let first=Token()
   let second=Token()})
  let _=task.await(s,take child)
 })
 Ok(())
}
"#,
        1,
        true,
    );
}
