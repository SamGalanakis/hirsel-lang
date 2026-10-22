//! Public human/JSON help and a manually applied two-binding repair.
use std::{fs, process::Command};

#[test]
fn mutable_pattern_diagnostics_17_human_json_and_fmt_keep_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.hir");
    for body in [
        "let Some(mut n) = value else { return 0 }; n += 1; n",
        "match value { Some(mut n) => n, None() => 0 }",
        "if let Some(mut n) = value { n } else { 0 }",
        "let Some(Some(mut n)) = nested else { return 0 }; n",
    ] {
        let source = format!(
            "// λ\nfn example(value: Option[u64], nested: Option[Option[u64]], pattern_value: u64) -> u64 {{ {body} }}\npub fn main() -> Result[(), ()] {{ Ok(()) }}\n"
        );
        fs::write(&path, &source).unwrap();
        let run = |command: &str, json: bool| {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            cmd.env("HIRSEL_DAEMON", "off").arg(command);
            if json {
                cmd.arg("--json");
            }
            cmd.arg(&path).output().unwrap()
        };
        let output = run("check", true);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let diagnostics: Vec<serde_json::Value> =
            serde_json::Deserializer::from_slice(&output.stdout)
                .into_iter()
                .collect::<Result<_, _>>()
                .unwrap();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let d = &diagnostics[0];
        assert_eq!(d["rule"], "LANG-13");
        let start = source.find("mut n").unwrap();
        assert_eq!(d["primary"]["start"], start);
        assert_eq!(d["primary"]["end"], start + 3);
        assert!(d["fix"].is_null());
        let text = run("check", false);
        assert_eq!(text.status.code(), Some(1));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&text.stdout),
            String::from_utf8_lossy(&text.stderr)
        );
        assert!(text.contains("local binding"), "{text}");
        for note in d["notes"].as_array().unwrap() {
            let note = note.as_str().unwrap();
            assert!(
                text.contains(note.strip_prefix("help: ").unwrap_or(note)),
                "{text}"
            );
        }
        assert!(
            d["notes"]
                .to_string()
                .contains("let mut mutable_value = pattern_value_2"),
            "{d}"
        );
        let formatted = run("fmt", false);
        assert_eq!(formatted.status.code(), Some(1), "{formatted:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }
}

#[test]
fn mutable_pattern_diagnostics_17_two_binding_repair_checks_and_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repaired.hir");
    fs::write(
        &path,
        r#"
fn example(value: Option[u64], pattern_value: u64) -> u64 {
  let Some(pattern_value_2) = value else { return 0 }
  let mut mutable_value = pattern_value_2
  mutable_value += 1
  mutable_value + pattern_value
}
pub fn main() -> Result[(), ()] {
  failure.check(example(Some(2), 40) == 43, Arithmetic())
  failure.check(example(None(), 40) == 0, Arithmetic())
  Ok(())
}
"#,
    )
    .unwrap();
    for command in ["check", "run"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .env("HIRSEL_DAEMON", "off")
            .arg(command)
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
    }
}
