//! Source-origin checks cannot be expressed by a conformance program.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_lir::ir::UnitOrigin;
use std::path::PathBuf;

#[path = "bufio_writer5.rs"]
mod bufio_writer5;

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
        // A corpus suite may raise its step budget with a `// steps:` header.
        let steps = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("// steps: ")?.parse().ok());
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            steps,
            Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
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
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
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
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
}

// Go TestReader's 4 MiB archive cannot be embedded in a bounded corpus program.
#[test]
fn zip_big_directory_executes_std_source() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zip-big.hir");
        let mut simulation = SimulationConfig::default();
        simulation.files.insert(
            "big.zip".into(),
            include_bytes!("../fixtures/zip-big.bin").to_vec(),
        );
        let outcome = run_path_with_step_budget(
            &path,
            simulation,
            Some(5_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
}

// Original Go pax-bad-hdr-large input is carried outside the bounded corpus.
#[test]
fn tar_large_pax_header_executes_std_source() {
    with_compiler_stack(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tar-pax-large.hir");
        let mut simulation = SimulationConfig::default();
        simulation.files.insert(
            "large.tar".into(),
            include_bytes!("../fixtures/tar-pax-large.bin").to_vec(),
        );
        let outcome = run_path_with_step_budget(
            &path,
            simulation,
            Some(5_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
}

// Go TestInvalidFiles scans 70 KiB of zeros beyond the corpus step budget.
#[test]
fn zip_invalid_zeros_execute_std_source() {
    with_compiler_stack(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zip-invalid-zeros.hir");
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(10_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
}

// A diff large enough to trip Myers' cost cutoff exceeds the corpus step
// budget; the corpus covers every algorithm on short inputs.
#[test]
fn diff_large_vectors_execute_std_source() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diff-large.hir");
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(1_000_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
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
        let outcome = native::run_path_native_backend_with_validation(
            &path,
            hirsel_driver::HostLaunch::empty(),
            None,
            native::Backend::Llvm,
            native::Output::Streams {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
            Verify::On,
            native::Validation::EveryStage,
        )
        .unwrap()
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    });
}

// The wall clock is not grantable to a corpus program; version 7 UUIDs take
// their timestamp from it. Random is random.splitmix64@1 with seed 0.
#[test]
fn uuid_v7_reads_the_wall_clock() {
    with_compiler_stack(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/uuid-v7-clock.hir");
        let prepared = request_tir(&path, Verify::On, None).unwrap();
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
            let execution = testing::observe_tir(
                &prepared,
                &RunConfiguration {
                    entry: prepared.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                simulation,
                hirsel_interp::Limits {
                    steps: None,
                    ..Default::default()
                },
                false,
            )
            .unwrap();
            let outcome = execution.outcome.unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{outcome:?}"
            );
            assert_eq!(
                String::from_utf8(execution.observation.unwrap().output).unwrap(),
                expected,
                "wall {wall}"
            );
        }
    });
}

// A 64 KiB frame takes the 64-bit length form; building one in the interpreter
// is too slow for any budget, so it runs only as a built executable.
#[test]
#[ignore = "full gate: native 64-bit length frame"]
fn full_gate_websocket_large_frame_native() {
    with_compiler_stack(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/websocket-large.hir");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = native::run_path_native(&path, &mut stdout, &mut stderr)
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
        assert!(stdout.is_empty(), "{stdout:?}");
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    });
}

// The corpus grants no wall clock, so jwt.verify's reading of one runs here at
// fixed simulated times, and once with the clock failing.
#[test]
fn jwt_verify_reads_the_wall_clock() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jwt-clock.hir");
        let cases: [(&str, i64, bool); 5] = [
            ("valid", 1_444_478_400_000_000_000, false),
            ("not-yet-valid", 1_444_478_399_999_999_999, false),
            ("before-epoch", -500_000_000, false),
            ("epoch", 0, false),
            ("unavailable", 1_444_478_400_000_000_000, true),
        ];
        for (expected, wall, fault) in cases {
            let mut simulation = SimulationConfig {
                args: vec![expected.into()],
                wall,
                ..Default::default()
            };
            simulation.grants.insert("WallClock".into());
            if fault {
                simulation.faults.insert(
                    "clock.wall".into(),
                    std::collections::VecDeque::from([
                        hirsel_interp::library::simulation::Fault::Fault {
                            code: 0,
                            partial: 0,
                        },
                    ]),
                );
            }
            let outcome = run_path_with_step_budget(
                &path,
                simulation,
                Some(5_000_000),
                hirsel_engine::Verify::On,
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{expected}: {outcome:?}"
            );
        }
    });
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
fn cli_error_context_5_example_prints_explicitly() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-cli-error-context-5-example-run.hir");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            None,
            Verify::On,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        assert!(matches!(
            outcome,
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ));
        assert!(stdout.is_empty());
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "argument 0 \"-o\": a value is required for \"--output <value>\" but none was supplied\n\
             Usage: example [options] [--] [arguments]\n  -o, --output <value>\toutput path\n"
        );
    });
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
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ));
            let observation = observed.observation.unwrap();
            let writes = observation.effects.iter().filter(|effect| matches!(&effect.effect, hirsel_interp::observation::Effect::CapabilityConsumed(event) if event.operation == "err.print")).count();
            assert_eq!(writes, 1);
            let state = observation.final_state.unwrap();
            let expected = if fault {
                b"".as_slice()
            } else {
                b"{\"level\":\"INFO\",\"msg\":\"a\\n\\\"b\"}\n".as_slice()
            };
            assert_eq!(state.stderr, expected);
        }
    });
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
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ));
            } else {
                assert!(
                    matches!(observed.outcome, Ok(RunOutcome::Failed { primary: hirsel_run_schema::Failure { ref rule, .. }, .. }) if rule.as_str() == "LANG-9")
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
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
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
fn telemetry_limits_agree_on_both_native_backends() {
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
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
        // The telemetry corpus cases (SEM-3-telemetry-run, -context-run, SEM-3-log-run)
        // run on both backends in the Cranelift parity set; the limits fixture,
        // past the corpus step budget, runs natively only here.
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = native::run_path_native_backend_with_validation(
                &limits,
                HostLaunch::Live {
                    config: LaunchConfig::Grants {
                        input: std::ffi::OsStr::new("{}"),
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
                hirsel_driver::native::Validation::EveryStage,
            )
            .unwrap()
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{backend:?}: {outcome:?}"
            );
            assert!(stdout.is_empty(), "{backend:?}: {stdout:?}");
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
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
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
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
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
