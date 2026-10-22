//! `remote.tls@1` across two processes: `hirsel serve` hosts a program's callee entries on a
//! Unix socket under mutual TLS, and a caller run of the same program sends it a call with
//! io and computes that succeed, fail and fault. The callee runs record their own tapes on
//! the host; the caller's tape replays alone, each host tape replays alone, and `hirsel
//! tapes check` joins them. A client without a certificate never reaches the host.
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

const PROGRAM: &str = r#"pub type AppError = Grant(GrantError) | Output(OutputError)

fn report(rt: Io, line: String) -> Result[u64, String] {
  match io.stdout(rt) {
    Ok(o) => {
      let _ = out.print(o, line)
      Ok(1u64)
    },
    Error(_) => Error("no stdout"),
  }
}

fn half(n: u64) -> Result[u64, String] {
  failure.check(n < 100u64, Arithmetic())
  if n % 2u64 == 1u64 {
    return Error("odd")
  }
  Ok(n / 2u64)
}

fn shown(outcome: Result[u64, RunError[String]]) -> String {
  match outcome {
    Ok(n) => if n == 4u64 {
      "ok 4\n"
    } else {
      "ok\n"
    },
    Error(Failed(e)) => if e == "odd" {
      "failed odd\n"
    } else {
      "failed\n"
    },
    Error(Faulted(_)) => "faulted\n",
    Error(NotRun(Denied())) => "denied\n",
    Error(NotRun(Unreachable())) => "unreachable\n",
    Error(_) => "refused\n",
  }
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  let far = io.runner(rt, "far") ? Grant
  let called = shown(runner.call(far, report, "remote\n"))
  let even = shown(runner.compute(far, half, 8u64))
  let odd = shown(runner.compute(far, half, 3u64))
  let large = shown(runner.compute(far, half, 200u64))
  out.print(output, called) ? Output
  out.print(output, even) ? Output
  out.print(output, odd) ? Output
  out.print(output, large) ? Output
  Ok(())
}
"#;

/// The TLS fixtures (crates/hirsel-rt/tests/it/tls-data/README.md): a self-signed host
/// identity for `localhost`, a client CA, and a client it issued for `agent-7.hirsel.test`.
const TLS: [(&str, &[u8]); 5] = [
    (
        "server.cert",
        include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.cert"),
    ),
    (
        "server.key",
        include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.key"),
    ),
    (
        "client-ca.cert",
        include_bytes!("../../../hirsel-rt/tests/it/tls-data/client-ca.cert"),
    ),
    (
        "client.cert",
        include_bytes!("../../../hirsel-rt/tests/it/tls-data/client.cert"),
    ),
    (
        "client.key",
        include_bytes!("../../../hirsel-rt/tests/it/tls-data/client.key"),
    ),
];

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        // A short directory: a Unix socket path is at most 107 bytes.
        let root_scratch = tempfile::Builder::new()
            .prefix(&format!("hirsel-remote-{name}-"))
            .tempdir_in("/tmp")
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        std::fs::create_dir_all(root.join("tapes")).unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"remote","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        let tls = root.join("tls");
        std::fs::create_dir_all(&tls).unwrap();
        for (name, bytes) in TLS {
            std::fs::write(tls.join(name), bytes).unwrap();
        }
        let file = |name: &str| tls.join(name).to_str().unwrap().to_owned();
        let socket = root.join("host.sock").to_str().unwrap().to_owned();
        std::fs::write(
            root.join("host.json"),
            serde_json::json!({
                "listen": {"unix": socket},
                "identity": {"chain": file("server.cert"), "key": file("server.key")},
                "clients": {"pem": file("client-ca.cert")},
                "callers": [{"name": "app", "identity": {"subject": "agent-7.hirsel.test"},
                             "grants": {"stdout": "stdout.bytes@1", "dirs": []},
                             "definitions": null}],
                "recording": "record",
                "tapes": root.join("tapes").to_str().unwrap(),
            })
            .to_string(),
        )
        .unwrap();
        for (name, client) in [
            ("caller.json", Some(("client.cert", "client.key"))),
            ("anonymous.json", None),
        ] {
            let identity = client
                .map(|(chain, key)| serde_json::json!({"chain": file(chain), "key": file(key)}));
            std::fs::write(
                root.join(name),
                serde_json::json!({"io": {"stdout": "stdout.bytes@1", "dirs": [], "runners": [{
                    "name": "far", "provider": "remote.tls@1",
                    "endpoint": {"unix": socket}, "server_name": "localhost",
                    "roots": {"pem": file("server.cert")}, "identity": identity,
                    "grants": {"stdout": "stdout.bytes@1", "dirs": []},
                    "definitions": null}]}})
                .to_string(),
            )
            .unwrap();
        }
        let _ = root_scratch.keep();
        Self(root)
    }
    fn hirsel(&self, args: &[&std::ffi::OsStr]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(args)
            .output()
            .unwrap()
    }
    /// Start the host and wait until it accepts connections.
    fn serve(&self) -> Host {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["serve".as_ref(), self.0.join("main.hir.json").as_os_str()])
            .arg("--host")
            .arg(self.0.join("host.json"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let mut seen = String::new();
        loop {
            match lines.next() {
                Some(Ok(line)) if line.starts_with("hirsel serve: listening on ") => break,
                Some(Ok(line)) => seen.push_str(&line),
                _ => {
                    let _ = child.kill();
                    panic!("the host did not start: {seen}");
                }
            }
        }
        // Keep draining the host's stderr so it never blocks on a full pipe.
        std::thread::spawn(move || lines.for_each(|line| eprintln!("host: {line:?}")));
        Host(Some(child))
    }
    fn call(&self, grants: &str, tape: Option<&Path>) -> Output {
        let manifest = self.0.join("main.hir.json");
        let grants = self.0.join(grants);
        let mut args = vec![
            "run".as_ref(),
            manifest.as_os_str(),
            "--grants".as_ref(),
            grants.as_os_str(),
        ];
        if let Some(tape) = tape {
            args.extend(["--record".as_ref(), tape.as_os_str()]);
        }
        self.hirsel(&args)
    }
    fn replay(&self, tape: &Path) -> Output {
        let manifest = self.0.join("main.hir.json");
        self.hirsel(&[
            "run".as_ref(),
            manifest.as_os_str(),
            "--replay".as_ref(),
            tape.as_os_str(),
        ])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running host, stopped when dropped.
struct Host(Option<Child>);
impl Host {
    /// Stop the host: what its callees printed.
    fn stop(mut self) -> String {
        let mut child = self.0.take().unwrap();
        let _ = child.kill();
        let output = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn calls_cross_to_a_host_over_a_unix_socket_and_every_tape_replays_alone() {
    let fixture = Fixture::new("e2e");
    let host = fixture.serve();
    let tape = fixture.0.join("T");
    let live = fixture.call("caller.json", Some(&tape));
    // The host requires a client certificate: a client without one never opens a session.
    let anonymous = fixture.call("anonymous.json", None);
    let printed = host.stop();
    assert!(live.status.success(), "{}", stderr(&live));
    assert_eq!(stdout(&live), "ok\nok 4\nfailed odd\nfaulted\n");
    // The call with io ran on the host, under the host's grant for this caller.
    assert_eq!(printed, "remote\n");
    assert!(anonymous.status.success(), "{}", stderr(&anonymous));
    assert_eq!(
        stdout(&anonymous),
        "unreachable\nunreachable\nunreachable\nunreachable\n"
    );

    let caller = fixture.replay(&tape);
    assert!(caller.status.success(), "{}", stderr(&caller));
    assert_eq!(stdout(&caller), "ok\nok 4\nfailed odd\nfaulted\n");

    let mut tapes: Vec<PathBuf> = std::fs::read_dir(fixture.0.join("tapes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        // Diagnostic timing sidecars accompany tapes but are not replay inputs.
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "tape")
        })
        .collect();
    tapes.sort();
    assert_eq!(tapes.len(), 4, "{tapes:?}");
    let mut replayed: Vec<String> = tapes
        .iter()
        .map(|callee| {
            let replay = fixture.replay(callee);
            assert!(
                !stderr(&replay).contains("replay"),
                "{}: {}",
                callee.display(),
                stderr(&replay)
            );
            stdout(&replay)
        })
        .collect();
    replayed.sort();
    assert_eq!(replayed, ["", "", "", "remote\n"]);

    let mut args = vec!["tapes".as_ref(), "check".as_ref(), tape.as_os_str()];
    args.extend(tapes.iter().map(|t| t.as_os_str()));
    let check = fixture.hirsel(&args);
    assert!(
        check.status.success(),
        "{}{}",
        stdout(&check),
        stderr(&check)
    );
    assert!(
        stdout(&check).ends_with("tapes: 4 links checked, 0 unchecked\n"),
        "{}",
        stdout(&check)
    );
}
