//! Clock faults and overdue host polls need injected provider behavior.
use crate::source_std;
use hirsel_engine::*;
use hirsel_interp::observation::Effect;

fn run(source: &str, config: SimulationConfig) {
    let mut engine = source_std::engine();
    let file = engine.sources.add("waits.hir", source.to_owned()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let run = RunConfiguration {
        entry: entry_path(&default_context()),
        hash_seed: 0,
        inputs: vec![],
    };
    let execution = run_observed_snapshot(&mut engine, &run, config.clone()).unwrap();
    assert!(
        matches!(
            execution.outcome,
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
        ),
        "{:?}",
        execution.outcome
    );
    let events: Vec<_> = execution
        .observation
        .as_ref()
        .unwrap()
        .effects
        .iter()
        .filter_map(|e| match &e.effect {
            Effect::CapabilityConsumed(input) => Some(input.clone()),
            _ => None,
        })
        .collect();
    // The recorded events replay the run exactly, with no waiting.
    let replay = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            inputs: events.clone(),
            ..run
        },
        config.clone(),
    )
    .unwrap();
    assert_eq!(replay.observation, execution.observation);
}
fn clocked(monotonic: u64) -> SimulationConfig {
    let mut config = SimulationConfig {
        monotonic,
        ..Default::default()
    };
    config.grants.insert("MonotonicClock".into());
    config
}

#[test]
fn an_unreadable_clock_fails_the_wait_with_its_clock_error() {
    with_compiler_stack(|| {
        let mut config = clocked(0);
        config.faults.insert(
            "clock.sleep_until".into(),
            [hirsel_interp::library::simulation::Fault::Fault {
                code: 0,
                partial: 0,
            }]
            .into(),
        );
        run(
            r#"
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  match io.monotonic_clock(rt) {
    Ok(c) => match time.sleep(c, duration.from_parts(1, 0)) {
      Error(Wait(Clock(Unavailable()))) => {},
      _ => failure.check(false, Arithmetic()),
    },
    Error(_) => failure.check(false, Arithmetic()),
  }
  Ok(())
}
"#,
            config,
        );
    });
}

// A corpus program cannot advance a provider's clock while runnable work remains.
// This adapter models a blocking host step so all suspended deadlines are overdue
// at one poll, independently of host speed or scheduling load.
#[test]
fn overdue_timers_resume_in_deadline_order_and_keep_ties_in_suspension_order() {
    use hirsel_interp::{
        Error, OperationCall, OperationContext, OperationResult, Operations, OperationsAdapter,
        Value, library::Library,
    };
    struct StalledHost {
        library: Library,
        parked: usize,
    }
    impl OperationsAdapter for StalledHost {
        fn operations(&self) -> &dyn Operations {
            &self.library
        }
        fn operations_mut(&mut self) -> &mut dyn Operations {
            &mut self.library
        }
        fn call_resolved(
            &mut self,
            operation: hirsel_interp::OperationId,
            call: OperationCall<'_>,
            cx: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            if call.path == &hirsel_syntax::DefPath::core("out.print")
                && call.arguments.get(1) == Some(&Value::String("advance\n".into()))
            {
                assert_eq!(
                    self.parked, 3,
                    "every timer must suspend before the blocking step"
                );
                self.library.simulation.config.monotonic = 100;
            }
            let result = self.library.call_resolved(operation, call, cx)?;
            if matches!(result, Some(OperationResult::Park { .. })) {
                self.parked += 1;
            }
            Ok(result)
        }
    }
    let source = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) { Ok(t) => t, Error(_) => return Ok(()) }
  let late = match instant.checked_add(c, start, 30u64) { Some(t) => t, None() => return Ok(()) }
  let early = match instant.checked_add(c, start, 10u64) { Some(t) => t, None() => return Ok(()) }
  task.scope(tasks, fn(s: Scope) {
    let a = task.async(s, fn() { let _ = clock.sleep_until(c, late); let _ = out.print(output, "late\n") })
    let b = task.async(s, fn() { let _ = clock.sleep_until(c, early); let _ = out.print(output, "early\n") })
    let d = task.async(s, fn() { let _ = clock.sleep_until(c, early); let _ = out.print(output, "tied\n") })
    let _ = out.print(output, "park\n")
    let _ = out.print(output, "advance\n")
    task.await(s, take a)
    task.await(s, take b)
    task.await(s, take d)
  })
  Ok(())
}
"#;
    let mut engine = source_std::engine();
    let file = engine.sources.add("overdue.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let runtime = engine.runtime_types().unwrap();
    let sources = engine.sources.clone();
    let run = RunConfiguration {
        entry: entry_path(&default_context()),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut config = SimulationConfig::default();
    config.grants.insert("Io".into());
    config.grants.insert("MonotonicClock".into());
    let mut host = StalledHost {
        library: Library::new(&run, config).unwrap(),
        parked: 0,
    };
    let root = host.library.simulation.grant_root("Io").unwrap();
    let execution = hirsel_interp::run_with_runtime(
        &mut engine,
        &sources,
        &run,
        vec![root],
        Default::default(),
        &mut host,
        &runtime,
    )
    .unwrap();
    assert!(
        matches!(
            execution.outcome,
            hirsel_interp::Outcome::Returned { exit_code: 0, .. }
        ),
        "{:?}",
        execution.outcome
    );
    assert_eq!(execution.stdout, b"park\nadvance\nearly\ntied\nlate\n");
}

// A layer that watches its waits is asked about a parked wait only once it reports the
// wait woken, whatever the number of scheduling points while it parks, and woken waits
// still resume in deadline order with ties in suspension order. Output alone cannot
// tell a scheduler that polls every parked wait from one that polls the woken ones.
#[test]
fn watched_waits_are_polled_only_when_woken_and_resume_in_poll_order() {
    use hirsel_interp::{
        Error, OperationCall, OperationContext, OperationResult, Operations, OperationsAdapter,
        Value, Woken, library::Library,
    };
    use std::collections::BTreeSet;
    struct WatchedTimers {
        library: Library,
        timers: BTreeSet<u64>,
        polls: usize,
        points: usize,
    }
    impl OperationsAdapter for WatchedTimers {
        fn operations(&self) -> &dyn Operations {
            &self.library
        }
        fn operations_mut(&mut self) -> &mut dyn Operations {
            &mut self.library
        }
        fn call_resolved(
            &mut self,
            operation: hirsel_interp::OperationId,
            call: OperationCall<'_>,
            cx: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            if call.path == &hirsel_syntax::DefPath::core("out.print")
                && call.arguments.get(1) == Some(&Value::String("advance\n".into()))
            {
                self.library.simulation.config.monotonic = 100;
            }
            self.library.call_resolved(operation, call, cx)
        }
        fn watched(&self, token: u64) -> bool {
            self.library.wait_deadline(token).is_some()
        }
        fn parked(&mut self, token: u64) {
            if self.library.wait_deadline(token).is_some() {
                self.timers.insert(token);
            }
            self.library.parked(token);
        }
        // Due timers, latest token first: the scheduler, not this list, orders the polls.
        fn woken(&mut self) -> Woken {
            self.points += 1;
            let now = self.library.simulation.config.monotonic;
            Woken::Tokens(
                self.timers
                    .iter()
                    .rev()
                    .copied()
                    .filter(|token| self.library.wait_deadline(*token).is_some_and(|d| d <= now))
                    .collect(),
            )
        }
        fn poll(
            &mut self,
            token: u64,
            cx: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            if self.timers.contains(&token) {
                self.polls += 1;
            }
            let result = self.library.poll(token, cx)?;
            if result.is_some() {
                self.timers.remove(&token);
            }
            Ok(result)
        }
    }
    let source = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) { Ok(t) => t, Error(_) => return Ok(()) }
  let late = match instant.checked_add(c, start, 30u64) { Some(t) => t, None() => return Ok(()) }
  let early = match instant.checked_add(c, start, 10u64) { Some(t) => t, None() => return Ok(()) }
  task.scope(tasks, fn(s: Scope) {
    let a = task.async(s, fn() { let _ = clock.sleep_until(c, late); let _ = out.print(output, "late\n") })
    let b = task.async(s, fn() { let _ = clock.sleep_until(c, early); let _ = out.print(output, "early\n") })
    let d = task.async(s, fn() { let _ = clock.sleep_until(c, early); let _ = out.print(output, "tied\n") })
    let _ = out.print(output, "park\n")
    let mut i = 0u32
    while i < 100u32 {
      let _ = clock.monotonic(c)
      i = i + 1u32
    }
    let _ = out.print(output, "advance\n")
    task.await(s, take a)
    task.await(s, take b)
    task.await(s, take d)
  })
  Ok(())
}
"#;
    let mut engine = source_std::engine();
    let file = engine.sources.add("woken.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let runtime = engine.runtime_types().unwrap();
    let sources = engine.sources.clone();
    let run = RunConfiguration {
        entry: entry_path(&default_context()),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut config = SimulationConfig::default();
    config.grants.insert("Io".into());
    config.grants.insert("MonotonicClock".into());
    let mut host = WatchedTimers {
        library: Library::new(&run, config).unwrap(),
        timers: BTreeSet::new(),
        polls: 0,
        points: 0,
    };
    let root = host.library.simulation.grant_root("Io").unwrap();
    let execution = hirsel_interp::run_with_runtime(
        &mut engine,
        &sources,
        &run,
        vec![root],
        Default::default(),
        &mut host,
        &runtime,
    )
    .unwrap();
    assert!(
        matches!(
            execution.outcome,
            hirsel_interp::Outcome::Returned { exit_code: 0, .. }
        ),
        "{:?}",
        execution.outcome
    );
    assert_eq!(execution.stdout, b"park\nadvance\nearly\ntied\nlate\n");
    assert!(host.points > 100, "{} scheduling points", host.points);
    assert_eq!(host.polls, 3, "each timer is polled once, when woken");
}

// A native Free close may wait for another task's resource epoch. The evaluator
// must park only the destructor, retaining any enclosing operation and cleanup.
#[test]
fn suspended_handle_drops_resume_once_after_sibling_progress() {
    use hirsel_interp::{
        Error, OperationContext, OperationResult, Operations, OperationsAdapter, Value,
        library::Library,
    };
    struct DelayedDrops {
        library: Library,
        pending: Option<OperationResult>,
        waiting: std::collections::BTreeMap<u64, (String, u64, usize, bool)>,
        next: u64,
        polls: usize,
        completed: usize,
        progressed: bool,
    }
    impl OperationsAdapter for DelayedDrops {
        fn operations(&self) -> &dyn Operations { &self.library }
        fn operations_mut(&mut self) -> &mut dyn Operations { &mut self.library }
        fn drop_handle(&mut self, kind: &str, id: u64, cx: &mut OperationContext<'_>) -> Result<(), Error> {
            if kind != "File" {
                return self.library.drop_handle(kind, id, cx);
            }
            assert!(self.pending.is_none());
            let token = self.next;
            self.next += 1;
            self.waiting.insert(token, (kind.into(), id, cx.stdout.len(), false));
            self.pending = Some(OperationResult::Park { token, wait: hirsel_interp::BlockedWait {
                spawn_path: self.library.task_path(),
                anchor: cx.anchor().unwrap().clone(),
                kind: "test.delayed.close".into(), targets: vec![],
            }});
            Ok(())
        }
        fn take_drop_wait(&mut self) -> Option<OperationResult> { self.pending.take() }
        fn poll(&mut self, token: u64, cx: &mut OperationContext<'_>) -> Result<Option<OperationResult>, Error> {
            let Some((kind, id, start, polled)) = self.waiting.get_mut(&token) else {
                return self.library.poll(token, cx);
            };
            self.polls += 1;
            if !*polled {
                *polled = true;
                return Ok(None);
            }
            self.progressed |= cx.stdout.len() > *start;
            self.library.drop_handle(kind, *id, cx)?;
            self.waiting.remove(&token);
            self.completed += 1;
            Ok(Some(OperationResult::returned(Value::Unit)))
        }
    }
    // Default forwarding must preserve both the overridden legacy drop and the
    // optional wait hook through a layer that overrides neither.
    struct Layer(DelayedDrops);
    impl OperationsAdapter for Layer {
        fn operations(&self) -> &dyn Operations { &self.0 }
        fn operations_mut(&mut self) -> &mut dyn Operations { &mut self.0 }
    }
    let source = r#"
pub type AppError = Grant(GrantError) | File(FileError)
fn files(d: Dir, output: Stdout) -> Result[(), FileError] {
  let _ = file.create(d, path.of("discarded")) ?
  let mut f = file.create(d, path.of("local")) ?
  file.write(d, mut f, b"closed once") ?
  let _ = out.print(output, "child body\n")
  Ok(())
}
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let d = io.dir(rt, path.of("")) ? File
  let done = task.scope(tasks, fn(s: Scope) -> Result[(), FileError] {
    let child = task.async(s, fn() -> Result[(), FileError] { files(d, output) })
    for i in 0u64..16u64 { let _ = out.print(output, "sibling\n") }
    task.await(s, take child)
  })
  done ? File
  let _ = out.print(output, "finished\n")
  Ok(())
}
"#;
    let mut engine = source_std::engine();
    let file = engine.sources.add("drop-waits.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let runtime = engine.runtime_types().unwrap();
    let sources = engine.sources.clone();
    let run = RunConfiguration { entry: entry_path(&default_context()), hash_seed: 0, inputs: vec![] };
    let mut host = Layer(DelayedDrops {
        library: Library::new(&run, SimulationConfig::default()).unwrap(),
        pending: None, waiting: Default::default(), next: 1 << 61,
        polls: 0, completed: 0, progressed: false,
    });
    let root = host.0.library.simulation.grant_root("Io").unwrap();
    let execution = hirsel_interp::run_with_runtime(&mut engine, &sources, &run, vec![root],
        Default::default(), &mut host, &runtime).unwrap();
    assert!(matches!(execution.outcome, hirsel_interp::Outcome::Returned { exit_code: 0, .. }), "{:?}", execution.outcome);
    assert_eq!(host.0.completed, 2, "each file closes once");
    assert!(host.0.polls >= 4, "each destructor actually returned Pending");
    assert!(host.0.progressed, "sibling progressed while destructor was parked");
    assert!(host.0.waiting.is_empty());
    assert!(host.0.pending.is_none());
    assert!(execution.stdout.ends_with(b"finished\n"));
    assert_eq!(execution.stdout.windows(b"child body\n".len()).filter(|s| *s == b"child body\n").count(), 1);
}
