//! Rust Vec::truncate/from_elem vectors and physical allocation controls.
use crate::source_std::load_program_source;
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

fn fixture(group: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance")
        .join(group)
        .join(format!("{name}.hir"))
}

fn check(path: &Path, source: Option<&str>, stdout: &[u8], failure: Option<&str>, campaign: bool) {
    let mut loaded = load_program_source(path, source).unwrap();
    loaded.check_for_execution().unwrap_or_else(|e| match e {
        ToolError::Query { error, .. } => panic!("{error:?}"),
        e => panic!("{e}"),
    });
    loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: Some(20_000_000),
            call_depth: 256,
            source: None,
        },
        ..Default::default()
    }));
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
    let observation = run.observation.as_ref().unwrap();
    assert_eq!(observation.output, stdout);
    match failure {
        Some(rule) => assert!(
            matches!(run.outcome,
            Ok(RunOutcome::Failed { primary: hirsel_run_schema::Failure { rule: ref actual, .. }, .. })
            if actual.as_str() == rule),
            "{:?}",
            run.outcome
        ),
        None => assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            run.outcome
        ),
    }
    let expected = native::Observation::of_reference(observation);
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input).unwrap();
    let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    let ready = native::build(&validated, &options).unwrap();
    let sites = native::Sites::new(validated.snapshot());
    let world = native::HeapWorld {
        steps: Some(20_000_000),
        ..Default::default()
    };
    let heap = if campaign {
        native::heap_campaign_in(&ready, &loaded.entry, &sites, &world)
    } else {
        native::run_heap_in(&ready, &loaded.entry, &sites, &world)
    }
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
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "list-filled-truncate-{}",
        path.file_stem().unwrap().to_str().unwrap()
    ));
    let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
    for (configuration, outcome) in
        native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
    {
        assert!(
            matches!(outcome, native::Outcome::Agree),
            "{configuration}: {outcome:?}"
        );
    }
}

#[test]
fn rust_repeat_and_truncate_vectors_on_all_engines() {
    with_compiler_stack(|| {
        check(
            &fixture("SEM", "SEM-4-list-filled-truncate-run"),
            None,
            b"",
            None,
            false,
        )
    });
}

#[test]
fn truncate_drop_counts_order_and_failure_cleanup_on_all_engines() {
    with_compiler_stack(|| {
        check(
            &fixture("DROP", "DROP-2-list-truncate-order-run"),
            None,
            b"before\nD\nE\nthree\nA\nB\nC\nzero\n",
            None,
            true,
        );
        let path = fixture("DROP", "DROP-2-list-truncate-failure");
        let source = std::fs::read_to_string(&path).unwrap();
        for condition in ["x.n != 2", "x.n != 3", "x.n != 2 && x.n != 3"] {
            // Retained A drops after all tail finalizers, even if the first
            // or middle tail finalizer fails. No tail is dropped twice.
            check(
                &path,
                Some(&source.replace("x.n != 2", condition)),
                b"B\nC\nD\nA\n",
                Some("SEM-1"),
                true,
            );
        }
    });
}

#[test]
fn impossible_large_inline_count_fails_before_filling_on_all_engines() {
    with_compiler_stack(|| {
        check(
            &fixture("SEM", "SEM-1-list-filled-capacity-failure"),
            None,
            b"",
            Some("SEM-1"),
            false,
        )
    });
}

#[test]
fn filled_zero_large_and_regrowth_keep_one_allocation_on_both_native_engines() {
    with_compiler_stack(|| {
        let path = fixture("SEM", "SEM-4-list-filled-truncate-run");
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("list-filled-8-counts");
        let tools = native::Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            native::RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        for (n, value) in [(0, 0), (1, 0), (4096, 0), (4096, 85)] {
            let source = format!(
                r#"
fn build(n: u64) -> List[u8] {{ list.filled(n, {value}u8) }}
pub fn main() -> Result[(), ()] {{
  let mut xs = build({n})
  failure.check(list.len(xs) == {n}, Bounds())
  for item in xs {{ failure.check(item == {value}u8, Bounds()) }}
  list.truncate(mut xs, {n} / 2)
  while list.len(xs) < {n} {{ list.push(mut xs, {value}u8) }}
  failure.check(list.len(xs) == {n}, Bounds())
  Ok(())
}}
"#
            );
            let mut loaded = load_program_source(&path, Some(&source)).unwrap();
            loaded
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{e:?}"));
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let mut options = native::Options::new(target.clone());
            options.regions = false;
            let ready = native::build(&validated, &options).unwrap();
            let heap = native::run_heap_in(
                &ready,
                &loaded.entry,
                &native::Sites::new(validated.snapshot()),
                &native::HeapWorld {
                    steps: Some(20_000_000),
                    ..Default::default()
                },
            )
            .unwrap();
            let expected = u64::from(n != 0);
            assert_eq!(heap.counters.allocations, expected, "n={n} value={value}");
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let case = dir.join(format!("{backend:?}-{n}-{value}"));
                std::fs::create_dir_all(&case).unwrap();
                let objects = native::emit_backend_objects(
                    &ready,
                    backend,
                    native::OptLevel::Speed,
                    native::Partition::Whole,
                    true,
                    None,
                )
                .unwrap();
                let exe =
                    native::link_objects(&ready, &loaded.entry, &objects, &tools, &case).unwrap();
                let totals = case.join("totals");
                let run = std::process::Command::new(exe)
                    .env_clear()
                    .env("HIRSEL_RT_MEASUREMENT", &totals)
                    .output()
                    .unwrap();
                assert!(
                    run.status.success(),
                    "{}",
                    String::from_utf8_lossy(&run.stderr)
                );
                let report = std::fs::read_to_string(totals).unwrap();
                let allocations: u64 = report
                    .lines()
                    .filter_map(|line| line.strip_prefix("class "))
                    .map(|line| {
                        line.split_whitespace()
                            .nth(1)
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                    })
                    .sum();
                assert_eq!(
                    allocations, expected,
                    "{backend:?} n={n} value={value}\n{report}"
                );
                eprintln!(
                    "list-filled-8 {backend:?} n={n} value={value}: {allocations} allocations (including truncate/regrow)"
                );
            }
        }
    });
}

#[test]
fn impossible_reserve_preserves_fixed_shared_empty_and_packed_contents() {
    with_compiler_stack(|| {
        check(
            &fixture("MEM", "MEM-14-wave1-reserve-caught-run"),
            None,
            b"fixed\nshared\nempty\npacked\ncaught\n",
            None,
            true,
        )
    });
}

#[test]
fn filled_plain_header_excludes_counted_seeds() {
    with_compiler_stack(|| {
        let path = fixture("SEM", "SEM-4-list-filled-truncate-run");
        let source =
            "pub fn main() { let seed: List[u64] = [1]; let rows = list.filled(2, copy seed) }";
        let mut loaded = load_program_source(&path, Some(source)).unwrap();
        let output = loaded.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(!output.diagnostics.is_empty());
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.message.contains("plain")),
            "{:?}",
            output.diagnostics
        );
    });
}
