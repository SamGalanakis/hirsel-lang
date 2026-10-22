//! Independent RFC6902 vectors, exact decimals, limits and atomic ownership on
//! H, checked heap, LLVM and Cranelift. Source attribution is in each fixture.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/json-patch-15/{name}.hir"))
}

fn checked(name: &str) {
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture(name)).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{name}: {error:?}"),
                error => panic!("{name}: {error}"),
            });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(100_000_000),
                ..Default::default()
            },
            ..Default::default()
        }));
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
            "{name}: {:?}",
            reference.outcome
        );
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
            &native::HeapWorld {
                steps: Some(100_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected, "{name}");
        assert_eq!(heap.counters.live_heap_bytes, 0, "{name}");
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
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("json-patch-15-{name}"));
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{name} {config}: {outcome:?}"
            );
        }
        eprintln!("json-patch-15 {name}: executed H, heap, LLVM, Cranelift; zero live heap bytes");
    });
}

macro_rules! vectors {
    ($test:ident, $name:literal) => {
        #[test]
        fn $test() {
            checked($name);
        }
    };
}
vectors!(official_0, "tests-0");
vectors!(official_1, "tests-1");
vectors!(official_2, "tests-2");
vectors!(official_3, "tests-3");
vectors!(official_4, "tests-4");
vectors!(official_5, "tests-5");
vectors!(rfc_examples, "spec_tests-0");
vectors!(atomic_numeric_controls, "controls");
vectors!(bounds, "limits");

vectors!(overlap_and_root_edges, "edges");
