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

#[test]
fn differential_native() {
    native_union("differential");
}
#[test]
fn invalid_native() {
    native_union("invalid");
}

// Seeds1..61 already completed in the retained first split run.
