//! Explicit mapper context is stable in text and JSON, with UTF-8 byte spans.
use std::{fs, process::Command};

#[test]
fn error_mapper_diagnostics_18_text_json_and_named_helper() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mapper.hir");
    for (arm, mapper, declaration) in [
        ("InvalidJson()", "InvalidJson", ""),
        ("InvalidJson(u64)", "InvalidJson", ""),
        (
            "InvalidJson()",
            "convert",
            "fn convert() -> CommandError { InvalidJson() }",
        ),
        (
            "InvalidJson()",
            "convert",
            "fn convert(e: json.JsonError) -> u64 { 0 }",
        ),
        (
            "InvalidJson()",
            "convert",
            "fn convert(mut e: json.JsonError) -> CommandError { InvalidJson() }",
        ),
    ] {
        let source = format!(
            "// café 🐑\npub type CommandError = {arm}\n{declaration}\nfn validate(candidate: json.Value) -> Result[(), CommandError] uses {{}} {{\n let encoded = json.stringify(candidate) ? {mapper}\n let _ = encoded\n Ok(())\n}}\n"
        );
        fs::write(&path, &source).unwrap();
        let run = |json| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.env("HIRSEL_DAEMON", "off").arg("check");
            if json {
                command.arg("--json");
            }
            command.arg(&path).output().unwrap()
        };
        let output = run(true);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let diagnostics: Vec<serde_json::Value> =
            serde_json::Deserializer::from_slice(&output.stdout)
                .into_iter()
                .collect::<Result<_, _>>()
                .unwrap();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let d = &diagnostics[0];
        assert_eq!(d["rule"], "LANG-8", "{d}");
        let primary = format!("json.stringify(candidate) ? {mapper}");
        let start = source.find(&primary).unwrap();
        assert_eq!(d["primary"]["start"], start);
        assert_eq!(d["primary"]["end"], start + primary.len());
        let labels = d["secondary"].as_array().unwrap();
        assert_eq!(labels.len(), 3, "{d}");
        for (label, site) in labels.iter().zip([
            "json.stringify(candidate)",
            "Result[(), CommandError]",
            if declaration.is_empty() {
                "CommandError"
            } else {
                "convert"
            },
        ]) {
            let start = source.find(site).unwrap();
            assert_eq!(label["span"]["start"], start, "{label}");
            assert_eq!(label["span"]["end"], start + site.len());
        }
        assert_eq!(labels[0]["message"], "propagates `std@1/json/JsonError`");
        assert_eq!(
            labels[1]["message"],
            "enclosing error channel is `CommandError`"
        );
        assert!(
            d["notes"]
                .to_string()
                .contains("fn(std@1/json/JsonError) -> CommandError"),
            "{d}"
        );
        assert!(d["fix"].is_null());
        assert_eq!(output.stdout, run(true).stdout);
        let text = run(false);
        assert_eq!(text.status.code(), Some(1));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&text.stdout),
            String::from_utf8_lossy(&text.stderr)
        );
        for label in labels {
            assert!(text.contains(label["message"].as_str().unwrap()), "{text}");
        }
        for note in d["notes"].as_array().unwrap() {
            let note = note.as_str().unwrap();
            assert!(
                text.contains(note.strip_prefix("help: ").unwrap_or(note)),
                "{text}"
            );
        }
        if arm == "InvalidJson()" && declaration.is_empty() {
            let help = d["notes"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|n| n.as_str())
                .find(|n| n.starts_with("help: to intentionally discard"))
                .unwrap();
            let helper = help
                .split('`')
                .find(|n| n.starts_with("fn map_error("))
                .unwrap();
            let repaired = source.replace("? InvalidJson", "? map_error") + "\n" + helper + "\n";
            fs::write(&path, repaired).unwrap();
            let fixed = run(true);
            assert!(fixed.status.success(), "{fixed:?}");
            assert!(fixed.stdout.is_empty(), "{fixed:?}");
        }
    }
}
