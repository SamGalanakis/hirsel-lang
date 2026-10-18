//! Record a real-host run that reads a file and stdin and serves one HTTP exchange,
//! then replay it with the file and socket gone: the same stdout, no host effects.
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const PROGRAM: &str = r#"pub type ServerError = Grant(GrantError) | Network(NetError) | Http(
  std@1/http.HttpError
) | Files(FileError) | Input(InputError)

pub fn main(rt: Io, args: Args) -> Result[(), ServerError] {
  let output = io.stdout(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  let greeting = file.read_bytes(d, path.of("greeting.txt")) ? Files
  let name = match std@1/input.read_line(io.stdin(rt) ? Grant) ? Input {
    Some(line) => line,
    None() => "nobody\n",
  }
  let n = io.net(rt) ? Grant
  let mut listener = net.listen(
    n,
    SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: 0u16)
  ) ? Network
  let bound = net.local_addr(n, listener) ? Network
  let _ = out.print(output, std@1/u16.to_string(bound.port))
  let _ = out.print(output, "\n")
  let mut (stream, _) = net.accept(n, mut listener) ? Network
  let mut config = std@1/http.default_config()
  config.max_head_bytes = 16384
  config.max_body_bytes = 1048576
  let mut connection = std@1/http.server_connection(n, take stream, config)
  let _ = std@1/http.receive_request(mut connection) ? Http
  std@1/http.respond(mut connection, 200u16, [], greeting, true) ? Http
  let _ = out.print(output, "served ")
  let _ = out.print(output, name)
  Ok(())
}
"#;

/// A program directory with its manifest, a granted data directory and a grants file.
struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hirsel-replay-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/greeting.txt"), "hello from a file\n").unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"replayed","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("grants.json"),
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","stdin":"stdin.bytes@1","dirs":[{"path":"data","host":root.join("data"),"access":"read"}],"net":{"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":"0","operation":"listen"}]}},"args":[]}).to_string(),
        )
        .unwrap();
        Self(root)
    }
    fn source(&self) -> PathBuf {
        self.0.join("main.hir")
    }
    fn tape(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run a recording launch: feed stdin, read the bound port, make one request, and
/// return the full stdout after a successful exit.
fn record(mut command: Command) -> Vec<u8> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"Ada\n").unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let Ok(port) = line.trim().parse::<u16>() else {
        panic!("no port: {line:?} {:?}", child.wait_with_output());
    };
    let mut stream = std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = vec![];
    stream.read_to_end(&mut response).unwrap();
    assert!(
        response.ends_with(b"\r\n\r\nhello from a file\n"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    let mut recorded = line.into_bytes();
    stdout.read_to_end(&mut recorded).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{output:?}"
    );
    recorded
}

fn hirsel(native: bool, source: &Path, args: &[&std::ffi::OsStr]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.arg("run").arg(source).args(args);
    if native {
        command.arg("--native");
    }
    command
}

fn record_run(fixture: &Fixture, native: bool, tape: &Path) -> Vec<u8> {
    let grants = fixture.0.join("grants.json");
    record(hirsel(
        native,
        &fixture.source(),
        &[
            "--grants".as_ref(),
            grants.as_os_str(),
            "--record".as_ref(),
            tape.as_os_str(),
        ],
    ))
}

/// Replay with stdin closed; the data directory is moved away by the caller.
fn replay(mut command: Command) -> std::process::Output {
    command.stdin(Stdio::null()).output().unwrap()
}

fn replay_run(fixture: &Fixture, native: bool, tape: &Path) -> std::process::Output {
    replay(hirsel(
        native,
        &fixture.source(),
        &["--replay".as_ref(), tape.as_os_str()],
    ))
}

fn assert_replayed(output: &std::process::Output, recorded: &[u8], what: &str) {
    assert!(output.status.success(), "{what}: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(recorded),
        "{what}"
    );
    assert!(output.stderr.is_empty(), "{what}: {output:?}");
}

/// An edited program, even one whose request prefix matches, refuses the recording
/// before entry.
fn assert_edited_refuses(fixture: &Fixture, native: bool, tape: &Path) {
    std::fs::write(
        fixture.source(),
        PROGRAM.replace("\"served \"", "\"done \""),
    )
    .unwrap();
    let output = replay_run(fixture, native, tape);
    std::fs::write(fixture.source(), PROGRAM).unwrap();
    assert!(!output.status.success(), "native={native}: {output:?}");
    assert!(output.stdout.is_empty(), "native={native}: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains(r#"replay refused: {"kind":"binding","field":"program"}"#),
        "native={native}: {output:?}"
    );
}

#[test]
fn interpreted_recording_replays_without_its_files_or_socket() {
    let fixture = Fixture::new("interpreted");
    let tape = fixture.tape("tape");
    let recorded = record_run(&fixture, false, &tape);
    assert!(recorded.ends_with(b"\nserved Ada\n"), "{recorded:?}");
    std::fs::rename(fixture.0.join("data"), fixture.0.join("gone")).unwrap();
    assert_replayed(
        &replay_run(&fixture, false, &tape),
        &recorded,
        "interpreted",
    );
    assert_edited_refuses(&fixture, false, &tape);
    // A truncated recording is an unknown suffix: it never replays as complete input.
    let text = std::fs::read_to_string(&tape).unwrap();
    let truncated = fixture.tape("truncated");
    std::fs::write(&truncated, &text[..text.rfind("complete ").unwrap()]).unwrap();
    let output = replay_run(&fixture, false, &truncated);
    assert!(
        !output.status.success() && output.stdout.is_empty(),
        "{output:?}"
    );
}

#[test]
fn native_and_interpreted_recordings_replay_in_either_engine_and_executables() {
    let fixture = Fixture::new("cross");
    let native_tape = fixture.tape("native");
    let interpreted_tape = fixture.tape("interpreted");
    let native = record_run(&fixture, true, &native_tape);
    let interpreted = record_run(&fixture, false, &interpreted_tape);
    // A standalone executable records with the same flags its launcher documents.
    let executable = fixture.0.join("app");
    let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("build")
        .arg(fixture.source())
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let executable_tape = fixture.tape("executable");
    let mut command = Command::new(&executable);
    command
        .arg("--hirsel-record")
        .arg(&executable_tape)
        .arg("--hirsel-grants")
        .arg(fixture.0.join("grants.json"));
    let standalone = record(command);
    std::fs::rename(fixture.0.join("data"), fixture.0.join("gone")).unwrap();
    for (tape, recorded) in [
        (&native_tape, &native),
        (&interpreted_tape, &interpreted),
        (&executable_tape, &standalone),
    ] {
        for engine in [false, true] {
            assert_replayed(
                &replay_run(&fixture, engine, tape),
                recorded,
                &format!("{} replayed native={engine}", tape.display()),
            );
        }
        let mut command = Command::new(&executable);
        command.arg("--hirsel-replay").arg(tape);
        assert_replayed(
            &replay(command),
            recorded,
            &format!("{} replayed by the executable", tape.display()),
        );
    }
    assert_edited_refuses(&fixture, true, &native_tape);
}

/// Runs a private copy of echo; replay returns its recorded output with the copy deleted,
/// so nothing is spawned.
const PROCESS_PROGRAM: &str = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let p = io.process(rt)?
  let program = match arg.path(args, 0u64) {
    Ok(program) => program,
    Error(_) => path.of("/missing"),
  }
  match proc.run(p, program, ["recorded"], b"", None(), 64) {
    Ok(o) => {
      if o.status == Exited(0) && o.stdout == b"recorded\n" {
        let _ = out.print(out, "echo ran\n")
      }
    },
    Error(_) => {
      let _ = out.print(out, "echo failed\n")
    },
  }
  Ok(())
}
"#;

#[test]
fn process_recordings_replay_in_either_engine_without_spawning() {
    let root = std::env::temp_dir().join(format!("hirsel-replay-process-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let echo = root.join("echo");
    std::fs::copy("/bin/echo", &echo).unwrap();
    let source = root.join("main.hir");
    std::fs::write(&source, PROCESS_PROGRAM).unwrap();
    let grants = root.join("grants.json");
    std::fs::write(
        &grants,
        serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[],"process":{"provider":"process.spawn@1","rules":[{"program":echo}]}},"args":[echo]}).to_string(),
    )
    .unwrap();
    let tapes = [false, true].map(|native| {
        let tape = root.join(format!("tape-{native}"));
        let output = hirsel(
            native,
            &source,
            &[
                "--grants".as_ref(),
                grants.as_os_str(),
                "--record".as_ref(),
                tape.as_os_str(),
            ],
        )
        .output()
        .unwrap();
        assert_eq!(output.stdout, b"echo ran\n", "native={native}: {output:?}");
        assert!(
            std::fs::read_to_string(&tape)
                .unwrap()
                .contains("process_run ")
        );
        tape
    });
    std::fs::remove_file(&echo).unwrap();
    for tape in &tapes {
        for native in [false, true] {
            assert_replayed(
                &replay(hirsel(
                    native,
                    &source,
                    &["--replay".as_ref(), tape.as_os_str()],
                )),
                b"echo ran\n",
                &format!("{} replayed native={native}", tape.display()),
            );
        }
    }
    std::fs::remove_dir_all(&root).unwrap();
}
