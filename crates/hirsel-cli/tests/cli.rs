use std::{
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn help_spellings_print_usage_to_stdout_and_exit_zero() {
    for command in ["--help", "help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg(command)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{command}");
        assert_eq!(
            output.stdout,
            b"usage: hirsel check [--json] <source|manifest.json> | run [--native | --simulation <json>] <source|manifest.json> | fmt [--check] <file> | export --tir <source|manifest.json> -o <file>\n",
            "{command}"
        );
        assert!(output.stderr.is_empty(), "{command}");
    }
}

fn file(text: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hirsel-cli-{}-{}.hir",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, text).unwrap();
    if text.contains("std@1/") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let sidecar = format!("{}.json", path.display());
        std::fs::write(sidecar, format!(r#"{{"schema":1,"package":{{"name":"standalone","version":"0"}},"entry":{{"module":[],"name":"main"}},"sources":[{{"path":"{}","module":[]}}],"dependencies":[{{"manifest":"{}","package":{{"name":"std","version":"1"}},"expose":[]}}]}}"#, path.display(), manifest.display())).unwrap();
    }
    path
}
#[test]
fn export_tir_writes_the_proof_seam_export_and_rejected_source_exits_one() {
    let path = file(
        "fn add(x: u64) -> u64 { x + 1u64 }\npub fn main() -> Result[(), ()] { let n = add(1u64); Ok(()) }\n",
    );
    let out = path.with_extension("tir");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["export", "--tir"])
        .arg(&path)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(
        text.starts_with("(Export\n  (Header \"hirsel.tir.export.v1\""),
        "{text}"
    );
    assert!(text.contains("\"SEM-1\" \"failure-edge\" 0"), "{text}");
    let bad = file("pub fn main() -> Result[(), ()] { let n: u64 = true; Ok(()) }\n");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["export", "--tir"])
        .arg(&bad)
        .arg("-o")
        .arg(bad.with_extension("tir"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(!bad.with_extension("tir").exists());
}
#[test]
fn binary_usage_and_io_errors_have_distinct_codes() {
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--bad", "file.hir"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
    let missing = std::env::temp_dir().join(format!("hirsel-missing-{}.hir", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("check")
        .arg(missing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}
#[test]
fn command_options_and_operand_counts_remain_specific_to_each_command() {
    for args in [
        vec![],
        vec!["--help", "extra"],
        vec!["check"],
        vec!["check", "--native", "file.hir"],
        vec!["check", "file.hir", "--json"],
        vec!["run", "--json", "file.hir"],
        vec!["run", "--simulation", "config.json"],
        vec!["run", "--native", "file.hir", "extra"],
        vec!["fmt", "--json", "file.hir"],
        vec!["fmt", "--check", "-file.hir"],
        vec!["export", "--tir", "file.hir"],
        vec!["export", "--tir", "file.hir", "--output", "out.tir"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .nth(1)
                .is_some_and(|line| line.starts_with("usage: hirsel ")),
            "{args:?}"
        );
    }
    // Standard lexopt spelling reaches IO rather than a usage error.
    for args in [
        vec!["check", "--", "/missing-hirsel-parser-input.hir"],
        vec![
            "export",
            "--tir",
            "/missing-hirsel-parser-input.hir",
            "-ounused.tir",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("usage:"));
    }
}

#[test]
fn real_pipeline_accepts_clean_source_and_checks_launch_shape() {
    let path = file("fn main() {}\n");
    for flags in [vec!["check"], vec!["check", "--json"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(flags)
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("entry must be pub fn"));
    std::fs::remove_file(path).unwrap();
    // A program without its entry checks, and running it names the entry.
    let path = file("fn probe() -> u64 {\n  1\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("declares no entry `pub fn main`"),
        "{stderr}"
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn lexical_rejection_uses_human_and_json_renderers() {
    let path = file("fn main() {}\n// \u{202e}\n");
    let human = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("check")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(1));
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(stderr.contains("error[SEM-3]"));
    assert!(stderr.contains(":2:4"));
    let json = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1));
    let diagnostic: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(diagnostic["schema_version"], 1);
    assert_eq!(diagnostic["rule"], "SEM-3");
    assert_eq!(diagnostic["primary"]["line"], 2);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn syntax_rejection_and_real_formatting_cover_exit_codes() {
    let invalid = file("fn main<T>() {}\n");
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--json"])
        .arg(&invalid)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let diagnostic: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(diagnostic["rule"], "LANG-1");
    assert_eq!(diagnostic["primary"]["line"], 1);
    let path = file("fn  main ( ) { }\n");
    let before = std::fs::read(&path).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("fmt")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}\n");
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(0));
    for path in [invalid, path] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn spec_opening_example_checks_formats_and_runs_with_simulated_directory() {
    let path = file(include_str!("fixtures/opening.hir"));
    let sidecar = path.with_extension("hir.json");
    let manifest = serde_json::json!({"schema":1,"package":{"name":"standalone","version":"0"},"entry":{"module":[],"name":"main"},"sources":[{"path":path.file_name().unwrap().to_str().unwrap(),"module":[]}],"dependencies":[{"manifest":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap(),"package":{"name":"std","version":"1"},"expose":[]}]});
    std::fs::write(&sidecar, manifest.to_string()).unwrap();
    let config =
        file(r#"{"args":["tree"],"files":{"tree/a.txt":"x\r\ny","tree/b.txt":"unchanged"}}"#);
    for flags in [vec!["check"], vec!["check", "--json"], vec!["fmt"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(flags)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&config)
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"1");
    for path in [path, config, sidecar] {
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn simulation_input_errors_are_tool_errors() {
    let source = file("fn main() {}\n");
    let config = file(r#"{"files":{"a":42}}"#);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&config)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected string values"));
    for path in [source, config] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn unavailable_execution_preserves_stdout_before_the_tool_error() {
    let source = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"before unavailable\\n\"); let s: Slots[u8] = slots.empty(); Ok(()) }",
    );
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"before unavailable\n");
    assert!(String::from_utf8_lossy(&result.stderr).contains("not implemented"));
    std::fs::remove_file(source).unwrap();
}

#[test]
fn violated_unchecked_contract_is_detected_with_prior_stdout() {
    let source = file(
        "pub fn main(rt: Io, args: Args, permit: Unsafe) -> Result[(), GrantError] {\n  let _ = out.print(io.stdout(rt)?, \"printed before the call\\n\")\n  let a: u8 = 200\n  let bad: u8 = int.unchecked_add(permit, a, 56)\n  Ok(())\n}\n",
    );
    let config = file(r#"{"grants":["Io","Args","Unsafe"]}"#);
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&config)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(result.stdout, b"printed before the call\n");
    let message = String::from_utf8(result.stderr).unwrap();
    assert!(
        message.contains("undefined behaviour detected: int.unchecked_add at line 4"),
        "{message}"
    );
    assert!(message.contains("sum must fit"));
    assert!(!message.contains("runtime failure"));
    assert!(!message.contains("invalid TIR"));
    for path in [source, config] {
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn invalid_utf8_is_a_located_language_diagnostic_for_every_entry_point() {
    let path = file("");
    let prefix = "// 日\nfn f() {}\n";
    let mut bytes = prefix.as_bytes().to_vec();
    bytes.extend_from_slice(&[0xff, b'x']);
    std::fs::write(&path, &bytes).unwrap();
    let json = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1));
    let diagnostic: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(diagnostic["rule"], "SEM-3");
    assert_eq!(
        diagnostic["primary"]["file"],
        path.to_string_lossy().as_ref()
    );
    assert_eq!(diagnostic["primary"]["start"], prefix.len());
    assert_eq!(diagnostic["primary"]["end"], prefix.len());
    assert_eq!(diagnostic["primary"]["line"], 3);
    assert_eq!(diagnostic["primary"]["column"], 1);
    for command in ["check", "fmt", "run"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg(command)
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let human = String::from_utf8(output.stderr).unwrap();
        assert!(human.contains("error[SEM-3]"), "{command}: {human}");
        assert!(human.contains(":3:1"), "{command}: {human}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::remove_file(path).unwrap();
}

/// `run --native` reports what `run` reports: the same first static
/// diagnostic, the same output and exit status, the same failure line. A
/// program native code cannot build yet is a tool outcome (exit 2) whose
/// source verdict is unchanged.
#[test]
fn native_run_matches_the_reference_run() {
    let run = |native: bool, path: &std::path::Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run");
        if native {
            command.arg("--native");
        }
        command.arg(path).output().unwrap()
    };
    let prints = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let _ = out.print(io.stdout(rt)?, \"hi\\n\")\n  Ok(())\n}\n",
    );
    let fails = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  let a: u8 = 255\n  let b = a + 1\n  Ok(())\n}\n",
    );
    let rejected = file("pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  missing\n}\n");
    let map = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  let mut m: Map[u64, u64] = map.empty()\n  map.insert(mut m, 4u64, 1u64)\n  Ok(())\n}\n",
    );
    let decimal = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let text = std@1/u64.to_string(4u64)\n  let _ = out.print(io.stdout(rt)?, if text == \"4\" { \"ok\\n\" } else { \"bad\\n\" })\n  Ok(())\n}\n",
    );
    let directory = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  let dir = io.dir(rt, path.of(\".\"))\n  Ok(())\n}\n",
    );
    for path in [&prints, &fails, &rejected, &map, &decimal, &directory] {
        let (reference, native) = (run(false, path), run(true, path));
        assert_eq!(native.status.code(), reference.status.code(), "{path:?}");
        assert_eq!(native.stdout, reference.stdout, "{path:?}");
        assert_eq!(
            String::from_utf8_lossy(&native.stderr),
            String::from_utf8_lossy(&reference.stderr),
            "{path:?}"
        );
    }
    assert_eq!(run(true, &prints).stdout, b"hi\n");
    assert_eq!(run(true, &fails).status.code(), Some(1));
    assert_eq!(run(true, &decimal).stdout, b"ok\n");
    // Acquiring Net is supported independently of M2 network operations.
    let network = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let net = io.net(rt)?\n  Ok(())\n}\n",
    );
    let reference = run(false, &network);
    let native = run(true, &network);
    assert_eq!(reference.status.code(), Some(0));
    assert_eq!(native.status.code(), Some(0));
    assert_eq!(native.stdout, reference.stdout);
    assert_eq!(native.stderr, reference.stderr);
}

#[test]
fn source_projection_controls_distinguish_granted_and_empty_io() {
    for op in ["stdout", "tasks", "net", "random"] {
        let source = file(&format!(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{ let output = io.{op}(rt)?; Ok(()) }}"
        ));
        for granted in [false, true] {
            let config = file(
                &serde_json::json!({
                    "grants": ["Io", "Args"],
                    "projections": if granted { vec![op] } else { vec![] },
                    "args": [],
                })
                .to_string(),
            );
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["run", "--simulation"])
                .arg(&config)
                .arg(&source)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if granted { 0 } else { 1 }),
                "{op}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
            std::fs::remove_file(config).unwrap();
        }
        std::fs::remove_file(source).unwrap();
    }
}
