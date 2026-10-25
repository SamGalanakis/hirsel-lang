//! Byte-for-byte cold/disk/resident diagnostics through the public commands.
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn command(directory: &Path) -> Command {
    let mut command = Command::new(std::path::absolute(env!("CARGO_BIN_EXE_hirsel")).unwrap());
    command
        .current_dir(directory)
        .env("HIRSEL_DAEMON", "auto")
        .env("HIRSEL_RESULTS", "dir@1")
        .env("XDG_RUNTIME_DIR", directory)
        .env("HIRSEL_RESULTS_DIR", directory.join("results"));
    command
}

#[test]
fn resident_stop_exits_only_the_selected_workspace_and_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let start = |name: &str| {
        let workspace = directory.path().join(name);
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        let mut server = Server(
            command(directory.path())
                .args(["serve", "--idle", "900", "--workspace"])
                .arg(&workspace)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut ready = String::new();
        BufReader::new(server.0.stderr.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert!(ready.contains("serving"), "{ready}");
        let socket = std::path::PathBuf::from(
            ready
                .split_once(" at ")
                .unwrap()
                .1
                .split(' ')
                .next()
                .unwrap(),
        );
        assert!(socket.exists());
        (server, workspace, socket)
    };
    // Share a runtime directory: only the workspace distinguishes these sockets.
    let (mut first, workspace, socket) = start("first");
    let (mut second, other_workspace, other_socket) = start("second");
    let stop = command(directory.path())
        .args(["serve", "--stop", "--workspace"])
        .arg(&workspace)
        .env("HIRSEL_RESULTS", "none@1") // shutdown needs no matching compile environment
        .output()
        .unwrap();
    assert!(stop.status.success(), "{stop:?}");
    assert!(!socket.exists(), "stop returned before socket removal");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = first.0.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "server did not exit");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let again = command(directory.path())
        .args(["serve", "--workspace"])
        .arg(&workspace)
        .arg("--stop")
        .output()
        .unwrap();
    assert!(again.status.success(), "{again:?}");
    let message = String::from_utf8(again.stderr).unwrap();
    assert_eq!(message.lines().count(), 1, "{message}");
    assert!(message.contains("no server running"), "{message}");
    assert!(second.0.try_wait().unwrap().is_none());
    assert!(std::os::unix::net::UnixStream::connect(&other_socket).is_ok());
    let stop = command(directory.path())
        .args(["serve", "--stop", "--workspace"])
        .arg(&other_workspace)
        .output()
        .unwrap();
    assert!(stop.status.success(), "{stop:?}");
    assert!(!other_socket.exists());
    assert!(second.0.wait().unwrap().success());

    // An unclean exit can leave a socket, but no running server is still a no-op.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    let stale = command(directory.path())
        .args(["serve", "--stop", "--workspace"])
        .arg(&workspace)
        .output()
        .unwrap();
    assert!(stale.status.success(), "{stale:?}");
    assert!(String::from_utf8_lossy(&stale.stderr).contains("no server running"));
}

#[test]
fn resident_edits_equal_cold_diagnostics_and_restart_loads_the_disk_store() {
    let directory = std::env::temp_dir().join(format!(
        "hirsel-resident-test-{}-{}",
        std::process::id(),
        super::NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.hir");
    std::fs::write(&file, "fn a() -> u8 { 1u8 }\nfn main() { let _ = a() }\n").unwrap();
    let mut child = command(&directory)
        .args(["serve", "--idle", "30"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut log = BufReader::new(child.stderr.take().unwrap());
    let mut ready = String::new();
    log.read_line(&mut ready).unwrap();
    assert!(ready.contains("serving"), "{ready}");
    let mut server = Server(child);
    let run = |flag: Option<&str>, daemon: &str, results: &str| {
        let mut c = command(&directory);
        c.env("HIRSEL_DAEMON", daemon)
            .env("HIRSEL_RESULTS", results)
            .arg("check");
        if let Some(flag) = flag {
            c.arg(flag);
        }
        c.arg("main.hir").output().unwrap()
    };
    // Keep the resident edit sequence contiguous: the independent fresh checks
    // can outlast the server's idle timeout on a shared worker. Retain every
    // result, then compare it with both fresh paths after stopping this server.
    let mut resident = vec![];
    for source in [
        "fn a() -> u8 { 1u8 }\nfn main() { let _ = a() }\n",
        "fn a() -> u8 { 2u8 }\nfn main() { let _ = a() }\n",
        "\n\nfn a() -> u32 { 2u32 }\nfn main() { let _ = a() }\n",
        "fn a() -> u8 { missing }\nfn main() { let _ = a() }\n",
        "fn main(",
        "fn main() {}\n",
    ] {
        std::fs::write(&file, source).unwrap();
        for flag in [None, Some("--json")] {
            let warm = run(flag, "auto", "dir@1");
            let mut served = String::new();
            log.read_line(&mut served).unwrap();
            assert!(
                served.contains("serve:") && served.contains("exited"),
                "{served}"
            );
            resident.push((source, flag, warm));
        }
    }
    server.0.kill().unwrap();
    server.0.wait().unwrap();
    for (source, flag, warm) in resident {
        std::fs::write(&file, source).unwrap();
        let disk = run(flag, "off", "dir@1");
        let cold = run(flag, "off", "none@1");
        assert_eq!(
            (&warm.status, &warm.stdout, &warm.stderr),
            (&cold.status, &cold.stdout, &cold.stderr),
            "resident: {source}"
        );
        assert_eq!(
            (&disk.status, &disk.stdout, &disk.stderr),
            (&cold.status, &cold.stdout, &cold.stderr),
            "disk: {source}"
        );
    }
    // A stale socket after an unclean shutdown is replaced on demand.
    let result = command(&directory)
        .env("HIRSEL_SERVE_IDLE", "1")
        .args(["check", "main.hir"])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    drop(server);
    std::fs::remove_dir_all(directory).unwrap();
}
