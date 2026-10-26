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
    let tests =
        testing::discover(&mut program, &[], Some(49), Selection::Tests).unwrap_or_else(|error| {
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

#[test]
fn ownership_native_source_emission_binds_checked_metadata_and_preserves_ordinary_objects() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::{
            self, Backend, OptLevel, Options, Partition, Sites, TargetSpec,
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("main.hir");
        std::fs::write(&source, "resource type Token { value: u64 }\ndrop(x) {}\npub fn main() -> Result[(), ()] {\n let token = Token(value: 7u64)\n Ok(())\n}\n").unwrap();
        let mut loaded = hirsel_driver::load_program(&source)
            .unwrap_or_else(|error| panic!("source fixture: {error}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("source fixture: {error}"));
        let input = loaded.engine.export_native_input().unwrap();
        let checked = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &checked,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = Sites::new(checked.snapshot());
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let ordinary = || {
                native::emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap()
            };
            let before = ordinary();
            let traced = native::ownership_trace::emit_leaf(
                &ready,
                &loaded.entry,
                &sites,
                backend,
                OptLevel::None,
            )
            .unwrap_or_else(|error| panic!("{backend:?} source ownership: {error}"));
            let after = ordinary();
            assert_eq!(before, after);
            assert_eq!(
                traced.program(),
                ready.revision(&loaded.entry).unwrap().digest()
            );
            let creates: Vec<_> = traced
                .metadata()
                .values()
                .flat_map(|unit| unit.values())
                .filter(|marker| marker.action == 0)
                .collect();
            assert_eq!(creates.len(), 1);
            assert!(creates[0].value_type.ends_with("::Token"));
            assert!(creates[0].source.file.ends_with("main.hir"));
            assert_eq!(creates[0].source.line, 4);
            assert!(!traced.objects()[0].bytes.is_empty());
        }
    });
}

#[test]
fn ownership_native_selected_runtime_decodes_the_same_source_lifetimes_on_four_engines() {
    observed_native_source(
        "resource type Token { value: u64 }\ndrop(x) {}\npub fn main() -> Result[(), ()] {\n let first = Token(value: 7u64)\n let second = Token(value: 7u64)\n let moved = take first\n Ok(())\n}\n",
        2,
        false,
    );
}

#[test]
fn ownership_native_local_calls_preserve_distinct_lifetimes_on_four_engines() {
    observed_native_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn make_local(pick: bool) -> u64 {
  let first = Token(value: 7u64)
  let moved = take first
  if pick { 1u64 } else { 2u64 }
}
pub fn main() -> Result[(), ()] {
  let held = Token(value: 7u64)
  let first = make_local(true)
  let second = make_local(false)
  if first + second == 3u64 { Ok(()) } else { Error(()) }
}
"#,
        3,
        true,
    );
}

fn observed_native_source(body: &str, created: u64, expect_calls: bool) {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::{
            self, Backend, OptLevel, Options, RuntimeBuild, Sites, TargetSpec, Toolchain,
        };
        use hirsel_run_schema::ownership_trace::Action;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("main.hir");
        std::fs::write(&source, body).unwrap();
        let mut traces = vec![];
        let mut host_outcome = None;
        for checked in [false, true] {
            let run = run_resources_on_host(
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
                1000,
            )
            .unwrap_or_else(|error| panic!("reference fixture: {error}"));
            if let Some(outcome) = &host_outcome {
                assert_eq!(&run.outcome, outcome);
            } else {
                host_outcome = Some(run.outcome.clone());
            }
            traces.push(run.trace.unwrap());
        }
        assert_eq!(traces[0], traces[1]);
        let mut loaded = hirsel_driver::load_program(&source)
            .unwrap_or_else(|error| panic!("source fixture: {error}"));
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let checked = loaded.engine.import_native_input(input).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let ready = native::build(&checked, &Options::new(target.clone())).unwrap();
        let selected: Vec<_> = ready.units.iter().collect();
        let calls = hirsel_lir::ownership_trace::native::local_calls(&selected).unwrap();
        assert_eq!(
            calls.values().any(|targets| !targets.is_empty()),
            expect_calls
        );
        if expect_calls {
            let caller = ready
                .units
                .iter()
                .find(|unit| unit.unit().header.definition == loaded.entry)
                .unwrap();
            assert!(
                hirsel_lir::ownership_trace::native::local_calls(&[caller]).is_err(),
                "an ordinary object must not fill a missing diagnostic call target"
            );
        }
        let sites = Sites::new(checked.snapshot());
        let variants = [
            RuntimeBuild::default(),
            RuntimeBuild {
                ownership_trace: true,
                ..Default::default()
            },
        ];
        let tools =
            Toolchain::in_tree_with_runtimes(&dir.path().join("tools"), &[&target], &variants)
                .unwrap();
        // The opt-in configuration must not put diagnostic exports into the
        // archive whose product still promises the default runtime variant.
        let default_bytes = std::fs::read(tools.archive(&target.triple).unwrap()).unwrap();
        let archive = object::read::archive::ArchiveFile::parse(&*default_bytes).unwrap();
        for member in archive.members() {
            let member = member.unwrap();
            if let Ok(object) = object::File::parse(member.data(default_bytes.as_slice()).unwrap())
            {
                use object::{Object, ObjectSymbol};
                assert!(!object.symbols().any(|symbol| {
                    symbol.is_definition()
                        && symbol
                            .name()
                            .is_ok_and(|name| name.starts_with("hirsel_rt_v2_ownership_"))
                }));
            }
        }
        let selected = tools.with_runtime(variants[1]);
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = native::ownership_trace::emit_leaf(
                &ready,
                &loaded.entry,
                &sites,
                backend,
                OptLevel::None,
            )
            .unwrap();
            assert!(
                objects
                    .launch(&ready, &tools, &dir.path().join("refused"), 1000)
                    .is_err()
            );
            let run = objects
                .launch(&ready, &selected, &dir.path().join(backend.name()), 1000)
                .unwrap_or_else(|error| panic!("{backend:?} native launch: {error}"));
            assert_eq!(run.execution.exit, Some(0));
            assert!(run.execution.stdout.is_empty());
            assert!(
                run.execution.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&run.execution.stderr)
            );
            let trace = run
                .trace
                .unwrap_or_else(|error| panic!("{backend:?} native trace: {error}"));
            let analysis = trace.analyze(run.program, 1000).unwrap();
            assert_eq!(
                (analysis.created, analysis.dropped, analysis.live.len()),
                (created, created, 0)
            );
            traces.push(trace);
        }
        assert_eq!(traces[2], traces[3]);
        // Compiler storage moves differ from evaluator storage moves. Compare
        // the actual logical token lifetimes, retaining identity and order.
        let lifecycle = |trace: &hirsel_run_schema::ownership_trace::Trace| {
            trace
                .events
                .iter()
                .filter_map(|event| match &event.action {
                    Action::Create {
                        token, value_type, ..
                    } => Some((0, token.clone(), Some(value_type.clone()))),
                    Action::Drop { token, .. } => Some((1, token.clone(), None)),
                    Action::Move { .. } => None,
                })
                .collect::<Vec<_>>()
        };
        for trace in &traces[1..] {
            assert_eq!(lifecycle(&traces[0]), lifecycle(trace));
        }
    });
}

#[test]
fn ownership_replacement_local_and_projected_resource_move_chains() {
    observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
type Pair { left: Token, right: Token }
fn exchange(mut slot: Token, take incoming: Token) -> Token {
  replace(mut slot, take incoming)
}
@test
fn replacements(t: Test) {
  let mut current = Token(value: 7u64)
  let previous = exchange(mut current, Token(value: 7u64))
  let mut pair = Pair(left: take previous, right: Token(value: 7u64))
  let old = replace(mut pair.left, take current)
  test.check(t, old.value == 7u64 && pair.left.value == 7u64)
}
"#,
        3,
    );
}

#[test]
fn ownership_replacement_implicit_assignment_drops_displaced_resource() {
    observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
@test
fn assignments(t: Test) {
  let mut current: Token in current = Token(value: 7u64)
  current = Token(value: 7u64)
  test.check(t, current.value == 7u64)
}
"#,
        2,
    );
}

#[test]
fn ownership_callback_captures_scope_moves_its_owned_resource_into_the_result() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn captured_scope(t: Test) {
  let tasks = test.tasks(t)
  let token = Token()
  let received = task.scope(tasks, fn(s: Scope) { take token })
}
"#,
        1,
    );
}

#[test]
fn ownership_callback_captures_shared_once_body_drops_an_owned_aggregate() {
    // Shared's reusable callback borrows from the owning once scope body.
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
fn inspect(token: Token) -> u64 { 7u64 }
@test
fn captured_shared(t: Test) {
  let tasks = test.tasks(t)
  let frozen = shared.freeze(unique 3u64)
  let pair = (Token(), Token())
  let answer = task.scope(tasks, fn(s: Scope) {
    let (left, right) = take pair
    shared.with(frozen, fn(value: u64) -> u64 uses {} {
      inspect(left) + inspect(right) + value
    })
  })
  test.check(t, answer == 17u64)
}
"#,
        2,
    );
    use hirsel_run_schema::ownership_trace::{
        Action, Projection as Step, Storage, TaskPath, Token,
    };
    for (sequence, field) in [(0, 0), (1, 1)] {
        let expected = Token {
            task: TaskPath::default(),
            sequence,
        };
        let suffix = vec![Step::Field { field }];
        let events = trace
            .events
            .iter()
            .filter(|event| event.action.token() == &expected)
            .collect::<Vec<_>>();
        let transfers = events
            .iter()
            .filter(|event| {
                matches!(&event.action,
            Action::Move { from, owner, .. }
                if from.path.len() == 2
                    && matches!(from.path.first(), Some(Step::Field { .. }))
                    && from.path[1..] == suffix && owner.path == suffix
                    && matches!((&from.storage, &owner.storage),
                        (Storage::Slot { frame: source, .. }, Storage::Slot { frame: target, .. })
                            if source != target))
            })
            .count();
        assert_eq!(
            transfers, 1,
            "{expected} must transfer once from environment.capture.field{field} to callback.field{field}; events={events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(&event.action, Action::Drop { .. })),
            "{expected} must drop after capture transfer at suffix {suffix:?}; events={events:?}"
        );
    }
}

#[test]
fn ownership_callback_captures_failed_mutex_body_releases_its_owned_capture() {
    // Mutex's reusable callback borrows the owning once scope body's resource;
    // its failure must unwind that owner and complete the captured lifetime.
    let trace = observed_task_source(
        r#"
resource type Token {}
drop(x) {}
fn inspect(token: Token) -> u64 { 7u64 }
@test
fn captured_failure(t: Test) {
  let tasks = test.tasks(t)
  let board = mutex.create(tasks, unique 0u64)
  let token = Token()
  task.scope(tasks, fn(s: Scope) {
    let held = take token
    mutex.with(tasks, board, fn(mut n: unique u64) -> () uses {} {
      failure.check(inspect(held) != 7u64, Arithmetic())
    })
  })
}
"#,
        1,
    );
    use hirsel_run_schema::ownership_trace::{Action, Storage};
    let captured = trace
        .events
        .iter()
        .find_map(|event| match &event.action {
            Action::Move { token, from, owner }
                if !from.path.is_empty()
                    && owner.path.is_empty()
                    && matches!((&from.storage, &owner.storage),
                    (Storage::Slot { frame: source, .. }, Storage::Slot { frame: target, .. })
                        if source != target) =>
            {
                Some(token)
            }
            _ => None,
        })
        .expect("the once callback must receive the owned capture");
    assert!(trace.events.iter().any(|event| matches!(&event.action,
        Action::Drop { token, .. } if token == captured)));
}

#[test]
fn ownership_callback_captures_mixed_owned_and_borrowed_equal_resources() {
    observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn read(token: Token) -> u64 { token.value }
@test
fn mixed_captures(t: Test) {
  let tasks = test.tasks(t)
  let borrowed = Token(value: 7u64)
  let owned = Token(value: 7u64)
  let received = task.scope(tasks, fn(s: Scope) {
    let n = read(borrowed)
    take owned
  })
  test.check(t, received.value == read(borrowed))
}
"#,
        2,
    );
}

#[test]
fn ownership_replacement_empty_scalar_updates_preserve_a_live_resource() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn scalar_updates(t: Test) {
  let token = Token()
  let mut count = 1u64
  count = count + 1u64
  let old = replace(mut count, count + 1u64)
  test.check(t, old == 2u64 && count == 3u64)
}
"#,
        1,
    );
}

#[test]
fn ownership_replacement_empty_variants_transfer_only_the_actual_resource() {
    observed_task_source(
        r#"
resource type Token {}
drop(x) {}
@test
fn optional_updates(t: Test) {
  let mut slot: Option[Token] = None()
  let empty = replace(mut slot, Some(Token()))
  let full = replace(mut slot, None())
  test.check(t, match empty { None() => true, Some(value) => false })
}
"#,
        1,
    );
}

#[test]
fn ownership_mutex_payload_last_root_release_drops_the_stored_resource() {
    let trace = observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
@test
fn stored_payload(t: Test) {
  let tasks = test.tasks(t)
  let store = mutex.create(tasks, Token(value: 7u64))
  let other = copy store
  let answer = mutex.with(tasks, other, fn(mut token: unique Token) -> u64 uses {} {
    token.value
  })
  test.check(t, answer == 7u64)
}
"#,
        1,
    );
    use hirsel_run_schema::ownership_trace::{Action, Storage};
    assert!(trace.events.iter().any(|event| matches!(&event.action,
        Action::Move { owner, .. } if owner.task.is_none()
            && matches!(&owner.storage, Storage::Object { name, .. } if name == "mutex"))));
}

#[test]
fn ownership_mutex_payload_replacement_returns_the_original_identity() {
    let trace = observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
@test
fn replaced_payload(t: Test) {
  let tasks = test.tasks(t)
  let store = mutex.create(tasks, Token(value: 7u64))
  let old = mutex.with(tasks, store, fn(mut token: unique Token) -> Token uses {} {
    replace(mut token, Token(value: 7u64))
  })
  test.check(t, old.value == 7u64)
}
"#,
        2,
    );
    use hirsel_run_schema::ownership_trace::{Action, Storage};
    assert!(trace.events.iter().any(|event| matches!(&event.action,
        Action::Move { token, from, owner } if token.sequence == 0
            && matches!(&from.storage, Storage::Object { name, .. } if name == "mutex")
            && matches!(&owner.storage, Storage::Slot { .. }))));
}

#[test]
fn ownership_mutex_payload_failed_callback_releases_the_stored_resource() {
    observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
@test
fn failed_payload(t: Test) {
  let tasks = test.tasks(t)
  let store = mutex.create(tasks, Token(value: 7u64))
  mutex.with(tasks, store, fn(mut token: unique Token) -> () uses {} {
    failure.check(token.value != 7u64, Arithmetic())
  })
}
"#,
        1,
    );
}

#[test]
fn ownership_mutex_payload_empty_variant_writebacks_preserve_lineage() {
    observed_task_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
@test
fn optional_payload(t: Test) {
  let tasks = test.tasks(t)
  let initial: unique Option[Token] = None()
  let store = mutex.create(tasks, take initial)
  let empty = mutex.with(tasks, store, fn(mut slot: unique Option[Token]) -> Option[Token] uses {} {
    replace(mut slot, Some(Token(value: 7u64)))
  })
  let full = mutex.with(tasks, store, fn(mut slot: unique Option[Token]) -> Option[Token] uses {} {
    replace(mut slot, None())
  })
  test.check(t, match full { None() => false, Some(value) => value.value == 7u64 })
}
"#,
        1,
    );
}

#[test]
fn ownership_native_call_inputs_borrowed_resource_retains_its_original_lifetime() {
    observed_native_source(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn inspect(token: Token) -> u64 {
  let owned = Token(value: 7u64)
  token.value + owned.value
}
pub fn main() -> Result[(), ()] {
  let held = Token(value: 7u64)
  let answer = inspect(held)
  if answer == 14u64 { Ok(()) } else { Error(()) }
}
"#,
        2,
        true,
    );
}

#[test]
fn ownership_native_call_inputs_require_original_projection_for_synthetic_moves() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::{self, Backend, OptLevel, Options, Sites, TargetSpec};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("main.hir");
        std::fs::write(&source, "resource type Token {}\ndrop(x) {}\nfn relay(take token: Token) -> Token { take token }\npub fn main() -> Result[(), ()] { let first = Token()\n let last = relay(take first)\n Ok(()) }\n").unwrap();
        let mut loaded = hirsel_driver::load_program(&source).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let checked = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &checked,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = Sites::new(checked.snapshot());
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let error = native::ownership_trace::emit_leaf(
                &ready,
                &loaded.entry,
                &sites,
                backend,
                OptLevel::None,
            )
            .err()
            .expect("source-less synthetic Ready moves cannot invent source provenance");
            assert!(
                error.message.contains("outside the source"),
                "{backend:?}: {error}"
            );
        }
    });
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn ownership_native_owned_direct_calls_keep_arguments_results_and_equal_tokens_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn make() -> Token { Token(value: 7u64) }
fn identity(take token: Token) -> Token { take token }
pub fn main() -> Result[(), ()] {
  let first = identity(make())
  let second = identity(Token(value: 7u64))
  let moved = take first
  Ok(())
}
"#,
        2,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn ownership_native_owned_join_keeps_selected_branch_token_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn choose(flag: bool, take first: Token, take second: Token) -> Token {
  if flag { take first } else { take second }
}
pub fn main() -> Result[(), ()] {
  let left = choose(true, Token(value: 7u64), Token(value: 7u64))
  let right = choose(false, Token(value: 7u64), Token(value: 7u64))
  Ok(())
}
"#,
        4,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn ownership_native_failed_finalizer_retires_callee_and_outer_resource_on_four_engines() {
    super::ownership_owned_call_49::source_calls_exit(
        r#"
resource type Token { bad: bool }
drop(x) {
  if x.bad { let zero: u8 = 0u8
    let _ = 1u8 / zero }
}
fn discard(take token: Token) {}
pub fn main() -> Result[(), ()] {
  let outer = Token(bad: false)
  discard(Token(bad: true))
  Ok(())
}
"#,
        2,
        hirsel_run_schema::exit::FAILED,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn ownership_native_once_capture_returns_original_resource_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn relay(take token: Token) -> Token {
  let captured: once fn() -> Token = fn() -> Token { take token }
  captured()
}
pub fn main() -> Result[(), ()] {
  let first = relay(Token(value: 7u64))
  let second = relay(Token(value: 7u64))
  Ok(())
}
"#,
        2,
    );
}

#[test]
#[ignore = "native: executes original uncalled capture cleanup on both backends"]
fn ownership_native_uncalled_once_environment_drops_original_resources_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn discard(take token: Token) {
    let captured: once fn() -> Token = fn() -> Token { take token }
}
pub fn main() -> Result[(), ()] {
    discard(Token(value: 7u64))
    discard(Token(value: 7u64))
    Ok(())
}
"#,
        2,
    );
}

#[test]
#[ignore = "native: executes partially consumed capture cleanup on both backends"]
fn ownership_native_called_once_environment_drops_only_retained_capture_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { value: u64 }
drop(x) {}
fn select(pick: bool, take first: Token, take second: Token) -> Token {
    let captured: once fn() -> Token = fn() -> Token {
        if pick { take second } else { take first }
    }
    captured()
}
pub fn main() -> Result[(), ()] {
    let left = select(false, Token(value: 7u64), Token(value: 7u64))
    let right = select(true, Token(value: 7u64), Token(value: 7u64))
    Ok(())
}
"#,
        4,
    );
}

#[test]
#[ignore = "native: observes actual failing capture finalizer and its locals on both backends"]
fn ownership_native_failed_capture_cleanup_retires_actual_resources_after_finalizer_locals_on_four_engines()
 {
    super::ownership_owned_call_49::source_calls_exit(
        r#"
resource type Audit { value: u64 }
drop(x) {}
resource type Token { bad: bool }
drop(x) {
    let audit = Audit(value: 7u64)
    if x.bad { let zero: u8 = 0u8
        let _ = 1u8 / zero }
}
fn discard(take token: Token) {
    let captured: once fn() -> Token = fn() -> Token { take token }
}
pub fn main() -> Result[(), ()] {
    let outer = Token(bad: false)
    discard(Token(bad: true))
    Ok(())
}
"#,
        4,
        hirsel_run_schema::exit::FAILED,
    );
}

#[test]
#[ignore = "native: executes original nested Resource field lifetimes on both backends"]
fn ownership_native_nested_aggregate_keeps_original_field_resources_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { value: u64 }
drop(x) {}
type Pair { left: Token, right: Token }
pub fn main() -> Result[(), ()] {
    let first = Token(value: 7u64)
    let second = Token(value: 7u64)
    let pair = Pair(left: take first, right: take second)
    let moved = take pair
    Ok(())
}
"#,
        2,
    );
}

#[test]
#[ignore = "native: executes aggregate parameter cleanup on both backends"]
fn ownership_native_compound_parameter_keeps_original_children_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { value: u64 }
drop(x) {}
type Pair { left: Token, right: Token }
fn discard(take pair: Pair) {}
pub fn main() -> Result[(), ()] {
    let first = Token(value: 7u64)
    let second = Token(value: 7u64)
    let pair = Pair(left: take first, right: take second)
    discard(take pair)
    Ok(())
}
"#,
        2,
    );
}

#[test]
#[ignore = "native: observes aggregate parameter child failure and continuation on both backends"]
fn ownership_native_compound_parameter_failure_retires_each_actual_child_on_four_engines() {
    super::ownership_owned_call_49::source_calls_exit(
        r#"
resource type Audit { value: u64 }
drop(x) {}
resource type Token { bad: bool }
drop(x) {
    let audit = Audit(value: 7u64)
    if x.bad { let zero: u8 = 0u8
        let _ = 1u8 / zero }
}
type Pair { left: Token, right: Token }
fn discard(take pair: Pair) {}
pub fn main() -> Result[(), ()] {
    let first = Token(bad: false)
    let second = Token(bad: true)
    let pair = Pair(left: take first, right: take second)
    discard(take pair)
    Ok(())
}
"#,
        4,
        hirsel_run_schema::exit::FAILED,
    );
}

#[test]
#[ignore = "native: executes retained aggregate capture cleanup on both backends"]
fn ownership_native_uncalled_once_environment_drops_nested_original_children_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { value: u64 }
drop(x) {}
type Pair { left: Token, right: Token }
fn discard(take pair: Pair) {
    let captured: once fn() -> Pair = fn() -> Pair { take pair }
}
pub fn main() -> Result[(), ()] {
    let first = Token(value: 7u64)
    let second = Token(value: 7u64)
    let pair = Pair(left: take first, right: take second)
    discard(take pair)
    Ok(())
}
"#,
        2,
    );
}
