//! Metadata preflight: no compiler, std image or native toolchain is needed.
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

fn check_manifest(path: &Path, seen: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let path = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !seen.insert(path.clone()) {
        return Ok(());
    }
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    for (field, key) in [("sources", "path"), ("dependencies", "manifest")] {
        let entries = manifest[field]
            .as_array()
            .ok_or_else(|| format!("{}: {field} must be an array", path.display()))?;
        for entry in entries {
            let declared = entry[key]
                .as_str()
                .ok_or_else(|| format!("{}: {field}.{key} must be a string", path.display()))?;
            let resolved = path.parent().unwrap().join(declared);
            if !resolved.is_file() {
                return Err(format!(
                    "{}: {field}.{key} {declared:?} is not a file ({})",
                    path.display(),
                    resolved.display()
                ));
            }
            if field == "dependencies" {
                check_manifest(&resolved, seen)?;
            }
        }
    }
    Ok(())
}

fn sidecars(dir: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            sidecars(&path, paths);
        } else if path.to_string_lossy().ends_with(".hir.json") {
            paths.push(path);
        }
    }
}

#[test]
fn checked_in_sidecars_parse_and_resolve_their_source_and_dependency_closure() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut paths = Vec::new();
    sidecars(&root.join("conformance"), &mut paths);
    paths.sort();
    assert!(
        paths
            .iter()
            .any(|p| { p.ends_with("SEM-3-markdown-commonmark-1-malformed-run.hir.json") })
    );
    let mut seen = BTreeSet::new();
    let errors: Vec<_> = paths
        .iter()
        .filter_map(|path| check_manifest(path, &mut seen).err())
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    println!("validated {} checked-in fixture sidecars", paths.len());
}

struct Control(PathBuf);
impl Control {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("fixture-manifest-18-{name}-{}", std::process::id()));
        fs::create_dir_all(dir.join("corpus/SEM")).unwrap();
        fs::create_dir_all(dir.join("stdlib/std@1")).unwrap();
        fs::write(dir.join("corpus/SEM/main.hir"), "").unwrap();
        fs::write(dir.join("stdlib/std@1/std.hir"), "").unwrap();
        fs::write(
            dir.join("stdlib/std@1/hirsel.json"),
            r#"{"sources":[{"path":"std.hir"}],"dependencies":[]}"#,
        )
        .unwrap();
        Self(dir)
    }

    fn check(&self, text: &str) -> Result<(), String> {
        let path = self.0.join("corpus/SEM/main.hir.json");
        fs::write(&path, text).unwrap();
        check_manifest(&path, &mut BTreeSet::new())
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

const VALID: &str = r#"{
  "sources": [{"path":"main.hir"}],
  "dependencies": [{"manifest":"../../stdlib/std@1/hirsel.json"}]
}"#;

#[test]
fn merge_markers_are_a_json_error_before_path_resolution() {
    let control = Control::new("merge");
    control.check(VALID).unwrap();
    // The inherited merge put a list_sort dependency and conflict markers
    // inside the std dependency, as well as an invalid five-level path.
    let corrupt = r#"{
  "sources": [{"path":"main.hir"}],
  "dependencies": [{"manifest":"../../../../../stdlib/std@1/hirsel.json","expose":[]
<<<<<<<< HEAD:crates/hirsel-driver/tests/fixtures/list_sort/SEM-5-list-sort-stable-6-run.hir.json
    }, {"manifest":"../../../../../packages/fixtures/list_sort_vectors@1/hirsel.json"}
========
>>>>>>>> incoming:conformance/SEM/SEM-3-markdown-commonmark-1-malformed-run.hir.json
  }]
}"#;
    let error = control.check(corrupt).unwrap_err();
    assert!(error.contains("main.hir.json"), "{error}");
    assert!(error.contains("expected `,` or `}` at line 4"), "{error}");
}

#[test]
fn valid_json_with_missing_source_or_wrong_dependency_path_fails() {
    let control = Control::new("paths");
    control.check(VALID).unwrap();
    for (text, expected) in [
        (
            VALID.replace("main.hir", "missing.hir"),
            "sources.path \"missing.hir\" is not a file",
        ),
        (
            VALID.replace("../../stdlib", "../../../../../stdlib"),
            "dependencies.manifest \"../../../../../stdlib/std@1/hirsel.json\" is not a file",
        ),
    ] {
        let error = control.check(&text).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    fs::remove_file(control.0.join("stdlib/std@1/std.hir")).unwrap();
    let error = control.check(VALID).unwrap_err();
    assert!(error.contains("stdlib/std@1/hirsel.json"), "{error}");
    assert!(
        error.contains("sources.path \"std.hir\" is not a file"),
        "{error}"
    );
}
