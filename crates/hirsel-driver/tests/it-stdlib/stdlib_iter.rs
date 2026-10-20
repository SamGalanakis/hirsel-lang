use hirsel_driver::*;
use hirsel_engine::{
    RunConfiguration, RunOutcome, SimulationConfig, run_observed_checked, with_compiler_stack,
};
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

// Corpus output checks cannot distinguish source bodies from privileged fallbacks.
#[test]
fn iterator_helpers_are_ordinary_source_native_units() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/CLO/CLO-4-std-iter-run.hir");
        let mut loaded = load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let mut found = BTreeSet::new();
        for unit in &ready.units {
            let header = &unit.unit().header;
            if header.definition.0.package.name_is("std") {
                assert_eq!(header.origin, UnitOrigin::Source);
                if header.definition.0.modules == ["iter"] {
                    found.insert(header.definition.0.name.as_str());
                }
            }
        }
        for name in ["filter", "take_n", "enumerate", "zip", "collect", "fold"] {
            assert!(
                found.contains(name),
                "missing source iter.{name}: {found:?}"
            );
        }
    });
}

// Peer scripts establish wire framing; the observation additionally rules out
// sending a successful terminator after the source error, even to a ready peer.
#[test]
fn byte_stream_upload_finishes_only_after_clean_eof() {
    use hirsel_interp::observation::Effect;
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-4-stream-bytes-upload-run.hir");
        let mut program = load_program(&path).unwrap();
        program
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        assert!(
            matches!(
                execution.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            execution.outcome
        );
        let observation = execution.observation.unwrap();
        let events: Vec<_> = observation
            .effects
            .iter()
            .filter_map(|effect| match &effect.effect {
                Effect::CapabilityConsumed(event) => Some(event),
                _ => None,
            })
            .collect();
        let terminators: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(event.operation.as_str(), "net.write" | "net.write_all")
                    && event.input.windows(3).any(|bytes| bytes == b"0\r\n")
            })
            .collect();
        assert_eq!(terminators.len(), 1, "{terminators:?}");
        assert!(
            terminators[0]
                .input
                .windows(b"0\r\nX-End: yes\r\n\r\n".len())
                .any(|bytes| bytes == b"0\r\nX-End: yes\r\n\r\n")
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.operation == "net.close")
                .count(),
            3
        );
    });
}

// Tokio ce685ee tokio-util/tests/io_reader_stream.rs:39-67. The exact 8000-byte
// vector exceeds the corpus's 1M steps; this execution test uses normal fuel.
#[test]
fn byte_stream_tokio_short_read_then_error_vector() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/stream_bytes_tokio/main.hir");
        let mut program = load_program(&path).unwrap();
        program
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        program.engine.adapters.interpreter = Some(Box::new(hirsel_engine::InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(16_000_000),
                call_depth: 256,
            },
            ..Default::default()
        }));
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let result =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        assert!(
            matches!(
                result.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            result.outcome
        );
    });
}
