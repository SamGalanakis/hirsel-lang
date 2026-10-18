//! Source-origin checks cannot be expressed by a conformance program.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_lir::ir::UnitOrigin;
use std::path::PathBuf;

fn source_codec(name: &str, module: &str, functions: &[&str]) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../conformance/SEM/SEM-3-{name}-run.hir"));
        let mut loaded = load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{error:?}"),
                error => panic!("{error}"),
            });
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        for function in functions {
            let unit = ready
                .units
                .iter()
                .find(|unit| {
                    let def = &unit.unit().header.definition.0;
                    def.package.name_is("std") && def.modules == [module] && def.name == *function
                })
                .unwrap_or_else(|| panic!("missing {module}/{function}"));
            assert_eq!(unit.unit().header.origin, UnitOrigin::Source);
        }
        let outcome = run_path(&path, &mut Vec::new()).unwrap();
        assert!(matches!(outcome, RunOutcome::Exit(0)), "{outcome:?}");
    });
}

#[test]
fn hex_is_ordinary_source() {
    source_codec("hex", "hex", &["encode", "decode"]);
}

#[test]
fn base64_is_ordinary_source() {
    source_codec("base64", "base64", &["encode", "decode"]);
}

#[test]
fn utf8_is_ordinary_source() {
    source_codec("utf8", "utf8", &["append_scalar", "decode_lossy"]);
}

#[test]
fn common_strings_are_ordinary_source() {
    source_codec("string-common", "string", &["contains", "lines", "replace"]);
}

#[test]
fn sha256_is_ordinary_source() {
    source_codec("sha256", "sha256", &["new", "update", "finish", "digest"]);
}

// The million-byte vector runs only as a built executable; bounded corpus
// vectors exercise the same source kernel through the interpreter.
#[test]
#[ignore = "full gate: native million-byte reference"]
fn full_gate_sha256_million_a_native_reference() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sha256-million-native.hir");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = native::run_path_native(&path, &mut stdout, &mut stderr)
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, RunOutcome::Exit(0)), "{outcome:?}");
        assert!(stdout.is_empty(), "{stdout:?}");
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    });
}

#[test]
fn unicode_properties_are_ordinary_source() {
    source_codec(
        "unicode-properties",
        "unicode",
        &[
            "is_whitespace",
            "is_letter",
            "is_digit",
            "simple_lower",
            "simple_upper",
        ],
    );
}

#[test]
fn unicode16_property_tables_regenerate() {
    assert!(
        std::process::Command::new("python3")
            .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .args(["stdlib/unicode16/generate.py", "--check"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn ascii_radix_is_ordinary_source() {
    source_codec("ascii-radix", "ascii", &["digit_value_radix"]);
}

#[test]
fn csv_is_ordinary_source() {
    source_codec(
        "csv",
        "csv",
        &["reader", "feed", "parse", "read_record", "write_record"],
    );
}

#[test]
fn cli_is_ordinary_source() {
    source_codec("cli", "cli", &["parse", "usage"]);
}

#[test]
fn log_is_ordinary_source() {
    source_codec("log", "log", &["format", "write"]);
}

// The source corpus cannot inject Stderr provider faults. This additionally
// proves that a filtered entry leaves the scripted write fault untouched.
#[test]
fn logging_filters_before_write_and_propagates_sink_errors() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-log-run.hir");
        for fault in [false, true] {
            let source = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let sink = io.stderr(rt)?
  let quiet = std@1/log.Entry(level: Debug(), message: "ignored", fields: [], timestamp: None())
  failure.check(std@1/log.write(sink, Info(), quiet) == Ok(()), TextInvariant())
  let entry = std@1/log.Entry(level: Info(), message: "a\n\"b", fields: [], timestamp: None())
  let outcome = std@1/log.write(sink, Info(), entry)
  $CHECK
  Ok(())
}
"#
            .replace(
                "$CHECK",
                if fault {
                    "failure.check(outcome == Error(OutputFault(7)), TextInvariant())"
                } else {
                    "failure.check(outcome == Ok(()), TextInvariant())"
                },
            );
            let mut loaded = load_program_source(&path, Some(&source)).unwrap();
            loaded.check_for_execution().unwrap();
            let mut simulation = SimulationConfig::default();
            if fault {
                simulation.faults.insert(
                    "err.print".into(),
                    std::collections::VecDeque::from([
                        hirsel_interp::library::simulation::Fault::Fault {
                            code: 7,
                            partial: 0,
                        },
                    ]),
                );
            }
            let observed = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry,
                    hash_seed: 0,
                    inputs: vec![],
                },
                simulation,
            )
            .unwrap();
            assert!(matches!(observed.outcome, Ok(RunOutcome::Exit(0))));
            let observation = observed.observation.unwrap();
            let writes = observation.effects.iter().filter(|effect| matches!(&effect.effect, hirsel_interp::observation::Effect::CapabilityConsumed(event) if event.operation == "err.print")).count();
            assert_eq!(writes, 1);
            let state = observation.final_state.unwrap();
            let expected = if fault {
                b"".as_slice()
            } else {
                b"{\"level\":\"info\",\"message\":\"a\\n\\\"b\",\"fields\":[],\"timestamp\":null}\n"
                    .as_slice()
            };
            assert_eq!(state.stderr, expected);
        }
    });
}

#[test]
fn extra_test_helpers_are_ordinary_source() {
    source_codec(
        "test-extra",
        "test",
        &["expect_ok", "expect_error", "for_all_cases"],
    );
}

// Corpus outcomes cannot count finalizers or their ordering relative to reports.
#[test]
fn owned_results_and_requested_property_cases_finalize_once_before_reporting() {
    with_compiler_stack(|| {
        use hirsel_interp::observation::Effect;
        for (relative, finalizers, cases) in [
            ("SEM/SEM-3-test-extra-run.hir", 5, 3),
            ("DROP/DROP-2-expect-ok-error-fail.hir", 1, 0),
        ] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance")
                .join(relative);
            let mut loaded = load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            let observed = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry,
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            if cases > 0 {
                assert!(matches!(observed.outcome, Ok(RunOutcome::Exit(0))));
            } else {
                assert!(
                    matches!(observed.outcome, Ok(RunOutcome::Failure { ref rule, .. }) if rule.as_str() == "LANG-9")
                );
            }
            let observation = observed.observation.unwrap();
            let drops: Vec<_> = observation.effects.iter().enumerate().filter_map(|(index, effect)| matches!(&effect.effect, Effect::Finalizer { definition } if definition.0.name == "Watch").then_some(index)).collect();
            let reports: Vec<_> = observation.effects.iter().enumerate().filter_map(|(index, effect)| matches!(&effect.effect, Effect::CapabilityConsumed(event) if event.operation == "test.check_case").then_some(index)).collect();
            assert_eq!(drops.len(), finalizers);
            assert_eq!(reports.len(), cases);
            if cases == 0 {
                let failure = observation.effects.iter().position(|effect| matches!(&effect.effect, Effect::CapabilityConsumed(event) if event.operation == "test.fail")).unwrap();
                assert!(drops[0] < failure);
            }
            for (index, report) in reports.iter().enumerate() {
                assert!(drops[index + 2] < *report);
                if index + 1 < reports.len() {
                    assert!(*report < drops[index + 3]);
                }
            }
        }
    });
}
