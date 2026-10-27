//! Generated isolate cleanup keeps its closure and protected result until catch.
use super::runtime50_continuation::ready;
use super::runtime50_resume::{compact_runs_with, exact_compact_custody};
use hirsel_driver::native::{self, Backend, Partition};
use hirsel_engine::with_compiler_stack;

const CAPTURE_RESULT: &str = r#"
resource type Guard { output: Stdout, n: u64 }
drop(x) { let _ = out.print(x.output, "x"); failure.check(false, Bounds()) }
resource type Returned { output: Stdout, n: u64 }
drop(x) { let _ = out.print(x.output, "x"); failure.check(false, Arithmetic()) }
fn held(tasks: Tasks, output: Stdout, seed: u64) -> u64 {
  let guard = Guard(output: output, n: seed)
  let status: Result[Returned, Failure] = task.isolate(tasks, fn() -> Returned {
    if guard.n == 0u64 { let _ = take guard }
    let _ = out.print(output, "x")
    Returned(output: output, n: seed)
  })
  match status {
    Ok(value) => { failure.check(false, Bounds()); value.n },
    Error(f) => {
      failure.check(f.rule == "MEM-14", Arithmetic())
      match f.cause {
        Some(cause) => failure.check(cause.rule == "SEM-1" && cause.cause == None(), Bounds()),
        None => failure.check(false, Bounds()),
      }
      seed
    },
  }
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let total = task.scope(tasks, fn(s: Scope) -> u64 {
    let mut set: TaskSet[Result[u64, ()]] of s = task.set(s)
    for seed in 1u64..5u64 {
      task.add(s, mut set, task.async(s, fn() -> Result[u64, ()] { Ok(held(tasks, output, seed)) }))
    }
    let mut total = 0u64
    if let Ok(rows) = std@1/task.all(s, take set) { for row in rows { total = total + row } }
    total
  })
  failure.check(total == 10u64, Arithmetic())
  Ok(())
}
"#;

fn physical(tasks: &str) {
    with_compiler_stack(|| {
        let program = ready(CAPTURE_RESULT);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let wrappers: Vec<_> = plans
            .plans
            .values()
            .filter(|p| p.isolate.is_some())
            .collect();
        assert!(
            !wrappers.is_empty(),
            "no isolate cleanup admitted: {:#?}",
            plans.plans
        );
        assert!(
            wrappers.iter().any(|p| p.cleanups.iter().any(|c| c
                .finalizers
                .as_ref()
                .is_some_and(|f| !f.finalizers.is_empty()))),
            "capture finalizer has no retained source frame"
        );
        for (index, trace) in
            compact_runs_with(CAPTURE_RESULT, "capture and protected result", true, tasks)
                .iter()
                .enumerate()
        {
            assert!(exact_compact_custody(trace, &format!("{tasks} {index}")) > 0);
            let finalizers: Vec<_> = trace
                .iter()
                .enumerate()
                .filter(|(_, row)| row.0 == "finalizer" && row.1 != 0)
                .collect();
            assert!(
                !finalizers.is_empty(),
                "{tasks} {index}: cleanup never returned Pending"
            );
            for (at, row) in finalizers {
                assert!(
                    trace[at + 1..]
                        .iter()
                        .any(|next| next.0 == "leave" && next.1 == row.1 && next.4),
                    "{tasks} {index}: cleanup kept its stack"
                );
            }
        }
    });
}

#[test]
fn isolate_cleanup_capture_and_protected_result_leave_and_replay_free() {
    physical("tasks.free@1");
}
#[test]
fn isolate_cleanup_capture_and_protected_result_leave_and_replay_ordered() {
    physical("tasks.cooperative@1");
}
#[test]
fn isolate_cleanup_capture_and_protected_result_checked_heap_and_partitions() {
    with_compiler_stack(|| {
        super::runtime50_resume::checked_heap_agrees(CAPTURE_RESULT);
        let program = ready(CAPTURE_RESULT);
        for backend in [Backend::Llvm, Backend::Cranelift] {
            for partition in [Partition::Whole, Partition::Singleton] {
                let (exported, entered) =
                    super::runtime50_resume::routing(&program, backend, partition);
                assert!(exported > 0 && entered, "{backend:?} {partition:?}");
            }
        }
    });
}

const CANCELED: &str = r#"
resource type Guard { output: Stdout, n: u64 }
drop(x) { let _ = out.print(x.output, "drop\n"); failure.check(false, Bounds()) }
fn child_body(tasks: Tasks, output: Stdout, take tx: Sender[u64]) -> u64 {
  let guard = Guard(output: output, n: 7u64)
  let status: Result[u64, Failure] = task.isolate(tasks, fn() -> u64 {
    if guard.n == 0u64 { let _ = take guard }
    let mut sender = take tx
    let _ = channel.send(tasks, mut sender, 3u64)
    let _ = channel.send(tasks, mut sender, 5u64)
    7u64
  })
  match status { Ok(n) => n, Error(_) => 11u64 }
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let total = task.scope(tasks, fn(s: Scope) -> u64 {
    let (sender, receiver): (Sender[u64], Receiver[u64]) = channel.open(tasks)
    let mut rx = take receiver
    let child = task.async(s, fn() -> u64 { child_body(tasks, output, take sender) })
    let first = match channel.receive(tasks, mut rx) { Ok(n) => n, Error(_) => 0u64 }
    task.cancel(s, child)
    first + task.await(s, take child)
  })
  let _ = out.print(output, f"{total}\n")
  Ok(())
}
"#;
#[test]
fn isolate_cleanup_cancellation_preserves_failure_and_capture_disposal() {
    with_compiler_stack(|| {
        eprintln!("isolate cancellation: source and plans");
        let program = ready(CANCELED);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        assert!(plans.plans.values().any(|p| p.isolate.is_some()));
        eprintln!("isolate cancellation: reference, heap, native and replay");
        super::runtime50_resume::agree_on(
            CANCELED,
            &native::Fixture::default(),
            "isolate canceled",
            true,
            &[false],
        );
    });
}

fn canceled_physical(tasks: &str) {
    with_compiler_stack(|| {
        for (index, trace) in compact_runs_with(CANCELED, "isolate canceled", true, tasks)
            .iter()
            .enumerate()
        {
            assert!(exact_compact_custody(trace, &format!("cancel {tasks} {index}")) > 0);
        }
    });
}

#[test]
fn isolate_cleanup_cancellation_physically_leaves_and_replays_free() {
    canceled_physical("tasks.free@1");
}

#[test]
fn isolate_cleanup_cancellation_physically_leaves_and_replays_ordered() {
    canceled_physical("tasks.cooperative@1");
}

#[test]
fn isolate_cleanup_source_and_cursor_forgeries_refuse_both_emitters() {
    with_compiler_stack(|| {
        use hirsel_lir::emission::{EmitInputs, OptLevel};
        let program = ready(CAPTURE_RESULT);
        let units: Vec<_> = program.units.iter().collect();
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let (&body, original) = plans
            .plans
            .iter()
            .find(|(_, p)| p.isolate.is_some())
            .unwrap();
        let ordinal = original
            .cleanups
            .iter()
            .position(|c| {
                c.finalizers
                    .as_ref()
                    .is_some_and(|f| !f.finalizers.is_empty())
            })
            .unwrap();
        let emit = |proposed: &hirsel_lir::continuation::ContinuationPlans, accepted| {
            let empty = std::collections::BTreeSet::new();
            let inputs = EmitInputs {
                ahead: None,
                continuation: Some(proposed),
                alternate: false,
                index: None,
                input: &program.input,
                target: &program.target,
                layouts: &program.table,
                keys: &program.keys,
                manifest: &program.manifest,
                statics: &program.statics,
                inline_instances: &program.inline_instances,
                available_externally: &empty,
                callbacks: !program.callbacks.is_empty(),
                tracking: program.tracking(),
                foreign: &program.foreign_dispatch,
                opt: OptLevel::Default,
                measurement: false,
                fuel: program.fuel,
                verify_backend: true,
            };
            assert_eq!(
                hirsel_llvm::emit_resume_object(&units, &inputs, proposed).is_ok(),
                accepted,
                "LLVM"
            );
            assert_eq!(
                hirsel_cranelift::emit_resume_object(&units, &inputs, proposed).is_ok(),
                accepted,
                "Cranelift"
            );
        };
        emit(&plans, true);
        for mutation in 0..11 {
            let mut bad = plans.clone();
            let wrapper = bad.plans.get_mut(&body).unwrap();
            match mutation {
                0 => wrapper
                    .isolate
                    .as_mut()
                    .unwrap()
                    .cleanup
                    .sites
                    .pop()
                    .map(|_| ())
                    .unwrap(),
                1 => wrapper.cleanups[ordinal].finalizers = None,
                2 => wrapper.cleanups[ordinal]
                    .finalizers
                    .as_mut()
                    .unwrap()
                    .finalizers
                    .clear(),
                3 => {
                    wrapper.cleanups[ordinal]
                        .finalizers
                        .as_mut()
                        .unwrap()
                        .finalizers[0]
                        .nested
                        .offset = 0
                }
                4 => {
                    wrapper.cleanups[ordinal].finalizers.as_mut().unwrap().root =
                        hirsel_lir::layout_types::LayoutId(u32::MAX)
                }
                5 => {
                    wrapper
                        .isolate
                        .as_mut()
                        .unwrap()
                        .cleanup
                        .sites
                        .iter_mut()
                        .find(|s| s.environment.is_some())
                        .unwrap()
                        .environment = None
                }
                6 => {
                    let value = wrapper.cleanups[ordinal].value;
                    wrapper.cleanups[ordinal].live.retain(|v| v.value != value);
                }
                7 => wrapper.isolate.as_mut().unwrap().cleanup.finalizers.clear(),
                8 => wrapper.isolate.as_mut().unwrap().callback_bodies.clear(),
                9 => wrapper.cleanups[ordinal]
                    .finalizers
                    .as_mut()
                    .unwrap()
                    .layouts
                    .pop()
                    .map(|_| ())
                    .unwrap(),
                10 => {
                    let coverage = wrapper.cleanups[ordinal].finalizers.as_mut().unwrap();
                    let source = coverage.finalizers.remove(0).source;
                    coverage.synchronous.push(source);
                }
                _ => unreachable!(),
            }
            assert!(
                native::continuation::validate(&program, &bad).is_err(),
                "mutation {mutation}"
            );
            emit(&bad, false);
        }
    });
}

#[test]
fn isolate_cleanup_source_edits_invalidate_cached_continuation_evidence() {
    with_compiler_stack(|| {
        let original = ready(CAPTURE_RESULT);
        let original_plans = native::continuation::plans(&original);
        native::continuation::validate(&original, &original_plans).unwrap();
        assert!(original_plans.plans.values().any(|p| p.isolate.is_some()));
        let changed = ready(&CAPTURE_RESULT.replacen(
            "failure.check(false, Bounds())",
            "failure.check(false, Arithmetic())",
            1,
        ));
        let changed_plans = native::continuation::plans(&changed);
        native::continuation::validate(&changed, &changed_plans).unwrap();
        assert!(changed_plans.plans.values().any(|p| p.isolate.is_some()));
        assert_ne!(original_plans.identity, changed_plans.identity);
        assert!(native::continuation::validate(&changed, &original_plans).is_err());
        assert!(native::continuation::validate(&original, &changed_plans).is_err());
    });
}

const STOP_PHASES: &str = r#"
fn block(tasks: Tasks) {
  let (sender, receiver): (Sender[u64], Receiver[u64]) = channel.open(tasks)
  let mut rx = take receiver
  let _ = channel.receive(tasks, mut rx)
  let _ = take sender
}
resource type Guard { tasks: Tasks, output: Stdout, n: u64 }
drop(x) {
  let _ = out.print(x.output, "capture\n")
  CAPTURE_STOP
  failure.check(false, Bounds())
}
resource type Returned { tasks: Tasks, output: Stdout, n: u64 }
drop(x) {
  let _ = out.print(x.output, "result\n")
  RESULT_STOP
  failure.check(false, Arithmetic())
}
fn held(tasks: Tasks, output: Stdout, seed: u64) -> u64 {
  BEFORE_BEGIN
  let guard = Guard(tasks: tasks, output: output, n: seed)
  let status: Result[Returned, Failure] = task.isolate(tasks, fn() -> Returned {
    CALLBACK_ENTRY
    if guard.n == 0u64 { let _ = take guard }
    let _ = out.print(output, "body\n")
    CALLBACK_STOP
    Returned(tasks: tasks, output: output, n: seed)
  })
  match status {
    Ok(value) => { failure.check(false, Bounds()); value.n },
    Error(f) => {
      let _ = out.print(output, "catch\n")
      CATCH_STOP
      failure.check(f.rule == "MEM-14", Arithmetic())
      seed
    },
  }
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let _ = task.scope(tasks, fn(s: Scope) -> u64 {
    let child = task.async(s, fn() -> u64 { held(tasks, output, 7u64) })
    task.await(s, take child)
  })
  let _ = out.print(output, "returned\n")
  RETURN_STOP
  Ok(())
}
"#;

fn stop_source(phase: usize) -> (String, &'static [u8]) {
    let sites = [
        ("BEFORE_BEGIN", "block(tasks)", b"".as_slice()),
        ("CALLBACK_ENTRY", "block(tasks)", b"".as_slice()),
        ("CALLBACK_STOP", "block(tasks)", b"body\n".as_slice()),
        (
            "CAPTURE_STOP",
            "block(x.tasks)",
            b"body\ncapture\n".as_slice(),
        ),
        (
            "RESULT_STOP",
            "block(x.tasks)",
            b"body\ncapture\nresult\n".as_slice(),
        ),
        (
            "CATCH_STOP",
            "block(tasks)",
            b"body\ncapture\nresult\ncatch\n".as_slice(),
        ),
        (
            "RETURN_STOP",
            "block(tasks)",
            b"body\ncapture\nresult\ncatch\nreturned\n".as_slice(),
        ),
    ];
    let mut source = STOP_PHASES.to_owned();
    for (index, (marker, blocked, _)) in sites.iter().enumerate() {
        source = source.replace(marker, if phase == index { blocked } else { "" });
    }
    (source, sites[phase].2)
}

fn stopped(phase: usize) {
    with_compiler_stack(|| {
        let (source, prefix) = stop_source(phase);
        let program = ready(&source);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        assert!(plans.plans.values().any(|p| p.isolate.is_some()));
        let observation = super::runtime50_resume::agree_on(
            &source,
            &native::Fixture {
                migrate: true,
                ..Default::default()
            },
            &format!("isolate phase {phase} terminal stop"),
            true,
            &[false],
        );
        assert!(observation.blocked);
        assert_eq!(observation.exit_code, None);
        assert_eq!(observation.output, prefix);
    });
}

#[test]
fn isolate_stop_before_begin() {
    stopped(0);
}
#[test]
fn isolate_stop_at_callback_entry() {
    stopped(1);
}
#[test]
fn isolate_stop_during_callback() {
    stopped(2);
}
#[test]
fn isolate_stop_during_capture_cleanup() {
    stopped(3);
}
#[test]
fn isolate_stop_during_protected_result_cleanup() {
    stopped(4);
}
#[test]
fn isolate_stop_in_caught_continuation() {
    stopped(5);
}
#[test]
fn isolate_stop_after_return() {
    stopped(6);
}

fn canceled_receive_source() -> String {
    CANCELED.replace(
        "    let _ = channel.send(tasks, mut sender, 5u64)",
        r#"    let (kept, waiting): (Sender[u64], Receiver[u64]) = channel.open(tasks)
    let mut rx = take waiting
    let canceled = channel.receive(tasks, mut rx) == Error(Canceled(Requested()))
    failure.check(canceled, Arithmetic())
    let refused = out.print(output, "unreachable\n") == Error(Canceled(Requested()))
    failure.check(refused, Arithmetic())
    let _ = take kept"#,
    )
    .replace(
        "match status { Ok(n) => n, Error(_) => 11u64 }",
        "match status { Ok(n) => n, Error(f) => { failure.check(f.rule == \"MEM-14\" && f.cause == None(), Arithmetic()); 11u64 } }",
    )
}

#[test]
fn isolate_cancellation_is_observed_and_capture_drop_remains_shielded() {
    with_compiler_stack(|| {
        let observation = super::runtime50_resume::agree_on(
            &canceled_receive_source(),
            &native::Fixture {
                migrate: true,
                ..Default::default()
            },
            "isolate observes cancellation before shielded capture cleanup",
            true,
            &[false],
        );
        assert_eq!(observation.output, b"drop\n14\n");
        assert_eq!(observation.exit_code, Some(0));
        assert!(observation.failures.is_empty());
    });
}

fn canceled_receive_physical(tasks: &str) {
    with_compiler_stack(|| {
        for (cell, trace) in compact_runs_with(
            &canceled_receive_source(),
            "isolate observed cancellation",
            true,
            tasks,
        )
        .iter()
        .enumerate()
        {
            assert!(exact_compact_custody(trace, &format!("observed cancel {tasks} {cell}")) > 0);
        }
    });
}

#[test]
fn isolate_observed_cancellation_leaves_and_replays_free() {
    canceled_receive_physical("tasks.free@1");
}

#[test]
fn isolate_observed_cancellation_leaves_and_replays_ordered() {
    canceled_receive_physical("tasks.cooperative@1");
}
