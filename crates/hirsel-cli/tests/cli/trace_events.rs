use super::*;

#[test]
fn trace_events_refuses_unqualified_execution_before_program_output_or_trace_creation() {
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
                r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]}}"#,
            ])
            .arg("--trace-events")
            .arg(path)
            .output()
            .unwrap();
        assert_ne!(output.status.code(), Some(0), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("canonical trace events require production-qualified Ordered execution"),
            "{output:?}"
        );
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read(&existing).unwrap(), b"sentinel");
        assert!(!missing.exists());
    }
    std::fs::remove_file(existing).unwrap();
    std::fs::remove_file(source).unwrap();
}
