//! Primitive work is part of both interpreter step limits and fuel budgets.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Execution, Limits, Outcome, RunBudgets};
use hirsel_tir::RunConfiguration;

fn engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("numeric-work.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(
        output
            .diagnostics
            .iter()
            .all(|d| d.severity != hirsel_diag::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    engine
}
fn run(
    engine: &mut QueryEngine,
    checked: bool,
    steps: Option<u64>,
    fuel: Option<u64>,
) -> Result<Execution, hirsel_interp::ExecutionError> {
    let sources = engine.sources.clone();
    let config = RunConfiguration {
        entry: entry_path(&default_context()),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut runtime = if checked {
        engine.checked_runtime_types().unwrap()
    } else {
        engine.runtime_types().unwrap()
    };
    runtime.budgets = RunBudgets {
        fuel,
        memory_bytes: None,
    };
    let mut library = hirsel_interp::library::Library::new(&config, Default::default()).unwrap();
    hirsel_interp::run_with_runtime(
        engine,
        &sources,
        &config,
        vec![],
        Limits {
            steps,
            call_depth: 256,
        },
        &mut library,
        &runtime,
    )
}
fn minimum_fuel(engine: &mut QueryEngine, checked: bool) -> u64 {
    let (mut low, mut high) = (0, 256);
    while low < high {
        let mid = (low + high) / 2;
        if matches!(
            run(engine, checked, None, Some(mid)).unwrap().outcome,
            Outcome::Returned { .. }
        ) {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    assert!(matches!(
        run(engine, checked, None, Some(low)).unwrap().outcome,
        Outcome::Returned { .. }
    ));
    low
}
#[test]
fn numeric_primitives_charge_each_byte_before_publishing() {
    with_compiler_stack(|| {
        for operation in ["json", "integer"] {
            let source = |text: &str| match operation {
                "json" => format!(
                    "pub fn main()->u64 {{ let (end,valid)=bytes.json_number_end(b\"{text}\",0); end }}"
                ),
                _ => format!(
                    "pub fn main()->u64 {{ let (high,low,status)=bytes.integer(b\"{text}\",10,false,0,18446744073709551615,0,0); low }}"
                ),
            };
            let mut short = engine(&source("1"));
            let mut long = engine(&source("12345678901234567890"));
            for checked in [false, true] {
                let before = run(&mut short, checked, None, None).unwrap();
                let after = run(&mut long, checked, None, None).unwrap();
                assert!(matches!(after.outcome, Outcome::Returned { .. }));
                assert_eq!(
                    after.steps - before.steps,
                    19,
                    "{operation}, checked={checked}"
                );
                assert_eq!(
                    minimum_fuel(&mut long, checked) - minimum_fuel(&mut short, checked),
                    19
                );
                let bounded = run(&mut long, checked, Some(before.steps), None).unwrap();
                assert_eq!(bounded.steps, before.steps);
                assert!(matches!(bounded.outcome, Outcome::ToolingLimit(_)));
            }
        }
    });
}

// This fixed vector batch exceeded the corpus 1M budget. Like the existing
// float_parse_scan_13 suite, keep it Rust-hosted without a corpus step cap.
#[test]
fn clinger_exact_bits_and_fallback_twins_on_four_engines() {
    with_compiler_stack(|| {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_numeric.hir");
        let mut loaded =
            load_program_source(&path, Some(include_str!("../fixtures/float-fast-38.hir")))
                .unwrap();
        loaded.check_for_execution().unwrap();
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: None,
                call_depth: 256,
            },
            ..Default::default()
        }));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let plain =
            run_observed_snapshot(&mut loaded.engine, &run, SimulationConfig::default()).unwrap();
        let checked =
            run_observed_checked(&mut loaded.engine, &run, SimulationConfig::default()).unwrap();
        for result in [&plain, &checked] {
            assert!(
                matches!(
                    result.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{:?}",
                result.outcome
            );
        }
        assert_eq!(plain.observation, checked.observation);
        let expected = native::Observation::of_reference(plain.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
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
        let scratch = tempfile::Builder::new()
            .prefix("float-fast-38-")
            .tempdir()
            .unwrap();
        let tools =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configs).unwrap();
        for (config, outcome) in native::differential(
            &validated,
            &loaded.entry,
            &expected,
            &configs,
            &tools,
            scratch.path(),
        ) {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{config}: {outcome:?}"
            );
        }
        eprintln!("418 exact-rational float vectors passed R, checked H, LLVM, Cranelift");
    });
}

#[test]
fn numeric_decimal_oracle_runs_heap_with_fault_injection() {
    with_compiler_stack(|| {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-numeric-work-38-run.hir");
        let mut loaded = load_program_source(&path, None).unwrap();
        loaded.check_for_execution().unwrap();
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
        let snapshot = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(snapshot).unwrap();
        // Precisely the failed smoke configuration; native/Cranelift and Io
        // replay greens are retained separately, without another whole matrix.
        let configs = vec![native::Configuration {
            options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            engine: native::Engine::Heap { campaign: true },
        }];
        let scratch = tempfile::Builder::new()
            .prefix("numeric-heap-38-")
            .tempdir()
            .unwrap();
        let tools =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configs).unwrap();
        for (config, outcome) in native::differential(
            &validated,
            &loaded.entry,
            &expected,
            &configs,
            &tools,
            scratch.path(),
        ) {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{config}: {outcome:?}"
            );
        }
    });
}
