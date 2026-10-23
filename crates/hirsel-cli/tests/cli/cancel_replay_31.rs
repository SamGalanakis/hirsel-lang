//! A signal cancels a child whose generic scope returns a Result error (CONC-2/3).
use serde_json::json;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

const PROGRAM: &str = r#"fn scoped[R](tasks: Tasks, take body: once fn(Scope) -> R) -> R {
  task.scope(tasks, take body)
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let signals = io.signals(rt)?
  let _ = out.print(output, "ready\n")
  task.scope(tasks, fn(s: Scope) {
    let child = task.async(s, fn() {
      let status: Result[(), u8] = scoped(tasks, fn(nested: Scope) -> Result[(), u8] {
        match signal.wait(signals, [Terminate()]) {
          Error(Canceled(_)) => Error(7u8),
          _ => Ok(()),
        }
      })
      failure.check(status == Error(7u8), Arithmetic())
    })
    let received = signal.wait(signals, [Interrupt()])
    failure.check(received == Ok(Interrupt()), Arithmetic())
    task.cancel(s, child)
    task.await(s, take child)
  })
  let _ = out.print(output, "canceled\n")
  Ok(())
}
"#;

fn assert_output(output: std::process::Output) {
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"ready\ncanceled\n");
    assert!(output.stderr.is_empty(), "{output:?}");
}

/// Install the same default disposition as a terminal, wait until the program
/// has projected Signals, then interrupt only this test's private process group.
fn record(command: Command) {
    let mut launch = Command::new("env");
    launch
        .arg("--default-signal=INT")
        .arg(command.get_program())
        .args(command.get_args());
    let mut child = launch
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    assert_eq!(ready, "ready\n", "{:?}", child.try_wait());
    assert!(
        Command::new("kill")
            .args(["-INT", "--", &format!("-{}", child.id())])
            .status()
            .unwrap()
            .success()
    );
    let mut bytes = ready.into_bytes();
    stdout.read_to_end(&mut bytes).unwrap();
    let mut output = child.wait_with_output().unwrap();
    output.stdout = bytes;
    assert_output(output);
}

#[test]
fn signal_cancel_twin_replays_on_reference_and_checked() {
    hirsel_engine::with_compiler_stack(|| {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        std::fs::write(dir.join("main.hir"), PROGRAM).unwrap();
        let manifest = dir.join("hirsel.json");
        std::fs::write(&manifest, json!({"schema":1,"package":{"name":"cancel_twin","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let grants = dir.join("grants.json");
        std::fs::write(&grants, json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[],"signals":{"provider":"signals.host@1","kinds":["interrupt","terminate"]}},"placement":{"cores":1,"commit":"shared","reorder":"off"}}).to_string()).unwrap();
        let cli = env!("CARGO_BIN_EXE_hirsel");
        for engine in ["reference", "llvm", "cranelift"] {
            let tape = dir.join(format!("{engine}.tape"));
            let mut command = Command::new(cli);
            if engine == "reference" {
                command
                    .arg("run")
                    .arg(&manifest)
                    .arg("--record")
                    .arg(&tape)
                    .arg("--grants")
                    .arg(&grants);
            } else {
                let executable = dir.join(engine);
                let built = Command::new(cli)
                    .args(["build", "--backend", engine])
                    .arg(&manifest)
                    .arg("-o")
                    .arg(&executable)
                    .output()
                    .unwrap();
                assert!(built.status.success(), "{built:?}");
                command = Command::new(&executable);
                command
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .arg("--hirsel-grants")
                    .arg(&grants);
            }
            record(command);
            let bytes = std::fs::read(&tape).unwrap();
            let plain =
                hirsel_driver::replay_observations(&manifest, &tape, Some(1_000_000)).unwrap();
            let checked =
                hirsel_driver::replay_observations_checked(&manifest, &tape, Some(1_000_000))
                    .unwrap();
            assert!(plain.equivalent(&checked), "{engine}");
            assert_eq!(plain.output, b"ready\ncanceled\n");
            assert_eq!(plain.exit_code, Some(0));
            assert!(plain.failures.is_empty());
            assert_eq!(std::fs::read(&tape).unwrap(), bytes);
            eprintln!("CANCEL_REPLAY_PASS {engine} reference checked");
        }
    });
}

#[test]
fn checked_signal_cancel_twin_runs_fresh() {
    hirsel_engine::with_compiler_stack(|| {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine
            .sources
            .add("cancel-twin.hir", PROGRAM.into())
            .unwrap();
        let checked = hirsel_engine::check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let config = hirsel_engine::RunConfiguration {
            entry: hirsel_engine::entry_path(&context),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut simulation = hirsel_engine::SimulationConfig {
            signals: Some(hirsel_run_schema::SignalsGrant {
                provider: "signals.script@1".into(),
                kinds: vec![
                    hirsel_run_schema::SignalKind::Interrupt,
                    hirsel_run_schema::SignalKind::Terminate,
                ],
                script: Some(vec![hirsel_run_schema::SignalKind::Interrupt]),
            }),
            ..Default::default()
        };
        simulation
            .grants
            .extend(["Tasks", "Stdout", "Signals"].map(String::from));
        let fresh = hirsel_engine::run_observed_checked(&mut engine, &config, simulation).unwrap();
        assert!(
            matches!(
                fresh.outcome,
                Ok(hirsel_engine::RunOutcome::Returned(
                    hirsel_run_schema::Returned::Ok
                ))
            ),
            "{:?}",
            fresh.outcome
        );
        let observed = fresh.observation.unwrap();
        assert_eq!(observed.output, b"ready\ncanceled\n");
        assert!(observed.failures.is_empty());
    });
}

#[test]
#[ignore = "requires immutable recordings made by an older runtime and their original source manifest"]
fn production_cancel_recordings_refuse_incompatible_runtime() {
    hirsel_engine::with_compiler_stack(|| {
        let root = std::path::PathBuf::from(
            std::env::var_os("HIRSEL_CANCEL_REPLAY_FIXTURES").expect("recording evidence root"),
        );
        let manifest = std::path::PathBuf::from(
            std::env::var_os("HIRSEL_CANCEL_REPLAY_MANIFEST").expect("original source manifest"),
        );
        let mut count = 0;
        let mut dirs: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|d| d.unwrap().path())
            .collect();
        dirs.sort();
        for dir in dirs {
            if !dir.join("PASS").is_file()
                || !dir
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("-cancel-")
            {
                continue;
            }
            let tape = dir.join("recording");
            let original = std::fs::read(&tape).unwrap();
            for checked in [false, true] {
                let attempt = if checked {
                    hirsel_driver::replay_observations_checked(&manifest, &tape, Some(1_000_000))
                } else {
                    hirsel_driver::replay_observations(&manifest, &tape, Some(1_000_000))
                };
                let message = attempt.unwrap_err().to_string();
                assert!(
                    message.contains("\"kind\":\"binding\"")
                        && message.contains("\"field\":\"program\""),
                    "{} checked={checked}: {message}",
                    dir.display()
                );
                eprintln!(
                    "PRODUCTION_CANCEL_INCOMPATIBLE {} checked={checked}",
                    dir.display()
                );
                count += 1;
            }
            assert_eq!(std::fs::read(&tape).unwrap(), original);
            assert!(!dir.join("work").exists());
        }
        assert!(count > 0, "no cancellation tapes selected");
        eprintln!("PRODUCTION_CANCEL_INCOMPATIBLE_COUNT {count}");
    });
}
