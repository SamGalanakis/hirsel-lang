//! `hirsel run --trace-ownership`: readable resource lineage on stderr and the
//! complete machine-readable trace in a new JSON sidecar.
use super::*;

fn traced(source: &str) -> (std::process::Output, std::path::PathBuf, tempfile::TempDir) {
    let path = file(source);
    let dir = tempfile::tempdir().unwrap();
    let sidecar = dir.path().join("ownership.json");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .arg("--trace-ownership")
        .arg(&sidecar)
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    (output, sidecar, dir)
}

#[test]
fn ownership_trace_49_run_names_a_forgotten_resource_and_its_move_chain() {
    let (output, sidecar, _dir) = traced(
        "resource type Token {}\ndrop(x) {}\npub fn main() -> Result[(), ()] {\n  let first = Token()\n  let held = take first\n  forget(take held)\n  Ok(())\n}\n",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(output.stdout.is_empty());
    let create = stderr
        .lines()
        .position(|line| line.contains(":4:") && line.contains("create root#0"))
        .unwrap_or_else(|| panic!("{stderr}"));
    let moves: Vec<_> = stderr
        .lines()
        .filter(|line| line.contains("move root#0"))
        .collect();
    assert!(moves.len() >= 2, "{stderr}");
    let live = stderr
        .lines()
        .position(|line| line.starts_with("live root#0") && line.contains("Forgotten"))
        .unwrap_or_else(|| panic!("{stderr}"));
    assert!(create < live, "{stderr}");
    assert!(stderr.contains("1 created, 0 dropped, 1 live"), "{stderr}");
    let trace: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(trace["complete"], true, "{trace}");
    assert_eq!(
        trace["events"].as_array().unwrap().len(),
        stderr
            .lines()
            .take_while(|line| !line.starts_with("live "))
            .count()
    );
}

#[test]
fn ownership_trace_49_run_drops_every_lifetime_and_keeps_the_program_outcome() {
    let (output, sidecar, dir) = traced(
        "resource type Token {}\ndrop(x) {}\nfn identity(take token: Token) -> Token { take token }\npub fn main() -> Result[(), ()] {\n  let first = Token()\n  let second = identity(take first)\n  let third = Token()\n  Error(())\n}\n",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("2 created, 2 dropped, 0 live"), "{stderr}");
    assert!(stderr.contains("drop root#0"), "{stderr}");
    assert!(stderr.contains("drop root#1"), "{stderr}");
    assert!(sidecar.exists());
    // The sidecar is created new: an existing file is a tool failure after the
    // program's own outcome, never a silently replaced trace.
    let path = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args([std::ffi::OsStr::new("run"), path.as_os_str()])
        .arg("--trace-ownership")
        .arg(dir.path().join("ownership.json"))
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("hirsel: ownership trace:"), "{stderr}");
}

#[test]
fn ownership_trace_49_run_refuses_engines_it_does_not_trace() {
    let path = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    for extra in [["--native", "--native"], ["--simulation", "{}"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(if extra[0] == "--native" {
                &extra[..1]
            } else {
                &extra[..]
            })
            .arg(&path)
            .args(["--trace-ownership", "unused.json"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(output.status.code(), Some(0), "{stderr}");
        assert!(stderr.contains("--trace-ownership"), "{stderr}");
    }
    std::fs::remove_file(path).unwrap();
}
