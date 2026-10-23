//! Bounded default-budget runs cover every case; native runs execute their union.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/base64-block-20/{name}.hir"))
}

fn batches(name: &str, seeds: std::ops::Range<u64>) {
    with_compiler_stack(|| {
        let mut loaded = load_program(&path(name)).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let mut ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let seeds: Vec<u64> = if name == "invalid_window" {
            seeds
                .filter(|seed| *seed > 61)
                .flat_map(|seed| (0..12).map(move |window| seed + window * 1216))
                .collect()
        } else {
            seeds.collect()
        };
        for seed in seeds {
            let reference = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig {
                    test_seed: seed,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                matches!(
                    reference.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{name} seed {seed}: {:?}",
                reference.outcome
            );
            let expected =
                native::Observation::of_reference(reference.observation.as_ref().unwrap());
            ready.launch.test_seed = seed;
            let heap = native::run_heap(
                &ready,
                &loaded.entry,
                &native::Sites::new(validated.snapshot()),
            )
            .unwrap_or_else(|error| panic!("{name} seed {seed}: {error}"));
            assert_eq!(heap.observation, expected, "{name} seed {seed}");
            assert_eq!(heap.counters.live_heap_bytes, 0, "{name} seed {seed}");
        }
    });
}

fn native_union(name: &str) {
    with_compiler_stack(|| {
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = native::run_path_native_backend_with_validation(
                &path(name),
                HostLaunch::Live {
                    config: LaunchConfig::Grants {
                        input: std::ffi::OsStr::new("{}"),
                        arguments: None,
                    },
                    record: None,
                    revocations: None,
                },
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap()
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{name} {backend:?}: {outcome:?}"
            );
            assert!(stdout.is_empty(), "{name} {backend:?}: {stdout:?}");
            assert!(
                stderr.is_empty(),
                "{name} {backend:?}: {}",
                String::from_utf8_lossy(&stderr)
            );
        }
    });
}

macro_rules! batch {
    ($test:ident, $source:literal, $start:literal, $end:literal) => {
        #[test]
        fn $test() {
            batches($source, $start..$end);
        }
    };
}

batch!(differential_0, "differential", 1, 151);
batch!(differential_150, "differential", 151, 301);
batch!(differential_300, "differential", 301, 451);
batch!(differential_450, "differential", 451, 601);

#[test]
fn differential_native() {
    native_union("differential");
}
#[test]
fn invalid_native() {
    native_union("invalid");
}

// Seeds1..61 already completed in the retained first split run.
batch!(invalid_standard_required_prefix, "invalid", 1, 62);
batch!(invalid_window_39, "invalid_window", 39, 77);
batch!(invalid_window_77, "invalid_window", 77, 115);
batch!(invalid_window_115, "invalid_window", 115, 153);
batch!(invalid_window_153, "invalid_window", 153, 191);
batch!(invalid_window_191, "invalid_window", 191, 229);
batch!(invalid_window_229, "invalid_window", 229, 267);
batch!(invalid_window_267, "invalid_window", 267, 305);
batch!(invalid_window_305, "invalid_window", 305, 343);
batch!(invalid_window_343, "invalid_window", 343, 381);
batch!(invalid_window_381, "invalid_window", 381, 419);
batch!(invalid_window_419, "invalid_window", 419, 457);
batch!(invalid_window_457, "invalid_window", 457, 495);
batch!(invalid_window_495, "invalid_window", 495, 533);
batch!(invalid_window_533, "invalid_window", 533, 571);
batch!(invalid_window_571, "invalid_window", 571, 609);
batch!(invalid_window_609, "invalid_window", 609, 647);
batch!(invalid_window_647, "invalid_window", 647, 685);
batch!(invalid_window_685, "invalid_window", 685, 723);
batch!(invalid_window_723, "invalid_window", 723, 761);
batch!(invalid_window_761, "invalid_window", 761, 799);
batch!(invalid_window_799, "invalid_window", 799, 837);
batch!(invalid_window_837, "invalid_window", 837, 875);
batch!(invalid_window_875, "invalid_window", 875, 913);
batch!(invalid_window_913, "invalid_window", 913, 951);
batch!(invalid_window_951, "invalid_window", 951, 989);
batch!(invalid_window_989, "invalid_window", 989, 1027);
batch!(invalid_window_1027, "invalid_window", 1027, 1065);
batch!(invalid_window_1065, "invalid_window", 1065, 1103);
batch!(invalid_window_1103, "invalid_window", 1103, 1141);
batch!(invalid_window_1141, "invalid_window", 1141, 1179);
batch!(invalid_window_1179, "invalid_window", 1179, 1217);
