//! Go dnsmessage model/name/message vectors, with raw-octet adaptations.
//! Independent wire fixtures; BSD-3-Clause attribution in third-party/NOTICES.md.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/dns-message-15/{name}.hir"))
}

fn all_engines(name: &str) {
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture(name)).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(10_000_000),
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
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{name}: {:?}",
            reference.outcome
        );
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        if name == "summary" {
            assert_eq!(expected.output, b"DNS id=4660 questions=1\n");
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
                steps: Some(10_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        assert!(heap.counters.peak_live_heap_bytes < 4_000_000);
        assert!(
            heap.counters.allocations < 100_000,
            "{name}: {:?}",
            heap.counters
        );
        eprintln!(
            "{name}: {} heap allocations, zero live bytes",
            heap.counters.allocations
        );
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
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("dns-message-15-{name}"));
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        let mut count = 0;
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{name}: {config}: {outcome:?}"
            );
            count += 1;
        }
        assert_eq!(count, 2);
        eprintln!("dns-message-15 {name}: interpreter + H + LLVM + Cranelift agree");
    });
}

#[test]
fn go_reference_vectors_all_engines() {
    all_engines("vectors");
}

#[test]
fn typed_errors_all_engines() {
    all_engines("errors");
}

#[test]
fn go_large_message_inspection() {
    all_engines("go-large");
}
#[test]
fn bounded_work_and_lengths() {
    all_engines("limits");
}

#[test]
fn literal_packet_summary_without_dns_traffic() {
    all_engines("summary");
}
