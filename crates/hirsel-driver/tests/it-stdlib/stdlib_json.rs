//! Source JSON conformance plus an independent Python json/number-lexeme oracle.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{path::PathBuf, process::Command};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn fixture(name: &str) -> PathBuf {
    root().join(format!("conformance/SEM/SEM-3-json-{name}-run.hir"))
}
fn execute_source(source: &str) {
    let mut loaded = load_program_source(&fixture("corpus"), Some(source)).unwrap();
    loaded.check_for_execution().unwrap();
    let run = run_observed_snapshot(
        &mut loaded.engine,
        &RunConfiguration {
            entry: loaded.entry,
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .unwrap();
    assert!(
        matches!(
            run.outcome,
            Ok(RunOutcome::Returned(hirsel_run_config::Returned::Ok))
        ),
        "{:?}",
        run.outcome
    );
}
fn python_oracle(fuzz: bool) -> String {
    let mut command = Command::new("python3");
    command.arg(root().join("crates/hirsel-driver/tests/fixtures/stdlib_json/oracle.py"));
    if fuzz {
        command.arg("--fuzz");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn python_json_oracle_preserves_every_valid_corpus_number_lexeme() {
    with_compiler_stack(|| execute_source(&python_oracle(false)));
}
#[test]
fn seeded_bounded_json_fuzz_matches_python_policy_and_exact_numbers() {
    // One 32-tree + 32-mutation campaign in the ordinary per-push driver tests.
    with_compiler_stack(|| execute_source(&python_oracle(true)));
}

#[test]
fn parsed_short_tokens_copy_only_their_own_storage_when_made_unique() {
    with_compiler_stack(|| {
        use hirsel_driver::native::{
            self, OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain,
        };
        let source = std::fs::read_to_string(
            root().join("crates/hirsel-driver/tests/fixtures/stdlib_json/owned_tokens.hir"),
        )
        .unwrap();
        let mut loaded = load_program_source(&fixture("corpus"), Some(&source)).unwrap();
        loaded.check_for_execution().unwrap();
        let snapshot = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(snapshot).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut options = Options::new(target.clone());
        options.regions = false;
        let ready = native::build(&validated, &options).unwrap();
        let dir =
            std::env::temp_dir().join(format!("hirsel-json-owned-tokens-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tools = Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        let exe = native::link(
            &ready,
            &loaded.entry,
            OptLevel::Speed,
            Partition::Whole,
            &tools,
            &dir,
        )
        .unwrap();
        let totals = dir.join("totals");
        let result = Command::new(exe)
            .env_clear()
            .env("HIRSEL_RT_MEASUREMENT", &totals)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report = std::fs::read_to_string(totals).unwrap();
        let copied: u64 = report
            .lines()
            .find_map(|line| line.strip_prefix("copied_bytes "))
            .unwrap()
            .parse()
            .unwrap();
        // A 64 KiB padded document with 256 one-byte tokens must not copy the
        // document 256 times (16 MiB). Allow ample room for container bookkeeping.
        assert!(copied < 1024 * 1024, "{report}");
        std::fs::remove_dir_all(dir).unwrap();
    });
}
