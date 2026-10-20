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
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{outcome:?}"
        );
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
    source_codec("utf8", "utf8", &["decode_lossy"]);
}

#[test]
fn char_is_ordinary_source() {
    source_codec("char", "char", &["format_into"]);
}

#[test]
fn common_strings_are_ordinary_source() {
    source_codec("string-common", "string", &["contains", "lines", "replace"]);
}

#[test]
fn sha256_is_ordinary_source() {
    source_codec("sha256", "sha256", &["new", "update", "finish", "digest"]);
}

// Multi-megabyte streams exceed the corpus interpreter budget. Both compiled
// backends must match reference digests; bounded corpus cases cover interpretation.
#[test]
#[ignore = "full gate: native multi-megabyte hash references"]
fn full_gate_large_hash_native_references_on_both_backends() {
    with_compiler_stack(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hash-large-native.hir");
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = native::run_path_native_backend(
                &path,
                HostLaunch::empty(),
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                hirsel_engine::Verify::On,
            )
            .unwrap()
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_config::Returned::Ok)
                ),
                "{backend:?}: {outcome:?}"
            );
            assert!(stdout.is_empty(), "{backend:?}: {stdout:?}");
            assert!(
                stderr.is_empty(),
                "{backend:?}: {}",
                String::from_utf8_lossy(&stderr)
            );
        }
    });
}

#[test]
fn flate_is_ordinary_source() {
    source_codec("flate-inflate", "flate", &["inflate"]);
}

// Go's gettysburg member and multi-block streams exceed the corpus step budget;
// the bounded corpus vectors exercise the same source kernel.
#[test]
fn flate_large_vectors_execute_std_source() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flate-large.hir");
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(5_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
}

// Go zip testdata readme.zip inflates past the corpus step budget.
#[test]
fn zip_large_vectors_execute_std_source() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zip-large.hir");
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(5_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
}

// zlib's output at window, block-split and stored-fallback scale runs only as
// a built executable; the corpus covers every level on short inputs.
#[test]
#[ignore = "full gate: native deflate reference"]
fn full_gate_deflate_matches_zlib_native() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/deflate-zlib-native.hir");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = native::run_path_native(&path, &mut stdout, &mut stderr)
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{outcome:?}"
        );
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    });
}

#[test]
fn cookie_is_ordinary_source() {
    source_codec("cookie", "cookie", &["new", "parse", "format"]);
}

#[test]
fn x509_is_ordinary_source() {
    source_codec("x509", "x509", &["certificates", "parse"]);
}

#[test]
fn shlex_is_ordinary_source() {
    source_codec("shlex-quote", "shlex", &["split", "quote", "join"]);
}

#[test]
fn uuid_is_ordinary_source() {
    source_codec("uuid-layout", "uuid", &["parse", "format_into", "v7_from"]);
    source_codec("uuid-random", "uuid", &["v4"]);
}

// The wall clock is not grantable to a corpus program; version 7 UUIDs take
// their timestamp from it. Random is random.splitmix64@1 with seed 0.
#[test]
fn uuid_v7_reads_the_wall_clock() {
    with_compiler_stack(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/uuid-v7-clock.hir");
        for (wall, expected) in [
            (
                1_645_557_742_123_456_789i64,
                "017f22e2-7a2b-7fcd-9d7b-39a820e2f465\n",
            ),
            (999_999, "00000000-0000-7fcd-9d7b-39a820e2f465\n"),
            (-1, "before epoch\n"),
        ] {
            let mut simulation = SimulationConfig::default();
            simulation.grants.insert("WallClock".into());
            simulation.wall = wall;
            let mut stdout = Vec::new();
            let outcome = run_path_with_step_budget(
                &path,
                simulation,
                None,
                hirsel_engine::Verify::On,
                &mut stdout,
                &mut Vec::new(),
            )
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_config::Returned::Ok)
                ),
                "{outcome:?}"
            );
            assert_eq!(String::from_utf8(stdout).unwrap(), expected, "wall {wall}");
        }
    });
}

#[test]
fn semver_is_ordinary_source() {
    source_codec(
        "semver-npm-match-1",
        "semver",
        &["parse", "parse_npm", "matches"],
    );
}

#[test]
fn diff_is_ordinary_source() {
    source_codec("diff-pairs-1", "diff", &["edits", "unified", "apply"]);
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
    source_codec("log", "log", &["to_string", "write"]);
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
            assert!(matches!(
                observed.outcome,
                Ok(RunOutcome::Returned(hirsel_run_config::Returned::Ok))
            ));
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
        &["expect_ok", "expect_error", "property_cases"],
    );
}

// Corpus outcomes cannot count finalizers or their ordering relative to reports.
#[test]
fn owned_results_and_requested_property_cases_finalize_once_before_reporting() {
    with_compiler_stack(|| {
        use hirsel_interp::observation::Effect;
        for (relative, finalizers, passes) in [
            ("SEM/SEM-3-test-extra-run.hir", 5, true),
            ("DROP/DROP-2-expect-ok-error-fail.hir", 1, false),
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
            if passes {
                assert!(matches!(
                    observed.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_config::Returned::Ok))
                ));
            } else {
                assert!(
                    matches!(observed.outcome, Ok(RunOutcome::Failed { primary: hirsel_run_config::Failure { ref rule, .. }, .. }) if rule.as_str() == "LANG-9")
                );
            }
            let observation = observed.observation.unwrap();
            let drops: Vec<_> = observation.effects.iter().enumerate().filter_map(|(index, effect)| matches!(&effect.effect, Effect::Finalizer { definition } if definition.0.name == "Watch").then_some(index)).collect();
            // Passing property cases report nothing; each value is released in order.
            assert_eq!(drops.len(), finalizers);
            assert!(observation.output.is_empty());
            if !passes {
                let failure = observation.effects.iter().position(|effect| matches!(&effect.effect, Effect::CapabilityConsumed(event) if event.operation == "test.fail")).unwrap();
                assert!(drops[0] < failure);
            }
        }
    });
}

#[test]
fn yaml_is_ordinary_source() {
    source_codec("yaml", "yaml", &["parse", "parse_all", "stringify"]);
}

#[test]
fn toml_is_ordinary_source() {
    source_codec("toml-edit-cargo", "toml", &["document", "set", "source"]);
}

#[test]
fn yaml_documents_are_ordinary_source() {
    source_codec(
        "yaml-edit-remove",
        "yaml",
        &["document", "source", "remove"],
    );
}

// Full 1024-byte quoted and 1025-byte explicit keys exceed the corpus step budget.
fn yaml_large_fixture(fixture: &str) {
    with_compiler_stack(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{fixture}.hir"));
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(5_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
}

#[test]
fn yaml_explicit_1025_byte_key_reads_back() {
    yaml_large_fixture("yaml-key-explicit");
}

#[test]
fn yaml_quoted_1024_byte_key_reads_back() {
    yaml_large_fixture("yaml-key-quoted");
}

// The full tagged invoice includes four floats and exceeds the corpus step limit.
#[test]
fn yaml_tagged_invoice_round_trips_beyond_corpus_budget() {
    yaml_large_fixture("yaml-tagged-invoice");
}

#[test]
fn telemetry_is_ordinary_source() {
    source_codec(
        "telemetry",
        "telemetry",
        &[
            "check_log",
            "render",
            "write_stderr",
            "enrich",
            "allow_fields",
        ],
    );
}

#[test]
fn telemetry_context_is_ordinary_source() {
    source_codec(
        "telemetry-context",
        "telemetry",
        &["extract_trace", "inject_trace", "validate_trace"],
    );
}

#[test]
fn telemetry_limits_and_goldens_agree_on_both_native_backends() {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let limits = root.join("crates/hirsel-driver/tests/fixtures/telemetry/limits.hir");
        let outcome = run_path_with_step_budget(
            &limits,
            SimulationConfig::default(),
            Some(20_000_000),
            Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{outcome:?}"
        );
        for fixture in [
            "conformance/SEM/SEM-3-telemetry-run.hir",
            "conformance/SEM/SEM-3-telemetry-context-run.hir",
            "conformance/SEM/SEM-3-log-run.hir",
            "crates/hirsel-driver/tests/fixtures/telemetry/limits.hir",
        ] {
            let path = root.join(fixture);
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                let grants =
                    if fixture.ends_with("context-run.hir") || fixture.ends_with("limits.hir") {
                        "{}"
                    } else {
                        r#"{"io":{"dirs":[],"stderr":"stderr.bytes@1"}}"#
                    };
                let outcome = native::run_path_native_backend(
                    &path,
                    HostLaunch::Live {
                        config: LaunchConfig::Grants {
                            input: std::ffi::OsStr::new(grants),
                            arguments: None,
                        },
                        record: None,
                        revocations: None,
                    },
                    None,
                    backend,
                    native::Output::Streams {
                        stdout: &mut stdout,
                        stderr: &mut stderr,
                    },
                    Verify::On,
                )
                .unwrap()
                .unwrap();
                assert!(
                    matches!(
                        outcome,
                        RunOutcome::Returned(hirsel_run_config::Returned::Ok)
                    ),
                    "{fixture} {backend:?}: {outcome:?}"
                );
                assert!(stdout.is_empty(), "{fixture} {backend:?}: {stdout:?}");
            }
        }
    });
}

#[test]
fn telemetry_stderr_filters_propagates_faults_and_refuses_changed_replay_bytes() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-telemetry-run.hir");
        let source = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let sink = io.stderr(rt)?
  let Ok(record) = std@1/telemetry.check_log(std@1/telemetry.LogRecord(
    level: Info(), name: "n", message: "redacted",
    metadata: std@1/telemetry.Metadata(trace: None(), fields: []))) else {
    failure.check(false, TextInvariant()); return Ok(())
  }
  failure.check(std@1/telemetry.write_stderr(sink, Error(), record) == Ok(()), TextInvariant())
  let outcome = std@1/telemetry.write_stderr(sink, Debug(), record)
  $CHECK
  Ok(())
}
"#;
        for fault in [false, true] {
            let source = source.replace(
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
            let run = RunConfiguration {
                entry: loaded.entry,
                hash_seed: 0,
                inputs: vec![],
            };
            let observed =
                run_observed_snapshot(&mut loaded.engine, &run, simulation.clone()).unwrap();
            assert!(
                matches!(
                    observed.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_config::Returned::Ok))
                ),
                "{:?}",
                observed.outcome
            );
            let observation = observed.observation.unwrap();
            let inputs: Vec<_> = observation
                .effects
                .iter()
                .filter_map(|effect| match &effect.effect {
                    hirsel_interp::observation::Effect::CapabilityConsumed(event) => {
                        Some(event.clone())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                inputs.iter().filter(|e| e.operation == "err.print").count(),
                1
            );
            let replay = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    inputs: inputs.clone(),
                    ..run.clone()
                },
                simulation.clone(),
            )
            .unwrap();
            assert!(
                matches!(
                    replay.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_config::Returned::Ok))
                ),
                "{:?}",
                replay.outcome
            );
            assert_eq!(observation, replay.observation.unwrap());
            let mut changed = inputs;
            let write = changed
                .iter_mut()
                .find(|e| e.operation == "err.print")
                .unwrap();
            assert!(!write.input.is_empty());
            *write.input.last_mut().unwrap() ^= 1;
            let refused = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    inputs: changed,
                    ..run
                },
                simulation.clone(),
            )
            .unwrap();
            assert!(
                refused.outcome.is_err(),
                "changed stderr bytes accepted: {:?}",
                refused.outcome
            );
        }
    });
}

#[test]
fn telemetry_native_recording_replays_and_changed_stderr_request_refuses() {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = root.join("conformance/SEM/SEM-3-telemetry-run.hir");
        let scratch =
            std::env::temp_dir().join(format!("hirsel-telemetry-replay-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let tape = scratch.join("recording.tape");
        let changed = scratch.join("changed.tape");
        let outcome = native::run_path_native_backend(
            &path,
            HostLaunch::Live {
                config: LaunchConfig::Grants {
                    input: std::ffi::OsStr::new(r#"{"io":{"dirs":[],"stderr":"stderr.bytes@1"}}"#),
                    arguments: None,
                },
                record: Some(&tape),
                revocations: None,
            },
            None,
            native::Backend::Cranelift,
            native::Output::Streams {
                stdout: &mut Vec::new(),
                stderr: &mut Vec::new(),
            },
            Verify::On,
        )
        .unwrap()
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{outcome:?}"
        );
        let outcome = run_path_on_host(&path, HostLaunch::Replay(&tape), None, Verify::On).unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{outcome:?}"
        );
        let mut record =
            hirsel_rt::launch::parse_recording(&std::fs::read_to_string(&tape).unwrap()).unwrap();
        let write = record
            .events
            .iter_mut()
            .find_map(|event| match &mut event.request {
                hirsel_rt::io::Request::PrintStderr { bytes, .. } => Some(bytes),
                _ => None,
            })
            .unwrap();
        let byte = write.iter_mut().find(|b| **b == b'o').unwrap();
        *byte = b'x';
        // Re-render a valid tape: refusal must come from the changed request,
        // not from syntax, the program revision, or an invalid run binding.
        std::fs::write(
            &changed,
            hirsel_rt::launch::render_recording(&record).unwrap(),
        )
        .unwrap();
        let refused =
            run_path_on_host(&path, HostLaunch::Replay(&changed), None, Verify::On).unwrap_err();
        assert!(refused.to_string().contains("replay"), "{refused}");
        std::fs::remove_dir_all(scratch).unwrap();
    });
}
