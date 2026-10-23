//! String addition remains rejected; the advice is executable language syntax.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn manifest(root: &Path, std: bool) -> std::path::PathBuf {
    let dependencies = if std {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        serde_json::json!([{"manifest":path,"package":{"name":"std","version":"1"},"expose":[]}])
    } else {
        serde_json::json!([])
    };
    let path = root.join("hirsel.json");
    fs::write(
        &path,
        serde_json::json!({"schema":1,"package":{"name":"concat","version":"0"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":dependencies,
        "entry":{"module":[],"name":"main"}})
        .to_string(),
    )
    .unwrap();
    path
}

fn cli(root: &Path, args: &[&str]) -> Output {
    let capped = root.join("capped.py");
    fs::write(&capped, include_str!("../../../../tools/capped")).unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_hirsel"))
        .canonicalize()
        .unwrap();
    Command::new("python3")
        .arg(capped)
        .args(["--timeout", "60"])
        .arg(binary)
        .args(args)
        .current_dir(root)
        .env("HIRSEL_DAEMON", "off")
        .env("HIRSEL_RESULTS", "none@1")
        .env("HIRSEL_JOBS", "1")
        .env("HIRSEL_MEMORY_LIMIT", "2G")
        .output()
        .unwrap()
}

#[test]
fn string_concat_guidance_24_human_json_and_selection() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    for (expression, types, std) in [
        (r#"text + "\n""#, "`String` and `String`", true),
        (r#"text + 1u64"#, "`String` and `u64`", true),
        (r#"1u64 + text"#, "`u64` and `String`", true),
        (r#"text + "\n""#, "`String` and `String`", false),
    ] {
        manifest(root, std);
        let source =
            format!("// café 🐑\npub fn main() {{ let text = \"x\"; let _ = {expression} }}\n");
        fs::write(root.join("main.hir"), &source).unwrap();
        let output = cli(root, &["check", "--json", "hirsel.json"]);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let ds = serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter::<serde_json::Value>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let [d] = &ds[..] else { panic!("{ds:?}") };
        assert_eq!(
            d["rule"],
            if types == "`u64` and `String`" {
                "SEM-1"
            } else {
                "LANG-7"
            }
        );
        assert!(d["message"].as_str().unwrap().contains(types), "{d}");
        let start = source.find(expression).unwrap();
        assert_eq!(d["primary"]["start"], start);
        assert_eq!(d["primary"]["end"], start + expression.len());
        assert!(d["fix"].is_null());
        let notes = d["notes"].to_string();
        assert!(!notes.contains("non-exhaustive"), "{d}");
        assert_eq!(notes.contains("string.concat"), std, "{d}");
        assert_eq!(notes.contains("select std@1"), !std, "{d}");
        let output = cli(root, &["check", "hirsel.json"]);
        assert_eq!(output.status.code(), Some(1));
        let human = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(human.contains(types), "{human}");
        for note in d["notes"].as_array().unwrap() {
            let note = note.as_str().unwrap();
            assert!(
                human.contains(note.strip_prefix("help: ").unwrap_or(note)),
                "{human}"
            );
        }
        // Compile and export the exact construction quoted by the help.
        if types == "`String` and `String`" {
            let help = d["notes"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|n| n.as_str())
                .find(|n| n.starts_with("help: "))
                .unwrap();
            let recipes: Vec<_> = help
                .split('`')
                .filter(|s| s.starts_with("f\"") || s.starts_with("std@1/string.concat("))
                .collect();
            assert_eq!(recipes.len(), if std { 3 } else { 2 }, "{help}");
            for recipe in recipes {
                manifest(root, true);
                let repaired = format!(
                    "pub fn main() {{ let left = \"x\"; let right = \"y\"; let text = \"x\"; let _ = {recipe} }}"
                );
                fs::write(root.join("main.hir"), repaired).unwrap();
                let check = cli(root, &["check", "--json", "hirsel.json"]);
                assert!(check.status.success(), "{recipe}: {check:?}");
                let export = cli(
                    root,
                    &["export", "--tir", "hirsel.json", "-o", "reachable.tir"],
                );
                assert!(export.status.success(), "{recipe}: {export:?}");
                assert!(fs::metadata(root.join("reachable.tir")).unwrap().len() > 0);
            }
        }
    }
}

#[test]
fn string_concat_guidance_24_interpreter_values_and_order() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    manifest(root, true);
    fs::write(
        root.join("main.hir"),
        include_str!("../../../hirsel-driver/tests/fixtures/string_concat_guidance_24.hir"),
    )
    .unwrap();
    fs::write(root.join("grants.json"), "{}").unwrap();
    for args in [
        vec!["check", "--json", "hirsel.json"],
        vec!["export", "--tir", "hirsel.json", "-o", "reachable.tir"],
        vec!["run", "hirsel.json", "--grants", "grants.json"],
    ] {
        let output = cli(root, &args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
    }
}
