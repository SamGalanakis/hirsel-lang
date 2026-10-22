//! Rust alloctests sort vectors: stable occurrence order, permutations and
//! panic-retain-original-set, with move-only resource owners and two failure sites.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn check(name: &str) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/list_sort/drift-{name}.hir"));
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(80_000_000),
                call_depth: 256,
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
        let outcome = reference.outcome.as_ref().unwrap();
        if name == "owned-fail" {
            assert!(
                matches!(outcome, RunOutcome::Failed { primary, .. }
                if primary.rule.as_str() == "MEM-14" && primary.site.line == 6),
                "{outcome:?}"
            );
        } else {
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{outcome:?}"
            );
        }
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        if name.starts_with("owned") {
            let text = std::str::from_utf8(&expected.output).unwrap();
            let mut ids: Vec<u64> = text
                .lines()
                .map(|line| line.strip_prefix("D ").unwrap().parse().unwrap())
                .collect();
            ids.sort_unstable();
            assert_eq!(ids, (0..129).collect::<Vec<_>>());
        }
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        let sites = native::Sites::new(validated.snapshot());
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &sites,
            &native::HeapWorld {
                steps: Some(80_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        if name.starts_with("owned") {
            assert_eq!(heap.counters.finalizers, 129);
        }
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
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("drift-{name}"));
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, result) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(result, native::Outcome::Agree),
                "{name}: {config}: {result:?}"
            );
        }
    });
}

#[test]
fn rust_stability_and_patterns() {
    check("vectors");
}
#[test]
fn move_only_ownership() {
    check("owned");
}
#[test]
fn comparator_failure_order_and_ownership() {
    check("owned-fail");
}

#[test]
fn scratch_policy_above_full_allocation_limit() {
    // Rust's 1_100_000-element length vector crosses the 8MB index scratch cap.
    // This explicit native harness avoids exceeding corpus/interpreter fuel.
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/list_sort/drift-vectors.hir");
        let source = std::fs::read_to_string(&path).unwrap();
        let prefix = source.split("pub fn main()").next().unwrap();
        let source = format!(
            "{prefix}pub fn main() -> Result[(), ()] {{\n  for pattern in 0u64..8u64 {{ check(1100000, pattern) }}\n  Ok(())\n}}\n"
        );
        let mut loaded = load_program_source(&path, Some(&source)).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
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
        let expected = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: vec![],
            failures: vec![],
            roots: vec![],
        };
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("drift-large");
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, result) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(result, native::Outcome::Agree),
                "{config}: {result:?}"
            );
        }
    });
}
