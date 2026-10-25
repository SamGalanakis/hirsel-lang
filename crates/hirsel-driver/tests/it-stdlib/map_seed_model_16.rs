//! Mixed SEM-5 traces; the oracle is shared with the private native seed harness.
#[path = "../../../hirsel-pipeline-tests/tests/rc/maps/model.rs"]
mod model;
use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn ordered_vector_traces_interpreted_and_checked_heap() {
    with_compiler_stack(|| {
        let traces = model::traces();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let source = model::source(&traces);
        let mut loaded = load_program_source(&path, Some(&source)).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            hirsel_engine::ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(10_000_000),
                call_depth: 256,
            },
            ..Default::default()
        }));
        let mut expected = None;
        for seed in model::SEEDS {
            let config = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: seed,
                inputs: vec![],
            };
            let run =
                run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
                    .unwrap();
            assert!(
                matches!(
                    run.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "seed={seed:#x}: {:?}; {}",
                run.outcome,
                report(run.observation.as_ref(), &traces)
            );
            let observation = native::Observation::of_reference(run.observation.as_ref().unwrap());
            assert!(observation.failures.is_empty(), "{observation:?}");
            let checked =
                run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default())
                    .unwrap();
            assert_eq!(
                native::Observation::of_reference(checked.observation.as_ref().unwrap()),
                observation
            );
            if let Some(expected) = &expected {
                assert_eq!(&observation, expected);
            } else {
                expected = Some(observation);
            }
        }
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
            &native::HeapWorld {
                steps: Some(10_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected.unwrap());
        assert_eq!(heap.counters.live_heap_bytes, 0);
        eprintln!(
            "32 traces: four seeds, interpreter + checked interpreter; H agrees, zero live heap bytes"
        );
        assert!(model::failure(1075, &traces).contains("shortest failing prefix=7"));
    });
}

fn report(
    observation: Option<&hirsel_interp::observation::Observable>,
    traces: &[model::Trace],
) -> String {
    observation
        .and_then(|o| o.returned.as_ref())
        .and_then(|value| {
            value.nodes.iter().find_map(|node| match node {
                hirsel_interp::observation::ObservedNode::Integer(value) => {
                    Some(value.bits() as u64)
                }
                _ => None,
            })
        })
        .map_or_else(
            || "no returned mismatch code".into(),
            |code| model::failure(code, traces),
        )
}

fn all_engines(name: &str, source: &str, returned: hirsel_run_schema::Returned) {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let mut loaded = load_program_source(&path, Some(source)).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            hirsel_engine::ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        let run = run_observed_snapshot(
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
            matches!(run.outcome, Ok(RunOutcome::Returned(actual)) if actual == returned),
            "{name}: {:?}",
            run.outcome
        );
        let expected = native::Observation::of_reference(run.observation.as_ref().unwrap());
        assert!(expected.failures.is_empty(), "{expected:?}");
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        let heap = native::run_heap(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
        )
        .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        let configs = [
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
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("map-seed-model-16-{name}"));
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        let outcomes =
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir);
        assert_eq!(outcomes.len(), 2);
        for (config, outcome) in outcomes {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{name}: {config}: {outcome:?}"
            );
        }
        eprintln!("{name}: interpreter + H + LLVM + Cranelift agree; zero live heap bytes");
    });
}
