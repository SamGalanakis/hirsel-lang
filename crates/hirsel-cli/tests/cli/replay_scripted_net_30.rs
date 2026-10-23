//! A blocked scripted connect consumes its peer attempt before another connect runs.
use serde_json::json;
use std::{fs, path::Path, process::Command};

const SOURCE: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let n = io.net(rt)?
  let output = io.stdout(rt)?
  let (_, refused) = task.scope(tasks, fn(s: Scope) -> (u64, bool) {
    let mut set: TaskSet[bool] of s = task.set(s)
    task.add(s, mut set, task.async(s, fn() -> bool {
      match net.unix_connect(n, path.of("/replay/up")) { Error(ConnectionRefused()) => true, _ => false, }
    }))
    task.add(s, mut set, task.async(s, fn() -> bool {
      match net.unix_connect(n, path.of("/replay/up")) { Error(ConnectionRefused()) => true, _ => false, }
    }))
    task.select(s, take set)
  })
  let _ = out.print(output, if refused { "refused\n" } else { "connected\n" })
  Ok(())
}
"#;

fn run(command: &mut Command, expected: &[u8]) {
    let output = command.env("HIRSEL_JOBS", "4").output().unwrap();
    assert!(output.status.success(), "{command:?}: {output:?}");
    assert_eq!(output.stdout, expected, "{command:?}");
    assert!(output.stderr.is_empty(), "{command:?}: {output:?}");
}

#[test]
fn unanswered_then_refused_replays_without_clock_deadline_or_queue() {
    hirsel_engine::with_compiler_stack(|| {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        fs::write(dir.join("main.hir"), SOURCE).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = dir.join("hirsel.json");
        fs::write(&manifest, json!({"schema":1,"package":{"name":"scripted_replay","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let grants = dir.join("grants.json");
        fs::write(&grants, json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[],"net":{"provider":"net.script@2","rules":[{"address":"/replay/up","service":"0","operation":"unix_connect"}],"peers":[{"address":"/replay/up","port":0,"resolves_to":[],"tls":"plain","connections":["unanswered","refused"]}]}},"placement":{"cores":1,"commit":"shared","reorder":"off"}}).to_string()).unwrap();
        let cli = env!("CARGO_BIN_EXE_hirsel");
        for backend in ["llvm", "cranelift"] {
            run(
                Command::new(cli)
                    .args(["build", "--backend", backend])
                    .arg(&manifest)
                    .arg("-o")
                    .arg(dir.join(backend)),
                b"",
            );
        }
        for (recorder, level) in [
            ("llvm", "record"),
            ("cranelift", "record"),
            ("llvm", "durable"),
            ("cranelift", "durable"),
        ] {
            let mut config: serde_json::Value =
                serde_json::from_slice(&fs::read(&grants).unwrap()).unwrap();
            config["recording"] = level.into();
            fs::write(&grants, config.to_string()).unwrap();
            let tape = dir.join(format!("{recorder}-{level}.tape"));
            run(
                Command::new(dir.join(recorder))
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .arg("--hirsel-grants")
                    .arg(&grants),
                b"refused\n",
            );
            let original = fs::read(&tape).unwrap();
            let plain =
                hirsel_driver::replay_observations(&manifest, &tape, Some(1_000_000)).unwrap();
            let checked =
                hirsel_driver::replay_observations_checked(&manifest, &tape, Some(1_000_000))
                    .unwrap();
            assert!(plain.equivalent(&checked));
            assert_eq!(plain.output, b"refused\n");
            assert_eq!(plain.exit_code, Some(0));
            for width in [1, 2, 4] {
                run(
                    Command::new(cli)
                        .arg("run")
                        .arg(&manifest)
                        .arg("--replay")
                        .arg(&tape)
                        .arg("--cores")
                        .arg(width.to_string()),
                    b"refused\n",
                );
                for backend in ["llvm", "cranelift"] {
                    run(
                        Command::new(dir.join(backend))
                            .env("HIRSEL_RT_WORKERS", width.to_string())
                            .arg("--hirsel-replay")
                            .arg(&tape),
                        b"refused\n",
                    );
                }
            }
            assert_eq!(fs::read(&tape).unwrap(), original);
            let mut record = hirsel_rt::host_launch::read_recording(&tape).unwrap();
            let event = record
                .events
                .iter_mut()
                .find(|e| matches!(e.request, hirsel_rt::io::Request::Print { .. }))
                .unwrap();
            let hirsel_rt::io::Request::Print { bytes, .. } = &mut event.request else {
                unreachable!()
            };
            bytes.push(b'!');
            let bad = dir.join("tampered.tape");
            fs::write(&bad, hirsel_rt::launch::render_recording(&record).unwrap()).unwrap();
            for checked in [false, true] {
                let replay = if checked {
                    hirsel_driver::replay_observations_checked
                } else {
                    hirsel_driver::replay_observations
                };
                assert!(
                    replay(&manifest, &bad, Some(1_000_000))
                        .unwrap_err()
                        .to_string()
                        .contains("ReplayMismatch")
                );
            }
            for backend in ["llvm", "cranelift"] {
                let output = Command::new(dir.join(backend))
                    .arg("--hirsel-replay")
                    .arg(&bad)
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(3));
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("ReplayMismatch"),
                    "{output:?}"
                );
            }
        }
    });
}
