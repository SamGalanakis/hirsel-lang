use super::*;

fn package(source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let std = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    std::fs::write(dir.path().join("main.hir"), source).unwrap();
    std::fs::write(
        dir.path().join("hirsel.json"),
        serde_json::json!({
            "schema":1, "package":{"name":"exitcode","version":"1"},
            "entry":{"module":[],"name":"main"}, "sources":[{"path":"main.hir","module":[]}],
            "dependencies":[{"manifest":std,"package":{"name":"std","version":"1"},"expose":[]}]
        })
        .to_string(),
    )
    .unwrap();
    dir
}

fn exit_code(wrapped: bool, code: i32) {
    let ty = if wrapped {
        "Result[std@1/process.ExitCode, String]"
    } else {
        "std@1/process.ExitCode"
    };
    let value = format!("std@1/process.exit_code({code}u8)");
    let value = if wrapped {
        format!("Ok({value})")
    } else {
        value
    };
    let dir = package(&format!("pub fn main() -> {ty} {{ {value} }}"));
    verify(dir.path(), code, "");
}

macro_rules! code_cases {
    ($wrapped:literal) => {
        #[test]
        fn zero() {
            super::exit_code($wrapped, 0);
        }
        #[test]
        fn one() {
            super::exit_code($wrapped, 1);
        }
        #[test]
        fn two() {
            super::exit_code($wrapped, 2);
        }
        #[test]
        fn maximum() {
            super::exit_code($wrapped, 255);
        }
    };
}

mod bare {
    code_cases!(false);
}
mod wrapped {
    code_cases!(true);
}

#[test]
fn error_and_offline_replay_cross_engines() {
    let dir = package(
        r#"pub fn main() -> Result[std@1/process.ExitCode, String] { Error(unique "missing data") }"#,
    );
    verify(dir.path(), 1, "error: missing data\n");
}

fn verify(dir: &std::path::Path, code: i32, stderr: &str) {
    let manifest = dir.join("hirsel.json");
    let grants = dir.join("grants.json");
    std::fs::write(&grants, "{}").unwrap();
    let tape = dir.join("recording");
    // Interpreter record, then replay without a grants file, on every execution path.
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--record"])
        .arg(&tape)
        .arg("--grants")
        .arg(&grants)
        .arg(&manifest)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    assert_eq!(output.stderr, stderr.as_bytes());
    assert!(output.stdout.is_empty());
    std::fs::remove_file(grants).unwrap();
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(&manifest).arg("--replay").arg(&tape);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{backend:?}: {output:?}");
        assert_eq!(output.stderr, stderr.as_bytes(), "{backend:?}");
        assert!(output.stdout.is_empty());
        let mut report = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        report
            .arg("run")
            .arg(&manifest)
            .arg("--replay")
            .arg(&tape)
            .arg("--json");
        if let Some(backend) = backend {
            report.args(["--native", "--backend", backend]);
        }
        let output = report.output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["run"]["exit_code"], code);
        assert_eq!(output.stderr, stderr.as_bytes());
        if let Some(backend) = backend {
            // Standalone launch also returns the code, recording it on a fresh native run.
            let executable = build_executable(&manifest, &["--backend", backend]);
            let native_tape = dir.join(format!("{backend}.tape"));
            let grant = dir.join("native-grants.json");
            std::fs::write(&grant, "{}").unwrap();
            let output = Command::new(&executable)
                .arg("--hirsel-record")
                .arg(&native_tape)
                .arg("--hirsel-grants")
                .arg(&grant)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(code), "{backend}: {output:?}");
            assert_eq!(output.stderr, stderr.as_bytes());
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&manifest)
                .arg("--replay")
                .arg(&native_tape)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(code), "{backend}: {output:?}");
            assert_eq!(output.stderr, stderr.as_bytes());
        }
    }
}

#[test]
fn exit_code_16_bare_byte_is_refused_before_running() {
    let dir = package("pub fn main() -> u8 { 2u8 }");
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(dir.path().join("hirsel.json"));
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("ExitCode"),
            "{output:?}"
        );
    }
}

#[test]
fn exit_code_16_return_runs_cleanup_before_process_status() {
    let source = r#"pub resource type Trace { output: Stdout }
    drop(x) { let _ = out.print(x.output, "drop\n") }
    pub fn main(rt: Io) -> Result[std@1/process.ExitCode, GrantError] {
        let trace = Trace(output: io.stdout(rt)?)
        Ok(std@1/process.exit_code(2u8))
    }"#;
    let dir = package(source);
    let manifest = dir.path().join("hirsel.json");
    let grants = dir.path().join("grants.json");
    std::fs::write(&grants, r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#).unwrap();
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .arg(&manifest)
            .arg("--grants")
            .arg(&grants);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{backend:?}: {output:?}");
        assert_eq!(output.stdout, b"drop\n", "{backend:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}
