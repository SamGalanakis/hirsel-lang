//! Independent snippet controls for the executable input-boundary recipes.
use std::{fs, path::PathBuf, process::Command};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

// Embed example bytes so remote execution has no undeclared example-file input.
fn write_boundary(dir: &std::path::Path) {
    fs::write(
        dir.join("input_boundaries.hir"),
        include_str!("../../../../examples/ownership-repairs/input_boundaries.hir"),
    )
    .unwrap();
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.env("HIRSEL_DAEMON", "off");
    command
}

fn success(output: &std::process::Output) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn authoring_input_recipes_13_package_engines() {
    let scratch = tempfile::tempdir().unwrap();
    fs::write(
        scratch.path().join("main.hir"),
        include_str!("../../../../examples/ownership-repairs/main.hir"),
    )
    .unwrap();
    let mut package: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../examples/ownership-repairs/hirsel.json"
    ))
    .unwrap();
    package["dependencies"][0]["manifest"] = root()
        .join("stdlib/std@1/hirsel.json")
        .to_str()
        .unwrap()
        .into();
    write_boundary(scratch.path());
    let manifest = scratch.path().join("hirsel.json");
    fs::write(&manifest, package.to_string()).unwrap();
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut command = command();
        command.args(["test", "--json"]);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.arg(&manifest).output().unwrap();
        success(&output);
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["summary"]["total"], 3, "{report}");
        assert_eq!(report["summary"]["passed"], 3, "{report}");
    }
}

#[test]
fn authoring_input_recipes_13_counted_bytes_reject_and_unique_repair() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("main.hir");
    let rejected =
        "fn make_bytes(take data: Bytes) -> Result[unique Bytes, ()] {\n  Ok(take data)\n}\n";
    fs::write(&path, rejected).unwrap();
    // Explicit std-free manifest keeps this ownership control independent of
    // all adapter definitions and std parser implementation details.
    fs::write(
        path.with_extension("hir.json"),
        serde_json::json!({
            "schema":1,"package":{"name":"snippet","version":"0"},
            "sources":[{"path":"main.hir","module":[]}],"dependencies":[]
        })
        .to_string(),
    )
    .unwrap();
    let output = command()
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let diagnostics: Vec<serde_json::Value> = output
        .stdout
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["rule"], "MODE-2", "{diagnostics:?}");
    let start = rejected.rfind("Ok(take data)").unwrap();
    assert_eq!(diagnostics[0]["primary"]["start"], start);
    assert_eq!(
        diagnostics[0]["primary"]["end"],
        start + "Ok(take data)".len()
    );
    fs::write(
        &path,
        r#"fn make_bytes(take data: Bytes) -> Result[unique Bytes, ()] {
  Ok(unique data)
}
pub fn main() -> Result[(), ()] {
  let mut data = bytes.from_list([0u8, 255u8, 13u8, 10u8])
  let owned = make_bytes(copy data)?
  data = b"changed"
  failure.check(owned == b"\0\xff\r\n" && data == b"changed", Arithmetic())
  Ok(())
}
"#,
    )
    .unwrap();
    let sidecar = path.with_extension("hir.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    manifest["entry"] = serde_json::json!({"module":[],"name":"main"});
    fs::write(sidecar, manifest.to_string()).unwrap();
    success(&command().arg("check").arg(&path).output().unwrap());
    success(&command().arg("run").arg(&path).output().unwrap());
}

#[test]
fn authoring_input_recipes_13_bytes_unicode_reject_and_utf8_repair() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("main.hir");
    let manifest = scratch.path().join("hirsel.json");
    fs::write(
        &manifest,
        serde_json::json!({
            "schema":1,"package":{"name":"byte_recipe","version":"0"},
            "sources":[{"path":"main.hir","module":[]}],"dependencies":[],
            "entry":{"module":[],"name":"main"}
        })
        .to_string(),
    )
    .unwrap();
    for (scalar, expected, adjacent) in [
        ("雪", &[0xe9u8, 0x9b, 0xaa][..], false),
        ("é", &[0xc3, 0xa9][..], true),
        ("雪", &[0xe9, 0x9b, 0xaa][..], true),
        ("🙂", &[0xf0, 0x9f, 0x99, 0x82][..], true),
    ] {
        // Include the exact configctl witness, then exercise a Unicode column
        // after another scalar and byte escapes immediately around the error.
        let source = if adjacent {
            format!("fn data()->Bytes {{ let _ = \"λ\"; b\"\\n{scalar}\\xff\"}}\n")
        } else {
            format!("fn data()->Bytes {{b\"{scalar}\"}}\n")
        };
        fs::write(&path, &source).unwrap();
        let output = command()
            .env("HIRSEL_JOBS", "1")
            .args(["check", "--json"])
            .arg(&manifest)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let diagnostics: Vec<serde_json::Value> =
            serde_json::Deserializer::from_slice(&output.stdout)
                .into_iter()
                .collect::<Result<_, _>>()
                .unwrap();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let d = &diagnostics[0];
        assert_eq!(d["rule"], "SEM-3");
        assert_eq!(d["message"], "non-ASCII character in bytes literal");
        let start = source.find(scalar).unwrap();
        let column = source[..start].chars().count() + 1;
        assert_eq!(d["primary"]["start"], start);
        assert_eq!(d["primary"]["end"], start + scalar.len());
        assert_eq!(d["primary"]["line"], 1);
        assert_eq!(d["primary"]["end_line"], 1);
        assert_eq!(d["primary"]["column"], column);
        assert_eq!(d["primary"]["end_column"], column + 1);
        let notes = d["notes"].as_array().unwrap();
        assert_eq!(notes.len(), 1);
        let help = notes[0].as_str().unwrap();
        assert!(help.contains("ASCII plus byte escapes"), "{help}");
        assert!(!help.contains("@format"), "{help}");
        let replacement = help.split('`').nth(1).unwrap();
        let expected_spelling = expected
            .iter()
            .map(|b| format!("\\x{b:02x}"))
            .collect::<String>();
        assert_eq!(replacement, expected_spelling);
        assert_eq!(scalar.as_bytes(), expected);
        let human = command()
            .env("HIRSEL_JOBS", "1")
            .arg("check")
            .arg(&manifest)
            .output()
            .unwrap();
        assert_eq!(human.status.code(), Some(1));
        let human = format!(
            "{}{}",
            String::from_utf8_lossy(&human.stdout),
            String::from_utf8_lossy(&human.stderr)
        );
        assert!(human.contains(&format!("main.hir:1:{column}")), "{human}");
        assert!(human.contains(help), "{human}");
        let expected = if adjacent {
            [vec![10], expected.to_vec(), vec![255]].concat()
        } else {
            expected.to_vec()
        };
        let numbers = expected
            .iter()
            .map(|b| format!("{b}u8"))
            .collect::<Vec<_>>()
            .join(",");
        let repaired = source.replace(scalar, replacement)
            + &format!(
                "pub fn main() -> Result[(), ()] {{ failure.check(data() == bytes.from_list([{numbers}]), Arithmetic()); Ok(()) }}\n"
            );
        fs::write(&path, repaired).unwrap();
        // Export checks source through V; run verifies the independent byte oracle.
        success(
            &command()
                .env("HIRSEL_JOBS", "1")
                .args(["export", "--tir"])
                .arg(&manifest)
                .arg("-o")
                .arg(scratch.path().join("repaired.tir"))
                .output()
                .unwrap(),
        );
        success(
            &command()
                .env("HIRSEL_JOBS", "1")
                .arg("run")
                .arg(&manifest)
                .output()
                .unwrap(),
        );
    }
}

#[test]
fn authoring_input_recipes_13_composed_input() {
    let scratch = tempfile::tempdir().unwrap();
    let dir = scratch.path();
    write_boundary(dir);
    let entry = dir.join("main.hir");
    fs::write(
        &entry,
        r#"pub fn main(rt: Io) -> Result[(), input_boundaries.InputError] {
  let _ = input_boundaries.grant_result(io.stdout(rt))?
  let data = input_boundaries.input(rt, path.of("input"))?
  failure.check(data == b"abc", Arithmetic())
  Ok(())
}
"#,
    )
    .unwrap();
    let manifest = dir.join("hirsel.json");
    fs::write(
        &manifest,
        serde_json::json!({
            "schema":1,"package":{"name":"boundary_io","version":"0"},
            "sources":[{"path":entry,"module":[]},
              {"path":dir.join("input_boundaries.hir"),
               "module":["input_boundaries"]}],
            "dependencies":[{"manifest":root().join("stdlib/std@1/hirsel.json"),
                "package":{"name":"std","version":"1"},"expose":[]}],
            "entry":{"module":[],"name":"main"}
        })
        .to_string(),
    )
    .unwrap();
    // Build once per backend; all input/grant observations reuse those binaries.
    let mut executables = Vec::new();
    for backend in ["llvm", "cranelift"] {
        let exe = dir.join(backend);
        success(
            &command()
                .args(["build", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&exe)
                .output()
                .unwrap(),
        );
        executables.push(exe);
    }
    for (case, contents, stdout, directory, expected) in [
        ("success", Some(b"abc".as_slice()), true, true, ""),
        ("missing", None, true, true, "error: File(NotFound)\n"),
        (
            "limit",
            Some([b'a'; 65].as_slice()),
            true,
            true,
            "error: Limit\n",
        ),
        (
            "directory",
            Some(b"abc".as_slice()),
            true,
            false,
            "error: Directory(NotGranted)\n",
        ),
        (
            "grant",
            Some(b"abc".as_slice()),
            false,
            true,
            "error: Grant(NotGranted)\n",
        ),
    ] {
        let input = dir.join("input");
        if let Some(contents) = contents {
            fs::write(&input, contents).unwrap();
        } else {
            fs::remove_file(&input).unwrap();
        }
        let grants = dir.join(format!("{case}.json"));
        let mut io = serde_json::json!({"dirs":[]});
        if stdout {
            io["stdout"] = "stdout.bytes@1".into();
        }
        if directory {
            io["dirs"] = serde_json::json!([{"path":"data","host":dir,"access":"read"}]);
        }
        fs::write(&grants, serde_json::json!({"io":io}).to_string()).unwrap();
        for exe in std::iter::once(None).chain(executables.iter().map(Some)) {
            let mut launch = if let Some(exe) = exe {
                let mut c = Command::new(exe);
                c.arg("--hirsel-grants");
                c
            } else {
                let mut c = command();
                c.arg("run").arg(&manifest).arg("--grants");
                c
            };
            let output = launch.arg(&grants).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(i32::from(!expected.is_empty())),
                "{case} {exe:?}: {output:?}"
            );
            assert!(output.stdout.is_empty(), "{case}: {output:?}");
            assert_eq!(output.stderr, expected.as_bytes(), "{case} {exe:?}");
        }
    }
}
