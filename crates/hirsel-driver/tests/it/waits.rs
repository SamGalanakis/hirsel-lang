//! Timer waits under the simulator's virtual time (core clock.sleep_until, std time.sleep).
use crate::source_std;
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
        config,
    )
    .unwrap();
    assert_eq!(replay.observation, execution.observation);
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
