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

fn workload(size: u64, chunks: u64, into: bool) -> String {
    include_str!("../fixtures/hex-builder-5-workload.hir")
        .replace("$SIZE", &format!("{size}u64"))
        .replace("$CHUNKS", &format!("{chunks}u64"))
        .replace(
            "$APPEND",
            if into {
                "std@1/hex.encode_into(data, mut out)"
            } else {
                "std@1/text.append(mut out, std@1/hex.encode(data))"
            },
        )
}

/// Explicit bounded harness: these workloads exceed the corpus step budget.
/// Run on the host with --ignored; emit standalone default and measurement
/// executables, then use reports/hex-builder-5/measure.py for interleaved perf.
#[test]
#[ignore = "hex-builder-5 bounded measurement artifact builder"]
fn measurement_artifacts() {
    with_compiler_stack(|| {
        let dir = PathBuf::from("/tmp/hex-builder-5-measure");
        std::fs::create_dir_all(&dir).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let variants = [
            native::RuntimeBuild::default(),
            native::RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        ];
        let tools =
            native::Toolchain::in_tree_with_runtimes(&dir.join("runtime"), &[&target], &variants)
                .unwrap();
        for size in [16, 32, 64, 65536] {
            let chunks = if size == 65536 { 1 } else { 2048 };
            for into in [false, true] {
                let label = format!("{size}-{}", if into { "into" } else { "encode" });
                let source = workload(size, chunks, into);
                std::fs::write(dir.join(format!("{label}.hir")), &source).unwrap();
                let mut loaded = load_program_source(&fixture("workload"), Some(&source)).unwrap();
                loaded.check_for_execution().unwrap_or_else(|e| match e {
                    ToolError::Query { error, .. } => panic!("{label}: {error:?}"),
                    e => panic!("{label}: {e}"),
                });
                let input = loaded.engine.export_native_input().unwrap();
                let validated = loaded.engine.import_native_input(input).unwrap();
                let ready =
                    native::build(&validated, &native::Options::new(target.clone())).unwrap();
                for (index, variant) in variants.into_iter().enumerate() {
                    let destination = dir.join(format!(
                        "{label}-{}",
                        if index == 0 { "default" } else { "diagnostic" }
                    ));
                    std::fs::create_dir_all(&destination).unwrap();
                    let objects = native::emit_objects(
                        &ready,
                        native::OptLevel::Speed,
                        native::Partition::Whole,
                        variant.measurement,
                        None,
                    )
                    .unwrap();
                    native::link_standalone_objects(
                        &ready,
                        &loaded.entry,
                        &objects,
                        &tools.with_runtime(variant),
                        &destination,
                    )
                    .unwrap();
                }
                eprintln!("hex-builder-5 artifact {label}");
            }
        }
    });
}

#[test]
fn existing_hex_decode_errors_on_all_engines() {
    check("corpus:hex", false);
}

#[test]
fn digest_hex_direct_importers_on_all_engines() {
    check("importers", false);
}

#[test]
fn text_wire_direct_importers_on_all_engines() {
    for name in ["multipart", "telemetry", "telemetry-context", "x509"] {
        check(&format!("corpus:{name}"), false);
    }
}
