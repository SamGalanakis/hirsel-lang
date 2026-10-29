//! Captured resources transfer at the actual committed child creation.
use hirsel_driver::{COMPILER, native, requests::*, resident::ObservedSession};
use hirsel_engine::{CompileConfig, runtime::ProcessBudget, store::MemStore};
use std::sync::Arc;

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_spawn_transfers_two_equal_resources_to_its_actual_child() {
    capture_source(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=Token()
  let second=Token()
  let child=task.async(s,fn(){
    let x=take first
    let y=take second
  })
  let _=task.await(s,take child)
 })
 Ok(())
}
"#,
        0,
    );
}

fn capture_source(source_text: &str, expected_exit: i32) {
    capture_with_tasks(source_text, expected_exit, "tasks.cooperative@1");
}

fn capture_with_tasks(source_text: &str, expected_exit: i32, tasks: &str) {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        std::fs::write(&source, source_text).unwrap();
        let path = directory.path().join("hirsel.json");
        std::fs::write(&path, r#"{"schema":1,"package":{"name":"ownership","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let grants_text = format!(r#"{{"io":{{"tasks":"{tasks}","dirs":[]}}}}"#);
        let grants = std::ffi::OsStr::new(&grants_text);
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
            assert_eq!(run.outcome.exit_code(), expected_exit);
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
                selection: hirsel_lir::ownership_trace::Selection::Resources,
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
            let run = execution
                .launch_with(
                    &objects,
                    &tools,
                    &directory.path().join(backend.name()),
                    1000,
                    hirsel_driver::HostLaunch::Live {
                        config: hirsel_driver::LaunchConfig::Grants {
                            input: grants,
                            arguments: None,
                        },
                        record: None,
                        revocations: None,
                    },
                )
                .unwrap();
            assert_eq!(run.execution.exit, Some(expected_exit));
            assert_eq!(
                execution
                    .observe_run(&objects, &run.execution)
                    .unwrap()
                    .exit_code(),
                expected_exit
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
            assert_eq!(paths.len(), 2);
            assert_eq!(paths, expected_paths);
            assert_eq!(paths.iter().filter(|path| !path.0.is_empty()).count(), 1);
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
fn child_failure_before_taking_equal_captures_retires_each_environment_field() {
    capture_source(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=Token()
  let second=Token()
  let child=task.async(s,fn(){
    let zero:u8=0u8
    let _=1u8/zero
    let x=take first
    let y=take second
  })
  let _=task.await(s,take child)
 })
 Ok(())
}
"#,
        hirsel_run_schema::exit::FAILED,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn failed_finalizers_retire_both_equal_captures_in_the_actual_child() {
    capture_source(
        r#"
resource type Token {}
drop(x) { let zero:u8=0u8
 let _=1u8/zero }
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=Token()
  let second=Token()
  let child=task.async(s,fn(){
    let x=take first
    let y=take second
  })
  let _=task.await(s,take child)
 })
 Ok(())
}
"#,
        hirsel_run_schema::exit::FAILED,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_free_spawn_transfers_equal_resources_and_retires_the_actual_child() {
    capture_with_tasks(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=Token()
  let second=Token()
  let child=task.async(s,fn(){
    let x=take first
    let y=take second
  })
  let _=task.await(s,take child)
 })
 Ok(())
}
"#,
        0,
        "tasks.free@1",
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_free_failed_finalizers_retain_both_actual_capture_lifetimes() {
    capture_with_tasks(
        r#"
resource type Token {}
drop(x) { let zero:u8=0u8
 let _=1u8/zero }
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=Token()
  let second=Token()
  let child=task.async(s,fn(){
    let x=take first
    let y=take second
  })
  let _=task.await(s,take child)
 })
 Ok(())
}
"#,
        hirsel_run_schema::exit::FAILED,
        "tasks.free@1",
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn canceling_child_before_taking_captures_retires_both_equal_resources() {
    for tasks in ["tasks.cooperative@1", "tasks.free@1"] {
        capture_with_tasks(
            r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=Token()
  let second=Token()
  let (ready_tx,ready_rx):(Sender[u8],Receiver[u8])=channel.open(tasks)
  let (gate_tx,gate_rx):(Sender[u8],Receiver[u8])=channel.open(tasks)
  let child=task.async(s,fn(){
    let mut ready=take ready_tx
    let _=channel.send(tasks,mut ready,7u8)
    let mut gate=take gate_rx
    let _=channel.receive(tasks,mut gate)
    let x=take first
    let y=take second
  })
  let mut ready=take ready_rx
  let _=channel.receive(tasks,mut ready)
  task.cancel(s,child)
  let _=task.await(s,take child)
  let _=gate_tx
 })
 Ok(())
}
"#,
            0,
            tasks,
        );
    }
}
