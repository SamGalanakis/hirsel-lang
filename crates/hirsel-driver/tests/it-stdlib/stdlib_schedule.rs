//! Corpus laws cover observable traces; these tests assert capability replay and
//! injected wait/entropy failures, which source fixtures cannot configure.
use crate::source_std;
use hirsel_engine::*;
use hirsel_interp::{library::simulation::Fault, observation::Effect};

fn run_replayed(engine: &mut QueryEngine, entry: hirsel_syntax::DefPath, config: SimulationConfig) {
    let run = RunConfiguration {
        entry,
        hash_seed: 0,
        inputs: vec![],
    };
    let execution = run_observed_snapshot(engine, &run, config.clone()).unwrap();
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
    assert!(events.iter().any(|e| e.operation == "clock.monotonic"));
    let replay = run_observed_snapshot(
        engine,
        &RunConfiguration {
            inputs: events,
            ..run
        },
        config,
    )
    .unwrap();
    assert_eq!(replay.observation, execution.observation);
}

fn clocked() -> SimulationConfig {
    let mut config = SimulationConfig::default();
    config.grants.insert("MonotonicClock".into());
    config
}

#[test]
fn schedule_seeded_waits_replay_exactly() {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-schedule-jitter-run.hir");
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        run_replayed(&mut loaded.engine, loaded.entry, clocked());
    });
}

#[test]
fn schedule_infrastructure_errors_preserve_last_error_and_attempt_count() {
    with_compiler_stack(|| {
        let mut engine = source_std::engine();
        let source = r#"
pub fn main(rt: Io, args: Args) -> Result[(),GrantError] {
  let expected = match arg.text(args,0) { Ok(text) => text, Error(_) => "" }
  let c = io.monotonic_clock(rt)?
  let r = io.random(rt)?
  let policy = schedule.max_attempts(schedule.jittered(schedule.spaced(10),Full()),3)
  let retried = schedule.retry(c,r,policy,fn() -> Result[(),u64] { Error(77) },fn(e:u64) -> bool { true })
  failure.check(match retried {
    Error(Interrupted(Some(77),1,Random(Source(Unavailable())))) => expected == "random.bytes",
    Error(Interrupted(Some(77),1,Sleep(Wait(Clock(Unavailable()))))) => expected == "clock.sleep_until",
    Error(Interrupted(None(),0,Clock(Unavailable()))) => expected == "clock.monotonic",
    _ => false,
  },Arithmetic())
  Ok(())
}
"#;
        let file = engine
            .sources
            .add("schedule-errors.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        for operation in ["random.bytes", "clock.sleep_until", "clock.monotonic"] {
            let mut config = clocked();
            config.args = vec![operation.into()];
            if operation == "random.bytes" {
                // Random has an explicit provider seam rather than a fault queue.
                let run = RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                };
                let mut library = hirsel_interp::library::Library::new(&run, config).unwrap();
                let args = vec![
                    library.simulation.grant_root("Io").unwrap(),
                    library.simulation.grant_root("Args").unwrap(),
                ];
                library.simulation.supply_random(Err(()));
                let runtime = engine.runtime_types().unwrap();
                let sources = engine.sources.clone();
                let execution = hirsel_interp::run_with_runtime(
                    &mut engine,
                    &sources,
                    &run,
                    args,
                    Default::default(),
                    &mut library,
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
                assert!(execution.failures.is_empty());
                continue;
            }
            config.faults.insert(
                operation.into(),
                [Fault::Fault {
                    code: 0,
                    partial: 0,
                }]
                .into(),
            );
            run_replayed(&mut engine, entry_path(&default_context()), config);
        }
    });
}
