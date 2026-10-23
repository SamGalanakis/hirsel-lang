//! Live process bridges can arrive while home merges a deadline readiness query.
use std::{
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

// Supplemental live coverage of the process/task-set/deadline diamond shape.
// This reduction does not reproduce the archived full dagbuild race by itself.
// pool::tests::home_role_merge_serves_a_bridge_from_the_role_owner forces
// the exact bridge/role interleavings; keep that deterministic regression too.
const PROGRAM: &str = r#"fn job(p: Proc, deadline: Deadline) -> bool {
  let cmd = Command(program: path.of("/fixture/child"), args: ["ok"], dir: None(), env: None(), own_group: false)
  let mut child = match proc.spawn(p, take cmd) {
    Ok(handle) => take handle,
    Error(_) => return false,
  }
  child.close_stdin(p, mut child)
  child.wait(p, take child, Some(deadline)) == Ok(Exited(0))
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let p = io.process(rt)?
  let c = io.monotonic_clock(rt)?
  let output = io.stdout(rt)?
  let Ok(now) = clock.monotonic(c) else { return Ok(()) }
  let Some(at) = instant.checked_add(c, now, 30000000000u64) else { return Ok(()) }
  let deadline = Deadline(clock: c, at: at)
  let done = task.scope_until(tasks, deadline, fn(s: Scope) -> bool {
    let mut members: TaskSet[bool] of s = task.set(s)
    task.add(s, mut members, task.async(s, fn() -> bool { job(p, deadline) }))
    let Some(root) = task.await_any(s, members) else { task.cancel_all(s, take members); return false }
    if !task.await_at(s, mut members, root) { task.cancel_all(s, take members); return false }
    task.add(s, mut members, task.async(s, fn() -> bool { job(p, deadline) }))
    task.add(s, mut members, task.async(s, fn() -> bool { job(p, deadline) }))
    for _ in 0u64..2u64 {
      let Some(branch) = task.await_any(s, members) else { task.cancel_all(s, take members); return false }
      if !task.await_at(s, mut members, branch) { task.cancel_all(s, take members); return false }
    }
    task.add(s, mut members, task.async(s, fn() -> bool { job(p, deadline) }))
    let Some(joined) = task.await_any(s, members) else { task.cancel_all(s, take members); return false }
    let value = task.await_at(s, mut members, joined)
    task.cancel_all(s, take members)
    value
  })
  if done == Finished(true) { let _ = out.print(output, "done\n") }
  Ok(())
}
"#;

fn bounded(command: &mut Command, label: &str) -> Output {
    eprintln!("LIVE_LIVENESS STARTED {label}");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{label}: {output:?}");
            assert_eq!(output.stdout, b"done\n", "{label}: {output:?}");
            assert!(output.stderr.is_empty(), "{label}: {output:?}");
            eprintln!("LIVE_LIVENESS COMPLETED {label}");
            return output;
        }
        if Instant::now() >= until {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("{label}: live process diamond did not finish within 10s: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn process_diamond_live_finishes_repeatedly_at_each_width() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let manifest = root.join("hirsel.json");
    std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"live_diamond","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let children = vec![
        serde_json::json!({"program":"/fixture/child","args":["ok"],"steps":[{"print":[79,79]},{"print_err":[69,69]},{"exit":{"exited":0}}]});
        4
    ];
    let mut grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[],"process":{"provider":"process.script@1","rules":[{"program":"/fixture/child"}],"runs":[],"children":children}},"placement":{"cores":4,"commit":"shared","reorder":"auto"}});
    for engine in ["llvm", "cranelift"] {
        let exe = root.join(engine);
        let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", engine])
            .arg(&manifest)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(built.status.success(), "{engine}: {built:?}");
        for width in [1, 2, 4, 8] {
            grants["placement"]["cores"] = width.into();
            let grant_path = root.join(format!("grants-{width}.json"));
            std::fs::write(&grant_path, grants.to_string()).unwrap();
            for iteration in 0..51 {
                // The first launch uses ordinary production placement. The
                // loop explicitly exercises worker topology even on a pool
                // action whose CPU quota clamps ordinary placement to one.
                // Requested width here is not a claim of eight CPU permits.
                let label = format!("{engine}-w{width}-{iteration}");
                let tape = root.join(format!("{label}.tape"));
                let mut command = Command::new(&exe);
                command
                    .env_remove("HIRSEL_RT_MIGRATE")
                    .env_remove("HIRSEL_RT_WORKERS");
                if iteration != 0 {
                    command.env("HIRSEL_RT_WORKERS", width.to_string());
                }
                bounded(
                    command
                        .arg("--hirsel-record")
                        .arg(&tape)
                        .arg("--hirsel-grants")
                        .arg(&grant_path),
                    &label,
                );
            }
        }
    }
}
