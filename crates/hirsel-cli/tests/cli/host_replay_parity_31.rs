//! Every suspension has one scheduling point; native tapes keep their exact readiness counts.
use serde_json::json;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
};

const SIGNALS: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let signals = io.signals(rt)?
  let output = io.stdout(rt)?
  task.scope(tasks, fn(s: Scope) {
    let mut set: TaskSet[bool] of s = task.set(s)
    task.add(s, mut set, task.async(s, fn() -> bool {
      let _ = io.stdout(rt)
      let _ = io.tasks(rt)
      let _ = io.dir(rt, path.of("work"))
      let Ok(input) = io.stdin(rt) else { return false }
      let mut buf: List[u8] = []
      let mut i = 0u64
      while i < 7 {
        let _ = stdin.read(input, mut buf, 1u64)
        i += 1
      }
      let _ = out.print(output, "ready\n")
      let _ = stdin.read(input, mut buf, 1u64)
      true
    }))
    task.add(s, mut set, task.async(s, fn() -> bool {
      signal.wait(signals, [Interrupt()]) == Ok(Interrupt())
    }))
    let (_, received) = task.select(s, take set)
    failure.check(received, Arithmetic())
  })
  let _ = out.print(output, "canceled\n")
  Ok(())
}
"#;
fn succeeded(command: &mut Command, expected: &[u8]) {
    let result = command.env("HIRSEL_JOBS", "4").output().unwrap();
    assert!(result.status.success(), "{command:?}: {result:?}");
    assert_eq!(result.stdout, expected, "{command:?}");
    assert!(result.stderr.is_empty(), "{command:?}: {result:?}");
}

fn record(app: &Path, manifest: Option<&Path>, tape: &Path, grants: &Path) {
    let mut command = Command::new("env");
    command.arg("--default-signal=INT").arg(app);
    if let Some(manifest) = manifest {
        command
            .arg("run")
            .arg(manifest)
            .arg("--record")
            .arg(tape)
            .arg("--grants")
            .arg(grants);
    } else {
        command
            .arg("--hirsel-record")
            .arg(tape)
            .arg("--hirsel-grants")
            .arg(grants);
    }
    let mut child = command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"recalc\n").unwrap();
    input.flush().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    output.read_line(&mut ready).unwrap();
    if ready != "ready\n" {
        panic!("{ready:?}: {:?}", child.wait_with_output().unwrap());
    }
    assert!(
        Command::new("kill")
            .args(["-INT", "--", &format!("-{}", child.id())])
            .status()
            .unwrap()
            .success()
    );
    drop(input);
    let mut tail = Vec::new();
    output.read_to_end(&mut tail).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(tail, b"canceled\n");
    assert!(result.stderr.is_empty(), "{result:?}");
}

#[test]
fn task_set_signal_suspensions_replay_exact_readiness() {
    hirsel_engine::with_compiler_stack(|| {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        fs::write(dir.join("main.hir"), SIGNALS).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = dir.join("hirsel.json");
        fs::write(&manifest, json!({"schema":1,"package":{"name":"host_parity","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        fs::create_dir(dir.join("work")).unwrap();
        let grants = dir.join("grants.json");
        fs::write(&grants, json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","stdin":"stdin.bytes@1","dirs":[{"path":"work","host":dir.join("work"),"access":"read_write"}],"signals":{"provider":"signals.host@1","kinds":["interrupt"]}},"placement":{"cores":1,"commit":"shared","reorder":"off"}}).to_string()).unwrap();
        let cli = env!("CARGO_BIN_EXE_hirsel");
        record(
            Path::new(cli),
            Some(&manifest),
            &dir.join("reference.tape"),
            &grants,
        );
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut world = hirsel_engine::SimulationConfig {
            stdin: b"recalc\n".to_vec(),
            signals: Some(hirsel_run_schema::SignalsGrant {
                provider: "signals.script@1".into(),
                kinds: vec![hirsel_run_schema::SignalKind::Interrupt],
                script: Some(vec![hirsel_run_schema::SignalKind::Interrupt]),
            }),
            ..Default::default()
        };
        world.grants.insert("Signals".into());
        let live = hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world)
            .unwrap()
            .observation
            .unwrap();
        assert_eq!(live.output, b"ready\ncanceled\n");
        assert_eq!(live.exit_code, Some(0));
        assert!(live.failures.is_empty());
        for backend in ["llvm", "cranelift"] {
            succeeded(
                Command::new(cli)
                    .args(["build", "--backend", backend])
                    .arg(&manifest)
                    .arg("-o")
                    .arg(dir.join(backend)),
                b"",
            );
            let tape = dir.join(format!("{backend}.tape"));
            record(&dir.join(backend), None, &tape, &grants);
            let original = fs::read(&tape).unwrap();
            for replay in [
                hirsel_driver::replay_observations,
                hirsel_driver::replay_observations_checked,
            ] {
                let observed = replay(&manifest, &tape, Some(1_000_000)).unwrap();
                assert_eq!(observed.output, b"ready\ncanceled\n");
                assert_eq!(observed.exit_code, Some(0));
                assert!(observed.failures.is_empty() && observed.blocked.is_none());
            }
            assert_eq!(fs::read(&tape).unwrap(), original);
        }
    });
}
const FILES: &str = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let Ok(dir) = io.dir(rt, path.of("data")) else { failure.check(false, Arithmetic()); return Ok(()) }
  let mode = match arg.text(args, 0u64) { Ok(word) => if word == "cancel" { 0u8 } else if word == "put_new" { 1u8 } else { 2u8 }, _ => 2u8, }
  task.scope(tasks, fn(s: Scope) {
    let child = task.async(s, fn() -> bool {
      let mut Ok(f) = file.open(dir, path.of("input")) else { return false }
      let mut bytes: List[u8] = []
      let reading: Result[u64, FileError] = file.read(dir, mut f, mut bytes, 1u64)
      match reading { Error(Canceled(Requested())) => mode == 0u8, Ok(1u64) => mode != 0u8, _ => false, }
    })
    let _ = file.metadata(dir, path.of("input"))
    if mode == 0u8 { task.cancel(s, child) }
    else if mode == 1u8 {
      let _ = file.put_new(dir, path.of("release"), b"x", Volatile())
      let _ = file.open(dir, path.of("release"))
      let _ = file.open(dir, path.of("release"))
    }
    else { let _ = file.write_bytes(dir, path.of("release"), b"x") }
    failure.check(task.await(s, take child), Arithmetic())
  })
  let _ = out.print(output, "done\n")
  Ok(())
}
"#;

#[test]
fn snapshot_file_waits_live_and_native_replay() {
    use hirsel_driver::native::*;
    use hirsel_engine::{
        RunConfiguration, run_observed_checked_bounded, run_observed_snapshot_bounded,
    };
    use hirsel_run_schema::{Access, FileReadScript};
    hirsel_engine::with_compiler_stack(|| {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("main.hir");
        fs::write(&source, FILES).unwrap();
        let mut loaded = hirsel_driver::load_program(&source).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = Sites::new(validated.snapshot());
        let tools = Toolchain::in_tree(&root.path().join("runtime"), &[&ready.target]).unwrap();
        let objects: Vec<_> = [Backend::Llvm, Backend::Cranelift]
            .into_iter()
            .map(|backend| {
                (
                    backend,
                    emit_backend_objects(
                        &ready,
                        backend,
                        OptLevel::None,
                        Partition::Whole,
                        false,
                        None,
                    )
                    .unwrap(),
                )
            })
            .collect();
        for mode in ["cancel", "write_bytes", "put_new"] {
            let fixture = Fixture {
                args: vec![mode.into()],
                files: [("data/input".into(), b"x".to_vec())].into(),
                mounts: [("data".into(), Access::ReadWrite)].into(),
                file_reads: [(
                    "data/input".into(),
                    if mode == "cancel" {
                        FileReadScript::Pending
                    } else {
                        FileReadScript::AfterWrite {
                            path: "data/release".into(),
                        }
                    },
                )]
                .into(),
                ..Default::default()
            };
            for checked in [false, true] {
                let execute = if checked {
                    run_observed_checked_bounded
                } else {
                    run_observed_snapshot_bounded
                };
                let executed =
                    execute(&mut loaded.engine, &run, (&fixture).into(), 1_000_000).unwrap();
                let observed = executed.observation.unwrap();
                assert_eq!(observed.blocked.is_some(), mode == "put_new");
                assert_eq!(observed.exit_code, (mode != "put_new").then_some(0));
                assert!(observed.failures.is_empty());
            }
            let mut runs = vec![];
            for (backend, objects) in &objects {
                let dir = root.path().join(format!("{mode}-{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &fixture)
                        .unwrap();
                let tape = dir.join("recording.tape");
                let launch = launch_fixture(
                    &runnable,
                    &ready.target,
                    &tools,
                    &dir,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                    &fixture,
                )
                .unwrap();
                let observed = observe(&launch, &ready, &sites).unwrap();
                assert_eq!(observed.blocked, mode == "put_new");
                assert_eq!(observed.exit_code, (mode != "put_new").then_some(0));
                assert!(observed.failures.is_empty());
                runs.push((runnable, dir, tape));
            }
            for (_, _, tape) in &runs {
                let original = fs::read(tape).unwrap();
                let recording = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
                assert!(recording.events.iter().any(|event| matches!(
                    (&event.request, &event.completion),
                    (
                        hirsel_rt::io::Request::FileRead { .. },
                        hirsel_rt::io::Completion::Blocked { committed: false }
                    )
                )));
                // Tape and provider state are fresh on each replay; the source
                // graph was already checked for the simulation and native runs.
                let plain = hirsel_driver::replay_loaded_observations(
                    &mut loaded,
                    tape,
                    Some(1_000_000),
                    false,
                )
                .unwrap();
                let checked = hirsel_driver::replay_loaded_observations(
                    &mut loaded,
                    tape,
                    Some(1_000_000),
                    true,
                )
                .unwrap();
                assert!(plain.equivalent(&checked));
                assert_eq!(plain.blocked.is_some(), mode == "put_new");
                assert_eq!(plain.exit_code, (mode != "put_new").then_some(0));
                assert_eq!(
                    plain.output,
                    if mode == "put_new" {
                        b"".as_slice()
                    } else {
                        b"done\n"
                    }
                );
                for (runnable, dir, _) in &runs {
                    let without_sources = Fixture {
                        files: Default::default(),
                        file_reads: Default::default(),
                        ..fixture.clone()
                    };
                    let launch = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        dir,
                        true,
                        Tape::Replay(tape),
                        &without_sources,
                    )
                    .unwrap();
                    let observed = observe(&launch, &ready, &sites).unwrap();
                    assert_eq!(observed.blocked, mode == "put_new");
                    assert_eq!(observed.exit_code, (mode != "put_new").then_some(0));
                    assert!(observed.failures.is_empty());
                }
                assert_eq!(fs::read(tape).unwrap(), original);
            }
            eprintln!(
                "HOST_PARITY_FILES_PASS {mode}: four live engines; two native tapes; four replay engines"
            );
        }
    });
}
#[test]
#[ignore = "requires HOST_REPLAY_PARITY_31_CASES with immutable host evidence"]
fn retained_production_tapes() {
    hirsel_engine::with_compiler_stack(|| {
        let cases = std::env::var_os("HOST_REPLAY_PARITY_31_CASES").unwrap();
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(cases).unwrap()).unwrap();
        assert!(!rows.is_empty());
        let mut results = vec![];
        for row in rows {
            let manifest = Path::new(row["manifest"].as_str().unwrap());
            let tape = Path::new(row["tape"].as_str().unwrap());
            let before = fs::read(tape).unwrap();
            let recorded = hirsel_rt::launch::parse_recording_bytes(&before).unwrap();
            eprintln!(
                "HOST_PARITY_RECORDED_END {} events={}",
                row["id"],
                recorded.events.len()
            );
            for (engine, replay) in [
                (
                    "reference",
                    hirsel_driver::replay_observations as fn(&Path, &Path, Option<u64>) -> _,
                ),
                ("checked", hirsel_driver::replay_observations_checked),
            ] {
                let attempt = replay(manifest, tape, row["steps"].as_u64());
                let result = match attempt {
                    Ok(observed) => {
                        assert!(row["binding_refusal"] != true);
                        assert_eq!(
                            observed.output,
                            fs::read(row["stdout"].as_str().unwrap()).unwrap()
                        );
                        assert!(observed.failures.is_empty() && observed.signaled.is_none());
                        if row["status"] == 4 {
                            assert!(observed.blocked.is_some() && observed.exit_code.is_none());
                        } else {
                            assert_eq!(observed.exit_code.map(i64::from), row["status"].as_i64());
                            assert!(observed.blocked.is_none());
                        }
                        json!({"id":row["id"],"engine":engine,"result":"PASS"})
                    }
                    Err(error) if row["binding_refusal"] == true => {
                        let message = error.to_string();
                        assert!(
                            message.contains("binding") && message.contains("program"),
                            "{message}"
                        );
                        json!({"id":row["id"],"engine":engine,"result":"INCOMPATIBLE","error":message})
                    }
                    Err(error) => panic!("{} {engine}: {error}", row["id"]),
                };
                eprintln!("HOST_PARITY_PRODUCTION {result}");
                results.push(result);
                assert_eq!(fs::read(tape).unwrap(), before);
            }
        }
        if let Some(output) = std::env::var_os("HOST_REPLAY_PARITY_31_RESULTS") {
            fs::write(output, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
        }
    });
}

#[test]
#[ignore = "requires HOST_REPLAY_PARITY_31_CHUNK with source and fixture evidence"]
fn record_chunk_blocked_on_current_runtime() {
    use hirsel_driver::native::*;
    hirsel_engine::with_compiler_stack(|| {
        let root =
            std::path::PathBuf::from(std::env::var_os("HOST_REPLAY_PARITY_31_CHUNK").unwrap());
        let source = root.join("hirsel.json");
        let (files, mounts) =
            serde_json::from_slice(&fs::read(root.join("fixture.json")).unwrap()).unwrap();
        let reads =
            serde_json::from_slice(&fs::read(root.join("file_reads.json")).unwrap()).unwrap();
        let fixture = Fixture {
            args: vec![
                "backup".into(),
                "input.json".into(),
                "daily".into(),
                "2".into(),
            ],
            files,
            mounts,
            file_reads: reads,
            steps: Some(20_000_000),
            ..Default::default()
        };
        let mut loaded = hirsel_driver::load_program(&source).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = Sites::new(validated.snapshot());
        let mut tools = Toolchain::in_tree(&root.join("runtime"), &[&ready.target]).unwrap();
        tools.execution = Execution::Executable;
        let mut runs = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let dir = root.join(format!("{backend:?}"));
            fs::create_dir(&dir).unwrap();
            let runnable =
                link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, &fixture)
                    .unwrap();
            let tape = dir.join("recording.tape");
            let launch = launch_fixture(
                &runnable,
                &ready.target,
                &tools,
                &dir,
                true,
                Tape::Live {
                    record: Some(&tape),
                    revocations: None,
                },
                &fixture,
            )
            .unwrap();
            let observed = observe(&launch, &ready, &sites).unwrap();
            assert!(
                observed.blocked && observed.failures.is_empty() && observed.exit_code.is_none()
            );
            fs::write(dir.join("stdout"), &observed.output).unwrap();
            for replay in [
                hirsel_driver::replay_observations,
                hirsel_driver::replay_observations_checked,
            ] {
                let replayed = replay(&source, &tape, Some(20_000_000)).unwrap();
                assert!(
                    replayed.blocked.is_some()
                        && replayed.failures.is_empty()
                        && replayed.exit_code.is_none()
                );
                assert_eq!(replayed.output, observed.output);
            }
            runs.push((backend, runnable, dir, tape));
            eprintln!("HOST_PARITY_CHUNK_PASS {backend:?} live + reference/checked replay at20M");
        }
        let none = Fixture {
            files: Default::default(),
            file_reads: Default::default(),
            ..fixture.clone()
        };
        for (producer, _, _, tape) in &runs {
            let before = fs::read(tape).unwrap();
            for (consumer, runnable, dir, _) in &runs {
                let replay = launch_fixture(
                    runnable,
                    &ready.target,
                    &tools,
                    dir,
                    true,
                    Tape::Replay(tape),
                    &none,
                )
                .unwrap();
                let observed = observe(&replay, &ready, &sites).unwrap();
                assert!(observed.blocked && observed.failures.is_empty());
                assert!(observed.exit_code.is_none());
                eprintln!("HOST_PARITY_CHUNK_NATIVE_PASS {producer:?} -> {consumer:?}");
            }
            assert_eq!(fs::read(tape).unwrap(), before);
        }
    });
}
