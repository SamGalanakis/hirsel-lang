use super::*;

#[test]
fn callee_metadata_is_loaded_before_runner_execution_on_both_native_backends() {
    let source = file(
        r#"fn half(n: u64) -> Result[u64, String] { Ok(n / 2u64) }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let runner = io.runner(rt, "worker")?
  match runner.compute(runner, half, 8u64) {
    Ok(n) => failure.check(n == 4u64, Arithmetic()),
    Error(_) => failure.check(false, Bounds()),
  }
  Ok(())
}
"#,
    );
    let grants = r#"{"io":{"tasks":"tasks.free@1","dirs":[],"runners":[{"name":"worker","provider":"local.threads@1","threads":2,"grants":null,"definitions":null}]}}"#;
    let gated_grants = grants.replace("\"tasks.free@1\"", "null");
    let grants_file = source.with_extension("grants.json");
    std::fs::write(&grants_file, grants).unwrap();
    for backend in ["llvm", "cranelift"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", backend])
            .arg(&source)
            .args(["--grants", grants])
            .output()
            .unwrap();
        assert!(output.status.success(), "{backend}: {output:?}");
        assert!(output.stderr.is_empty(), "{backend}: {output:?}");
        assert!(output.stdout.is_empty());
        let gated = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", backend])
            .arg(&source)
            .args(["--grants", &gated_grants])
            .output()
            .unwrap();
        assert!(!gated.status.success(), "{backend}: {gated:?}");
        assert!(
            String::from_utf8_lossy(&gated.stderr)
                .contains("runners run natively only under the harness launcher"),
            "{backend}: {gated:?}"
        );
        assert!(gated.stdout.is_empty());

        // A built executable owns the same metadata; authority is compiled,
        // independent of the source launcher's process environment.
        let executable = source.with_extension(format!("{backend}.exe"));
        let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", backend])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(built.status.success(), "{backend}: {built:?}");
        let output = Command::new(&executable)
            .arg("--hirsel-grants")
            .arg(&grants_file)
            .output()
            .unwrap();
        assert!(output.status.success(), "{backend}: {output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty(), "{backend}: {output:?}");
        std::fs::write(&grants_file, &gated_grants).unwrap();
        let output = Command::new(&executable)
            .arg("--hirsel-grants")
            .arg(&grants_file)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{backend}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("runners run natively only under the harness launcher"),
            "{backend}: {output:?}"
        );
        assert!(output.stdout.is_empty());
        std::fs::write(&grants_file, grants).unwrap();
        std::fs::remove_file(executable).unwrap();
    }
    std::fs::remove_file(grants_file).unwrap();
    std::fs::remove_file(source).unwrap();
}

#[test]
fn unused_callee_references_do_not_prevent_ordinary_native_execution() {
    let source = file(
        r#"fn half(n: u64) -> Result[u64, String] { Ok(n / 2u64) }
pub fn main() -> Result[(), ()] { let _ = half; Ok(()) }
"#,
    );
    for backend in ["llvm", "cranelift"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", backend])
            .arg(&source)
            .output()
            .unwrap();
        assert!(output.status.success(), "{backend}: {output:?}");
    }
    std::fs::remove_file(source).unwrap();
}
