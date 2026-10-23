//! Parser guidance, executable existing recipes, and affected application cells.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn cli() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    cmd.envs([
        ("HIRSEL_DAEMON", "off"),
        ("HIRSEL_RESULTS", "none@1"),
        ("HIRSEL_JOBS", "1"),
        ("HIRSEL_MEMORY_LIMIT", "2G"),
    ]);
    cmd
}

fn package(root: &Path, source: &str) -> PathBuf {
    fs::write(root.join("main.hir"), source).unwrap();
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let manifest = serde_json::json!({
        "schema": 1, "package": {"name": "contextual", "version": "0"},
        "sources": [{"path": "main.hir", "module": []}],
        "entry": {"module": [], "name": "main"},
        "dependencies": [{"manifest": std, "package": {"name": "std", "version": "1"}, "expose": []}]
    });
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    path
}

#[test]
fn contextual_error_guidance_24_human_json_and_rejection_controls() {
    let scratch = tempfile::tempdir().unwrap();
    for (body, guided) in [
        (
            "let n = u64.parse(\"bad\")? fn(take e: scalar.ParseError) -> () { () }",
            true,
        ),
        (
            "let n = u64.parse(\"bad\")? /* café */ fn(take e: scalar.ParseError) -> () { () }",
            true,
        ),
        (
            "let n = u64.parse(\"bad\") fn(take e: scalar.ParseError) -> () { () }",
            false,
        ),
    ] {
        let source = format!("// 🐑\npub fn main() -> Result[(), ()] uses {{}} {{ {body} }}\n");
        let manifest = package(scratch.path(), &source);
        let output = cli()
            .args(["check", "--json"])
            .arg(&manifest)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let diagnostics = serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter::<serde_json::Value>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let d = &diagnostics[0];
        assert_eq!(d["rule"], "LANG-1", "{d}");
        let start = source.find("fn(take").unwrap();
        assert_eq!(d["primary"]["start"], start);
        assert_eq!(d["primary"]["end"], start + 2);
        assert!(d["fix"].is_null());
        assert_eq!(d["notes"].to_string().contains("results.map_error"), guided);
        let human = cli().arg("check").arg(manifest).output().unwrap();
        assert_eq!(human.status.code(), Some(1));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&human.stdout),
            String::from_utf8_lossy(&human.stderr)
        );
        assert!(text.contains("expected newline or semicolon"));
        for note in d["notes"].as_array().unwrap() {
            assert!(
                text.contains(note.as_str().unwrap().trim_start_matches("help: ")),
                "{text}"
            );
        }
    }
    // Captures cannot mutate an outer counter or move a borrowed payload global.
    for source in [
        "pub fn main() -> Result[(), ()] uses {} { let mut counter = 0u64; let value: Result[u64, u64] = Error(1u64); let mapped = results.map_error(take value, fn(take e: u64) -> u64 { counter += 1; e }); Ok(()) }",
        "fn bad(xs: List[u64]) -> Result[u64, List[u64]] uses {} { let value: Result[u64, u64] = Error(1u64); results.map_error(take value, fn(take e: u64) -> List[u64] { xs }) }\npub fn main() -> Result[(), ()] uses {} { let _ = bad([1u64]); Ok(()) }",
    ] {
        let manifest = package(scratch.path(), source);
        let output = cli()
            .args(["check", "--json"])
            .arg(manifest)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("unary named path"));
        let d: serde_json::Value = serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter()
            .next()
            .unwrap()
            .unwrap();
        assert!(
            ["CLO-1", "CLO-3", "CLO-4", "LANG-5", "MODE-1", "OWN-1"]
                .contains(&d["rule"].as_str().unwrap()),
            "{d}"
        );
    }
}

const VECTORS: &str = r#"
fn convert(take error: u64) -> u64 uses {} { error + 1 }
fn propagated() -> Result[u64, u64] uses {} {
  let value: Result[u64, u64] = Error(1u64)
  Ok(value? convert)
}
pub fn main(rt: Io) -> Result[(), ()] {
  let Ok(output) = io.stdout(rt) else { return Error(()) }
  // Rust coretests Result vectors: Ok(1) stays Ok(1), Err(1) maps to Err(2).
  let success: Result[u64, u64] = Ok(1u64)
  let mapped = results.map_error(take success, fn(take e: u64) -> u64 uses {} {
    failure.check(false, Arithmetic())
    e + 1
  })
  failure.check(mapped == Ok(1u64), Arithmetic())
  let failed: Result[u64, u64] = Error(1u64)
  let token: unique List[u64] = [1u64]
  // Moving the owned capture makes this callable once. The Error result proves
  // it ran, and the helper's once contract prevents a second invocation.
  let mapped_error = results.map_error(take failed, fn(take e: u64) -> u64 uses {Stdout} {
    let owned = take token
    let Ok(()) = out.print(output, "mapped\n") else { failure.check(false, Arithmetic()); return 0u64 }
    e + owned[0]
  })
  failure.check(mapped_error == Error(2u64), Arithmetic())
  failure.check(propagated() == Error(2u64), Arithmetic())
  Ok(())
}
"#;

fn execute_recipe(root: &Path, source: &str, expected_output: &[u8]) {
    let manifest = package(root, source);
    let checked = cli()
        .args(["check", "--json"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(checked.status.success(), "{checked:?}");
    let exported = cli()
        .args(["export", "--tir"])
        .arg(&manifest)
        .arg("-o")
        .arg(root.join("reachable.tir"))
        .output()
        .unwrap();
    assert!(exported.status.success(), "{exported:?}");
    assert!(fs::metadata(root.join("reachable.tir")).unwrap().len() > 0);
    hirsel_engine::with_compiler_stack(|| {
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let plain = hirsel_engine::run_observed_snapshot(
            &mut loaded.engine,
            &config,
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        let checked = hirsel_engine::run_observed_checked(
            &mut loaded.engine,
            &config,
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        for execution in [&plain, &checked] {
            assert!(
                matches!(
                    execution.outcome,
                    Ok(hirsel_engine::RunOutcome::Returned(
                        hirsel_run_schema::Returned::Ok
                    ))
                ),
                "{:?}",
                execution.outcome
            );
        }
        assert_eq!(plain.observation, checked.observation);
        let observed =
            hirsel_driver::native::Observation::of_reference(checked.observation.as_ref().unwrap());
        assert_eq!(observed.output, expected_output);
    });
}

#[test]
fn contextual_error_guidance_24_documented_recipes_and_scalar_vectors() {
    let scratch = tempfile::tempdir().unwrap();
    let language = include_str!("../../../../docs/spec/language.md");
    let library = include_str!("../../../../docs/spec/std-library.md");
    for source in [
        language
            .split("```hirsel\n// contextual-error recipe\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap(),
        library
            .split("```hirsel\n// contextual-error recovery\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap(),
        VECTORS,
    ] {
        execute_recipe(
            scratch.path(),
            source,
            if source == VECTORS { b"mapped\n" } else { b"" },
        );
    }
}

#[test]
fn contextual_error_guidance_24_affected_app_cells_three_engines() {
    for (name, main, model, manifest, verifier, selected, count) in [
        (
            "invoicecheck",
            include_str!("../../../../examples/invoicecheck/main.hir"),
            include_str!("../../../../examples/invoicecheck/model.hir"),
            include_str!("../../../../examples/invoicecheck/hirsel.json"),
            include_str!("../support/invoicecheck_23/verify.py"),
            "success,invalid,width,malformed,width-row2,header",
            6,
        ),
        (
            "versionaudit",
            include_str!("../../../../examples/versionaudit/main.hir"),
            include_str!("../../../../examples/versionaudit/model.hir"),
            include_str!("../../../../examples/versionaudit/hirsel.json"),
            include_str!("../support/versionaudit_23/verify.py"),
            "success,bad-requirement,bad-version,bad-requirement-row2,bad-version-row2,order",
            6,
        ),
    ] {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        fs::write(root.join("main.hir"), main).unwrap();
        fs::write(root.join("model.hir"), model).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_str(manifest).unwrap();
        manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .into();
        fs::write(root.join("hirsel.json"), manifest.to_string()).unwrap();
        fs::create_dir(root.join("fixtures")).unwrap();
        // Explicit embedded inputs survive the remote test sandbox.
        let fixtures = if name == "invoicecheck" {
            [
                (
                    "success.csv",
                    include_str!("../../../../examples/invoicecheck/fixtures/success.csv"),
                ),
                (
                    "invalid.csv",
                    include_str!("../../../../examples/invoicecheck/fixtures/invalid.csv"),
                ),
            ]
        } else {
            [
                (
                    "success.json",
                    include_str!("../../../../examples/versionaudit/fixtures/success.json"),
                ),
                (
                    "invalid.json",
                    include_str!("../../../../examples/versionaudit/fixtures/invalid.json"),
                ),
            ]
        };
        for (file, source) in fixtures {
            fs::write(root.join("fixtures").join(file), source).unwrap();
        }
        fs::write(
            root.join("capped"),
            include_str!("../../../../tools/capped"),
        )
        .unwrap();
        fs::write(root.join("verify.py"), verifier).unwrap();
        let output = Command::new("python3")
            .arg(root.join("verify.py"))
            .args(["--hirsel", env!("CARGO_BIN_EXE_hirsel"), "--root"])
            .arg(root)
            .arg("--output")
            .arg(root.join("evidence"))
            .arg("--cases")
            .arg(selected)
            .output()
            .unwrap();
        if !output.status.success() {
            panic!(
                "{name} evidence={} stdout={} stderr={}",
                scratch.keep().display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(summary["cases"], count);
        assert_eq!(summary["observations"], count * 3);
        eprintln!("{name}: {summary}");
    }
}
