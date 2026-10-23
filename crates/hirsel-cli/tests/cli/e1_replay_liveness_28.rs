//! A deadline/select with an unanswered connect exposed a home/role wait cycle.
use std::{
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

const PROGRAM: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let n = io.net(rt)?
  let output = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let Ok(now) = clock.monotonic(c) else { return Ok(()) }
  let Some(at) = instant.checked_add(c, now, 50000000u64) else { return Ok(()) }
  let bounded = task.scope_until(tasks, Deadline(clock: c, at: at), fn(s: Scope) -> (u64, bool) {
    let mut set: TaskSet[bool] of s = task.set(s)
    task.add(s, mut set, task.async(s, fn() -> bool {
      let Ok(connected) = net.unix_connect(n, path.of("/firstreply/0.sock")) else { return false }
      let mut stream = take connected
      let mut buf: List[u8] = []
      net.read(n, mut stream, mut buf, 1u64) == Ok(1u64)
    }))
    task.add(s, mut set, task.async(s, fn() -> bool {
      match net.unix_connect(n, path.of("/firstreply/1.sock")) {
        Ok(_) => true,
        Error(_) => false,
      }
    }))
    task.select(s, take set)
  })
  let _ = out.print(output, "done\n")
  Ok(())
}
"#;

fn bounded(command: &mut Command, label: &str) -> Output {
    eprintln!("REPLAY_LIVENESS STARTED {label}");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success() && output.stderr.is_empty(),
                "{label}: {output:?}"
            );
            eprintln!("REPLAY_LIVENESS COMPLETED {label}");
            return output;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("{label}: replay did not finish within 20s: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn package(root: &Path) -> std::path::PathBuf {
    std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let manifest = root.join("hirsel.json");
    std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"select_replay","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    manifest
}

#[test]
fn deadline_select_native_replay_finishes_repeatedly_at_each_width() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let manifest = package(root);
    let cli = env!("CARGO_BIN_EXE_hirsel");
    let grants = root.join("grants.json");
    std::fs::write(&grants, serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[],"net":{"provider":"net.script@2","rules":[{"address":"/firstreply/0.sock","service":"0","operation":"unix_connect"},{"address":"/firstreply/1.sock","service":"0","operation":"unix_connect"}],"peers":[{"address":"/firstreply/0.sock","port":0,"resolves_to":[],"tls":"plain","connections":[{"open":[{"send":[120]}]}]},{"address":"/firstreply/1.sock","port":0,"resolves_to":[],"tls":"plain","connections":["unanswered"]}]}},"placement":{"cores":4,"commit":"shared","reorder":"auto"}}).to_string()).unwrap();
    for engine in ["llvm", "cranelift"] {
        let exe = root.join(engine);
        let tape = root.join(format!("{engine}.tape"));
        // Compilation is setup, outside the replay's normal completion bound.
        let built = Command::new(cli)
            .args(["build", "--backend", engine])
            .arg(&manifest)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(built.status.success(), "{engine}: {built:?}");
        let recorded = bounded(
            Command::new(&exe)
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-grants")
                .arg(&grants),
            &format!("{engine} record"),
        );
        assert_eq!(recorded.stdout, b"done\n");
        let original = std::fs::read(&tape).unwrap();
        let interpreted = bounded(
            Command::new(cli)
                .arg("run")
                .arg(&manifest)
                .arg("--replay")
                .arg(&tape),
            &format!("{engine} interpreter replay"),
        );
        assert_eq!(interpreted.stdout, recorded.stdout);
        for width in [1, 2, 4] {
            for repeat in 0..50 {
                let output = bounded(
                    Command::new(&exe)
                        .arg("--hirsel-replay")
                        .arg(&tape)
                        .env("HIRSEL_RT_WORKERS", width.to_string())
                        .env("HIRSEL_JOBS", "4"),
                    &format!("{engine} workers={width} repeat={repeat}"),
                );
                assert_eq!(output.stdout, recorded.stdout);
            }
        }
        assert_eq!(std::fs::read(tape).unwrap(), original);
    }
}
