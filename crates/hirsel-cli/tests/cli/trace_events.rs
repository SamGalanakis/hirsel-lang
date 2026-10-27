use super::*;

#[test]
fn trace_events_refuses_non_ordered_execution_before_program_output_or_trace_creation() {
    let source = file(
        "pub fn main(rt: Io) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"entry ran\\n\"); Ok(()) }\n",
    );
    let existing = source.with_extension("existing.jsonl");
    let missing = source.with_extension("missing.jsonl");
    std::fs::write(&existing, b"sentinel").unwrap();
    for path in [&existing, &missing] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", "llvm"])
            .arg(&source)
            .args([
                "--grants",
                r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#,
            ])
            .arg("--trace-events")
            .arg(path)
            .output()
            .unwrap();
        assert_ne!(output.status.code(), Some(0), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(
                "canonical trace events require a qualified Free or FIFO cooperative task provider"
            ),
            "{output:?}"
        );
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read(&existing).unwrap(), b"sentinel");
        assert!(!missing.exists());
    }
    std::fs::remove_file(existing).unwrap();
    std::fs::remove_file(source).unwrap();
}

#[test]
fn trace_events_exports_default_production_ordered_execution() {
    let source = file(
        r#"pub fn main(rt: Io) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, "entry ran\n"); Ok(()) }"#,
    );
    let trace = source.with_extension("production.jsonl");
    for backend in ["llvm", "cranelift"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", backend])
            .arg(&source)
            .args(["--grants", r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"placement":{"cores":8,"commit":"dedicated","reorder":"auto"}}"#])
            .arg("--trace-events").arg(&trace).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{backend}: {output:?}");
        assert_eq!(output.stdout, b"entry ran\n");
        let text = std::fs::read_to_string(&trace).unwrap();
        assert!(text.contains("host.request"), "{text}");
        assert!(text.contains("task.end"), "{text}");
        for line in text.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
        std::fs::remove_file(&trace).unwrap();
    }
    std::fs::remove_file(source).unwrap();
}
