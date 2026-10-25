//! Native CLI consumes the selected source diagnostic and original recipes.
use super::*;

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn ownership_native_cli_runs_selected_source_plans_on_both_backends() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("main.hir"), "resource type Token { value:u64 }\ndrop(x) {}\npub fn main()->Result[(),u64] { let first=Token(value:7u64)\nlet moved=take first\nOk(()) }\n").unwrap();
    let manifest = directory.path().join("hirsel.json");
    std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"ownership","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    let mut traces = Vec::new();
    for backend in ["llvm", "cranelift"] {
        let sidecar = directory.path().join(format!("{backend}.json"));
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", backend])
            .arg(&manifest)
            .arg("--trace-ownership")
            .arg(&sidecar)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{backend}: {stderr}");
        assert!(output.stdout.is_empty(), "{backend}: {output:?}");
        assert!(
            stderr.contains("1 created, 1 dropped, 0 live"),
            "{backend}: {stderr}"
        );
        assert!(stderr.contains("move root#0"), "{backend}: {stderr}");
        let trace: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
        assert_eq!(trace["complete"], true, "{trace}");
        assert!(
            trace["events"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["site"]["definition"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("ownership@1::"))),
            "{trace}"
        );
        traces.push(trace);
    }
    assert_eq!(traces[0], traces[1]);
}
