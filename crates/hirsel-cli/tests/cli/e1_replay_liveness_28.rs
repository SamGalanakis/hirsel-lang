//! A deadline/select with an unanswered connect exposed a home/role wait cycle.
use std::path::Path;

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

pub(super) fn package(root: &Path) -> std::path::PathBuf {
    std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let manifest = root.join("hirsel.json");
    std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"select_replay","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    manifest
}

pub(super) fn write_grants(grants: &Path) {
    std::fs::write(grants, serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[],"net":{"provider":"net.script@2","rules":[{"address":"/firstreply/0.sock","service":"0","operation":"unix_connect"},{"address":"/firstreply/1.sock","service":"0","operation":"unix_connect"}],"peers":[{"address":"/firstreply/0.sock","port":0,"resolves_to":[],"tls":"plain","connections":[{"open":[{"send":[120]}]}]},{"address":"/firstreply/1.sock","port":0,"resolves_to":[],"tls":"plain","connections":["unanswered"]}]}},"placement":{"cores":4,"commit":"shared","reorder":"auto"}}).to_string()).unwrap();
}
