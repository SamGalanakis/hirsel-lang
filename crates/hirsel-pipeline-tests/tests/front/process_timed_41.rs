//! Timed process fixtures share the byte machine and existing quiescent timer seam.
use hirsel_driver::{native::*, record_loaded_fixture_observations, replay_loaded_observations};
use hirsel_engine::{
    RunConfiguration, run_observed_checked, run_observed_snapshot, with_compiler_stack,
};
use hirsel_run_schema::ProcessGrant;
use serde_json::json;

fn campaign(id: &str, source: &str, fixture: Fixture, output: &[u8]) {
    campaign_mode(id, source, fixture, output, false, false);
}
fn campaign_mode(
    id: &str,
    source: &str,
    fixture: Fixture,
    output: &[u8],
    native_only: bool,
    checked_only: bool,
) {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        let retained_source =
            std::env::var_os("HIRSEL_TIMED_RETAINED_SOURCE").map(std::path::PathBuf::from);
        let mut _restored_source = None;
        let path = if let Some(original) = retained_source {
            let parent = original.parent().unwrap();
            assert!(parent.starts_with(std::env::temp_dir()));
            assert_eq!(original.file_name().unwrap(), "main.hir");
            std::fs::create_dir(parent).unwrap();
            _restored_source = Some(RestoredSource(parent.to_owned()));
            assert_eq!(
                tape_sha(source.as_bytes()),
                std::env::var("HIRSEL_TIMED_RETAINED_SOURCE_SHA").unwrap()
            );
            original
        } else if id == "S01" {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/suite-runner/hirsel.json")
        } else {
            root.join("main.hir")
        };
        if id != "S01" {
            std::fs::write(&path, source).unwrap();
        }
        eprintln!("TIMED_PHASE {id} source-load start");
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{id}: {e}"));
        eprintln!("TIMED_PHASE {id} source-load done; source-check start");
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            hirsel_engine::ToolError::Query { error, .. } => panic!("{id}: {error:?}"),
            e => panic!("{id}: {e}"),
        });
        eprintln!("TIMED_PHASE {id} source-check done");
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let expected = if native_only || checked_only {
            let mut expected = Observation {
                exit_code: Some(if id == "S01" { 1 } else { 0 }),
                blocked: false,
                budget: None,
                signaled: None,
                output: output.to_vec(),
                failures: vec![],
                roots: vec![],
            };
            if matches!(id, "E04" | "L02") {
                expected.exit_code = None;
                let (line, text) = source
                    .lines()
                    .enumerate()
                    .find(|(_, text)| {
                        text.contains(if id == "L02" {
                            "proc.spawn_pipeline"
                        } else {
                            "child.close_stdin"
                        })
                    })
                    .unwrap();
                expected.failures.push(Failure {
                    rule: "LANG-9".into(),
                    definition: loaded.entry.clone(),
                    file: path.display().to_string(),
                    line: line as u32 + 1,
                    column: text
                        .find(if id == "L02" {
                            "proc.spawn_pipeline"
                        } else {
                            "child.close_stdin"
                        })
                        .unwrap() as u32
                        + 1,
                    operation: None,
                });
            }
            if id == "S01" {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(output).unwrap(),
                    oracle("S01")["expected"]["summary"]
                );
            }
            if checked_only {
                let checked =
                    run_observed_checked(&mut loaded.engine, &run, (&fixture).into()).unwrap();
                eprintln!("TIMED_CHECKED_OUTCOME {id} {:?}", checked.outcome);
                assert_eq!(
                    Observation::of_reference(checked.observation.as_ref().unwrap()),
                    expected
                );
                return;
            }
            expected
        } else {
            eprintln!("TIMED_STARTED {id} reference");
            let reference =
                run_observed_snapshot(&mut loaded.engine, &run, (&fixture).into()).unwrap();
            let observable = reference.observation.as_ref().unwrap();
            let expected = Observation::of_reference(observable);
            let failed = matches!(
                id,
                "E03" | "E04" | "E05" | "L02" | "U02-mismatch" | "U02-close"
            );
            let blocked = matches!(id, "G03" | "R02-hidden");
            assert_eq!(
                expected.exit_code,
                if failed || blocked {
                    None
                } else {
                    Some(if id == "S01" { 1 } else { 0 })
                },
                "{id}: {expected:?}"
            );
            assert_eq!(expected.blocked, blocked, "{id}: {expected:?}");
            if failed {
                assert_eq!(expected.failures.len(), 1);
                assert_eq!(expected.failures[0].rule, "LANG-9");
            } else {
                assert!(expected.failures.is_empty(), "{id}: {expected:?}");
            }
            if id == "S01" {
                let oracle = oracle("S01");
                let parsed: serde_json::Value = serde_json::from_slice(&expected.output).unwrap();
                assert_eq!(parsed, oracle["expected"]["summary"]);
                assert_eq!(expected.output.last(), Some(&b'\n'));
                let state = observable.final_state.as_ref().unwrap();
                assert_eq!(state.stderr, b"");
                assert!(state.trees.values().any(|(_, files)| {
                    files
                        .values()
                        .any(|bytes| bytes == &expected.output[..expected.output.len() - 1])
                }));
                assert!(!state.handles.values().any(|g| matches!(
                    g,
                    hirsel_interp::library::simulation::ObservedGrant::Child { .. }
                )));
            } else {
                assert_eq!(expected.output, output, "{id}");
            }
            eprintln!(
                "TIMED_OUTPUT {id} {}",
                String::from_utf8_lossy(&expected.output)
            );
            eprintln!("TIMED_PASS {id} reference");
            let checked =
                run_observed_checked(&mut loaded.engine, &run, (&fixture).into()).unwrap();
            if checked.observation != reference.observation {
                eprintln!("TIMED_CHECKED_OUTCOME {id} {:?}", checked.outcome);
                panic!("{id} checked H differs from the full reference observation");
            }
            eprintln!("TIMED_PASS {id} checked-H");
            expected
        };
        eprintln!("TIMED_PHASE {id} native-export start");
        let input = loaded.engine.export_native_input().unwrap();
        eprintln!("TIMED_PHASE {id} native-export done");
        eprintln!("TIMED_PHASE {id} native-import start");
        let input = loaded.engine.import_native_input(input).unwrap();
        eprintln!("TIMED_PHASE {id} native-import done");
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = [
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.launch_timeout = std::time::Duration::from_secs(60);
        tools.execution = Execution::RunnerExec;
        tools.wrapper = vec![
            "/bin/sh".into(),
            "-c".into(),
            "ulimit -t 60; ulimit -d 2097152; exec \"$@\"".into(),
            "timed-process-caps".into(),
        ];
        if let Some(floor) = &tools.glibc_floor {
            let libs = floor.join(&ready_target_triple());
            tools.wrapper.extend([
                libs.join("ld-linux-x86-64.so.2").display().to_string(),
                "--library-path".into(),
                libs.display().to_string(),
            ]);
        }
        eprintln!("TIMED_PHASE {id} native-build start");
        let ready = build(&input, &options).unwrap();
        eprintln!("TIMED_PHASE {id} native-build done");
        let sites = Sites::new(input.snapshot());
        let mut runnables = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = root.join(format!("{backend:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            eprintln!("TIMED_PHASE {id} {backend:?}-emit start");
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            eprintln!("TIMED_PHASE {id} {backend:?}-emit done; link start");
            let runnable =
                link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, &fixture)
                    .unwrap();
            eprintln!("TIMED_PHASE {id} {backend:?}-link done");
            runnables.push((runnable, dir));
        }
        let mut producers = Vec::new();
        let recovery = std::env::var("HIRSEL_TIMED_RECOVERY_TAPES_JSON").ok();
        if let Some(json) = &recovery {
            let originals: Vec<(String, Vec<u8>)> = serde_json::from_str(json).unwrap();
            for (producer, bytes) in originals {
                let original = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                let current = fixture_run_record(&ready, &loaded.entry, &fixture).unwrap();
                assert_eq!(
                    original.bound_run().unwrap().canonical(),
                    current.canonical()
                );
                assert_eq!(original.run_identity, current.identity());
                let path = root.join(format!("retained-{producer}"));
                std::fs::write(&path, &bytes).unwrap();
                eprintln!(
                    "TIMED_INTEGRITY {id} {producer} complete sha256={}",
                    tape_sha(&bytes)
                );
                producers.push((producer, path));
            }
        }
        let retained = if matches!(id, "L02" | "S01") {
            std::env::var("HIRSEL_TIMED_RETAINED_TAPE_JSON").ok()
        } else {
            None
        };
        if let Some(json) = &retained {
            let bytes: Vec<u8> = serde_json::from_str(json).unwrap();
            assert_eq!(
                tape_sha(&bytes),
                std::env::var("HIRSEL_TIMED_RETAINED_TAPE_SHA").unwrap()
            );
            let original = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
            let bound = original.bound_run().unwrap();
            let current = fixture_run_record(&ready, &loaded.entry, &fixture).unwrap();
            assert_eq!(
                bound.canonical(),
                current.canonical(),
                "original producer binding must match without rewriting"
            );
            assert_eq!(original.run_identity, current.identity());
            let advances: Vec<_> = original
                .events
                .iter()
                .filter_map(|e| match e.request {
                    hirsel_rt::io::Request::ProcessTime { from, to } => Some((from, to)),
                    _ => None,
                })
                .collect();
            if id == "S01" {
                assert_suite_record(&original, &expected.output);
            } else {
                assert_eq!(advances, [(0, 3), (3, 5)]);
            }
            let tape = root.join("retained-reference-recording");
            std::fs::write(&tape, &bytes).unwrap();
            eprintln!(
                "TIMED_INTEGRITY {id} original-reference complete sha256={}",
                tape_sha(&bytes)
            );
            producers.push(("reference-retained".to_owned(), tape));
        }
        if recovery.is_none() {
            if matches!(id, "R04" | "E04" | "L02" | "S01") || !native_only {
                for (name, checked) in [("reference", false), ("checked-H", true)] {
                    if retained.is_some() && name == "reference" {
                        continue;
                    }
                    let tape = root.join(format!("{name}-recording"));
                    let observed = record_loaded_fixture_observations(
                        &mut loaded,
                        &runnables[0].1.join("run.json"),
                        &fixture,
                        &tape,
                        checked,
                    )
                    .unwrap();
                    let bytes = std::fs::read(&tape).unwrap();
                    assert_eq!(
                        Observation::of_reference(&observed),
                        expected,
                        "{id} {name} producer"
                    );
                    if id == "S01" {
                        assert_suite_record(
                            &hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap(),
                            &expected.output,
                        );
                    }
                    eprintln!("TIMED_PASS {id} {name} live");
                    producers.push((name.to_string(), tape));
                }
            }
            for (recorder, dir) in &runnables {
                eprintln!("TIMED_STARTED {id} {:?}", dir.file_name().unwrap());
                let tape = dir.join("recording");
                let recording_result = launch_fixture(
                    recorder,
                    &ready.target,
                    &tools,
                    dir,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                    &fixture,
                );
                let bytes = std::fs::read(&tape).unwrap_or_default();
                let recorded = recording_result.unwrap();
                assert_eq!(
                    observe(&recorded, &ready, &sites).unwrap(),
                    expected,
                    "{id}: {recorded:?}"
                );
                let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                if id == "S01" {
                    assert_suite_record(&record, &expected.output);
                }
                assert!(
                    record
                        .events
                        .iter()
                        .any(|e| matches!(e.request, hirsel_rt::io::Request::ProcessTime { .. }))
                        || id == "R02-hidden"
                        || id == "D04"
                        || id.starts_with("U")
                        || id == "C03-before"
                        || id == "C03-cancel-first"
                        || matches!(id, "E05" | "B02" | "B03" | "G03"),
                    "{id}: no time advance recorded"
                );
                eprintln!(
                    "TIMED_PASS {id} {:?} live events={}",
                    dir.file_name().unwrap(),
                    record.events.len()
                );
                producers.push((format!("{:?}", dir.file_name().unwrap()), tape));
            }
        }
        let mut replay_errors = Vec::new();
        for (producer, tape) in producers {
            let replay_fixture = Fixture {
                files: Default::default(),
                process: None,
                monotonic: None,
                ..fixture.clone()
            };
            let valid_producers = std::env::var("HIRSEL_TIMED_VALID_REPLAY_PRODUCERS").ok();
            if valid_producers
                .as_ref()
                .is_none_or(|names| names.split(',').any(|name| name == producer))
            {
                for (runnable, replay_dir) in &runnables {
                    let consumer = format!(
                        "{:?}->{:?}-replay",
                        producer,
                        replay_dir.file_name().unwrap()
                    );
                    let replayed = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &replay_fixture,
                    );
                    let replayed = match replayed.and_then(|r| observe(&r, &ready, &sites)) {
                        Ok(actual) if actual == expected => actual,
                        result => {
                            eprintln!("TIMED_FAIL {id} {consumer}: {result:?}");
                            replay_errors.push(consumer);
                            continue;
                        }
                    };
                    assert_eq!(replayed, expected);
                    eprintln!(
                        "TIMED_PASS {id} {:?} -> {:?} replay",
                        producer,
                        replay_dir.file_name().unwrap()
                    );
                }
                for (name, checked) in [("reference", false), ("checked-H", true)] {
                    let consumer = format!("{producer}->{name}-replay");
                    let result = replay_loaded_observations(&mut loaded, &tape, None, checked);
                    let result = match result {
                        Ok(actual) if Observation::of_reference(&actual) == expected => actual,
                        result => {
                            eprintln!(
                                "TIMED_FAIL {id} {consumer}: {}",
                                result
                                    .as_ref()
                                    .err()
                                    .map(|e| e.to_string())
                                    .unwrap_or_else(|| "observation mismatch".into())
                            );
                            replay_errors.push(consumer);
                            continue;
                        }
                    };
                    assert_eq!(Observation::of_reference(&result), expected);
                    eprintln!("TIMED_PASS {id} {:?} -> {name} replay", producer);
                }
            }
            if matches!(id, "R02" | "R02-hidden") && producer.contains("Llvm") {
                let replay_fixture = Fixture {
                    files: Default::default(),
                    process: None,
                    monotonic: None,
                    ..fixture.clone()
                };
                replay_errors.extend(timed_refusal_controls(
                    id,
                    &mut loaded,
                    &tape,
                    &runnables,
                    &ready,
                    &tools,
                    &replay_fixture,
                ));
            }
        }
        assert!(
            replay_errors.is_empty(),
            "{id}: failed consumers: {replay_errors:?}"
        );
    });
}
fn grant(steps: serde_json::Value) -> ProcessGrant {
    hirsel_run_config::validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{"program":"/bin/test"}],"runs":[],"children":[{"program":"/bin/test","args":[],"steps":steps}]})).unwrap()).unwrap()
}
const WAIT: &str = r#"
fn after(c: MonotonicClock, start: Instant of c) -> Deadline {
  match instant.checked_add(c, start, 5u64 - instant.ticks(start)) {
    Some(at) => Deadline(clock: c, at: at),
    None() => Deadline(clock: c, at: start),
  }
}
fn outcome(r: Result[ExitStatus, ProcessError]) -> String {
  match r {
    Ok(Exited(code)) => f"exited {code}\n",
    Ok(Signaled(n)) => f"signaled {n}\n",
    Error(TimedOut()) => "timed out\n",
    Error(_) => "failed\n",
  }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  let start = match clock.monotonic(c) { Ok(t) => t, Error(_) => return Ok(()), }
  match proc.spawn(p, Command(program: path.of("/bin/test"), args: [], dir: None(), env: None(), own_group: false)) {
    Ok(ch) => { let _ = out.print(output, outcome(child.wait(p, take ch, Some(after(c, start))))) },
    Error(_) => return Ok(()),
  }
  match clock.monotonic(c) {
    Ok(now) => { let _ = out.print(output, f"at {instant.ticks(now)}\n") },
    Error(_) => return Ok(()),
  }
  Ok(())
}
"#;
#[test]
fn timed_d01_before_deadline() {
    campaign(
        "D01",
        WAIT,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(json!([{"at":4},{"exit":{"exited":7}}]))),
            ..Default::default()
        },
        b"exited 7\nat 4\n",
    );
}
#[test]
fn timed_d02_deadline_equality() {
    campaign(
        "D02",
        WAIT,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(json!([{"at":5},{"exit":{"exited":7}}]))),
            ..Default::default()
        },
        b"timed out\nat 5\n",
    );
}
#[test]
fn timed_d03_after_deadline() {
    campaign(
        "D03",
        WAIT,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(json!([{"at":6},{"exit":{"exited":7}}]))),
            ..Default::default()
        },
        b"timed out\nat 5\n",
    );
}
#[test]
fn timed_d04_already_reached_deadline() {
    campaign(
        "D04",
        WAIT,
        Fixture {
            monotonic: Some(5),
            process: Some(grant(json!([{"exit":{"exited":7}}]))),
            ..Default::default()
        },
        b"timed out\nat 5\n",
    );
}

fn ready_target_triple() -> String {
    "x86_64-unknown-linux-gnu".into()
}
const SCOPE_WAIT: &str = r#"
fn scope_end(value: Bounded[Result[ExitStatus, ProcessError]]) -> String {
  match value { Finished(_) => "finished\n", Expired(_) => "expired\n", }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  let tasks = io.tasks(rt)?
  let start = match clock.monotonic(c) { Ok(t) => t, Error(_) => return Ok(()), }
  let at = match instant.checked_add(c, start, 5u64) { Some(t) => t, None() => return Ok(()), }
  let bounded = task.scope_until(tasks, Deadline(clock: c, at: at), fn(s: Scope) -> Result[ExitStatus, ProcessError] {
    let ch = proc.spawn(p, Command(program: path.of("/bin/test"), args: [], dir: None(), env: None(), own_group: false))?
    child.wait(p, take ch, Some(Deadline(clock: c, at: at)))
  })
  let _ = out.print(output, scope_end(take bounded))
  match clock.monotonic(c) { Ok(now) => { let _ = out.print(output, f"at {instant.ticks(now)}\n") }, Error(_) => return Ok(()), }
  Ok(())
}
"#;
#[test]
fn timed_c02_scope_cancellation_wins_deadline_equality() {
    campaign(
        "C02",
        SCOPE_WAIT,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(json!([{"at":5},{"exit":{"exited":7}}]))),
            ..Default::default()
        },
        b"expired\nat 5\n",
    );
}
const READ: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  match proc.spawn(p, Command(program: path.of("/bin/test"), args: [], dir: None(), env: None(), own_group: false)) {
    Ok(ch) => {
      let mut child = take ch
      let mut buffer: List[u8] = []
      let read = child.read_stdout(p, mut child, mut buffer, 100u64)
      match read { Ok(n) => { let _ = out.print(output, f"read {n}\n") }, Error(_) => { let _ = out.print(output, "failed\n") }, }
      let _ = child.wait(p, take child, None())
    },
    Error(_) => return Ok(()),
  }
  match clock.monotonic(c) { Ok(now) => { let _ = out.print(output, f"at {instant.ticks(now)}\n") }, Error(_) => return Ok(()), }
  Ok(())
}
"#;
#[test]
fn timed_r04_delayed_read_and_exact_replay() {
    campaign(
        "R04",
        READ,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(
                json!([{"at":5},{"print":[120]},{"exit":{"exited":0}}]),
            )),
            ..Default::default()
        },
        b"read 1\nat 5\n",
    );
}

fn oracle(id: &str) -> serde_json::Value {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../native-tests/fixtures/process-script-timed/vectors.json"),
    )
    .unwrap();
    let packet: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    packet["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == id)
        .unwrap()
        .clone()
}
fn suite_fixture() -> Fixture {
    use hirsel_run_schema::Access;
    use std::collections::BTreeMap;
    let vector = oracle("S01");
    let process = hirsel_run_config::validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{"program":"/fixture/child"}],"runs":[],"children":[{"program":"/fixture/child","args":[],"steps":vector["steps"]}]})).unwrap()).unwrap();
    Fixture {
        args: vec!["run".into(), "deadline.json".into(), "report.json".into()],
        monotonic: Some(0),
        process: Some(process),
        files: BTreeMap::from([
            (
                "fixtures/deadline.json".into(),
                vector["manifest_utf8"]
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ),
            ("reports/report.json".into(), b"previous".to_vec()),
        ]),
        mounts: BTreeMap::from([
            ("fixtures".into(), Access::Read),
            ("reports".into(), Access::ReadWrite),
        ]),
        ..Default::default()
    }
}
// Captured from the package serializer in the original reference S01 receipt.
const S01_BYTES: &[u8] = b"{\"version\":1,\"cases\":[{\"index\":0,\"id\":\"slow\",\"suite\":\"tiny\",\"status\":\"failed\",\"reason\":\"selected\",\"attempts\":[{\"number\":1,\"status\":\"timeout\",\"exit\":null,\"signal\":null}]}],\"selected\":1,\"passed\":0,\"failed\":1,\"skipped\":0,\"not_run\":0,\"incomplete\":0,\"attempts\":1,\"failed_attempts\":1,\"expired\":false,\"cutoff\":false,\"output\":\"discarded\"}\n";
#[test]
fn timed_s01_suite_runner_timeout_oracle() {
    campaign("S01", "", suite_fixture(), S01_BYTES);
}
#[test]
fn timed_s01_missing_native_complements() {
    campaign_mode("S01", "", suite_fixture(), S01_BYTES, true, false);
}
#[test]
fn timed_s01_checked_outcome_diagnostic() {
    campaign_mode("S01", "", suite_fixture(), S01_BYTES, false, true);
}

fn tape_sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn timed_e04_delayed_close_is_lang9() {
    let vector = oracle("E04");
    let source = READ.replace("/bin/test", "/bin/cat").replace(
        "      let read = child.read_stdout",
        "      let _ = child.write(p, mut child, b\"a\")\n      child.close_stdin(p, mut child)\n      let read = child.read_stdout",
    );
    let process = hirsel_run_config::validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{"program":"/bin/cat"}],"runs":[],"children":[{"program":"/bin/cat","args":[],"steps":vector["steps"]}]})).unwrap()).unwrap();
    campaign(
        "E04",
        &source,
        Fixture {
            monotonic: Some(0),
            process: Some(process),
            ..Default::default()
        },
        vector["expected"]["at_5"].as_str().unwrap().as_bytes(),
    );
}

#[test]
fn timed_d02_native_complement() {
    campaign_mode(
        "D02",
        WAIT,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(oracle("D02")["steps"].clone())),
            ..Default::default()
        },
        b"timed out\nat 5\n",
        true,
        false,
    );
}
#[test]
fn timed_c02_native_complement() {
    campaign_mode(
        "C02",
        SCOPE_WAIT,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(json!([{"at":5},{"exit":{"exited":7}}]))),
            ..Default::default()
        },
        b"expired\nat 5\n",
        true,
        false,
    );
}
#[test]
fn timed_r04_four_producer_complement() {
    campaign_mode(
        "R04",
        READ,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(oracle("R04")["steps"].clone())),
            ..Default::default()
        },
        b"read 1\nat 5\n",
        true,
        false,
    );
}

fn oracle_fixture(id: &str) -> Fixture {
    let v = oracle(id);
    let program = v
        .get("program")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("/bin/cat");
    let process = hirsel_run_config::validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{"program":program}],"runs":[],"children":[{"program":program,"args":[],"steps":v["steps"]}]})).unwrap()).unwrap();
    Fixture {
        monotonic: Some(
            v.get("initial_ns")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        ),
        process: Some(process),
        ..Default::default()
    }
}
const VECTOR_HELPERS: &str = r#"
fn utf8(take bytes: List[u8]) -> String {
  match text.finish(take bytes) { Ok(s) => s, Error(_) => "bad UTF-8", }
}
fn sleep_to(c: MonotonicClock, ticks: u64) -> () {
  let start = match clock.monotonic(c) { Ok(t) => t, Error(_) => return (), }
  match instant.checked_add(c, start, ticks - instant.ticks(start)) {
    Some(at) => { let _ = clock.sleep_until(c, at) }, None() => (),
  }
}
"#;
fn vector_program(body: &str) -> String {
    format!("{VECTOR_HELPERS}\n{}", READ.replace("/bin/test", "/bin/cat").replace(
        "      let mut buffer: List[u8] = []\n      let read = child.read_stdout(p, mut child, mut buffer, 100u64)\n      match read { Ok(n) => { let _ = out.print(output, f\"read {n}\\n\") }, Error(_) => { let _ = out.print(output, \"failed\\n\") }, }\n      let _ = child.wait(p, take child, None())", body))
}
fn vector_read(stream: &str, max: u64, ordinal: usize) -> String {
    format!(
        r#"      let mut buffer{ordinal}: List[u8] = []
      match child.read_{stream}(p, mut child, mut buffer{ordinal}, {max}u64) {{
        Ok(n) => {{ let _ = out.print(output, f"{{n}}:{{utf8(take buffer{ordinal})}}\n") }},
        Error(_) => {{ let _ = out.print(output, "read failed\n") }},
      }}
"#
    )
}
fn vector_read_wait(prefix: &str) -> String {
    format!(
        "{prefix}{}      let _ = child.wait(p, take child, None())",
        vector_read("stdout", 100, 0)
    )
}
#[test]
fn timed_p01_checkpoint_and_drop_preserve_origin() {
    let source = vector_program("      sleep_to(c, 101000000u64)");
    campaign("P01", &source, oracle_fixture("P01"), b"at 101000000\n");
}
#[test]
fn timed_p02_gate_boundary() {
    let source = vector_program(&vector_read_wait("      sleep_to(c, 300999999u64)\n"));
    campaign(
        "P02",
        &source,
        oracle_fixture("P02"),
        b"1:x\nat 301000000\n",
    );
}
#[test]
fn timed_g03_inaccessible_gate_is_finite_blocked() {
    campaign(
        "G03",
        &vector_program(&vector_read_wait("")),
        oracle_fixture("G03"),
        b"",
    );
}
#[test]
fn timed_e01_expect_hides_gate_until_late_write() {
    let source = vector_program(&vector_read_wait(
        "      let _ = child.write(p, mut child, b\"a\")\n      sleep_to(c, 7u64)\n      let _ = child.write(p, mut child, b\"b\")\n",
    ));
    campaign("E01", &source, oracle_fixture("E01"), b"1:x\nat 7\n");
}
#[test]
fn timed_e02_early_writes_retain_surplus() {
    let source = vector_program(&vector_read_wait(
        "      let _ = child.write(p, mut child, b\"a\")\n      let _ = child.write(p, mut child, b\"bextra\")\n",
    ));
    campaign("E02", &source, oracle_fixture("E02"), b"1:x\nat 5\n");
}
#[test]
fn timed_e03_short_contradiction_waits_for_complete_input() {
    let source = vector_program(
        "      let _ = child.write(p, mut child, b\"z\")\n      sleep_to(c, 5u64)\n      let _ = child.write(p, mut child, b\"q\")",
    );
    let v = oracle("E03");
    campaign(
        "E03",
        &source,
        oracle_fixture("E03"),
        v["expected"]["report"].as_str().unwrap().as_bytes(),
    );
}
#[test]
fn timed_e05_drop_at_reached_expect_reports() {
    let source = vector_program("      let _ = child.write(p, mut child, b\"a\")");
    let v = oracle("E05");
    campaign(
        "E05",
        &source,
        oracle_fixture("E05"),
        v["expected"]["report"].as_str().unwrap().as_bytes(),
    );
}
#[test]
fn timed_b01_stream_chunks_and_eof_are_exact() {
    let mut body = String::new();
    for (i, (stream, max)) in [
        ("stdout", 1),
        ("stdout", 10),
        ("stdout", 10),
        ("stdout", 10),
        ("stderr", 1),
        ("stderr", 10),
        ("stderr", 10),
    ]
    .into_iter()
    .enumerate()
    {
        body.push_str(&vector_read(stream, max, i));
    }
    body.push_str("      let _ = child.wait(p, take child, None())");
    let v = oracle("B01");
    let mut expected = Vec::new();
    for bytes in &v["expected"].as_array().unwrap()[..7] {
        let bytes: Vec<u8> = serde_json::from_value(bytes.clone()).unwrap();
        expected.extend_from_slice(format!("{}:", bytes.len()).as_bytes());
        expected.extend_from_slice(&bytes);
        expected.push(b'\n');
    }
    expected.extend_from_slice(b"at 5\n");
    campaign(
        "B01",
        &vector_program(&body),
        oracle_fixture("B01"),
        &expected,
    );
}
#[test]
fn timed_b03_drop_before_gate_removes_future_events() {
    campaign("B03", &vector_program(""), oracle_fixture("B03"), b"at 0\n");
}
#[test]
fn timed_c01_borrowing_cancel_keeps_buffer() {
    let source = format!(
        "{VECTOR_HELPERS}\n{}",
        r#"
fn description(take b: Bounded[String]) -> String {
  match take b { Finished(text) => f"finished {text}\n", Expired(text) => f"expired {text}\n", }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  let tasks = io.tasks(rt)?
  let start = match clock.monotonic(c) { Ok(t) => t, Error(_) => return Ok(()), }
  let at = match instant.checked_add(c, start, 5u64) { Some(t) => t, None() => return Ok(()), }
  let bounded = task.scope_until(tasks, Deadline(clock: c, at: at), fn(s: Scope) -> String {
    let mut child = match proc.spawn(p, Command(program: path.of("/bin/cat"), args: [], dir: None(), env: None(), own_group: false)) { Ok(ch) => take ch, Error(_) => return "spawn failed", }
    let mut buffer: List[u8] = [90u8]
    let canceled = match child.read_stdout(p, mut child, mut buffer, 1u64) { Error(Canceled(DeadlineExceeded())) => true, _ => false, }
    let unchanged = buffer == [90u8]
    child.kill(p, mut child, Kill())
    let _ = child.wait(p, take child, None())
    if canceled && unchanged { "canceled unchanged" } else { "wrong" }
  })
  let _ = out.print(output, description(take bounded))
  match clock.monotonic(c) { Ok(now) => { let _ = out.print(output, f"at {instant.ticks(now)}\n") }, Error(_) => (), }
  Ok(())
}
"#
    );
    campaign(
        "C01",
        &source,
        oracle_fixture("C01"),
        b"expired canceled unchanged\nat 5\n",
    );
}

// Host-tape observations carry no interpreter SimulationState. Validate the
// report publication and resource lifecycle from the exact validated effects;
// snapshot runs above additionally compare the full final SimulationState.
fn assert_suite_record(record: &hirsel_rt::launch::Recording, output: &[u8]) {
    use hirsel_rt::io::{Completion as C, Request as R};
    assert_s01_time(record);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(output).unwrap(),
        oracle("S01")["expected"]["summary"]
    );
    let mut atomic = None;
    let mut report = Vec::new();
    let mut published = false;
    let mut stdout = Vec::new();
    let mut children = std::collections::BTreeSet::new();
    let mut canceled_waits = 0;
    let mut pending_waits = std::collections::BTreeSet::new();
    let mut canceled_children = std::collections::BTreeSet::new();
    for event in &record.events {
        match (&event.request, &event.completion) {
            (R::CreateAtomic { directory, path }, C::Handle(Ok(file))) if path == "report.json" => {
                assert!(atomic.replace((*directory, *file)).is_none());
            }
            (
                R::WriteAll {
                    directory,
                    file,
                    bytes,
                },
                C::Unit(Ok(())),
            ) if atomic == Some((*directory, *file)) => report.extend_from_slice(bytes),
            (R::Commit { directory, file }, C::Unit(Ok(())))
                if atomic == Some((*directory, *file)) =>
            {
                assert!(!published);
                assert_eq!(report, output[..output.len() - 1]);
                published = true;
            }
            (R::Print { bytes, .. }, C::Unit(Ok(()))) => {
                assert!(published, "report commits before stdout");
                stdout.extend_from_slice(bytes);
            }
            (R::PrintStderr { bytes, .. }, _) => assert!(bytes.is_empty()),
            (_, C::Spawned(Ok(child))) => {
                assert!(children.insert(*child));
            }
            (
                R::ChildWait {
                    child, deadline, ..
                },
                C::Blocked { .. },
            ) => {
                assert_eq!(deadline.as_ref().map(|at| at.ticks), Some(5_000_000));
                pending_waits.insert(*child);
            }
            (R::ProcessSettle { canceled }, C::Ready) => {
                canceled_children.extend(canceled.iter().copied())
            }
            (R::ChildWait { child, .. }, C::ChildExit(result)) => {
                assert!(children.remove(child));
                pending_waits.remove(child);
                assert!(matches!(
                    result,
                    Err(hirsel_run_schema::ProcessFailure::Canceled(3))
                ));
                canceled_waits += 1;
            }
            (R::Close { handle }, C::Unit(Ok(()))) => {
                // io_checks::cancel returns the typed Canceled result without
                // recording a second wait. Its consuming close is recorded.
                if pending_waits.remove(handle) {
                    assert!(canceled_children.contains(handle));
                    canceled_waits += 1;
                }
                children.remove(handle);
            }
            (_, C::ChildBytes(Ok(bytes))) => {
                assert!(bytes.is_empty(), "no delayed child output delivered")
            }
            _ => (),
        }
    }
    assert!(published);
    assert_eq!(stdout, output);
    assert_eq!(canceled_waits, 1);
    assert!(pending_waits.is_empty());
    assert!(children.is_empty());
}
#[test]
fn timed_e04_native_and_host_tape_complement() {
    let vector = oracle("E04");
    let source = READ.replace("/bin/test", "/bin/cat").replace("      let read = child.read_stdout", "      let _ = child.write(p, mut child, b\"a\")\n      child.close_stdin(p, mut child)\n      let read = child.read_stdout");
    campaign_mode(
        "E04",
        &source,
        oracle_fixture("E04"),
        vector["expected"]["at_5"].as_str().unwrap().as_bytes(),
        true,
        false,
    );
}

fn pipeline_fixture(a: serde_json::Value, b: serde_json::Value) -> Fixture {
    let process = hirsel_run_config::validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{"program":"/fixture/a"},{"program":"/fixture/b"}],"runs":[],"children":[{"program":"/fixture/a","args":[],"steps":a},{"program":"/fixture/b","args":[],"steps":b}]})).unwrap()).unwrap();
    Fixture {
        monotonic: Some(0),
        process: Some(process),
        ..Default::default()
    }
}
fn pipeline_program(body: &str, cap: u64) -> String {
    format!("{VECTOR_HELPERS}\n{}", r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  match proc.spawn_pipeline(p, [Command(program: path.of("/fixture/a"), args: [], dir: None(), env: None(), own_group: false), Command(program: path.of("/fixture/b"), args: [], dir: None(), env: None(), own_group: false)], CAPu64) {
    Ok(ch) => {
      let mut group = take ch
      BODY
    },
    Error(_) => return Ok(()),
  }
  match clock.monotonic(c) { Ok(now) => { let _ = out.print(output, f"at {instant.ticks(now)}\n") }, Error(_) => (), }
  Ok(())
}
"#.replace("CAP",&cap.to_string()).replace("BODY",body))
}
#[test]
fn timed_l01_pipeline_feed_precedes_upstream_close() {
    let source = pipeline_program(
        r#"
      let mut buffer: List[u8] = []
      match group.read_stdout(p, mut group, mut buffer, 100u64) {
        Ok(n) => { let _ = out.print(output, f"{n}:{utf8(take buffer)}\n") }, Error(_) => (),
      }
      match proc.wait_group(p, take group, None()) {
        Ok(value) => {
          if value.statuses == [Exited(0),Exited(3)] && value.stderr == [b"",b""] { let _ = out.print(output, "statuses 0 3; stderr empty\n") }
        }, Error(_) => (),
      }
"#,
        2,
    );
    let fixture = pipeline_fixture(
        json!([{"at":5},{"print":[97,98]},{"exit":{"exited":0}}]),
        json!([{"expect":[97,98]},{"print":[99,100]},{"exit":{"exited":3}}]),
    );
    campaign(
        "L01",
        &source,
        fixture,
        b"2:cd\nstatuses 0 3; stderr empty\nat 5\n",
    );
}
fn l02_case() -> (String, Fixture, Vec<u8>) {
    let source = pipeline_program(
        r#"
      let mut buffer: List[u8] = []
      let _ = group.read_stdout(p, mut group, mut buffer, 100u64)
"#,
        2,
    );
    let fixture = pipeline_fixture(
        json!([{"at":3},{"print":[97]},{"exit":{"exited":0}}]),
        json!([{"at":5},{"expect":[97,98]},{"exit":{"exited":0}}]),
    );
    (
        source,
        fixture,
        oracle("L02")["expected"]["report"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec(),
    )
}
#[test]
fn timed_l02_pipeline_deferred_close_reports_at_downstream_gate() {
    let (source, fixture, output) = l02_case();
    campaign("L02", &source, fixture, &output);
}
#[test]
fn timed_l02_missing_native_complement() {
    let (source, fixture, output) = l02_case();
    campaign_mode("L02", &source, fixture, &output, true, false);
}

fn stderr_cap_case(cap: u64) {
    let source = pipeline_program(
        r#"
      match proc.wait_group(p, take group, None()) {
        Ok(value) => {
          if value.statuses == [Exited(0),Exited(3)] && value.stderr == [b"EE",b""] { let _ = out.print(output, "statuses 0 3; stderr EE\n") }
        }, Error(OutputLimit()) => { let _ = out.print(output, "output limit\n") }, Error(_) => (),
      }
"#,
        cap,
    );
    let fixture = pipeline_fixture(
        json!([{"at":5},{"print":[120]},{"print_err":[69,69]},{"exit":{"exited":0}}]),
        json!([{"exit":{"exited":3}}]),
    );
    campaign(
        "L03",
        &source,
        fixture,
        if cap == 2 {
            b"statuses 0 3; stderr EE\nat 5\n"
        } else {
            b"output limit\nat 5\n"
        },
    );
}
#[test]
fn timed_l03_exact_stderr_cap_and_ended_downstream() {
    stderr_cap_case(2);
}
#[test]
fn timed_l03_over_stderr_cap_kills_group() {
    stderr_cap_case(1);
}
#[test]
fn timed_g01_shared_domain_across_narrowed_process_grants() {
    let source = format!(
        "{VECTOR_HELPERS}\n{}",
        r#"
fn read_at(p: Proc, mut child: Child of p, c: MonotonicClock, output: Stdout) -> () {
  let mut buffer: List[u8] = []
  match child.read_stdout(p, mut child, mut buffer, 100u64) { Ok(n) => { let _ = out.print(output, f"{n}:{utf8(take buffer)}\n") }, Error(_) => (), }
  match clock.monotonic(c) { Ok(now) => { let _ = out.print(output, f"at {instant.ticks(now)}\n") }, Error(_) => (), }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  let q = match proc.narrow(p, [ProcessRule(program: "/bin/cat", env: [])]) { Ok(q) => q, Error(_) => return Ok(()), }
  let mut a = match proc.spawn(p, Command(program: path.of("/bin/cat"), args: [], dir: None(), env: None(), own_group: false)) { Ok(ch) => take ch, Error(_) => return Ok(()), }
  let mut b = match proc.spawn(q, Command(program: path.of("/bin/cat"), args: [], dir: None(), env: None(), own_group: false)) { Ok(ch) => take ch, Error(_) => return Ok(()), }
  read_at(p, mut a, c, output)
  let _ = child.wait(p, take a, None())
  read_at(q, mut b, c, output)
  let _ = child.wait(q, take b, None())
  Ok(())
}
"#
    );
    let v = oracle("G01");
    let process = hirsel_run_config::validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{"program":"/bin/cat"}],"runs":[],"children":[{"program":"/bin/cat","args":[],"steps":[{"at":v["expected"]["advances_ns"][0]},{"print":[97]},{"exit":{"exited":0}}]},{"program":"/bin/cat","args":[],"steps":[{"at":v["expected"]["advances_ns"][1]},{"print":[98]},{"exit":{"exited":0}}]}]})).unwrap()).unwrap();
    campaign(
        "G01",
        &source,
        Fixture {
            monotonic: Some(0),
            process: Some(process),
            ..Default::default()
        },
        b"1:a\nat 1000000000\n1:b\nat 2000000000\n",
    );
}
#[test]
fn timed_g02_runnable_sibling_prevents_jump() {
    let source = format!(
        "{VECTOR_HELPERS}\n{}",
        r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  let tasks = io.tasks(rt)?
  task.scope(tasks, fn(s: Scope) -> () {
    let mut child = match proc.spawn(p, Command(program: path.of("/bin/cat"), args: [], dir: None(), env: None(), own_group: false)) { Ok(ch) => take ch, Error(_) => return (), }
    let sibling = task.async(s, fn() -> () {
      for i in 0u64..3u64 { match clock.monotonic(c) { Ok(now) => { let _ = out.print(output, f"runnable {instant.ticks(now)}\n") }, Error(_) => (), } }
    })
    let mut buffer: List[u8] = []
    match child.read_stdout(p, mut child, mut buffer, 100u64) { Ok(n) => { let _ = out.print(output, f"{n}:{utf8(take buffer)}\n") }, Error(_) => (), }
    let _ = child.wait(p, take child, None())
    task.await(s, take sibling)
  })
  match clock.monotonic(c) { Ok(now) => { let _ = out.print(output, f"at {instant.ticks(now)}\n") }, Error(_) => (), }
  Ok(())
}
"#
    );
    campaign(
        "G02",
        &source,
        oracle_fixture("G02"),
        b"runnable 0\nrunnable 0\nrunnable 0\n1:x\nat 5\n",
    );
}
#[test]
fn timed_b01_terminal_status_complement() {
    let source = vector_program(
        r#"
      match child.wait(p, take child, None()) {
        Ok(Exited(3)) => { let _ = out.print(output, "exited 3\n") }, _ => (),
      }
"#,
    );
    campaign(
        "B01-status",
        &source,
        oracle_fixture("B01"),
        b"exited 3\nat 5\n",
    );
}
#[test]
fn timed_b02_kill_write_and_closed_input_checks() {
    let source = vector_program(
        r#"
      child.kill(p, mut child, Kill())
      match child.write(p, mut child, b"x") { Error(InputClosed()) => { let _ = out.print(output, "input closed\n") }, _ => (), }
      match child.write(p, mut child, b"") { Ok(()) => { let _ = out.print(output, "empty accepted\n") }, _ => (), }
      child.close_stdin(p, mut child)
      match child.write(p, mut child, b"") { Error(InvalidInput()) => { let _ = out.print(output, "invalid input\n") }, _ => (), }
      match child.wait(p, take child, None()) { Ok(Signaled(9)) => { let _ = out.print(output, "signaled 9\n") }, _ => (), }
"#,
    );
    campaign(
        "B02",
        &source,
        oracle_fixture("B02"),
        b"input closed\nempty accepted\ninvalid input\nsignaled 9\nat 0\n",
    );
}
#[test]
fn timed_c03_kill_before_gate_and_after_due_batch() {
    for before in [true, false] {
        let prefix = if before {
            ""
        } else {
            "      sleep_to(c, 5u64)\n"
        };
        let body = format!(
            "{prefix}{}",
            r#"
      child.kill(p, mut child, Kill())
      match child.wait(p, take child, None()) {
        Ok(Signaled(9)) => { let _ = out.print(output, "signaled 9\n") },
        Ok(Exited(7)) => { let _ = out.print(output, "exited 7\n") }, _ => (),
      }
"#
        );
        campaign(
            if before { "C03-before" } else { "C03-after" },
            &vector_program(&body),
            oracle_fixture("C03"),
            if before {
                b"signaled 9\nat 0\n"
            } else {
                b"exited 7\nat 5\n"
            },
        );
    }
}
#[test]
fn timed_a01_denied_spawn_preserves_matching_entry() {
    let source = vector_program(&vector_read_wait(""));
    let source = source.replace("  match proc.spawn(p,", r#"
  match proc.spawn(p, Command(program: path.of("/bin/cat"), args: [], dir: None(), env: Some([("UNGRANTED", "value")]), own_group: false)) {
    Error(NotGranted()) => { let _ = out.print(output, "not granted\n") }, _ => (),
  }
  match proc.spawn(p,"#);
    campaign(
        "A01",
        &source,
        oracle_fixture("A01"),
        b"not granted\n1:x\nat 5\n",
    );
}
fn untimed_fixture(steps: serde_json::Value) -> Fixture {
    let process = hirsel_run_config::validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script@1","rules":[{"program":"/bin/cat"}],"runs":[],"children":[{"program":"/bin/cat","args":[],"steps":steps}]})).unwrap()).unwrap();
    Fixture {
        monotonic: Some(0),
        process: Some(process),
        ..Default::default()
    }
}
#[test]
fn timed_u01_unchanged_provider_native_and_replay_companion() {
    // The original shared unit test remains byte-for-byte unchanged. Blocking
    // probes here use the adapter unit witness; all observable actions follow it.
    let mut body = String::new();
    for (i, max) in [100, 1, 100].into_iter().enumerate() {
        body.push_str(&vector_read("stdout", max, i));
    }
    body.push_str(
        r#"
      let _ = child.write(p, mut child, b"pi")
      let _ = child.write(p, mut child, b"ng\nextra")
"#,
    );
    body.push_str(&vector_read("stdout", 100, 3));
    body.push_str(&vector_read("stdout", 100, 4));
    body.push_str(r#"
      match child.write(p, mut child, b"x") { Error(InputClosed()) => { let _ = out.print(output, "input closed\n") }, _ => (), }
      match child.write(p, mut child, b"") { Ok(()) => { let _ = out.print(output, "empty accepted\n") }, _ => (), }
      child.close_stdin(p, mut child)
      match child.wait(p, take child, None()) { Ok(Exited(3)) => { let _ = out.print(output, "exited 3\n") }, _ => (), }
"#);
    campaign(
        "U01",
        &vector_program(&body),
        untimed_fixture(oracle("U01")["steps"].clone()),
        b"6:ready\n\n1:g\n1:o\n5:pong\n\n0:\ninput closed\nempty accepted\nexited 3\nat 0\n",
    );
}
#[test]
fn timed_u02_unchanged_provider_failure_companions() {
    for (id, body, expected) in [
        (
            "U02-mismatch",
            "      let _ = child.write(p, mut child, b\"pong\")",
            "test failure: child /bin/cat expected b\"ping\" but the program wrote b\"pong\"\n",
        ),
        (
            "U02-close",
            "      let _ = child.write(p, mut child, b\"pi\")\n      child.close_stdin(p, mut child)",
            "test failure: child /bin/cat expected b\"ping\" but the program closed its input after writing b\"pi\"\n",
        ),
    ] {
        campaign(
            id,
            &vector_program(body),
            untimed_fixture(oracle("U02")["steps"].clone()),
            expected.as_bytes(),
        );
    }
}
#[test]
fn timed_u02_kill_preserves_terminal_status_companions() {
    for (steps, signal, expected) in [
        (
            oracle("U02")["steps"].clone(),
            "Terminate()",
            b"signaled 15\nat 0\n".as_slice(),
        ),
        (json!([]), "Kill()", b"exited 0\nat 0\n".as_slice()),
    ] {
        let body = r#"
      child.kill(p, mut child, SIGNAL)
      child.close_stdin(p, mut child)
      match child.wait(p, take child, None()) {
        Ok(Signaled(15)) => { let _ = out.print(output, "signaled 15\n") },
        Ok(Exited(0)) => { let _ = out.print(output, "exited 0\n") }, _ => (),
      }
"#
        .replace("SIGNAL", signal);
        campaign(
            "U02-kill",
            &vector_program(&body),
            untimed_fixture(steps),
            expected,
        );
    }
}
#[test]
fn timed_c03_explicit_cancel_before_read_and_after_commit() {
    for after in [false, true] {
        let source=format!("{VECTOR_HELPERS}\n{}",r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output=io.stdout(rt)?
  let c=io.monotonic_clock(rt)?
  let p=io.process(rt)?
  let tasks=io.tasks(rt)?
  let child=match proc.spawn(p,Command(program:path.of("/bin/cat"),args:[],dir:None(),env:None(),own_group:false)) { Ok(ch)=>take ch, Error(_)=>return Ok(()), }
  task.scope(tasks,fn(s:Scope)->() {
    let worker=task.async(s,fn()->String {
      let mut ch=take child
      let mut buffer:List[u8]=[90u8]
      let answer=match child.read_stdout(p,mut ch,mut buffer,1u64) {
        Ok(1)=>if buffer==[90u8,120u8] { "committed" } else { "wrong bytes" },
        Error(Canceled(_))=>if buffer==[90u8] { "canceled unchanged" } else { "wrong cancellation bytes" },
        _=>"wrong result",
      }
      answer
    })
    BEFORE_CANCEL
    task.cancel(s,worker)
    let _=out.print(output,f"{task.await(s,take worker)}\n")
  })
  match clock.monotonic(c) { Ok(now)=>{let _=out.print(output,f"at {instant.ticks(now)}\n")},Error(_)=>(), }
  Ok(())
}
"#.replace("BEFORE_CANCEL",if after { "sleep_to(c, 6u64)" } else { "" }));
        campaign(
            if after {
                "C03-read-first"
            } else {
                "C03-cancel-first"
            },
            &source,
            oracle_fixture("C03"),
            if after {
                b"committed\nat 6\n"
            } else {
                b"canceled unchanged\nat 0\n"
            },
        );
    }
}
#[test]
fn timed_a03_max_tick_and_checked_add_overflow() {
    let mut fixture = oracle_fixture("R04");
    fixture.monotonic = Some(u64::MAX - 1);
    fixture.process=Some(hirsel_run_config::validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{"program":"/bin/cat"}],"runs":[],"children":[{"program":"/bin/cat","args":[],"steps":[{"at":u64::MAX},{"print":[120]},{"exit":{"exited":0}}]}]})).unwrap()).unwrap());
    let body = format!(
        "{}{}",
        vector_read("stdout", 1, 0),
        r#"
      match clock.monotonic(c) {
        Ok(now)=>match instant.checked_add(c,now,1u64) { None()=>{let _=out.print(output,"overflow none\n")},Some(_)=>(), }, Error(_)=>(),
      }
      let _=child.wait(p,take child,None())
"#
    );
    campaign(
        "A03",
        &vector_program(&body),
        fixture,
        b"1:x\noverflow none\nat 18446744073709551615\n",
    );
}

// Reuse the existing native_file_faults_33::refusal_controls protocol: keep
// immutable originals, derive a separate candidate and require refusal in each
// native and host-tape consumer. No header or runtime revision is rebound.
fn timed_refusal_controls(
    id: &str,
    loaded: &mut hirsel_engine::LoadedProgram,
    tape: &std::path::Path,
    runnables: &[(Runnable, std::path::PathBuf)],
    ready: &ReadyProgram,
    tools: &Toolchain,
    fixture: &Fixture,
) -> Vec<String> {
    use hirsel_rt::io::{Completion as C, Event, Request as R};
    let original = std::fs::read(tape).unwrap();
    let record = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
    let mut failures = Vec::new();
    let mut modes = if id == "R02-hidden" {
        vec!["hidden-at"]
    } else {
        vec![
            "initial-binding",
            "script-binding",
            "grants-binding",
            "program-binding",
            "time-regression",
            "duplicate-time",
            "canceled-bytes",
            "early-eof",
            "missing",
            "suffix",
            "unknown-provider",
            "host-clock",
            "host-wall",
            "host-network",
            "unknown-tape-version",
            "corrupt-frame",
            "truncated-frame",
            "incomplete-due-batch",
        ]
    };
    if let Ok(selected) = std::env::var("HIRSEL_TIMED_REFUSAL_MODES") {
        modes.retain(|mode| selected.split(',').any(|selected| selected == *mode));
        assert!(!modes.is_empty());
    }
    for mode in modes {
        let mut changed = record.clone();
        let expected_error = match mode {
            "initial-binding" | "script-binding" | "grants-binding" => {
                let binding = changed.binding.as_mut().unwrap();
                let mut run: serde_json::Value =
                    serde_json::from_slice(&binding.canonical).unwrap();
                match mode {
                    "initial-binding" => run["inputs"]["fixture_monotonic"] = json!(1),
                    "script-binding" => {
                        run["io"]["process"]["children"][0]["steps"][0]["at"] = json!(6)
                    }
                    _ => run["io"]["process"]["rules"] = json!([]),
                }
                binding.canonical =
                    hirsel_run_config::RunConfig::validate(&serde_json::to_vec(&run).unwrap())
                        .unwrap()
                        .canonical()
                        .to_vec();
                "recording.run_identity".to_string()
            }
            "unknown-provider" | "host-clock" | "host-wall" | "host-network" => {
                let binding = changed.binding.as_mut().unwrap();
                let mut run: serde_json::Value =
                    serde_json::from_slice(&binding.canonical).unwrap();
                let field = match mode {
                    "unknown-provider" => {
                        run["io"]["process"]["provider"] = json!("process.script.timed@99");
                        "io.process"
                    }
                    "host-clock" => {
                        run["inputs"]["fixture_monotonic"] = serde_json::Value::Null;
                        "io.process.clock"
                    }
                    "host-wall" => {
                        run["io"]["wall_clock"] = json!("clock.wall.unix_ns@1");
                        "io.process.clock"
                    }
                    _ => {
                        run["io"]["net"] = json!({"provider":"net.stream@1","rules":[]});
                        "io.process.clock"
                    }
                };
                binding.canonical = serde_json::to_vec(&run).unwrap();
                let error = changed.bound_run().unwrap_err().to_string();
                assert!(error.contains(field), "{mode}: {error}");
                field.to_owned()
            }
            "unknown-tape-version"
            | "corrupt-frame"
            | "truncated-frame"
            | "incomplete-due-batch" => String::new(),
            "program-binding" => {
                changed.run_identity[0] ^= 1;
                "recording.run_identity".into()
            }
            "time-regression" => {
                let event = changed
                    .events
                    .iter_mut()
                    .find(|e| matches!(e.request, R::ProcessTime { .. }))
                    .unwrap();
                if let R::ProcessTime { from, .. } = &mut event.request {
                    *from += 1;
                }
                "InvalidCompletion".into()
            }
            "duplicate-time" => {
                let index = changed
                    .events
                    .iter()
                    .position(|e| matches!(e.request, R::ProcessTime { .. }))
                    .unwrap();
                changed
                    .events
                    .insert(index + 1, changed.events[index].clone());
                format!("ReplayMismatch({})", index + 1)
            }
            "canceled-bytes" | "early-eof" => {
                let index = changed
                    .events
                    .iter()
                    .position(|e| {
                        matches!(e.request, R::ChildRead { .. })
                            && matches!(&e.completion,C::ChildBytes(Ok(bytes)) if !bytes.is_empty())
                    })
                    .unwrap();
                changed.events[index].completion = if mode == "early-eof" {
                    C::ChildBytes(Ok(vec![]))
                } else {
                    C::ChildBytes(Err(hirsel_run_schema::ProcessFailure::Canceled(1)))
                };
                format!("ReplayMismatch({index})")
            }
            "missing" => {
                changed.events.pop().unwrap();
                format!("MissingInput({})", changed.events.len())
            }
            "suffix" => {
                changed.events.push(changed.events.last().unwrap().clone());
                format!("UnconsumedInput({})", record.events.len())
            }
            "hidden-at" => {
                changed.events.push(Event {
                    request: R::ProcessTime { from: 0, to: 5 },
                    completion: C::Ready,
                    fault: None,
                });
                "InvalidCompletion".into()
            }
            _ => unreachable!(),
        };
        let candidate = tape.with_file_name(format!("timed-refused-{mode}"));
        let mut bytes = hirsel_rt::launch::render_recording(&changed)
            .unwrap()
            .into_bytes();
        let expected_error = if matches!(
            mode,
            "unknown-tape-version" | "corrupt-frame" | "truncated-frame" | "incomplete-due-batch"
        ) {
            let mut header = record.clone();
            header.events.clear();
            bytes.clear();
            {
                let mut writer = hirsel_rt::tape_binary::Writer::new(&mut bytes, &header).unwrap();
                for event in &record.events {
                    writer.append(event).unwrap();
                    if mode == "incomplete-due-batch"
                        && matches!(event.request, R::ProcessTime { .. })
                    {
                        break;
                    }
                }
                if mode != "incomplete-due-batch" {
                    writer.finish().unwrap();
                }
            }
            match mode {
                "unknown-tape-version" => bytes[15] = b'9',
                "corrupt-frame" => {
                    *bytes.last_mut().unwrap() ^= 1;
                }
                "truncated-frame" => {
                    bytes.pop();
                }
                _ => (),
            }
            hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap_err()
        } else {
            expected_error
        };
        std::fs::write(&candidate, &bytes).unwrap();
        for (name, checked) in [("reference", false), ("checked-H", true)] {
            let consumer = format!("refusal-{mode}-{name}");
            let (error, refused) =
                match replay_loaded_observations(loaded, &candidate, None, checked) {
                    Err(e) => (e.to_string(), true),
                    Ok(o) => (format!("unexpected success: {o:?}"), false),
                };
            if refused && error.contains(&expected_error) {
                eprintln!("TIMED_REFUSAL_PASS {id} {mode} {name} {error}");
            } else {
                failures.push(format!(
                    "{mode}/{name}: expected {expected_error}, got {error}"
                ));
            }
        }
        for (runnable, dir) in runnables {
            let consumer = format!("refusal-{mode}-{:?}", dir.file_name().unwrap());
            let (error, refused) = match launch_fixture(
                runnable,
                &ready.target,
                tools,
                dir,
                true,
                Tape::Replay(&candidate),
                fixture,
            ) {
                Err(e) => (e.message, true),
                Ok(o) => (o.record, o.exit == Some(3)),
            };
            if refused && error.contains(&expected_error) {
                eprintln!(
                    "TIMED_REFUSAL_PASS {id} {mode} {:?} {error}",
                    dir.file_name().unwrap()
                );
            } else {
                failures.push(format!(
                    "{mode}/{dir:?}: expected {expected_error}, refused={refused}, got {error}"
                ));
            }
        }
        assert_eq!(
            std::fs::read(tape).unwrap(),
            original,
            "the original tape is immutable"
        );
    }
    failures
}
#[test]
fn timed_r01_r02_r03_four_consumer_refusals() {
    campaign_mode(
        "R02",
        READ,
        Fixture {
            monotonic: Some(0),
            process: Some(grant(
                json!([{"at":5},{"print":[120]},{"exit":{"exited":0}}]),
            )),
            ..Default::default()
        },
        b"read 1\nat 5\n",
        std::env::var_os("HIRSEL_TIMED_RECOVERY_TAPES_JSON").is_some(),
        false,
    );
}
#[test]
fn timed_r02_forged_at_behind_unmet_expect_four_consumers() {
    campaign(
        "R02-hidden",
        &vector_program(&vector_read_wait("")),
        oracle_fixture("G03"),
        b"",
    );
}

fn assert_s01_time(record: &hirsel_rt::launch::Recording) {
    let advances: Vec<_> = record
        .events
        .iter()
        .filter_map(|event| match event.request {
            hirsel_rt::io::Request::ProcessTime { from, to } => Some((from, to)),
            _ => None,
        })
        .collect();
    assert_eq!(
        advances,
        [(0, 5_000_000)],
        "S01 removes both the 6ms child gate and 20ms global timer on completion"
    );
}

struct RestoredSource(std::path::PathBuf);
impl Drop for RestoredSource {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
