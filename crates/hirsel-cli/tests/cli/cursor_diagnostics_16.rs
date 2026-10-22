//! Text and JSON must expose identical source-backed cursor explanations.
use std::{fs, process::Command};

fn checked(source: &str, labels: &[(&str, &str, &str)], rule: &str, primary: &str) {
    let root = tempfile::tempdir().unwrap();
    let helper = "// λ helper\npub fn owned(a: Bytes) -> Bytes in a uses {} { copy a }\n";
    fs::write(root.path().join("helper.hir"), helper).unwrap();
    fs::write(root.path().join("main.hir"), source).unwrap();
    let manifest = root.path().join("hirsel.json");
    fs::write(
        &manifest,
        serde_json::json!({
            "schema":1, "package":{"name":"cursor", "version":"0"},
            "sources":[{"path":"main.hir","module":[]},{"path":"helper.hir","module":["helper"]}],
            "dependencies":[]
        })
        .to_string(),
    )
    .unwrap();
    let run = |json| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        cmd.env("HIRSEL_DAEMON", "off").arg("check");
        if json {
            cmd.arg("--json");
        }
        let output = cmd.arg(&manifest).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        output
    };
    let output = run(true);
    let diagnostics: Vec<serde_json::Value> = serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let d = &diagnostics[0];
    assert_eq!(d["rule"], rule);
    let start = source.find(primary).unwrap();
    assert_eq!(d["primary"]["start"], start);
    assert_eq!(d["primary"]["end"], start + primary.len());
    assert_eq!(d["primary"]["line"], source[..start].lines().count());
    assert_eq!(
        d["primary"]["column"],
        source[..start].rsplit('\n').next().unwrap().chars().count() + 1
    );
    assert!(d["fix"].is_null());
    let text = run(false);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&text.stdout),
        String::from_utf8_lossy(&text.stderr)
    );
    assert!(text.contains(rule), "{text}");
    let secondary = d["secondary"].as_array().unwrap();
    assert_eq!(secondary.len(), labels.len(), "{d}");
    for (label, (needle, slice, fragment)) in secondary.iter().zip(labels) {
        let start = source.find(*needle).unwrap() + needle.find(*slice).unwrap();
        assert_eq!(label["span"]["start"], start, "{d}");
        assert_eq!(label["span"]["end"], start + slice.len(), "{d}");
        assert_eq!(
            label["span"]["line"],
            source[..start].lines().count(),
            "{d}"
        );
        assert_eq!(
            label["span"]["column"],
            source[..start].rsplit('\n').next().unwrap().chars().count() + 1,
            "{d}"
        );
        assert_eq!(label["span"]["file"], d["primary"]["file"], "{d}");
        assert!(
            label["span"]["file"]
                .as_str()
                .unwrap()
                .ends_with("/main.hir"),
            "{d}"
        );
        let message = label["message"].as_str().unwrap();
        assert!(message.contains(fragment), "{d}");
        assert!(text.contains(message), "{text}");
    }
    for note in d["notes"].as_array().unwrap() {
        let note = note.as_str().unwrap();
        assert!(
            text.contains(note.strip_prefix("help: ").unwrap_or(note)),
            "{text}"
        );
        assert!(
            !note.contains("copy ")
                && !note.contains("take ")
                && !note.contains("declare the result"),
            "{d}"
        );
    }
    // Repeated CLI invocations must retain the same ordering and locations.
    assert_eq!(output.stdout, run(true).stdout);
}

#[test]
fn cursor_diagnostics_16_multifile_utf8() {
    checked(
        "// é caller\nfn probe(a: Bytes) {\n  let mut cursor = a\n  cursor = helper.owned(a)\n}\n",
        &[
            ("let mut cursor", "cursor", "read-only borrow cursor"),
            ("helper.owned(a)", "helper.owned(a)", "owned value"),
            (
                "helper.owned(a)",
                "helper.owned",
                "result relation does not turn an owned return",
            ),
        ],
        "LANG-15",
        "cursor = helper.owned(a)",
    );
}

#[test]
fn cursor_diagnostics_16_computed_pair_and_refills() {
    checked(
        "// λ computed pair\ntype Value = Leaf | Array(List[Value])\nfn probe(a: Value, b: Value) {\n  let mut left: Value in {a, b} = a\n  match (left, b) { (Array(xs), Array(_)) => { left = xs[0u64] }, _ => () }\n}\n",
        &[
            ("let mut left", "left", "rooted in `a`, `b`"),
            ("Array(xs)", "xs", "view of `xs`"),
            ("(left, b)", "(left, b)", "distinct storage origin"),
        ],
        "MODE-1",
        "left = xs[0u64]",
    );
    for refill in ["copy a", "take owned"] {
        checked(
            &format!("// é\nfn f(a: Bytes, take owned: Bytes) {{ let mut c = a; c = {refill} }}"),
            &[
                ("let mut c", "c", "read-only borrow cursor"),
                (&format!("c = {refill}"), refill, "owned value"),
            ],
            "LANG-15",
            &format!("c = {refill}"),
        );
    }
}
