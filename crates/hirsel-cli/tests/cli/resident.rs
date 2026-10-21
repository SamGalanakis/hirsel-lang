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
            let run = |daemon: &str, results: &str| {
                let mut c = command(&directory);
                c.env("HIRSEL_DAEMON", daemon)
                    .env("HIRSEL_RESULTS", results)
                    .arg("check");
                if let Some(flag) = flag {
                    c.arg(flag);
                }
                c.arg("main.hir").output().unwrap()
            };
            let warm = run("auto", "dir@1");
            let mut served = String::new();
            log.read_line(&mut served).unwrap();
            assert!(
                served.contains("serve:") && served.contains("exited"),
                "{served}"
            );
            let disk = run("off", "dir@1");
            let cold = run("off", "none@1");
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
    }
    server.0.kill().unwrap();
    server.0.wait().unwrap();
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
