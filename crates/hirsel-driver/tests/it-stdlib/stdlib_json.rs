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
    let mut loaded =
        load_program_source(&fixture("corpus"), Some(source)).unwrap_or_else(|e| panic!("{e}"));
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
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
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

fn native_measurement(source: &str, label: &str) -> String {
    use hirsel_driver::native::{
        self, OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain,
    };
    let mut loaded =
        load_program_source(&fixture("corpus"), Some(source)).unwrap_or_else(|e| panic!("{e}"));
    loaded
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{e}"));
    let snapshot = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(snapshot).unwrap();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut options = Options::new(target.clone());
    options.regions = false;
    let ready = native::build(&validated, &options).unwrap();
    let dir = std::env::temp_dir().join(format!("hirsel-json-{label}-{}", std::process::id()));
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
    std::fs::remove_dir_all(dir).unwrap();
    report
}
fn allocations(report: &str) -> u64 {
    report
        .lines()
        .filter_map(|line| line.strip_prefix("class "))
        .map(|line| {
            line.split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .sum()
}
fn counter(report: &str, name: &str) -> u64 {
    report
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn parsed_short_tokens_copy_only_their_own_storage_when_made_unique() {
    with_compiler_stack(|| {
        let source = std::fs::read_to_string(
            root().join("crates/hirsel-driver/tests/fixtures/stdlib_json/owned_tokens.hir"),
        )
        .unwrap();
        let report = native_measurement(&source, "owned-tokens");
        // 256 tiny tokens in a 64 KiB document must not copy the whole backing.
        assert!(counter(&report, "copied_bytes") < 1024 * 1024, "{report}");
    });
}

#[test]
fn derived_byte_decoder_moves_recursive_results_without_detach() {
    with_compiler_stack(|| {
        let source = std::fs::read_to_string(
            root().join("crates/hirsel-driver/tests/fixtures/stdlib_json/owned_decoder.hir"),
        )
        .unwrap();
        let derived = native_measurement(&source, "derived-decoder");
        let direct = native_measurement(
            &source.replace(
                "let decoded_tree = tree_from_json_bytes(",
                "let decoded_tree = direct(",
            ),
            "direct-decoder",
        );
        eprintln!("derived decoder:\n{derived}direct reader factory:\n{direct}");
        assert_eq!(
            allocations(&derived),
            allocations(&direct),
            "extra decoder allocations\n{derived}\n{direct}"
        );
        assert_eq!(
            counter(&derived, "live_bytes"),
            0,
            "early cleanup: {derived}"
        );
        assert_eq!(counter(&direct, "deep_copies"), 0, "{direct}");
        for name in ["deep_copies", "copied_blocks", "copied_bytes"] {
            assert_eq!(
                counter(&derived, name),
                counter(&direct, name),
                "extra final detach: {name}\n{derived}\n{direct}"
            );
        }
        let errors = format!(
            "{}{}",
            source.split("pub fn main()").next().unwrap(),
            r#"
        pub fn main() -> Result[(), ()] {
          let inputs: List[Bytes] = [
            b"{\"Branch\":[{\"Leaf\":\"built\"},{\"Leaf\":false}]}",
            b"{\"Branch\":[{\"Leaf\":\"built\"}]} trailing",
            b"{\"Branch\":[{\"Leaf\":\"built\"},"
          ]
          for input in inputs {
            let decoded = tree_from_json_bytes(input)
            if results.is_ok(decoded) { return Error(()) }
          }
          Ok(())
        }
        "#
        );
        let cleanup = native_measurement(&errors, "decoder-errors");
        assert_eq!(
            counter(&cleanup, "live_bytes"),
            0,
            "early error cleanup: {cleanup}"
        );
    });
}

// Match the existing collection-vector harness: large vectors run natively,
// without changing the interpreter's one-million-step guard.
fn execute_native_source(name: &str, source: &str) {
    with_compiler_stack(|| {
        let dir = std::env::temp_dir().join(format!("hirsel-json-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.hir");
        std::fs::write(&path, source).unwrap();
        let manifest = serde_json::json!({
            "schema": 1,
            "package": {"name": "json_vectors", "version": "1"},
            "entry": {"module": [], "name": "main"},
            "sources": [{"path": "main.hir", "module": []}],
            "dependencies": [{
                "manifest": root().join("stdlib/std@1/hirsel.json").canonicalize().unwrap(),
                "package": {"name": "std", "version": "1"}, "expose": []
            }]
        });
        std::fs::write(path.with_extension("hir.json"), manifest.to_string()).unwrap();
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let executable = dir.join(backend.name());
            native::build_path_native_backend_with_validation(
                &path,
                &executable,
                native::TargetSpec::x86_64_unknown_linux_gnu(),
                native::BuildProfile::Debug,
                backend,
                None,
                false,
                hirsel_engine::Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap()
            .unwrap();
            let output = Command::new(&executable).output().unwrap();
            assert!(
                output.status.success(),
                "{name}/{backend:?}: {:?}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                output.stdout.is_empty(),
                "{name}/{backend:?}: {:?}",
                output.stdout
            );
            assert!(
                output.stderr.is_empty(),
                "{name}/{backend:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn scoped_wide_objects_and_nested_key_reuse() {
    execute_native_source(
        "wide",
        include_str!("../fixtures/stdlib_json/scoped_wide.hir"),
    );
}

#[test]
fn scoped_wide_duplicate_error_offsets_and_precedence() {
    execute_native_source(
        "duplicates",
        include_str!("../fixtures/stdlib_json/scoped_duplicates.hir"),
    );
}

#[test]
fn direct_json_float_scalars_do_not_allocate_temporary_number_strings() {
    with_compiler_stack(|| {
        let source = r#"
type Floats { double: f64, single: f32 }
fn floats_from_json_reader(mut cursor: std@1/json.Reader) -> Result[unique Floats, String] uses {} = comptime derive.from_json_reader(shape(Floats))
fn floats_from_json_bytes(data: Bytes) -> Result[unique Floats, String] uses {} = comptime derive.from_json_bytes(shape(Floats))
@preserves_unique(out)
fn floats_json_into(value: Floats, mut out: List[u8], depth: u64) -> Result[(), std@1/json.JsonError] uses {} = comptime derive.json_into(shape(Floats))
pub fn main(t: Test) -> Result[(), ()] {
  let mut out: List[u8] = []
  list.reserve(mut out, 128)
  for _ in 0u64..1024 {
    let Ok(value) = floats_from_json_bytes(b"{\"double\":0.125,\"single\":0.5}") else { return Error(()) }
    test.equal(t, value.double, 0.125)
    test.equal(t, value.single, 0.5f32)
    let Ok(()) = floats_json_into(value, mut out, 0) else { return Error(()) }
    test.equal(t, list.len(out), 29u64)
    while list.len(out) > 0 { let _ = list.pop(mut out) }
  }
  Ok(())
}
"#;
        let report = native_measurement(source, "float-stream");
        assert!(allocations(&report) < 128, "{report}");
        assert_eq!(counter(&report, "live_bytes"), 0, "{report}");
    });
}
