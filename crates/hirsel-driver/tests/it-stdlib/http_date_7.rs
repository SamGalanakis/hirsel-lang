//! Strict HTTP dates: fixed text/epoch oracles and typed failures on H.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn check(name: &str) {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let path = if name == "vectors" {
            root.join("tests/fixtures/http-date-7-calendar.hir")
        } else {
            root.join(format!(
                "../../conformance/SEM/SEM-3-http-date-7-{name}-run.hir"
            ))
        };
        let mut loaded = load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{error:?}"),
                error => panic!("{error}"),
            });
        // The full 33-vector workload belongs in this bounded harness, not the corpus.
        let steps = (name == "vectors").then_some(5_000_000);
        if let Some(steps) = steps {
            loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(steps),
                    call_depth: 256,
                    source: None,
                },
                ..Default::default()
            }));
        }
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            reference.outcome
        );
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = native::Sites::new(validated.snapshot());
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &sites,
            &native::HeapWorld {
                steps,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        if name == "vectors" {
            let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
            let configs = vec![
                native::Configuration {
                    options: options.clone(),
                    engine: native::Engine::Native {
                        opt: native::OptLevel::Speed,
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    },
                },
                native::Configuration {
                    options,
                    engine: native::Engine::Cranelift {
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    },
                },
            ];
            let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("http-date-7-calendar");
            let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
            for (config, outcome) in
                native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
            {
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{config}: {outcome:?}"
                );
            }
        }
    });
}

#[test]
fn canonical_calendar_vectors() {
    check("vectors");
}

#[test]
fn strict_parse_errors() {
    check("reject");
}

#[test]
fn format_range_and_precision() {
    check("format");
}
