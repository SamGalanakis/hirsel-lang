//! Host signals (`signals.host@1`): a run that projects its Signals consumes an Interrupt
//! and shuts down cleanly, a run without the grant dies by it, and a second Interrupt while
//! the first is unhandled keeps the default kill, interpreted and compiled. Signals go to the
//! launcher's process group, as a terminal sends them, so a compiled run's launcher must
//! leave them to its executable.
use std::io::{BufRead, BufReader};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const WAIT: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let signals = io.signals(rt)?
  let _ = out.print(out, "ready\n")
  let _ = match signal.wait(signals, [Interrupt(), Terminate()]) {
    Ok(Interrupt()) => out.print(out, "stopping on interrupt\n"),
    Ok(_) => out.print(out, "stopping\n"),
    Error(_) => out.print(out, "wait failed\n"),
  }
  Ok(())
}
"#;

const SLEEP: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let c = io.monotonic_clock(rt)?
  let _ = match io.signals(rt) {
    Ok(_) => out.print(out, "ready observing\n"),
    Error(_) => out.print(out, "ready default\n"),
  }
  let now = match clock.monotonic(c) {
    Ok(now) => now,
    Error(_) => return Ok(()),
  }
  let _ = match instant.checked_add(c, now, 3600000000000u64) {
    Some(at) => clock.sleep_until(c, at),
    None() => Ok(()),
  }
  let _ = out.print(out, "woke\n")
  Ok(())
}
"#;

struct Source(PathBuf);
impl Source {
    fn new(name: &str, text: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("hirsel-signals-{name}-{}.hir", std::process::id()));
        std::fs::write(&path, text).unwrap();
        Self(path)
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn grants(signals: bool) -> String {
    let mut io = serde_json::json!({
        "stdout": "stdout.bytes@1",
        "monotonic_clock": "clock.monotonic.ns@1",
        "dirs": [],
    });
    if signals {
        io["signals"] =
            serde_json::json!({"provider": "signals.host@1", "kinds": ["interrupt", "terminate"]});
    }
    serde_json::json!({"io": io, "args": []}).to_string()
}

/// Launch `source`, wait for its first line, send `count` Interrupts to its process group,
/// one after its handler has seen the previous, and return its status and every line.
fn interrupted(
    source: &Source,
    signals: bool,
    native: bool,
    count: usize,
) -> (std::process::ExitStatus, Vec<String>) {
    // A launcher may start tests with SIGINT ignored, as a shell starts background jobs; a
    // terminal's foreground job has the default disposition.
    let mut command = Command::new("env");
    command
        .arg("--default-signal=INT")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source.0)
        .arg("--grants")
        .arg(grants(signals));
    if native {
        command.arg("--native");
    }
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut lines = vec![String::new()];
    stdout.read_line(&mut lines[0]).unwrap();
    if !lines[0].starts_with("ready") {
        let output = child.wait_with_output().unwrap();
        panic!("{lines:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
    for _ in 0..count {
        let sent = Command::new("kill")
            .args(["-INT", "--", &format!("-{}", child.id())])
            .status()
            .unwrap();
        assert!(sent.success());
        // The handler only enqueues; give it time to run before the next arrival.
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .status();
            panic!("still running after {count} Interrupts: {lines:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    lines.extend(stdout.lines().map(Result::unwrap));
    (
        status,
        lines
            .into_iter()
            .map(|l| l.trim_end().to_string())
            .collect(),
    )
}

fn engines() -> [bool; 2] {
    [false, true]
}

#[test]
fn a_granted_run_consumes_an_interrupt_and_returns_ok() {
    let source = Source::new("wait", WAIT);
    for native in engines() {
        let (status, lines) = interrupted(&source, true, native, 1);
        assert_eq!(status.code(), Some(0), "native {native}: {lines:?}");
        assert_eq!(lines, ["ready", "stopping on interrupt"], "native {native}");
    }
}

#[test]
fn a_run_without_the_grant_dies_by_the_interrupt() {
    let source = Source::new("default", SLEEP);
    for native in engines() {
        let (status, lines) = interrupted(&source, false, native, 1);
        assert_eq!(status.signal(), Some(SIGINT), "native {native}: {lines:?}");
        assert_eq!(lines, ["ready default"], "native {native}");
    }
}

#[test]
fn a_second_interrupt_while_the_first_is_unhandled_kills_the_run() {
    let source = Source::new("double", SLEEP);
    for native in engines() {
        let (status, lines) = interrupted(&source, true, native, 2);
        assert_eq!(status.signal(), Some(SIGINT), "native {native}: {lines:?}");
        assert_eq!(lines, ["ready observing"], "native {native}");
    }
}

const SIGINT: i32 = 2;
