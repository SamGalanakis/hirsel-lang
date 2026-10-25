//! Append encoding: independent byte oracle, alias control and audited failure cleanup.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/hex-builder-5-{name}.hir"))
}

fn check(name: &str, campaign: bool) {
    with_compiler_stack(|| {
        let path = if let Some(corpus) = name.strip_prefix("corpus:") {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/SEM")
                .join(format!("SEM-3-{corpus}-run.hir"))
        } else {
            fixture(name)
        };
        let mut loaded = load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        // The combined importer control is an explicit bounded Rust harness,
        // rather than a corpus program above the corpus's 1M-step cap.
        let importer_steps = (name == "importers").then_some(20_000_000);
        if let Some(steps) = importer_steps {
            loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(steps),
                    call_depth: 256,
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
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        for name in if name.starts_with("corpus:") || name == "importers" {
            &[][..]
        } else {
            &["encode_into", "append_digits"][..]
        } {
            let unit = ready
                .units
                .iter()
                .find(|u| {
                    let d = &u.unit().header.definition.0;
                    d.package.name_is("std") && d.modules == ["hex"] && d.name == *name
                })
                .unwrap();
            assert_eq!(
                unit.unit().header.origin,
                hirsel_lir::ir::UnitOrigin::Source
            );
        }
        let sites = native::Sites::new(validated.snapshot());
        let heap = if campaign {
            native::heap_campaign(&ready, &loaded.entry, &sites)
        } else {
            native::run_heap_in(
                &ready,
                &loaded.entry,
                &sites,
                &native::HeapWorld {
                    steps: importer_steps,
                    ..Default::default()
                },
            )
        }
        .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        if name == "cleanup" {
            assert_eq!(heap.counters.finalizers, 1);
        }
        eprintln!(
            "hex-builder-5 {name}: failure_sites={} allocations={} frees={} peak_bytes={}",
            heap.sites,
            heap.counters.allocations,
            heap.counters.frees,
            heap.counters.peak_live_heap_bytes
        );
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
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("hex-builder-5-{name}"));
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{name}: {config}: {outcome:?}"
            );
        }
    });
}

#[test]
fn vectors_on_interpreter_heap_llvm_and_cranelift() {
    check("vectors", false);
}

#[test]
fn aliases_uniqueness_and_failure_cleanup() {
    check("cleanup", true);
}
