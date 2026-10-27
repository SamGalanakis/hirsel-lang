//! Text/JSON source ranges agree across module files and UTF-8 prefixes.
use std::{fs, process::Command};

fn package(source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let main = "// λ caller\npub fn main() -> Result[(), ()] { helper.probe(); Ok(()) }\n";
    fs::write(dir.path().join("main.hir"), main).unwrap();
    fs::write(dir.path().join("helper.hir"), source).unwrap();
    let manifest = dir.path().join("hirsel.json");
    fs::write(
        &manifest,
        serde_json::json!({
            "schema": 1, "package": {"name":"unique_loss", "version":"0"},
            "sources":[{"path":"main.hir","module":[]},{"path":"helper.hir","module":["helper"]}],
            "dependencies":[{"manifest":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap(),"package":{"name":"std","version":"1"},"expose":[]}]
        })
        .to_string(),
    )
    .unwrap();
    dir
}

fn accepts(source: &str) {
    let dir = package(source);
    for json in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.env("HIRSEL_DAEMON", "off").arg("check");
        if json {
            command.arg("--json");
        }
        let output = command
            .arg(dir.path().join("hirsel.json"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{output:?}"
        );
    }
}

fn check(source: &str, primary: &str, causes: &[&str], explanation: &str) {
    let dir = package(source);
    let manifest = dir.path().join("hirsel.json");
    let run = |json| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.env("HIRSEL_DAEMON", "off").arg("check");
        if json {
            command.arg("--json");
        }
        let output = command.arg(&manifest).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        output
    };
    let json = run(true);
    let diagnostics: Vec<serde_json::Value> = serde_json::Deserializer::from_slice(&json.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let d = &diagnostics[0];
    assert_eq!(d["rule"], "MODE-2", "{d}");
    assert!(d["message"].as_str().unwrap().contains(explanation), "{d}");
    assert!(!d["message"].as_str().unwrap().contains("map.empty()"));
    let assert_span = |span: &serde_json::Value, fragment: &str| {
        let start = span["start"].as_u64().unwrap() as usize;
        let end = span["end"].as_u64().unwrap() as usize;
        assert_eq!(&source[start..end], fragment);
        assert!(
            span["file"].as_str().unwrap().ends_with("/helper.hir"),
            "{d}"
        );
        assert_eq!(span["line"], source[..start].lines().count());
        assert_eq!(
            span["column"],
            source[..start].rsplit('\n').next().unwrap().chars().count() + 1
        );
    };
    assert_span(&d["primary"], primary);
    assert!(d["fix"].is_null());
    let human = run(false);
    let human = format!(
        "{}{}",
        String::from_utf8_lossy(&human.stdout),
        String::from_utf8_lossy(&human.stderr)
    );
    assert!(human.contains("error[MODE-2]"), "{human}");
    for &cause in causes {
        let label = d["secondary"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| {
                let span = &l["span"];
                let start = span["start"].as_u64().unwrap() as usize;
                let end = span["end"].as_u64().unwrap() as usize;
                &source[start..end] == cause
            })
            .unwrap_or_else(|| panic!("missing {cause}: {d}"));
        assert_span(&label["span"], cause);
        assert!(
            human.contains(label["message"].as_str().unwrap()),
            "{human}"
        );
    }
    assert!(!human.contains("`map.empty()` may share"), "{human}");
    assert_eq!(json.stdout, run(true).stdout);
}

#[test]
fn uniqueness_loss_diagnostics_17_multifile_utf8_copied_key_and_handback() {
    accepts(
        "// é helper\nfn edit(mut m: Map[u64,u64]) {}\npub fn probe() {\n let mut m: unique Map[u64,u64] = map.empty()\n edit(mut m)\n let _ = map.remove(mut m,0)\n}\n",
    );
    check(
        "// é helper\npub fn probe() {\n  let prefix = \"λ\"; let key = f\"word\"\n  let mut counts: unique Map[String,u32] = map.empty()\n  map.insert(mut counts, copy key, 1)\n  let _ = map.remove(mut counts,key)\n}\n",
        "map.remove(mut counts,key)",
        &["copy key"],
        "lost its deep uniqueness",
    );
    check(
        "// é helper\npub fn edit(mut m: Map[u64,u64]) {}\npub fn probe() {\n let mut m: unique Map[u64,u64] = map.empty()\n edit(mut m)\n let _ = map.remove(mut m,0)\n}\n",
        "map.remove(mut m,0)",
        &["edit(mut m)"],
        "lost its deep uniqueness",
    );
}

#[test]
fn uniqueness_loss_diagnostics_17_multifile_loop_merge() {
    check(
        "// λ helper\npub fn probe() {\n let mut m: unique Map[u64,List[u64]] = map.empty()\n let ops: List[u64] = [0u64,1u64]\n for tag in ops {\n  if tag == 0 { map.insert(mut m,0,[1]); let _ = map.get(m,0) }\n  else { let _ = map.remove(mut m,0) }\n  for (_,value) in m { let _ = unique value; break }\n }\n}\n",
        "map.remove(mut m,0)",
        &["map.get(m,0)"],
        "not proved deeply unique",
    );
}
