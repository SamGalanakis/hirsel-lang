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
fn ownership_trace_49_run_refuses_launches_it_does_not_trace() {
    let path = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    for extra in [&["--simulation", "{}"][..], &["--native", "--cores", "1"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&path)
            .args(extra)
            .args(["--trace-ownership", "unused.json"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{extra:?}: {stderr}");
        assert!(stderr.contains("--trace-ownership"), "{extra:?}: {stderr}");
    }
    std::fs::remove_file(path).unwrap();
}

/// The reference interpreter, LLVM and Cranelift report the same logical
/// lifetimes in the same order; storage moves are engine-specific.
#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn ownership_trace_49_run_agrees_on_lifetimes_across_engines() {
    let path = file(
        "resource type Token { value: u64 }\ndrop(x) {}\npub fn main() -> Result[(), ()] {\n  let first = Token(value: 7u64)\n  let second = Token(value: 7u64)\n  let moved = take first\n  Ok(())\n}\n",
    );
    let dir = tempfile::tempdir().unwrap();
    let mut lifetimes = vec![];
    for engine in [
        &[][..],
        &["--native", "--backend", "llvm"],
        &["--native", "--backend", "cranelift"],
    ] {
        let sidecar = dir.path().join(format!("{}.json", lifetimes.len()));
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(engine)
            .arg(&path)
            .arg("--trace-ownership")
            .arg(&sidecar)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{engine:?}: {stderr}");
        assert!(
            stderr.contains("2 created, 2 dropped, 0 live"),
            "{engine:?}: {stderr}"
        );
        let trace: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
        let lifecycle: Vec<_> = trace["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "create" || event["kind"] == "drop")
            .map(|event| {
                (
                    event["kind"].clone(),
                    event["token"].clone(),
                    event.get("value_type").cloned(),
                )
            })
            .collect();
        assert_eq!(lifecycle.len(), 4, "{trace}");
        lifetimes.push(lifecycle);
    }
    assert_eq!(lifetimes[0], lifetimes[1]);
    assert_eq!(lifetimes[0], lifetimes[2]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn ownership_trace_49_test_traces_each_selected_test_with_its_ordinary_verdict() {
    let path = file(
        "resource type Token {}\ndrop(x) {}\n@test\nfn kept(t: Test) {\n  let first = Token()\n  let second = take first\n  test.check(t, true)\n}\n@test\nfn refused(t: Test) {\n  let first = Token()\n  test.check(t, false)\n}\n",
    );
    let dir = tempfile::tempdir().unwrap();
    let traces = dir.path().join("traces");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("test")
        .arg("--trace-ownership")
        .arg(&traces)
        .arg(&path)
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let kept = stdout
        .find("test standalone@0/kept ... ok")
        .unwrap_or_else(|| panic!("{stdout}"));
    let refused = stdout
        .find("test standalone@0/refused ... FAILED")
        .unwrap_or_else(|| panic!("{stdout}"));
    let summaries: Vec<_> = stdout
        .match_indices("1 created, 1 dropped, 0 live")
        .collect();
    assert_eq!(summaries.len(), 2, "{stdout}");
    assert!(
        kept < summaries[0].0 && summaries[0].0 < refused,
        "{stdout}"
    );
    assert!(stdout[kept..refused].contains("move root#0"), "{stdout}");
    let mut written: Vec<_> = std::fs::read_dir(&traces)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    written.sort();
    assert_eq!(written.len(), 2, "{written:?}");
    assert!(
        written.iter().all(|name| name.ends_with(".json")),
        "{written:?}"
    );
}
