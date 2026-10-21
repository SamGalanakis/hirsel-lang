//! Split streams in both engines (core net.split and net.join): a reader task and a writer
//! task share one scripted connection through its halves, the recording names each half,
//! and every tape replays in either engine with no peer.
use std::path::PathBuf;
use std::process::{Command, Stdio};

const PROGRAM: &str = r#"pub type EchoError = Grant(GrantError) | Network(NetError) | Output(OutputError)

fn reader(n: Net, take r: ReadHalf of n) -> bool {
  let mut half = take r
  let mut received: List[u8] = []
  let mut open = true
  while open {
    open = match read_half.read(n, mut half, mut received, 8u64) {
      Ok(count) => count > 0u64,
      Error(_) => false,
    }
  }
  bytes.from_list(take received) == b"hi!"
}

fn writer(n: Net, take w: WriteHalf of n) -> bool {
  let mut half = take w
  let first = write_half.write_all(n, mut half, b"hi") == Ok(())
  let second = write_half.write(n, mut half, b"!") == Ok(1u64)
  first && second
}

pub fn main(rt: Io, args: Args) -> Result[(), EchoError] {
  let tasks = io.tasks(rt) ? Grant
  let n = io.net(rt) ? Grant
  let stdout = io.stdout(rt) ? Grant
  let stream = net.connect(n, "localhost", 7u16, Unbounded()) ? Network
  let (r, w) = net.split(n, take stream)
  let joined = net.join(n, take r, take w) ? Network
  let (read, write) = net.split(n, take joined)
  let (echoed, sent) = task.scope(
    tasks,
    fn(s: Scope) -> (bool, bool) {
      let mine = take read
      let yours = take write
      let reading = task.async(s, fn() -> bool { reader(n, take mine) })
      let writing = task.async(s, fn() -> bool { writer(n, take yours) })
      let sent = task.await(s, take writing)
      (task.await(s, take reading), sent)
    }
  )
  out.print(stdout, if sent && echoed { "ok\n" } else { "bad\n" }) ? Output
  Ok(())
}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("hirsel-split-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"split","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("grants.json"),
            serde_json::json!({"io": {
                "stdout": "stdout.bytes@1",
                "tasks": "tasks.cooperative@1",
                "dirs": [],
                "net": {
                    "provider": "net.script@2",
                    "rules": [{"address": "localhost", "service": "7", "operation": "connect"}],
                    "peers": [{
                        "address": "localhost", "port": 7, "resolves_to": ["127.0.0.1"],
                        "tls": "plain", "connections": [{"open": [
                            {"expect": [104, 105]}, {"send": [104, 105]},
                            {"expect": [33]}, {"send": [33]}, "close"
                        ]}]
                    }]
                },
            }, "args": []})
            .to_string(),
        )
        .unwrap();
        Self(root)
    }
    fn run(&self, backend: Option<&str>, args: &[&std::ffi::OsStr]) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .arg(self.0.join("main.hir.json"))
            .args(args);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        command.stdin(Stdio::null()).output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_ok(output: &std::process::Output, what: &str) {
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{what}: {output:?}"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n", "{what}");
}

/// Record a scripted run in each of `engines`, then replay every tape interpreted and
/// compiled.
fn round_trip(engines: &[Option<&str>]) {
    let fixture = Fixture::new(engines[0].unwrap_or("interpreted"));
    let grants = fixture.0.join("grants.json");
    for &engine in engines {
        let tape = fixture
            .0
            .join(format!("tape-{}", engine.unwrap_or("interpreted")));
        let live = fixture.run(
            engine,
            &[
                "--grants".as_ref(),
                grants.as_os_str(),
                "--record".as_ref(),
                tape.as_os_str(),
            ],
        );
        assert_ok(&live, &format!("live {engine:?}"));
        let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
        assert!(
            text.contains("net_split") && text.contains("net_join"),
            "{text}"
        );
        for replay in [None, Some("llvm")] {
            assert_ok(
                &fixture.run(replay, &["--replay".as_ref(), tape.as_os_str()]),
                &format!("{engine:?} replayed by {replay:?}"),
            );
        }
    }
}

#[test]
fn split_echo_records_and_replays_in_either_engine() {
    round_trip(&[Some("llvm"), None]);
}

#[test]
#[ignore = "full gate"]
fn full_gate_cranelift_split_echo_records_and_replays_in_either_engine() {
    round_trip(&[Some("cranelift")]);
}
