//! Catalogue-only CLI contract; no package, compiler session or writable cache.
hirsel_guard::install!();

use std::{
    ffi::OsStr,
    path::PathBuf,
    process::{Command, Output},
};

struct EmptyCwd(PathBuf);
impl EmptyCwd {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rule-explain-5-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        Self(path)
    }
    fn run(&self, args: &[impl AsRef<OsStr>]) -> Output {
        let missing = self.0.join("nonexistent");
        Command::new(std::fs::canonicalize(env!("CARGO_BIN_EXE_hirsel")).unwrap())
            .current_dir(&self.0)
            .env_clear()
            .env("HOME", &missing)
            .env("XDG_CACHE_HOME", &missing)
            .env("HIRSEL_CACHE", &missing)
            .env("HIRSEL_NATIVE_CACHE", &missing)
            .env("HIRSEL_PRODUCTS", &missing)
            .env("HIRSEL_STD_OBJECTS", &missing)
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for EmptyCwd {
    fn drop(&mut self) {
        assert_eq!(std::fs::read_dir(&self.0).unwrap().count(), 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::remove_dir(&self.0).unwrap();
    }
}

fn successful(output: Output) -> Vec<u8> {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    output.stdout
}

#[test]
fn rule_explain_5_every_entry_matches_guidance_and_reference_without_a_package() {
    let cwd = EmptyCwd::new();
    let reference = include_str!("../../../docs/diagnostics.md");
    let mut expected = std::collections::BTreeMap::new();
    for line in include_str!("../../hirsel-diag/src/guidance.tsv").lines() {
        let fields: Vec<_> = line.split('\t').collect();
        expected.insert(
            fields[0],
            (
                fields[1],
                fields[2],
                fields[3].strip_prefix("help: ").unwrap(),
            ),
        );
    }
    // Independently read the related sections from the existing reference,
    // rather than deriving the expected list from the implementation accessor.
    for section in reference.split("\n### ").skip(1) {
        let mut parts = section.split('\n');
        let (rule, title) = parts.next().unwrap().split_once(": ").unwrap();
        let meaning = parts.find(|line| !line.is_empty()).unwrap();
        let repair = parts
            .find_map(|line| line.strip_prefix("Typical fix: "))
            .unwrap();
        expected.entry(rule).or_insert((title, meaning, repair));
    }
    for (rule, (title, meaning, repair)) in &expected {
        let text = String::from_utf8(successful(cwd.run(&["explain", rule]))).unwrap();
        assert!(
            text.starts_with(&format!("{rule}: {title}\n\n{meaning}\n\n")),
            "{text}"
        );
        assert!(text.contains(*repair), "{text}");
        let json: serde_json::Value =
            serde_json::from_slice(&successful(cwd.run(&["explain", rule, "--json"]))).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["rule"], *rule);
        assert_eq!(json["title"], *title);
        assert_eq!(json["meaning"], *meaning);
        assert_eq!(json["repair"], *repair);
        assert!(json["related"].is_array());
        if *rule == "LANG-8" {
            assert_eq!(json["related"].as_array().unwrap().len(), 4);
        }
        if *rule == "MODE-4" {
            assert_eq!(json["related"][0]["rule"], "CONC-5");
        }
    }
    let rules: Vec<_> = expected.keys().copied().collect();
    assert_eq!(rules.len(), 28);
    assert_eq!(
        String::from_utf8(successful(cwd.run(&["explain", "--list"]))).unwrap(),
        format!("{}\n", rules.join("\n"))
    );
    let list: serde_json::Value =
        serde_json::from_slice(&successful(cwd.run(&["explain", "--list", "--json"]))).unwrap();
    assert_eq!(
        list,
        serde_json::json!({"schema_version": 1, "rules": rules})
    );
    assert_eq!(
        successful(cwd.run(&["explain", "--json", "OWN-2"])),
        successful(cwd.run(&["explain", "OWN-2", "--json"]))
    );
}

#[test]
fn rule_explain_5_errors_are_exit_two_with_no_fabricated_explanation() {
    let cwd = EmptyCwd::new();
    // Build the deliberately unknown ID so it is not a source rule citation.
    let oversized = format!("OWN-{}", "9".repeat(48));
    for args in [
        vec![],
        vec!["own-2"],
        vec!["OWN-0"],
        vec!["OWN-02"],
        vec!["OWN--2"],
        vec!["OWN-2x"],
        vec!["OWN-２"],
        vec![" OWN-2"],
        vec!["OWN-2\n"],
        vec!["UNKNOWN-999"],
        vec![oversized.as_str()],
        vec!["error_code"],
        vec!["2"],
        vec!["FAIL-1"],
        vec!["--json"],
        vec!["OWN-2", "CAP-2"],
        vec!["--list", "OWN-2"],
        vec!["OWN-2", "--list"],
        vec!["--list", "--list"],
        vec!["OWN-2", "--json", "--json"],
        vec!["--list", "--json=true"],
        vec!["--unknown"],
        vec!["OWN-2", "--backend", "llvm"],
        vec!["UNKNOWN-999", "--json"],
        vec!["own-2", "--json"],
        vec!["OWN-2", "-o", "out"],
    ] {
        let mut full = vec!["explain"];
        full.extend(args);
        let output = cwd.run(&full);
        assert_eq!(output.status.code(), Some(2), "{full:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{full:?}: {output:?}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.starts_with("hirsel: explain: "), "{error}");
        assert!(error.contains("usage: hirsel explain"), "{error}");
        if full.contains(&"UNKNOWN-999") || full.contains(&"FAIL-1") {
            assert!(error.contains("no catalogued guidance"), "{error}");
        }
    }
}

#[cfg(unix)]
#[test]
fn rule_explain_5_non_utf8_rule_is_a_tool_error() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let cwd = EmptyCwd::new();
    let output = cwd.run(&[OsString::from("explain"), OsString::from_vec(vec![255])]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("rule ID must be UTF-8")
    );
}

#[test]
fn rule_explain_5_help_links_the_dedicated_documentation() {
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8(successful(output)).unwrap();
    assert!(help.contains("explain RULE [--json]"));
    assert!(help.contains("docs/rule-explain.md"));
}
