//! Public human/JSON guidance and exact host output stream checks.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
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
    let manifest = serde_json::json!({"schema":1,"package":{"name":"output_guidance","version":"0"},"sources":[{"path":"main.hir","module":[]}],"entry":{"module":[],"name":"main"},"dependencies":[{"manifest":std,"package":{"name":"std","version":"1"},"expose":[]}]});
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    path
}
fn human(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
#[test]
fn output_capability_guidance_24_human_json_argument_spans() {
    let scratch = tempfile::tempdir().unwrap();
    for (operation, expected, wrong) in [
        ("out.print", "Stdout", "Stderr"),
        ("err.print", "Stderr", "Stdout"),
    ] {
        for (argument, actual) in [("sink, ", Some(wrong)), ("1u64, ", Some("u64")), ("", None)] {
            let call = format!("{operation}({argument}\"x\")");
            let source = format!(
                "// λ🐑\nfn emit(sink:{wrong}){{let _={call}}}\npub fn main()->Result[(),()]{{Ok(())}}"
            );
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
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            let d = &diagnostics[0];
            assert_eq!(d["rule"], "CAP-1");
            assert_eq!(d["primary"]["start"], source.find(&call).unwrap());
            assert_eq!(
                d["primary"]["end"],
                source.find(&call).unwrap() + call.len()
            );
            let message = d["message"].as_str().unwrap();
            assert!(message.contains(expected) && message.contains("`s`"), "{d}");
            if let Some(actual) = actual {
                assert!(message.contains(actual), "{d}");
                let supplied = argument.trim_end_matches(", ");
                let start = source.find(&format!("{operation}(")).unwrap() + operation.len() + 1;
                assert!(
                    d["secondary"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|l| l["span"]["start"] == start
                            && l["span"]["end"] == start + supplied.len()
                            && l["message"].as_str().unwrap().contains(actual)
                            && l["message"].as_str().unwrap().contains(expected)),
                    "{d}"
                );
            } else {
                assert!(message.contains("omitted"));
            }
            assert!(d["fix"].is_null());
            let output = cli().arg("check").arg(manifest).output().unwrap();
            assert_eq!(output.status.code(), Some(1));
            let rendered = human(&output);
            assert!(rendered.contains(message), "{rendered}");
            for label in d["secondary"].as_array().unwrap() {
                assert!(
                    rendered.contains(label["message"].as_str().unwrap()),
                    "{rendered}"
                );
            }
            for note in d["notes"].as_array().unwrap() {
                assert!(
                    rendered.contains(note.as_str().unwrap().trim_start_matches("help: ")),
                    "{rendered}"
                );
            }
        }
    }
}

#[test]
fn output_capability_guidance_24_repairs_check_export_and_launch_exact_streams() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let source = r#"pub fn main(rt:Io)->Result[(),()] {
  let Ok(output)=io.stdout(rt) else {return Error(())}
  let Ok(errors)=io.stderr(rt) else {return Error(())}
  let Ok(())=out.print(output,"stdout λ\n") else {return Error(())}
  let Ok(())=err.print(errors,"stderr 🐑\n") else {return Error(())}
  Ok(())
}"#;
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
    let grants = root.join("grants.json");
    fs::write(
        &grants,
        r#"{"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","dirs":[]},"args":[]}"#,
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(root.join("capped"))
        .args(["--timeout", "60"])
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(manifest)
        .arg("--grants")
        .arg(grants)
        .envs([
            ("HIRSEL_DAEMON", "off"),
            ("HIRSEL_RESULTS", "none@1"),
            ("HIRSEL_JOBS", "1"),
            ("HIRSEL_MEMORY_LIMIT", "2G"),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, "stdout λ\n".as_bytes());
    let stderr = std::str::from_utf8(&output.stderr)
        .unwrap()
        .split_inclusive('\n')
        .filter(|l| !l.starts_with("capped: "))
        .collect::<String>();
    assert_eq!(stderr, "stderr 🐑\n");
}
