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
