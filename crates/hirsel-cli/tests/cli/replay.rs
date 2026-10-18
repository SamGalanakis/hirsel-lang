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
  let _ = std@1/http.receive_request(mut connection, None()) ? Http
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
  match proc.run(p, Command(program: program, args: ["recorded"], dir: None(), env: None()), b"", 64, None()) {
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

/// A held process can only time out; the success run gets a fresh hour-long budget.
/// See docs/spec/core-library.md#processes.
const DEADLINE_PROGRAM: &str = r#"fn grant(take e: GrantError) -> () {}
fn clock_error(take e: ClockError) -> () {}

pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let out = io.stdout(rt) ? grant
  let p = io.process(rt) ? grant
  let c = io.monotonic_clock(rt) ? grant
  let sleep = match arg.path(args, 0u64) {
    Ok(program) => program,
    Error(_) => path.of("/missing"),
  }
  let now = clock.monotonic(c) ? clock_error
  let soon = match instant.checked_add(c, now, 200000000u64) {
    Some(at) => at,
    None() => now,
  }
  match proc.run(p, Command(program: sleep, args: ["infinity"], dir: None(), env: None()), b"", 64, Some(Deadline(clock: c, at: soon))) {
    Error(TimedOut()) => {
      let _ = out.print(out, "timed out\n")
    },
    _ => {
      let _ = out.print(out, "ran on\n")
    },
  }
  let fresh = clock.monotonic(c) ? clock_error
  let later = match instant.checked_add(c, fresh, 3600000000000u64) {
    Some(at) => at,
    None() => fresh,
  }
  match proc.run(p, Command(program: sleep, args: ["0"], dir: None(), env: None()), b"", 64, Some(Deadline(clock: c, at: later))) {
    Ok(o) => {
      if o.status == Exited(0) {
        let _ = out.print(out, "in time\n")
      }
    },
    Error(_) => {
      let _ = out.print(out, "failed\n")
    },
  }
  Ok(())
}
"#;

/// The host signals stdin only after the script has published its PID through a
/// FIFO. The script never exits by itself, so cancellation cannot race its exit.
const CANCEL_PROGRAM: &str = r#"fn grant(take e: GrantError) -> () {}

pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let out = io.stdout(rt) ? grant
  let p = io.process(rt) ? grant
  let tasks = io.tasks(rt) ? grant
  let input = io.stdin(rt) ? grant
  let script = match arg.path(args, 0u64) {
    Ok(program) => program,
    Error(_) => path.of("/missing"),
  }
  let canceled = task.scope(
    tasks,
    fn(s: Scope) -> bool {
      let h = task.async(
        s,
        fn() -> bool {
          proc.run(p, Command(program: script, args: [], dir: None(), env: None()), b"", 64, None()) == Error(Canceled(Requested))
        }
      )
      let mut signal: List[u8] = []
      let _ = stdin.read(input, mut signal, 1u64)
      task.cancel(s, h)
      task.await(s, take h)
    }
  )
  if canceled {
    let _ = out.print(out, "canceled\n")
  }
  Ok(())
}
"#;

/// A line protocol with a streaming `cat`: each line written comes back, closing its input
/// ends it, a killed child is Signaled(9), and a dropped one is killed.
const STREAM_PROGRAM: &str = r#"fn grant(take e: GrantError) -> () {}
fn failed(take e: ProcessError) -> () {}

fn echo(p: Proc, mut c: Child of p, take text: Bytes) -> Result[Bytes, ProcessError] {
  let want = bytes.len(text)
  child.write(p, mut c, text)?
  let mut got: List[u8] = []
  while list.len(got) < want {
    if child.read_stdout(p, mut c, mut got, 64u64)? == 0u64 {
      return Error(InputClosed())
    }
  }
  Ok(bytes.from_list(take got))
}

fn show(out: Stdout, take b: Bytes) -> () {
  match string.from_bytes(take b) {
    Ok(s) => {
      let _ = out.print(out, s)
    },
    Error(_) => {},
  }
}

pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let out = io.stdout(rt) ? grant
  let p = io.process(rt) ? grant
  let cat = match arg.path(args, 0u64) {
    Ok(program) => program,
    Error(_) => path.of("/missing"),
  }
  let mut c = proc.spawn(p, Command(program: cat, args: [], dir: None(), env: None())) ? failed
  show(out, echo(p, mut c, b"ping\n") ? failed)
  show(out, echo(p, mut c, b"pong\n") ? failed)
  child.close_stdin(p, mut c)
  let mut rest: List[u8] = []
  if child.read_stdout(p, mut c, mut rest, 64u64) == Ok(0u64) {
    let _ = out.print(out, "eof\n")
  }
  if child.wait(p, take c) == Ok(Exited(0)) {
    let _ = out.print(out, "exited 0\n")
  }
  let mut killed = proc.spawn(p, Command(program: cat, args: [], dir: None(), env: None())) ? failed
  child.kill(p, mut killed)
  if child.wait(p, take killed) == Ok(Signaled(9)) {
    let _ = out.print(out, "killed\n")
  }
  let _ = proc.spawn(p, Command(program: cat, args: [], dir: None(), env: None()))
  Ok(())
}
"#;

/// Record `program` with `grants` in both engines, then delete the granted program and
/// replay each tape in both engines: nothing is spawned, and the output is the recorded one.
fn process_round_trip(
    name: &str,
    program: &str,
    child: &Path,
    expected: &[u8],
    ready: Option<&Path>,
) -> PathBuf {
    let root = child.parent().unwrap();
    let source = root.join(format!("{name}.hir"));
    std::fs::write(&source, program).unwrap();
    let grants = root.join(format!("{name}.json"));
    std::fs::write(
        &grants,
        serde_json::json!({"io":{"stdout":"stdout.bytes@1","stdin":"stdin.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[],"process":{"provider":"process.spawn@1","rules":[{"program":child}]}},"args":[child]}).to_string(),
    )
    .unwrap();
    let tapes = [false, true].map(|native| {
        let tape = root.join(format!("{name}-tape-{native}"));
        let mut command = hirsel(
            native,
            &source,
            &[
                "--grants".as_ref(),
                grants.as_os_str(),
                "--record".as_ref(),
                tape.as_os_str(),
            ],
        );
        let output = if let Some(ready) = ready {
            let mut running = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            // Blocking FIFO IO is the readiness handshake: no sleeps or polling.
            let mut pid = String::new();
            std::fs::File::open(ready)
                .unwrap()
                .read_to_string(&mut pid)
                .unwrap();
            let pid: u32 = pid.trim().parse().unwrap();
            running.stdin.take().unwrap().write_all(b"!").unwrap();
            let output = running.wait_with_output().unwrap();
            // Cancellation has killed and reaped the exact child that signaled readiness.
            assert!(
                Command::new("/bin/sh")
                    .args(["-c", "! kill -0 \"$1\" 2>/dev/null", "sh", &pid.to_string()])
                    .status()
                    .unwrap()
                    .success(),
                "native={native}: child {pid} survived cancellation"
            );
            output
        } else {
            command.output().unwrap()
        };
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "native={native}: {output:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(expected),
            "native={native}: {output:?}"
        );
        tape
    });
    let copy = std::fs::read(child).unwrap();
    std::fs::remove_file(child).unwrap();
    for tape in &tapes {
        for native in [false, true] {
            assert_replayed(
                &replay(hirsel(
                    native,
                    &source,
                    &["--replay".as_ref(), tape.as_os_str()],
                )),
                expected,
                &format!("{} replayed native={native}", tape.display()),
            );
        }
    }
    std::fs::write(child, copy).unwrap();
    root.to_path_buf()
}

fn process_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("hirsel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

#[test]
fn timed_out_runs_kill_their_child_and_replay_without_spawning() {
    let root = process_root("replay-deadline");
    let sleep = root.join("sleep");
    std::fs::copy("/bin/sleep", &sleep).unwrap();
    process_round_trip(
        "deadline",
        DEADLINE_PROGRAM,
        &sleep,
        b"timed out\nin time\n",
        None,
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_canceled_parent_kills_its_running_child_and_replays_without_spawning() {
    use std::os::unix::fs::PermissionsExt;
    let root = process_root("replay-cancel");
    let ready = root.join("ready");
    // A FIFO lets the child acknowledge startup without a clock or a filesystem poll.
    unsafe extern "C" {
        fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
    }
    let pipe = std::ffi::CString::new(ready.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: pipe is a live NUL-terminated path; mode grants only this user access.
    assert_eq!(unsafe { mkfifo(pipe.as_ptr(), 0o600) }, 0);
    let script = root.join("child.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec /bin/sleep infinity\n",
            ready.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    process_round_trip(
        "cancel",
        CANCEL_PROGRAM,
        &script,
        b"canceled\n",
        Some(&ready),
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_streaming_cat_child_round_trips_lines_and_replays_without_spawning() {
    let root = process_root("replay-stream");
    let cat = root.join("cat");
    std::fs::copy("/bin/cat", &cat).unwrap();
    process_round_trip(
        "stream",
        STREAM_PROGRAM,
        &cat,
        b"ping\npong\neof\nexited 0\nkilled\n",
        None,
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn filesystem_breadth_source_replays_across_backends_without_following_outside_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new("filesystem-breadth");
    std::fs::write(
        fixture.source(),
        include_str!("../../../../conformance/CAP/CAP-2-filesystem-breadth-host-run.hir"),
    )
    .unwrap();
    let outside = fixture.0.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"outside").unwrap();
    let grants = fixture.0.join("fs-grants.json");
    std::fs::write(&grants, serde_json::json!({"io":{"dirs":[{"path":"","host":fixture.0.join("data"),"access":"read_write"}]}}).to_string()).unwrap();
    let mut tapes = vec![];
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let data = fixture.0.join("data");
        std::fs::create_dir_all(data.join("tree")).unwrap();
        symlink(&outside, data.join("tree/link")).unwrap();
        symlink(&outside, data.join("outside")).unwrap();
        let tape = fixture.tape(&format!("filesystem-{}.tape", backend.unwrap_or("interp")));
        let mut command = hirsel(
            backend.is_some(),
            &fixture.source(),
            &[
                "--grants".as_ref(),
                grants.as_os_str(),
                "--record".as_ref(),
                tape.as_os_str(),
            ],
        );
        if let Some(backend) = backend {
            command.args(["--backend", backend]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "{backend:?}: {output:?}"
        );
        assert!(!data.join("tree").exists());
        assert!(!data.join("outside").exists());
        assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"outside");
        assert_eq!(
            std::fs::metadata(data.join("source"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        tapes.push(tape);
    }
    std::fs::remove_dir_all(fixture.0.join("data")).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
    for tape in tapes {
        for backend in [None, Some("llvm"), Some("cranelift")] {
            let mut command = hirsel(
                backend.is_some(),
                &fixture.source(),
                &["--replay".as_ref(), tape.as_os_str()],
            );
            if let Some(backend) = backend {
                command.args(["--backend", backend]);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success() && output.stderr.is_empty(),
                "{} {backend:?}: {output:?}",
                tape.display()
            );
        }
    }
}
