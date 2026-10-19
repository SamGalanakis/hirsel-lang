//! Cooperative host readiness under several parked requests (DET-1): waits that become ready
//! at the same scheduling point are recorded there in creation order, before any is re-issued,
//! and a server whose connections interleave by readiness replays its live output exactly.
//! Each live run is recorded in both engines, and every tape replays in both.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Three children sleep until one shared instant a second away, which each reaches parked
/// however slowly the engine starts them: their timers are due together, so one scheduling
/// point finds all three ready, and they wake in creation order.
const SLEEPERS: &str = r#"fn sleeper(out: Stdout, c: MonotonicClock, child: u64, at: Instant of c) -> bool {
  match clock.sleep_until(c, at) {
    Ok(()) => {
      let _ = out.print(out, f"child {child} woke\n")
      true
    },
    Error(_) => false,
  }
}

pub fn main(rt: Io) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let tasks = io.tasks(rt)?
  let c = io.monotonic_clock(rt)?
  let Ok(now) = clock.monotonic(c) else {
    return Ok(())
  }
  let Some(at) = instant.checked_add(c, now, 1000000000u64) else {
    return Ok(())
  }
  task.scope(
    tasks,
    fn(s: Scope) -> () {
      let a = task.async(s, fn() -> bool { sleeper(out, c, 0, at) })
      let b = task.async(s, fn() -> bool { sleeper(out, c, 1, at) })
      let d = task.async(s, fn() -> bool { sleeper(out, c, 2, at) })
      let _ = task.await(s, take a)
      let _ = task.await(s, take b)
      let _ = task.await(s, take d)
    }
  )
  Ok(())
}
"#;

/// The scope's owner accepts three connections and serves each in a task of its own while
/// three client tasks connect, send and read their echo, so connects, accepts and reads park
/// together and complete in whatever order the host makes them ready.
const SERVER: &str = r#"fn network(e: NetError) -> () uses {} { () }
fn grant(e: GrantError) -> () uses {} { () }

fn serve(n: Net, take incoming: Stream of n) -> Result[(), ()] {
  let mut stream = take incoming
  let mut data: List[u8] = []
  net.read(n, mut stream, mut data, 64) ? network
  net.write_all(n, mut stream, bytes.from_list(take data)) ? network
  Ok(())
}

fn client(out: Stdout, n: Net, address: SocketAddr, id: u64) -> Result[(), ()] {
  let mut stream = net.connect_addr(n, address, Unbounded()) ? network
  net.write_all(n, mut stream, string.as_bytes(f"hello {id}")) ? network
  let mut echo: List[u8] = []
  let count = net.read(n, mut stream, mut echo, 64) ? network
  let _ = out.print(out, f"client {id} read {count}\n")
  Ok(())
}

pub fn main(rt: Io) -> Result[(), ()] {
  let out = io.stdout(rt) ? grant
  let n = io.net(rt) ? grant
  let tasks = io.tasks(rt) ? grant
  let address = SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: PORTu16)
  let listener = net.listen(n, address) ? network
  task.scope(tasks, fn(s: Scope) -> Result[(), ()] {
    let a = task.async(s, fn() -> Result[(), ()] { client(out, n, address, 0) })
    let b = task.async(s, fn() -> Result[(), ()] { client(out, n, address, 1) })
    let c = task.async(s, fn() -> Result[(), ()] { client(out, n, address, 2) })
    let mut accepting = take listener
    let mut i: u64 = 0
    while i < 3 {
      let (stream, _) = net.accept(n, mut accepting) ? network
      let child = task.async(s, fn() -> Result[(), ()] { serve(n, take stream) })
      task.detach(s, take child)
      i += 1
    }
    task.await(s, take a)?
    task.await(s, take b)?
    task.await(s, take c)
  })
}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str, program: &str, grants: serde_json::Value) -> Self {
        let root = std::env::temp_dir().join(format!("hirsel-ready-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("main.hir"), program).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"ready","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        std::fs::write(root.join("grants.json"), grants.to_string()).unwrap();
        Self(root)
    }
    fn run(&self, native: bool, args: &[&std::ffi::OsStr]) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .arg(self.0.join("main.hir.json"))
            .args(args);
        if native {
            command.arg("--native");
        }
        command.stdin(Stdio::null()).output().unwrap()
    }
    /// A live run recorded by one engine: its tape's text and its output.
    fn record(&self, native: bool) -> (PathBuf, String, String) {
        let grants = self.0.join("grants.json");
        let tape = self.0.join(format!("tape-{native}"));
        let live = stdout(
            &self.run(
                native,
                &[
                    "--grants".as_ref(),
                    grants.as_os_str(),
                    "--record".as_ref(),
                    tape.as_os_str(),
                ],
            ),
            &format!("live native={native}"),
        );
        let text = std::fs::read_to_string(&tape).unwrap();
        (tape, text, live)
    }
    /// The tape replays its live output in both engines.
    fn replay(&self, tape: &Path, live: &str) {
        for native in [false, true] {
            assert_eq!(
                stdout(
                    &self.run(native, &["--replay".as_ref(), tape.as_os_str()]),
                    &format!("{tape:?} replayed native={native}"),
                ),
                live,
                "{tape:?} replayed native={native}"
            );
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stdout(output: &std::process::Output, what: &str) -> String {
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{what}: {output:?}"
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// The `ready` lines of a tape, in order.
fn ready_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with("ready ")).collect()
}

#[test]
fn timers_due_together_are_ready_in_creation_order_and_replay_in_either_engine() {
    let fixture = Fixture::new(
        "sleepers",
        SLEEPERS,
        serde_json::json!({"io": {
            "stdout": "stdout.bytes@1",
            "tasks": "tasks.cooperative@1",
            "monotonic_clock": "clock.monotonic.ns@1",
            "dirs": [],
        }}),
    );
    let mut recorded = Vec::new();
    for native in [false, true] {
        let (tape, text, live) = fixture.record(native);
        assert_eq!(
            live, "child 0 woke\nchild 1 woke\nchild 2 woke\n",
            "native={native}"
        );
        // One scheduling point readies all three waits, recorded together in creation order
        // before any of them is re-issued.
        let ready: Vec<String> = ready_lines(&text).into_iter().map(str::to_owned).collect();
        assert_eq!(ready.len(), 3, "native={native}: {text}");
        for (wait, line) in ready.iter().enumerate() {
            assert!(
                line.starts_with(&format!("ready {wait} ")),
                "native={native}: {text}"
            );
        }
        let first = text.find(&ready[0]).unwrap();
        let last = text.find(&ready[2]).unwrap();
        assert!(
            !text[first..last].contains("sleep_until"),
            "a re-issued wait between ready events: {text}"
        );
        fixture.replay(&tape, &live);
        recorded.push(ready);
    }
    // Both engines pass the same scheduling points while the waits are parked.
    assert_eq!(recorded[0], recorded[1]);
}

#[test]
fn a_multi_connection_server_replays_its_live_interleaving_in_either_engine() {
    for native in [false, true] {
        let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        let fixture = Fixture::new(
            &format!("server-{native}"),
            &SERVER.replace("PORT", &port.to_string()),
            serde_json::json!({"io": {
                "stdout": "stdout.bytes@1",
                "tasks": "tasks.cooperative@1",
                "dirs": [],
                "net": {"provider": "net.stream@1", "rules": [
                    {"address": "127.0.0.1", "service": port.to_string(), "operation": "listen"},
                    {"address": "127.0.0.1", "service": port.to_string(), "operation": "connect"},
                ]},
            }}),
        );
        let (tape, text, live) = fixture.record(native);
        let mut lines: Vec<&str> = live.lines().collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            ["client 0 read 7", "client 1 read 7", "client 2 read 7"],
            "native={native}"
        );
        assert!(!ready_lines(&text).is_empty(), "nothing parked: {text}");
        fixture.replay(&tape, &live);
    }
}
