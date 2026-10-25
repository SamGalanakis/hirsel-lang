//! Explicit Writer admission and flush retain only uncommitted bytes.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn bufio_writer5_script_record_replay_is_exact() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/bufio-writer5-replay.hir");
        let mut loaded = load_program(&path).unwrap();
        let run = RunConfiguration {
            entry: loaded.entry,
            hash_seed: 0,
            inputs: vec![],
        };
        let simulation = SimulationConfig::default();
        let observed = run_observed_snapshot(&mut loaded.engine, &run, simulation.clone()).unwrap();
        assert!(
            matches!(
                observed.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            observed.outcome
        );
        let observation = observed.observation.unwrap();
        assert_eq!(
            observation.output,
            b"offer 6162636465\noffer 636465\noffer 636465\nabcde\n"
        );
        let inputs: Vec<_> = observation
            .effects
            .iter()
            .filter_map(|effect| match &effect.effect {
                hirsel_interp::observation::Effect::CapabilityConsumed(event) => {
                    Some(event.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            inputs.iter().filter(|e| e.operation == "out.print").count(),
            4
        );
        let replay = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration { inputs, ..run },
            simulation,
        )
        .unwrap();
        assert!(
            matches!(
                replay.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            replay.outcome
        );
        assert_eq!(observation, replay.observation.unwrap());
    });
}

// Go's complete 10x10 writer-size matrix runs as bounded native code rather
// than increasing the corpus's 1M interpreter-step budget.
#[test]
fn bufio_writer5_go_writer_matrix_native() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/bufio-writer5-go-matrix.hir");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = native::run_path_native(&path, &mut stdout, &mut stderr)
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    });
}
