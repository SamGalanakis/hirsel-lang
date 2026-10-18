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
