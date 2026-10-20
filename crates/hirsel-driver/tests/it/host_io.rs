use crate::source_std;
use hirsel_driver::*;
use hirsel_engine::*;

fn execute(source: &str, simulation: SimulationConfig) -> hirsel_engine::ObservedExecution {
    let mut engine = source_std::engine();
    let file = engine.sources.add("host-io.hir", source.into()).unwrap();
    let context = default_context();
    let checked = check_progress(&mut engine, file, &context).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let run = RunConfiguration {
        entry: entry_path(&context),
        hash_seed: 0,
        inputs: vec![],
    };
    let result = run_observed_snapshot(&mut engine, &run, simulation.clone()).unwrap();
    assert!(
        matches!(
            result.outcome,
            Ok(RunOutcome::Returned(hirsel_run_config::Returned::Ok))
        ),
        "{:?}",
        result.outcome
    );
    let events = result
        .observation
        .as_ref()
        .unwrap()
        .effects
        .iter()
        .filter_map(|e| match &e.effect {
            hirsel_interp::observation::Effect::CapabilityConsumed(input) => Some(input.clone()),
            _ => None,
        })
        .collect();
    let mut replay_input = simulation;
    replay_input.stdin.clear(); // Recorded chunks are the only stdin provider on replay.
    let replay = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            inputs: events,
            ..run
        },
        replay_input,
    )
    .unwrap();
    assert!(
        matches!(
            replay.outcome,
            Ok(RunOutcome::Returned(hirsel_run_config::Returned::Ok))
        ),
        "{:?}",
        replay.outcome
    );
    assert_eq!(result.observation, replay.observation);
    result
}

#[test]
fn rust_read_until_vectors_preserve_newlines_unterminated_lines_and_eof() {
    with_compiler_stack(|| {
        for (stdin, args) in [
            ("", vec![]),
            ("12", vec!["12"]),
            ("12\n\n", vec!["12\n", "\n"]),
            ("λ\r\nlast", vec!["λ\r\n", "last"]),
        ] {
            execute(
                include_str!("../../../../conformance/CAP/CAP-1-stdin-lines-run.hir"),
                SimulationConfig {
                    stdin: stdin.as_bytes().to_vec(),
                    args: args.into_iter().map(str::to_owned).collect(),
                    ..Default::default()
                },
            );
        }
    });
}

#[test]
fn environment_names_distinguish_denial_unset_and_empty_with_replay() {
    with_compiler_stack(|| {
        execute(
            include_str!("../../../../conformance/CAP/CAP-1-env-names-run.hir"),
            SimulationConfig {
                args: vec!["check".into()],
                env: [
                    ("SET".into(), Some("λ🐑".into())),
                    ("UNSET".into(), None),
                    ("EMPTY".into(), Some("".into())),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        );
    });
}

#[test]
fn stderr_and_stdout_remain_separate_in_semantic_observations() {
    with_compiler_stack(|| {
        let result = execute(
            include_str!("../../../../conformance/CAP/CAP-1-stderr-stream-run.hir"),
            SimulationConfig::default(),
        );
        let observation = result.observation.unwrap();
        assert_eq!(observation.output, b"stdout\n");
        assert_eq!(observation.final_state.unwrap().stderr, b"stderr\n");
    });
}

#[test]
fn invalid_utf8_line_is_consumed_without_replacement_text() {
    with_compiler_stack(|| {
        execute(
            include_str!("../../../../conformance/CAP/CAP-1-stdin-invalid-line-run.hir"),
            SimulationConfig {
                stdin: vec![255, 10],
                args: vec!["invalid".into()],
                ..Default::default()
            },
        );
    });
}

// Extra simulator points must suspend once without duplicating a store, root
// release or charge; neither these points nor race hints enter observations.
#[test]
fn optional_simulator_points_fire_once_and_diagnostics_preserve_observations() {
    use hirsel_interp::library::simulation::{SchedulePoint, SimulationDebug};
    with_compiler_stack(|| {
        let source = r#"
fn release(take root: Shared[String]) {}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let counter: Atomic[u64] = atomic.create(tasks, 0u64)
  atomic.store(tasks, counter, 1u64)
  let built: String = "root"
  let root = shared.freeze(take built)
  release(take root)
  Ok(())
}
"#;
        let mut engine = source_std::engine();
        let file = engine.sources.add("sim-points.hir", source.into()).unwrap();
        let context = default_context();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let signature = engine.item_signature(&entry).unwrap().value;
        let runtime = engine.runtime_types().unwrap();
        let sources = engine.sources.clone();
        let config = RunConfiguration {
            entry,
            hash_seed: 0,
            inputs: vec![],
        };
        let mut run = |debug| {
            hirsel_interp::library::launch_with_runtime(
                &mut engine,
                &sources,
                &signature,
                &config,
                SimulationConfig {
                    debug,
                    ..Default::default()
                },
                hirsel_interp::Limits::default(),
                &runtime,
            )
            .unwrap()
        };
        let ordinary = run(SimulationDebug::default());
        let debug = SimulationDebug {
            races: true,
            root_releases: true,
            atomic_stores: true,
        };
        let explored = run(debug);
        let points: Vec<_> = explored
            .execution
            .trace
            .iter()
            .filter_map(|event| match event {
                hirsel_interp::TraceEvent::SimulationPoint(point) => Some(*point),
                _ => None,
            })
            .collect();
        assert_eq!(
            points,
            [SchedulePoint::AtomicStore, SchedulePoint::RootRelease]
        );
        assert!(!ordinary.execution.trace.iter().any(|e| matches!(
            e,
            hirsel_interp::TraceEvent::SimulationPoint(_)
                | hirsel_interp::TraceEvent::RaceCandidate(_)
        )));
        assert_eq!(ordinary.execution.steps, explored.execution.steps);
        assert_eq!(
            ordinary.observation(&sources),
            explored.observation(&sources)
        );
        assert_eq!(
            explored
                .events
                .iter()
                .filter(|event| event.operation == "atomic.store")
                .count(),
            1
        );
    });
}
