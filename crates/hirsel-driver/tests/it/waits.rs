//! Timer waits under the simulator's virtual time (core clock.sleep_until, std time.sleep).
//! H checks each run without injected clock faults, with every deadline also fired early at
//! each of its timer choice points (`crate::heap_clock`).
use crate::{heap_clock, source_std};
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::observation::Effect;

fn run(source: &str, config: SimulationConfig) -> RunObservation {
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
        matches!(execution.outcome, Ok(RunOutcome::Exit(0))),
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
    // H's model injects no clock fault.
    if config.faults.is_empty() {
        let expected = native::Observation::of_reference(execution.observation.as_ref().unwrap());
        let world = native::HeapWorld {
            monotonic: Some(config.monotonic),
            process: None,
        };
        heap_clock::check(&mut engine, &expected, &world);
    }
    RunObservation {
        operations: events.iter().map(|e| e.operation.clone()).collect(),
    }
}
struct RunObservation {
    operations: Vec<String>,
}
fn clocked(monotonic: u64) -> SimulationConfig {
    let mut config = SimulationConfig {
        monotonic,
        ..Default::default()
    };
    config.grants.insert("MonotonicClock".into());
    config
}

const SINGLE: &str = r#"
fn require(value: bool) -> () {
  failure.check(value, Arithmetic())
}

fn sample(c: MonotonicClock) -> Result[Instant of c, time.SleepError] {
  match clock.monotonic(c) {
    Ok(t) => Ok(t),
    Error(e) => Error(Wait(Clock(e))),
  }
}

fn exercise(c: MonotonicClock) -> Result[(), time.SleepError] {
  let start = sample(c)?
  time.sleep(c, duration.from_parts(1, 500000000))?
  let after = sample(c)?
  require(time.between(c, start, after).nanoseconds == 1500000000)
  // An elapsed deadline returns at once and leaves the virtual clock alone.
  match clock.sleep_until(c, start) {
    Ok(()) => {},
    Error(_) => require(false),
  }
  let same = sample(c)?
  require(instant.ticks(same) == instant.ticks(after))
  time.sleep(c, duration.from_parts(0, 0))?
  match time.sleep(c, duration.from_parts(0, -1)) {
    Error(NegativeDuration()) => {},
    _ => require(false),
  }
  match time.deadline(c, duration.from_parts(18446744074, 0)) {
    Error(OutOfRange()) => {},
    _ => require(false),
  }
  match instant.checked_add(c, after, 18446744073709551615) {
    None() => {},
    Some(_) => require(false),
  }
  match instant.checked_add(c, start, 7) {
    Some(t) => require(instant.ticks(t) == instant.ticks(start) + 7),
    None() => require(false),
  }
  let until = time.deadline(c, duration.from_parts(2, 0))?
  require(instant.ticks(until) == instant.ticks(after) + 2000000000)
  match clock.sleep_until(c, until) {
    Ok(()) => {},
    Error(_) => require(false),
  }
  let last = sample(c)?
  require(instant.ticks(last) == instant.ticks(until))
  Ok(())
}

pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  match io.monotonic_clock(rt) {
    Ok(c) => match exercise(c) {
      Ok(()) => {},
      Error(_) => require(false),
    },
    Error(_) => require(false),
  }
  Ok(())
}
"#;

#[test]
fn a_sleep_advances_virtual_time_exactly_and_checks_its_duration() {
    with_compiler_stack(|| {
        let observed = run(SINGLE, clocked(1000));
        assert_eq!(
            observed
                .operations
                .iter()
                .filter(|o| *o == "clock.sleep_until.suspend")
                .count(),
            2
        );
        // The zero sleep and the elapsed deadline complete without suspending.
        assert_eq!(
            observed
                .operations
                .iter()
                .filter(|o| *o == "clock.sleep_until")
                .count(),
            4
        );
    });
}

// Sleepers are the only live tasks while every wait is pending: the simulator
// still terminates, waking the shorter sleep first whatever the spawn order.
const SLEEPERS: &str = r#"
fn require(value: bool) -> () {
  failure.check(value, Arithmetic())
}

pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let c = match io.monotonic_clock(rt) {
    Ok(c) => c,
    Error(e) => return Error(e),
  }
  let (sender, receiver): (Sender[u64], Receiver[u64]) = channel.open(tasks)
  task.scope(
    tasks,
    fn(s: Scope) {
      let late_tx = sender.dup(tasks, sender)
      let late = task.async(
        s,
        fn() -> bool {
          let slept = match time.sleep(c, duration.from_parts(30, 0)) {
            Ok(()) => true,
            Error(_) => false,
          }
          let mut tx = take late_tx
          let sent = match channel.send(tasks, mut tx, 30u64) {
            Ok(()) => true,
            Error(_) => false,
          }
          slept && sent
        }
      )
      let early = task.async(
        s,
        fn() -> bool {
          let slept = match time.sleep(c, duration.from_parts(10, 0)) {
            Ok(()) => true,
            Error(_) => false,
          }
          let mut tx = take sender
          let sent = match channel.send(tasks, mut tx, 10u64) {
            Ok(()) => true,
            Error(_) => false,
          }
          slept && sent
        }
      )
      require(task.await(s, take late))
      require(task.await(s, take early))
    }
  )
  let mut rx = take receiver
  let first: bool = match channel.receive(tasks, mut rx) {
    Ok(n) => n == 10,
    Error(_) => false,
  }
  let second: bool = match channel.receive(tasks, mut rx) {
    Ok(n) => n == 30,
    Error(_) => false,
  }
  require(first && second)
  match clock.monotonic(c) {
    Ok(t) => require(instant.ticks(t) == 30000000000),
    Error(_) => require(false),
  }
  Ok(())
}
"#;

#[test]
fn sleepers_alone_terminate_and_wake_in_deadline_order() {
    with_compiler_stack(|| {
        run(SLEEPERS, clocked(0));
    });
}

// Cancelling a pending wait removes its timer: the clock never jumps to it.
const CANCELED: &str = r#"
fn require(value: bool) -> () {
  failure.check(value, Arithmetic())
}

pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let c = match io.monotonic_clock(rt) {
    Ok(c) => c,
    Error(e) => return Error(e),
  }
  task.scope(
    tasks,
    fn(s: Scope) {
      let sleeper = task.async(
        s,
        fn() -> bool {
          match clock.monotonic(c) {
            Ok(start) => match instant.checked_add(c, start, 3600000000000) {
              Some(until) => match clock.sleep_until(c, until) {
                Error(Canceled(Requested())) => true,
                _ => false,
              },
              None() => false,
            },
            Error(_) => false,
          }
        }
      )
      let short = task.async(
        s,
        fn() -> bool {
          match time.sleep(c, duration.from_parts(1, 0)) {
            Ok(()) => true,
            Error(_) => false,
          }
        }
      )
      require(task.await(s, take short))
      task.cancel(s, sleeper)
      require(task.await(s, take sleeper))
      // A cancelled scope fails a later wait before it starts, deadline or not.
    }
  )
  match clock.monotonic(c) {
    Ok(t) => require(instant.ticks(t) == 5 + 1000000000),
    Error(_) => require(false),
  }
  Ok(())
}
"#;

#[test]
fn cancelling_a_pending_wait_removes_its_timer() {
    with_compiler_stack(|| {
        run(CANCELED, clocked(5));
    });
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
        fn call(
            &mut self,
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
            let result = self.library.call(call, cx)?;
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
