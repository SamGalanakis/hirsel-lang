//! The admitted source program runs independently of its resource diagnostics.
use hirsel_driver::ownership_trace::run_resources_on_host;
use hirsel_driver::{HostLaunch, LaunchConfig};
use hirsel_engine::Verify;

#[test]
fn ownership_host_reference_and_checked_h_agree_on_real_source_lifetimes() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("source fixture: {error}"));
    let source = dir.path().join("main.hir");
    std::fs::write(&source, "resource type Token {}\ndrop(x) {}\n\nfn identity(take token: Token) -> Token { take token }\npub fn main() -> Result[(), ()] {\n let first = Token()\n let second = identity(take first)\n let third = take second\n Ok(())\n}\n").unwrap_or_else(|error| panic!("source fixture: {error}"));
    let run = |checked| {
        run_resources_on_host(
            &source,
            HostLaunch::Live {
                config: LaunchConfig::Grants {
                    input: std::ffi::OsStr::new("{}"),
                    arguments: None,
                },
                record: None,
                revocations: None,
            },
            Some(100_000),
            Verify::On,
            checked,
            100,
        )
        .unwrap()
    };
    let reference = run(false);
    let checked = run(true);
    assert_eq!(reference.outcome, checked.outcome);
    assert_eq!(reference.program, checked.program);
    let trace = reference
        .trace
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    assert_eq!(trace, checked.trace.unwrap());
    let analysis = trace
        .analyze(reference.program, 100)
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert!(
        trace
            .events
            .iter()
            .all(|e| e.site.file.ends_with("main.hir"))
    );
    assert!(trace.events.iter().any(|e| matches!(
        e.action,
        hirsel_run_schema::ownership_trace::Action::Move { .. }
    )));
}

#[test]
fn ownership_host_trace_overflow_does_not_change_the_source_outcome() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("source fixture: {error}"));
    let source = dir.path().join("main.hir");
    std::fs::write(&source,"resource type Token {}\ndrop(x) {}\npub fn main() -> Result[(), ()] { let token = Token()\n Ok(()) }\n").unwrap_or_else(|error| panic!("source fixture: {error}"));
    let launch = || HostLaunch::Live {
        config: LaunchConfig::Grants {
            input: std::ffi::OsStr::new("{}"),
            arguments: None,
        },
        record: None,
        revocations: None,
    };
    let ordinary = hirsel_driver::run_path_on_host(&source, launch(), Some(100_000), Verify::On)
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    let traced = run_resources_on_host(&source, launch(), Some(100_000), Verify::On, false, 0)
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    assert_eq!(ordinary, traced.outcome);
    assert!(traced.trace.is_err());
}

#[test]
fn ownership_host_plain_builtin_calls_preserve_live_resource_custody_in_both_engines() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("main.hir");
    std::fs::write(&source, "resource type Token {}\ndrop(x) {}\npub fn main() -> Result[(), ()] {\n let token = Token()\n let n = string.byte_len(\"abc\")\n if n == 3u64 { Ok(()) } else { Error(()) }\n}\n").unwrap();
    let mut traces = vec![];
    for checked in [false, true] {
        let launch = || HostLaunch::Live {
            config: LaunchConfig::Grants {
                input: std::ffi::OsStr::new("{}"),
                arguments: None,
            },
            record: None,
            revocations: None,
        };
        let ordinary =
            hirsel_driver::run_path_on_host(&source, launch(), Some(100_000), Verify::On)
                .unwrap_or_else(|error| panic!("source fixture: {error}"));
        let traced =
            run_resources_on_host(&source, launch(), Some(100_000), Verify::On, checked, 100)
                .unwrap_or_else(|error| panic!("source fixture: {error}"));
        assert_eq!(traced.outcome, ordinary);
        let trace = traced.trace.unwrap();
        let analysis = trace.analyze(traced.program, 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (1, 1, 0)
        );
        traces.push(trace);
    }
    assert_eq!(traces[0], traces[1]);
}

#[test]
fn ownership_simulation_test_entry_preserves_source_results_and_restores_the_adapter() {
    use hirsel_driver::ownership_trace::execute_test_resources;
    use hirsel_driver::{
        COMPILER,
        testing::{self, Selection},
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tests.hir");
    std::fs::write(&source,"resource type Token {}\ndrop(x) {}\n@test\nfn resource_lifetime(t: Test) {\n let first = Token()\n let second = take first\n test.check(t, true)\n}\n").unwrap();
    let mut program = COMPILER
        .with_verify(Verify::On)
        .load_test_package(&source)
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    let tests = testing::discover(&mut program, &[], Some(49), Selection::Tests)
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    assert_eq!(tests.len(), 1);
    let mut traces = vec![];
    for checked in [false, true] {
        let ordinary = testing::execute(&mut program, &tests[0], checked, 0)
            .unwrap_or_else(|error| panic!("source fixture: {error}"));
        let traced = execute_test_resources(
            &mut program,
            &tests[0],
            checked,
            0,
            hirsel_interp::Limits::default(),
            100,
        )
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
        assert_eq!(
            ordinary.outcome.as_ref().unwrap(),
            traced.execution.outcome.as_ref().unwrap()
        );
        assert_eq!(ordinary.observation, traced.execution.observation);
        assert_eq!(ordinary.step_usage, traced.execution.step_usage);
        let trace = traced.trace.unwrap();
        let analysis = trace.analyze(traced.program, 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (1, 1, 0)
        );
        assert!(
            program
                .engine
                .adapters
                .interpreter
                .as_mut()
                .unwrap()
                .take_resource_trace()
                .is_none()
        );
        traces.push(trace);
    }
    assert_eq!(traces[0], traces[1]);
}

fn observed_task_source(body: &str, creates: usize) -> hirsel_run_schema::ownership_trace::Trace {
    use hirsel_driver::ownership_trace::execute_test_resources;
    use hirsel_driver::{
        COMPILER,
        testing::{self, Selection},
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tasks.hir");
    std::fs::write(&source, body).unwrap();
    let mut program = COMPILER
        .with_verify(Verify::On)
        .load_test_package(&source)
        .unwrap_or_else(|error| {
            if let hirsel_engine::ToolError::Query {
                error: hirsel_engine::QueryError::Diagnostics(diagnostics),
                sources,
                ..
            } = &error
            {
                let rendered = diagnostics
                    .iter()
                    .take(4)
                    .map(|diagnostic| diagnostic.render_human(sources))
                    .collect::<String>();
                panic!("task fixture: {rendered}");
            }
            panic!("task fixture: {error}");
        });
    let tests = testing::discover(&mut program, &[], Some(49), Selection::Tests)
        .unwrap_or_else(|error| panic!("task fixture: {error}"));
    assert_eq!(tests.len(), 1);
    let mut traces = vec![];
    for checked in [false, true] {
        let ordinary = testing::execute(&mut program, &tests[0], checked, 0)
            .unwrap_or_else(|error| panic!("task fixture: {error}"));
        let traced = execute_test_resources(
            &mut program,
            &tests[0],
            checked,
            0,
            hirsel_interp::Limits::default(),
            1000,
        )
        .unwrap_or_else(|error| panic!("task fixture: {error}"));
        assert_eq!(
            ordinary.outcome.as_ref().unwrap(),
            traced.execution.outcome.as_ref().unwrap()
        );
        assert_eq!(ordinary.observation, traced.execution.observation);
        assert_eq!(ordinary.step_usage, traced.execution.step_usage);
        let trace = traced
            .trace
            .unwrap_or_else(|error| panic!("task fixture: {error}"));
        let analysis = trace.analyze(traced.program, 1000).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (creates as u64, creates as u64, 0)
        );
        traces.push(trace);
    }
    assert_eq!(traces[0], traces[1]);
    traces.remove(0)
}

#[test]
fn ownership_tasks_equal_resources_have_distinct_canonical_child_identities() {
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn children(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let first = task.async(s, fn() { let token = Token() })
    let second = task.async(s, fn() { let token = Token() })
    let _ = task.await(s, take first)
    let _ = task.await(s, take second)
  })
}
"#,
        2,
    );
    let tasks: std::collections::BTreeSet<_> = trace
        .events
        .iter()
        .filter(|event| {
            matches!(
                event.action,
                hirsel_run_schema::ownership_trace::Action::Create { .. }
            )
        })
        .map(|event| event.task.clone())
        .collect();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|path| path.0.len() == 1));
}

#[test]
fn ownership_tasks_cancellation_drops_a_child_local_after_readiness() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn canceled_child(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let (ready_tx, ready_rx): (Sender[u8], Receiver[u8]) = channel.open(tasks)
    let (gate_tx, gate_rx): (Sender[u8], Receiver[u8]) = channel.open(tasks)
    let child = task.async(s, fn() {
      let token = Token()
      let mut ready = take ready_tx
      let _ = channel.send(tasks, mut ready, 7u8)
      let mut gate = take gate_rx
      let _ = channel.receive(tasks, mut gate)
    })
    let mut ready = take ready_rx
    let _ = channel.receive(tasks, mut ready)
    task.cancel(s, child)
    let _ = task.await(s, take child)
    let _ = gate_tx
  })
}
"#,
        1,
    );
}

#[test]
fn ownership_spawn_moves_owned_aggregate_captures_into_the_actual_child() {
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn captured_children(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let pair = (Token(), Token())
    let child = task.async(s, fn() { let carried = take pair })
    let _ = task.await(s, take child)
  })
}
"#,
        2,
    );
    use hirsel_run_schema::ownership_trace::{Action, Storage};
    assert!(trace.events.iter().any(|event| matches!(&event.action,
        Action::Move { owner, .. } if owner.task.is_none() && matches!(&owner.storage, Storage::Object {name, ..} if name == "task.spawn"))));
    let child_moves: Vec<_> = trace.events.iter().filter(|event| matches!(&event.action,
        Action::Move { from, owner, .. } if from.task.is_none() && owner.task.as_ref().is_some_and(|task| !task.0.is_empty()))).collect();
    assert_eq!(child_moves.len(), 2);
    assert!(
        child_moves
            .iter()
            .all(|event| event.action.token().task.0.is_empty())
    );
}

#[test]
fn ownership_spawn_cancellation_releases_the_transferred_capture() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn canceled_capture(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let token = Token()
    let (gate_tx, gate_rx): (Sender[u8], Receiver[u8]) = channel.open(tasks)
    let child = task.async(s, fn() {
      let held = take token
      let mut gate = take gate_rx
      let _ = channel.receive(tasks, mut gate)
    })
    task.cancel(s, child)
    let _ = task.await(s, take child)
    let _ = gate_tx
  })
}
"#,
        1,
    );
}

#[test]
fn ownership_results_await_moves_the_child_aggregate_into_the_receiving_parent() {
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn awaited_result(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let child = task.async(s, fn() { (Token(), Token()) })
    let received = task.await(s, take child)
  })
}
"#,
        2,
    );
    use hirsel_run_schema::ownership_trace::{Action, Storage};
    let acquired: Vec<_> = trace
        .events
        .iter()
        .filter(|event| {
            matches!(&event.action,
        Action::Move {from, owner, ..} if from.task.is_none()
        && matches!(&from.storage, Storage::Object {name, ..} if name == "task.result")
        && owner.task.as_ref().is_some_and(|task| task.0.is_empty()))
        })
        .collect();
    assert_eq!(acquired.len(), 2);
    assert!(
        acquired
            .iter()
            .all(|event| event.action.token().task.0.len() == 1)
    );
}

#[test]
fn ownership_results_a_finished_child_drops_its_retained_value_in_its_rest() {
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn retained_result(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let child = task.async(s, fn() { Token() })
    task.detach(s, take child)
  })
}
"#,
        1,
    );
    use hirsel_run_schema::ownership_trace::{Action, Storage};
    assert!(trace.events.iter().any(|event| matches!(&event.action,
        Action::Move {from, owner, ..} if from.task.is_none()
        && matches!(&from.storage, Storage::Object {name, ..} if name == "task.result")
        && owner.task.as_ref().is_some_and(|task| !task.0.is_empty()))));
}

#[test]
fn ownership_results_an_already_detached_child_disposes_its_return_before_completion() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn detached_result(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let (gate_tx, gate_rx): (Sender[u8], Receiver[u8]) = channel.open(tasks)
    let child = task.async(s, fn() {
      let mut gate = take gate_rx
      let _ = channel.receive(tasks, mut gate)
      Token()
    })
    task.detach(s, take child)
    let mut gate = take gate_tx
    let _ = channel.send(tasks, mut gate, 7u8)
  })
}
"#,
        1,
    );
}

#[test]
fn ownership_channels_fifo_includes_plain_messages_between_equal_resources() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn fifo(t: Test) {
  let tasks = test.tasks(t)
  let (sender, receiver): (Sender[Option[Token]], Receiver[Option[Token]]) = channel.open(tasks)
  let mut tx = take sender
  let mut rx = take receiver
  let _ = channel.send(tasks, mut tx, Some(Token()))
  let _ = channel.send(tasks, mut tx, None())
  let _ = channel.send(tasks, mut tx, Some(Token()))
  let first = channel.receive(tasks, mut rx)
  let plain = channel.receive(tasks, mut rx)
  let second = channel.receive(tasks, mut rx)
}
"#,
        2,
    );
}

#[test]
fn ownership_channels_resource_delivery_crosses_actual_task_paths() {
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn delivery(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let (sender, receiver): (Sender[Token], Receiver[Token]) = channel.open(tasks)
    let child = task.async(s, fn() {
      let mut rx = take receiver
      let received = channel.receive(tasks, mut rx)
    })
    let mut tx = take sender
    let _ = channel.send(tasks, mut tx, Token())
    let _ = task.await(s, take child)
  })
}
"#,
        1,
    );
    use hirsel_run_schema::ownership_trace::{Action, Storage};
    assert!(trace.events.iter().any(|event| matches!(&event.action,
        Action::Move {from, owner, ..} if from.task.is_none()
        && matches!(&from.storage, Storage::Object {name, ..} if name == "channel")
        && owner.task.as_ref().is_some_and(|task| task.0.len() == 1))));
}

#[test]
fn ownership_channels_a_closed_send_returns_the_same_payload_for_cleanup() {
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
fn discard[T](take value: T) {}
@test
fn closed_send(t: Test) {
  let tasks = test.tasks(t)
  let (sender, receiver): (Sender[Token], Receiver[Token]) = channel.open(tasks)
  discard(take receiver)
  let mut tx = take sender
  let returned = channel.send(tasks, mut tx, Token())
}
"#,
        1,
    );
    use hirsel_run_schema::ownership_trace::{Action, Storage};
    assert!(!trace.events.iter().any(|event| matches!(&event.action,
        Action::Move {owner, ..} if matches!(&owner.storage, Storage::Object {name, ..} if name == "channel"))));
}

#[test]
fn ownership_channels_last_receiver_drops_queued_resources_in_actual_fifo_order() {
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
fn discard[T](take value: T) {}
@test
fn queued_cleanup(t: Test) {
  let tasks = test.tasks(t)
  let (sender, receiver): (Sender[Token], Receiver[Token]) = channel.open(tasks)
  let mut tx = take sender
  let _ = channel.send(tasks, mut tx, Token())
  let _ = channel.send(tasks, mut tx, Token())
  discard(take receiver)
}
"#,
        2,
    );
    use hirsel_run_schema::ownership_trace::Action;
    let dropped: Vec<_> = trace
        .events
        .iter()
        .filter_map(|event| match &event.action {
            Action::Drop { token, .. } => Some(token.sequence),
            _ => None,
        })
        .collect();
    assert_eq!(dropped, [0, 1]);
}

#[test]
fn ownership_channels_canceled_receive_keeps_the_queued_payload_for_receiver_cleanup() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn canceled_receive(t: Test) {
  let tasks = test.tasks(t)
  task.scope(tasks, fn(s: Scope) {
    let (ready_tx, ready_rx): (Sender[u8], Receiver[u8]) = channel.open(tasks)
    let (gate_tx, gate_rx): (Sender[u8], Receiver[u8]) = channel.open(tasks)
    let (sender, receiver): (Sender[Token], Receiver[Token]) = channel.open(tasks)
    let child = task.async(s, fn() {
      let mut ready = take ready_tx
      let _ = channel.send(tasks, mut ready, 7u8)
      let mut gate = take gate_rx
      let _ = channel.receive(tasks, mut gate)
      let mut rx = take receiver
      let canceled = channel.receive(tasks, mut rx)
    })
    let mut ready = take ready_rx
    let _ = channel.receive(tasks, mut ready)
    let mut tx = take sender
    let _ = channel.send(tasks, mut tx, Token())
    task.cancel(s, child)
    let _ = task.await(s, take child)
    let _ = gate_tx
  })
}
"#,
        1,
    );
}

#[test]
fn ownership_callbacks_scope_returns_an_owned_aggregate_without_losing_lineage() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn scope_result(t: Test) {
  let tasks = test.tasks(t)
  let received = task.scope(tasks, fn(s: Scope) { (Token(), Token()) })
}
"#,
        2,
    );
}

#[test]
fn ownership_callbacks_nested_mutex_and_shared_results_keep_the_created_identity() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn nested_results(t: Test) {
  let tasks = test.tasks(t)
  let board = mutex.create(tasks, unique 0u64)
  let frozen = shared.freeze(unique 3u64)
  let received = mutex.with(tasks, board, fn(mut n: unique u64) -> Token uses {} {
    shared.with(frozen, fn(value: u64) -> Token uses {} { Token() })
  })
}
"#,
        1,
    );
}

#[test]
fn ownership_callbacks_failed_mutex_body_releases_its_local_resource() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn failed_callback(t: Test) {
  let tasks = test.tasks(t)
  let board = mutex.create(tasks, unique 0u64)
  mutex.with(tasks, board, fn(mut n: unique u64) -> () uses {} {
    let token = Token()
    failure.check(false, Arithmetic())
  })
}
"#,
        1,
    );
}

#[test]
fn ownership_borrows_reusable_moved_closure_keeps_the_single_original_resource_owner() {
    observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn read(token: Token) -> u64 { token.value }
@test
fn borrowed_reader(t: Test) {
  let token = Token(value: 7u64)
  let reader = fn() -> u64 uses {} { read(token) }
  let moved = take reader
  let first = moved()
  let second = moved()
  test.check(t, first == 7u64 && second == 7u64)
}
"#,
        1,
    );
}

#[test]
fn ownership_borrows_an_environment_survives_aggregate_and_generic_call_returns() {
    observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn identity[T](take value: T in value) -> T in value { take value }
@test
fn returned_reader(t: Test) {
  let token = Token(value: 7u64)
  let wrapped = (fn() -> u64 uses {} { token.value }, 9u64)
  let (reader, number) = identity(take wrapped)
  test.check(t, reader() == 7u64 && number == 9u64)
}
"#,
        1,
    );
}

#[test]
fn ownership_borrows_a_mutex_callback_reads_its_captured_resource_without_owning_it() {
    observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn read(token: Token) -> u64 { token.value }
@test
fn callback_reader(t: Test) {
  let tasks = test.tasks(t)
  let board = mutex.create(tasks, unique 0u64)
  let token = Token(value: 7u64)
  let answer = mutex.with(tasks, board, fn(mut n: unique u64) -> u64 uses {} {
    read(token)
  })
  test.check(t, answer == 7u64)
}
"#,
        1,
    );
}
