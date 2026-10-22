//! Go net/http/httptest server.go:215–231,319–346 and server_test.go:79–97
//! (ebe2feefc400f813aefdc8a036e66d2619da6649): retain a loopback port-zero
//! listener, publish its bound port, make real requests and own its cleanup.
//! Hirsel runs in a child with explicit grants; both pipes need concurrent drains.
use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) fn replace_once(source: &str, anchor: &str, replacement: &str) -> String {
    assert_eq!(
        source.matches(anchor).count(),
        1,
        "hello-web source anchor must occur exactly once: {anchor:?}"
    );
    source.replacen(anchor, replacement, 1)
}

pub(super) fn grants(concurrent: bool) -> serde_json::Value {
    serde_json::json!({"io":{
        "stdout":"stdout.bytes@1",
        "tasks":"tasks.cooperative@1",
        "monotonic_clock":concurrent.then_some("clock.monotonic.ns@1"),
        "dirs":[],
        "net":{"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":"0","operation":"listen"}]}
    },"args":[]})
}

#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    end: Option<Result<(), String>>,
}

struct Pipe {
    captured: Arc<Mutex<Captured>>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}
impl Pipe {
    fn new(mut input: impl Read + AsRawFd + Send + 'static) -> io::Result<Self> {
        let fd = input.as_raw_fd();
        // One reader owns the fd. Nonblocking reads plus a short poll make joining
        // bounded even when a descendant has inherited the pipe's write end.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let captured = Arc::new(Mutex::new(Captured::default()));
        let output = Arc::clone(&captured);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let reader = thread::spawn(move || {
            let mut buffer = [0; 4096];
            let result = loop {
                match input.read(&mut buffer) {
                    Ok(0) => break Ok(()),
                    Ok(n) => output.lock().unwrap().bytes.extend_from_slice(&buffer[..n]),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        if stopping.load(Ordering::Relaxed) {
                            break Ok(());
                        }
                        let mut event = libc::pollfd {
                            fd,
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        if unsafe { libc::poll(&mut event, 1, 10) } < 0 {
                            let e = io::Error::last_os_error();
                            if e.kind() != io::ErrorKind::Interrupted {
                                break Err(e.to_string());
                            }
                        }
                    }
                    Err(e) => break Err(e.to_string()),
                }
            };
            output.lock().unwrap().end = Some(result);
        });
        Ok(Self {
            captured,
            stop,
            reader: Some(reader),
        })
    }
    fn join(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(reader) = self.reader.take() {
            if reader.join().is_err() {
                self.captured.lock().unwrap().end = Some(Err("pipe reader panicked".into()));
            }
        }
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        self.join();
    }
}

pub(super) struct Server {
    child: Child,
    stdout: Option<Pipe>,
    stderr: Option<Pipe>,
    cleaned: bool,
    outcome: String,
}
impl Server {
    pub(super) fn spawn(command: &mut Command) -> Result<Self, String> {
        let child = command.process_group(0).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|e| format!("server spawn failed: {e}; process=not spawned; readiness=[]; stderr=unavailable"))?;
        let mut server = Self {
            child,
            stdout: None,
            stderr: None,
            cleaned: false,
            outcome: String::new(),
        };
        server.stdout = Some(
            Pipe::new(server.child.stdout.take().unwrap())
                .map_err(|e| server.failure(&format!("stdout reader setup: {e}")))?,
        );
        server.stderr = Some(
            Pipe::new(server.child.stderr.take().unwrap())
                .map_err(|e| server.failure(&format!("stderr reader setup: {e}")))?,
        );
        Ok(server)
    }

    pub(super) fn ready(&mut self, timeout: Duration) -> Result<u16, String> {
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    return Err(self.failure(&format!("child exited before readiness: {status}")));
                }
                Err(e) => return Err(self.failure(&format!("child status error: {e}"))),
                Ok(None) => {}
            }
            let result = {
                let output = self.stdout.as_ref().unwrap().captured.lock().unwrap();
                if let Some(Err(e)) = &output.end {
                    Some(Err(format!("readiness read error: {e}")))
                } else if let Some(newline) = output.bytes.iter().position(|&b| b == b'\n') {
                    let line = &output.bytes[..newline];
                    Some(if line.is_empty() {
                        Err("empty readiness line".into())
                    } else {
                        std::str::from_utf8(line)
                            .ok()
                            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
                            .and_then(|s| s.parse::<u16>().ok())
                            .filter(|&p| p != 0)
                            .ok_or_else(|| "invalid readiness port (expected nonzero u16)".into())
                    })
                } else if output.end.is_some() {
                    Some(Err("EOF before readiness newline".into()))
                } else {
                    None
                }
            };
            if let Some(result) = result {
                return result.map_err(|reason| self.failure(&reason));
            }
            if started.elapsed() >= timeout {
                return Err(self.failure("server readiness timeout"));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait(&mut self, timeout: Duration) -> Result<ExitStatus, String> {
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Err(e) => return Err(format!("wait failed: {e}")),
                Ok(None) => {}
            }
            if started.elapsed() >= timeout {
                return Err("owned child exit timeout".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn cleanup(&mut self) {
        if self.cleaned {
            return;
        }
        // The child is the leader of the group created by spawn above. Kill
        // only this owned group, including inherited pipe writers, then reap.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        self.outcome = match self.wait(Duration::from_secs(10)) {
            Ok(status) => status.to_string(),
            Err(e) => e,
        };
        for pipe in [&mut self.stdout, &mut self.stderr].into_iter().flatten() {
            pipe.join();
        }
        self.cleaned = true;
    }

    fn failure(&mut self, reason: &str) -> String {
        let before = self.child.try_wait();
        self.cleanup();
        let stdout = self
            .stdout
            .as_ref()
            .map(|p| p.captured.lock().unwrap().bytes.clone());
        let stderr = self.stderr.as_ref().map(|p| {
            let output = p.captured.lock().unwrap();
            format!(
                "{:?}; reader={:?}",
                String::from_utf8_lossy(&output.bytes),
                output.end
            )
        });
        format!(
            "{reason}; process before cleanup={before:?}; process outcome={}; readiness bytes={stdout:?}; stderr={stderr:?}",
            self.outcome
        )
    }

    pub(super) fn finish(&mut self) -> Result<(), String> {
        let status = self
            .wait(Duration::from_secs(10))
            .map_err(|e| self.failure(&e))?;
        self.cleanup();
        if !status.success() {
            return Err(self.failure(&format!("server failed: {status}")));
        }
        let stderr = self.stderr.as_ref().unwrap().captured.lock().unwrap();
        let error = !stderr.bytes.is_empty() || !matches!(stderr.end, Some(Ok(())));
        drop(stderr);
        if error {
            return Err(self.failure("server stderr was not empty or failed to drain"));
        }
        Ok(())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[test]
fn replacement_requires_exactly_one_anchor() {
    for source in ["absent", "anchor anchor"] {
        assert!(std::panic::catch_unwind(|| replace_once(source, "anchor", "new")).is_err());
    }
    assert_eq!(replace_once("one anchor", "anchor", "new"), "one new");
}

fn shell(script: &str) -> Server {
    Server::spawn(Command::new("/bin/sh").args(["-c", script])).unwrap()
}

#[test]
fn early_exit_reports_stderr_and_outcome() {
    let mut server = shell("printf 'startup failed' >&2; exit 7");
    let error = server.ready(Duration::from_secs(10)).unwrap_err();
    assert!(
        error.contains("EOF") || error.contains("child exited"),
        "{error}"
    );
    assert!(error.contains("exit status: 7"), "{error}");
    assert!(error.contains("startup failed"), "{error}");
    assert!(error.contains("readiness bytes=Some([])"), "{error}");
    assert!(server.cleaned);
}

#[test]
fn invalid_readiness_preserves_bytes_and_cleans_up() {
    for (value, reason) in [
        ("", "empty readiness"),
        ("abc", "invalid readiness"),
        ("0", "invalid readiness"),
        ("65536", "invalid readiness"),
    ] {
        let mut server = shell(&format!(
            "printf 'diagnostic' >&2; printf '{value}\\n'; sleep 30 & wait"
        ));
        let error = server.ready(Duration::from_secs(10)).unwrap_err();
        assert!(error.contains(reason), "{error}");
        assert!(error.contains("diagnostic"), "{error}");
        assert!(error.contains("process outcome="), "{error}");
        assert!(server.cleaned);
        assert!(server.stdout.as_ref().unwrap().reader.is_none());
        assert!(server.stderr.as_ref().unwrap().reader.is_none());
    }
}

#[test]
fn timeout_preserves_partial_readiness_and_drains_large_stderr() {
    let mut server = shell("head -c 131072 /dev/zero >&2; printf partial; exec sleep 30");
    let error = server.ready(Duration::from_millis(250)).unwrap_err();
    assert!(error.contains("server readiness timeout"), "{error}");
    assert_eq!(
        server
            .stdout
            .as_ref()
            .unwrap()
            .captured
            .lock()
            .unwrap()
            .bytes,
        b"partial"
    );
    assert_eq!(
        server
            .stderr
            .as_ref()
            .unwrap()
            .captured
            .lock()
            .unwrap()
            .bytes
            .len(),
        131072
    );
    assert!(server.cleaned);
}

#[test]
fn readiness_uses_the_actual_nonzero_loopback_port() {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut server = shell(&format!("printf '{port}\\n'; exec sleep 30"));
    let ready = server.ready(Duration::from_secs(10)).unwrap();
    assert_eq!(ready, port);
    assert_ne!(ready, 0);
    let _client = std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, ready)).unwrap();
    let _accepted = listener.accept().unwrap();
    drop(server);
}

#[test]
fn spawn_failure_names_the_missing_process() {
    let error = Server::spawn(&mut Command::new("/hello-web-readiness-8/nonexistent"))
        .err()
        .unwrap();
    assert!(error.contains("spawn failed"), "{error}");
    assert!(error.contains("process=not spawned"), "{error}");
}

#[test]
fn readiness_read_error_is_distinct_from_timeout() {
    let mut server = shell("printf 'read failure diagnostic' >&2; exec sleep 30");
    let started = Instant::now();
    while server
        .stderr
        .as_ref()
        .unwrap()
        .captured
        .lock()
        .unwrap()
        .bytes
        .is_empty()
    {
        assert!(started.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(5));
    }
    server.stdout = Some(Pipe::new(std::fs::File::open("/").unwrap()).unwrap());
    let error = server.ready(Duration::from_secs(10)).unwrap_err();
    assert!(error.contains("readiness read error"), "{error}");
    assert!(error.contains("read failure diagnostic"), "{error}");
    assert!(!error.contains("readiness timeout"), "{error}");
}

#[test]
fn compile_failure_reports_diagnostic_and_process_outcome() {
    let path = super::file("pub fn main() -> Result[(), ()] { missing_readiness_8; Ok(()) }");
    let mut server = Server::spawn(
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&path),
    )
    .unwrap();
    let error = server.ready(Duration::from_secs(90)).unwrap_err();
    assert!(error.contains("missing_readiness_8"), "{error}");
    assert!(error.contains("process outcome=exit status:"), "{error}");
    assert!(error.contains("readiness bytes=Some([])"), "{error}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn denied_tasks_reports_the_original_startup_failure_on_all_engines() {
    let original = include_str!("../../../../examples/hello-web/main.hir");
    let path = super::std_file(original);
    let mut denied = grants(false);
    denied["io"]["tasks"] = serde_json::Value::Null;
    let record = super::run_record(&path, denied);
    for flags in [
        vec![],
        vec!["--native"],
        vec!["--native", "--backend", "cranelift"],
    ] {
        let mut command = super::host_run(&path, &record, false);
        command.args(&flags);
        let mut server = Server::spawn(&mut command).unwrap();
        let error = server.ready(Duration::from_secs(90)).unwrap_err();
        assert!(error.contains("Grant(NotGranted)"), "{flags:?}: {error}");
        assert!(
            error.contains("process outcome=exit status: 1"),
            "{flags:?}: {error}"
        );
        assert!(
            error.contains("readiness bytes=Some([])"),
            "{flags:?}: {error}"
        );
        assert!(!error.contains("invalid readiness"), "{flags:?}: {error}");
    }
    for p in [
        record,
        PathBuf::from(format!("{}.json", path.display())),
        path,
    ] {
        std::fs::remove_file(p).unwrap();
    }
}

#[test]
fn eof_with_partial_readiness_reports_the_bytes() {
    let mut server = shell("printf 'early diagnostic' >&2; printf 123; exec 1>&-; exec sleep 30");
    let error = server.ready(Duration::from_secs(10)).unwrap_err();
    assert!(error.contains("EOF before readiness newline"), "{error}");
    assert!(error.contains("Some([49, 50, 51])"), "{error}");
    assert!(error.contains("early diagnostic"), "{error}");
}

#[test]
fn drop_reaps_owned_group_and_joins_both_readers_during_unwind() {
    let mut server = shell("printf '12345\\n'; sleep 30 & wait");
    assert_eq!(server.ready(Duration::from_secs(10)).unwrap(), 12345);
    let pid = server.child.id();
    let stdout = Arc::clone(&server.stdout.as_ref().unwrap().captured);
    let stderr = Arc::clone(&server.stderr.as_ref().unwrap().captured);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owned = server;
            panic!("exercise owned-server cleanup");
        }))
        .is_err()
    );
    assert!(stdout.lock().unwrap().end.is_some());
    assert!(stderr.lock().unwrap().end.is_some());
    assert_eq!(
        unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}
