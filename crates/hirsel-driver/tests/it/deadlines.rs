//! Scope deadlines under the simulator's virtual time (core task.scope_until, CONC-3): a
//! deadline cancels its scope during a sleep, a channel receive, a network read and a
//! streaming child's read; one that never fires leaves no timer; nested deadlines expire in
//! either order; a deadline already reached cancels the scope before its body runs. Each run
//! replays exactly from its recorded events.
use crate::source_std;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::observation::Effect;

/// The run's stdout and its recorded operations, after checking that the recorded events
/// replay the same observation.
fn run(program: &str, config: SimulationConfig) -> (String, Vec<String>) {
    let source = format!("{PRELUDE}{program}");
    let mut engine = source_std::engine();
    let file = engine.sources.add("deadlines.hir", source).unwrap();
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
        "{:?} {}",
        execution.outcome,
        String::from_utf8_lossy(&execution.observation.as_ref().unwrap().output)
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
    (
        String::from_utf8(execution.observation.unwrap().output).unwrap(),
        events.iter().map(|e| e.operation.clone()).collect(),
    )
}

fn clocked() -> SimulationConfig {
    let mut config = SimulationConfig::default();
    config.grants.insert("MonotonicClock".into());
    config
}

fn expiries(operations: &[String]) -> usize {
    operations
        .iter()
        .filter(|o| *o == "task.scope_until.expire")
        .count()
}

const PRELUDE: &str = r#"
fn cause(e: CancelCause) -> String {
  match e {
    Requested() => "requested",
    Parent() => "parent",
    ChildFailed() => "child failed",
    DeadlineExceeded() => "deadline",
  }
}

fn report(b: Bounded[String]) -> String {
  match b {
    Finished(text) => f"finished {text}\n",
    Expired(text) => f"expired {text}\n",
  }
}

fn after(c: MonotonicClock, start: Instant of c, ns: u64) -> Deadline {
  match instant.checked_add(c, start, ns) {
    Some(at) => Deadline(clock: c, at: at),
    None() => Deadline(clock: c, at: start),
  }
}

fn elapsed(c: MonotonicClock, start: Instant of c) -> String {
  match clock.monotonic(c) {
    Ok(now) => f"at {instant.ticks(now) - instant.ticks(start)}\n",
    Error(_) => "no clock\n",
  }
}

// A task's result must be sendable, so a child reports its cancellation, not text.
fn canceled_sleep(c: MonotonicClock, start: Instant of c, ns: u64) -> Option[CancelCause] {
  match instant.checked_add(c, start, ns) {
    Some(at) => match clock.sleep_until(c, at) {
      Error(Canceled(e)) => Some(e),
      _ => None(),
    },
    None() => None(),
  }
}

fn sleep(c: MonotonicClock, start: Instant of c, ns: u64) -> String {
  match canceled_sleep(c, start, ns) {
    Some(e) => cause(e),
    None() => "slept",
  }
}
"#;

const SLEEP: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let bounded = task.scope_until(
    tasks,
    after(c, start, 1000u64),
    fn(s: Scope) -> String {
      sleep(c, start, 5000u64)
    }
  )
  let _ = out.print(output, report(bounded))
  let _ = out.print(output, elapsed(c, start))
  Ok(())
}
"#;

#[test]
fn a_deadline_cancels_a_sleep_and_the_clock_stops_at_the_deadline() {
    with_compiler_stack(|| {
        let (stdout, operations) = run(SLEEP, clocked());
        assert_eq!(stdout, "expired deadline\nat 1000\n");
        assert_eq!(expiries(&operations), 1);
    });
}

// The child waits on a channel whose sender stays live: only the deadline ends it.
const RECEIVE: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let (sender, receiver): (Sender[u64], Receiver[u64]) = channel.open(tasks)
  let bounded = task.scope_until(
    tasks,
    after(c, start, 1000u64),
    fn(s: Scope) -> String {
      let child = task.async(
        s,
        fn() -> Option[CancelCause] {
          let mut r = take receiver
          match channel.receive(tasks, mut r) {
            Error(Canceled(e)) => Some(e),
            _ => None(),
          }
        }
      )
      let received = match task.await(s, take child) {
        Some(e) => cause(e),
        None() => "received",
      }
      let checked = match task.check_canceled(s) {
        Ok(()) => "running",
        Error(Canceled(e)) => cause(e),
      }
      f"{received} {checked}"
    }
  )
  let _ = out.print(output, report(bounded))
  let _ = out.print(output, elapsed(c, start))
  let mut tx = take sender
  let closed: bool = match channel.send(tasks, mut tx, 1u64) {
    Ok(()) => false,
    Error(_) => true,
  }
  failure.check(closed, Arithmetic())
  Ok(())
}
"#;

#[test]
fn a_deadline_cancels_a_childs_channel_receive_and_the_scope() {
    with_compiler_stack(|| {
        let (stdout, operations) = run(RECEIVE, clocked());
        assert_eq!(stdout, "expired deadline deadline\nat 1000\n");
        assert_eq!(expiries(&operations), 1);
    });
}

// The peer sends two bytes, then stalls: the second read waits until the deadline.
const NET: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let n = io.net(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let bounded = task.scope_until(
    tasks,
    after(c, start, 1000u64),
    fn(s: Scope) -> String {
      match net.connect(n, "example.com", 80u16, Unbounded()) {
        Ok(stream) => {
          let mut st = take stream
          let mut buffer: List[u8] = []
          let first = match net.read(n, mut st, mut buffer, 16u64) {
            Ok(count) => f"read {count}",
            Error(_) => "failed",
          }
          let second = match net.read(n, mut st, mut buffer, 16u64) {
            Ok(count) => f"read {count}",
            Error(Canceled(e)) => cause(e),
            Error(_) => "failed",
          }
          f"{first}, {second}"
        },
        Error(_) => "unconnected",
      }
    }
  )
  let _ = out.print(output, report(bounded))
  let _ = out.print(output, elapsed(c, start))
  Ok(())
}
"#;

#[test]
fn a_deadline_cancels_a_stalled_network_read() {
    with_compiler_stack(|| {
        let mut config = clocked();
        config.net = Some(
            serde_json::from_value(serde_json::json!({
                "provider": "net.script@2",
                "rules": [{"address": "example.com", "service": "80", "operation": "connect"}],
                "peers": [{
                    "address": "example.com", "port": 80, "resolves_to": ["127.0.0.1"],
                    "tls": "plain", "connections": [{"open": [{"send": [104, 105]}, "stall"]}]
                }]
            }))
            .unwrap(),
        );
        let (stdout, operations) = run(NET, config);
        assert_eq!(stdout, "expired read 2, deadline\nat 1000\n");
        assert_eq!(expiries(&operations), 1);
    });
}

// The child prints a line, then waits for input that never comes: the second read waits
// until the deadline. A kill is no cancellation point, so the body can still end the child.
const CHILD: &str = r#"
fn utf8(take bytes: List[u8]) -> String {
  match text.finish(take bytes) {
    Ok(s) => s,
    Error(_) => "(not UTF-8)",
  }
}

pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let bounded = task.scope_until(
    tasks,
    after(c, start, 1000u64),
    fn(s: Scope) -> String {
      let mut test = match proc.spawn(p, path.of("/bin/test"), [], None()) {
        Ok(child) => take child,
        Error(_) => return "not spawned",
      }
      let mut first: List[u8] = []
      let _ = child.read_stdout(p, mut test, mut first, 16u64)
      let mut second: List[u8] = []
      let outcome = match child.read_stdout(p, mut test, mut second, 16u64) {
        Ok(count) => f"read {count}",
        Error(Canceled(e)) => cause(e),
        Error(_) => "failed",
      }
      child.kill(p, mut test)
      f"{utf8(take first)}{outcome}"
    }
  )
  let _ = out.print(output, report(bounded))
  let _ = out.print(output, elapsed(c, start))
  Ok(())
}
"#;

#[test]
fn a_deadline_cancels_a_streaming_childs_read() {
    with_compiler_stack(|| {
        let mut config = clocked();
        config.process = Some(
            serde_json::from_value(serde_json::json!({
                "provider": "process.script@1",
                "rules": [{"program": "/bin/test"}],
                "runs": [],
                "children": [{"program": "/bin/test", "args": [], "steps": [
                    {"print": b"a\n".as_slice()}, {"expect": b"go".as_slice()},
                    {"exit": {"exited": 0}}
                ]}]
            }))
            .unwrap(),
        );
        let (stdout, operations) = run(CHILD, config);
        assert_eq!(stdout, "expired a\ndeadline\nat 1000\n");
        assert_eq!(expiries(&operations), 1);
    });
}

// A body that ends first returns Finished, and its exit removes the timer: the clock never
// moves to the deadline.
const NEVER: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let bounded = task.scope_until(
    tasks,
    after(c, start, 1000u64),
    fn(s: Scope) -> String {
      sleep(c, start, 100u64)
    }
  )
  let _ = out.print(output, report(bounded))
  let _ = out.print(output, elapsed(c, start))
  Ok(())
}
"#;

#[test]
fn a_deadline_that_never_fires_leaves_no_timer() {
    with_compiler_stack(|| {
        let (stdout, operations) = run(NEVER, clocked());
        assert_eq!(stdout, "finished slept\nat 100\n");
        assert_eq!(expiries(&operations), 0);
    });
}

// The inner deadline is earlier: only the inner scope expires, and the outer body goes on.
const INNER_FIRST: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let outer = task.scope_until(
    tasks,
    after(c, start, 2000u64),
    fn(s: Scope) -> String {
      let inner = task.scope_until(
        tasks,
        after(c, start, 1000u64),
        fn(t: Scope) -> String {
          sleep(c, start, 5000u64)
        }
      )
      let _ = out.print(output, f"inner {report(inner)}")
      sleep(c, start, 1500u64)
    }
  )
  let _ = out.print(output, f"outer {report(outer)}")
  let _ = out.print(output, elapsed(c, start))
  Ok(())
}
"#;

#[test]
fn an_earlier_inner_deadline_expires_only_the_inner_scope() {
    with_compiler_stack(|| {
        let (stdout, operations) = run(INNER_FIRST, clocked());
        assert_eq!(
            stdout,
            "inner expired deadline\nouter finished slept\nat 1500\n"
        );
        assert_eq!(expiries(&operations), 1);
    });
}

// The outer deadline is earlier: the inner body sees it, so the inner scope finishes with
// the cancellation and the outer one expires.
const OUTER_FIRST: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let outer = task.scope_until(
    tasks,
    after(c, start, 1000u64),
    fn(s: Scope) -> String {
      let inner = task.scope_until(
        tasks,
        after(c, start, 2000u64),
        fn(t: Scope) -> String {
          sleep(c, start, 5000u64)
        }
      )
      match inner {
        Finished(text) => f"inner finished {text}",
        Expired(text) => f"inner expired {text}",
      }
    }
  )
  let _ = out.print(output, f"outer {report(outer)}")
  let _ = out.print(output, elapsed(c, start))
  Ok(())
}
"#;

#[test]
fn an_earlier_outer_deadline_cancels_the_inner_body_and_expires_the_outer_scope() {
    with_compiler_stack(|| {
        let (stdout, operations) = run(OUTER_FIRST, clocked());
        assert_eq!(stdout, "outer expired inner finished deadline\nat 1000\n");
        assert_eq!(expiries(&operations), 1);
    });
}

// A deadline already reached cancels the scope before its body runs: the body's own checks
// and waits see DeadlineExceeded, a child spawned into it is cancelled as its Parent.
const PAST: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let bounded = task.scope_until(
    tasks,
    Deadline(clock: c, at: start),
    fn(s: Scope) -> String {
      let checked = match task.check_canceled(s) {
        Ok(()) => "running",
        Error(Canceled(e)) => cause(e),
      }
      let child = task.async(
        s,
        fn() -> Option[CancelCause] {
          canceled_sleep(c, start, 5000u64)
        }
      )
      let spawned = match task.await(s, take child) {
        Some(e) => cause(e),
        None() => "slept",
      }
      f"{checked} {spawned} {sleep(c, start, 5000u64)}"
    }
  )
  let _ = out.print(output, report(bounded))
  let _ = out.print(output, elapsed(c, start))
  Ok(())
}
"#;

#[test]
fn a_deadline_already_reached_cancels_the_scope_before_its_body_runs() {
    with_compiler_stack(|| {
        let (stdout, operations) = run(PAST, clocked());
        assert_eq!(stdout, "expired deadline parent deadline\nat 0\n");
        assert_eq!(expiries(&operations), 1);
    });
}

// A child of a plain scope the body opened inside the deadline scope is cancelled too, as
// its scope's Parent.
const NESTED_CHILD: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let bounded = task.scope_until(
    tasks,
    after(c, start, 1000u64),
    fn(s: Scope) -> String {
      task.scope(
        tasks,
        fn(inner: Scope) -> String {
          let child = task.async(
            inner,
            fn() -> Option[CancelCause] {
              canceled_sleep(c, start, 5000u64)
            }
          )
          match task.await(inner, take child) {
            Some(e) => cause(e),
            None() => "slept",
          }
        }
      )
    }
  )
  let _ = out.print(output, report(bounded))
  let _ = out.print(output, elapsed(c, start))
  Ok(())
}
"#;

#[test]
fn a_deadline_cancels_the_children_of_scopes_nested_in_its_body() {
    with_compiler_stack(|| {
        let (stdout, operations) = run(NESTED_CHILD, clocked());
        assert_eq!(stdout, "expired parent\nat 1000\n");
        assert_eq!(expiries(&operations), 1);
    });
}
