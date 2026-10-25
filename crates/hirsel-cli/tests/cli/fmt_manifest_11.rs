//! Input/expected-output checks adapted from gofmt_test.go:67–115 at
//! ebe2feefc400f813aefdc8a036e66d2619da6649; BSD-3-Clause (Go),
//! Copyright 2009 The Go Authors. See third-party/NOTICES.md.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const INPUT: &str = "fn  first ( ) { }\r\n";
const EXPECTED: &str = "fn first() {}\n";
const SECOND: &str = "fn second() {}\n";

struct Package(PathBuf);
impl Package {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hirsel-fmt-manifest-11-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let package = Self(root);
        package.write("first.hir", INPUT);
        package.write("second.hir", "fn  second ( ) { }\n");
        package.manifest(false);
        package
    }
    fn path(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
    fn write(&self, path: &str, text: impl AsRef<[u8]>) {
        fs::write(self.path(path), text).unwrap();
    }
    fn manifest(&self, entry: bool) {
        let mut manifest = serde_json::json!({
            "schema": 1,
            "package": {"name": "sample", "version": "1"},
            // Deliberately reverse lexical order and repeat a canonical path.
            "sources": [
                {"path": "second.hir", "module": ["model"]},
                {"path": "first.hir", "module": []},
                {"path": "./first.hir", "module": []}
            ],
            "dependencies": [{
                "manifest": "dep.json",
                "package": {"name": "dep", "version": "1"},
                "expose": []
            }]
        });
        if entry {
            // Formatting requires syntax, not an existing launch definition.
            manifest["entry"] = serde_json::json!({"module": [], "name": "main"});
        }
        self.write("hirsel.json", manifest.to_string());
        self.write("dep.hir", "fn  dependency ( ) { }\n");
        self.write("dep.json", r#"{"schema":1,"package":{"name":"dep","version":"1"},"sources":[{"path":"dep.hir","module":[]}],"dependencies":[]}"#);
    }
    fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        files.sort();
        files
    }
    fn run(&self, check: bool, input: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("fmt");
        if check {
            command.arg("--check");
        }
        // Use an absolute manifest from a different directory: source paths
        // must resolve relative to the manifest, not to the working directory.
        command.arg(self.path(input)).output().unwrap()
    }
}
impl Drop for Package {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn exit(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn set_old_mtime(path: &Path) -> SystemTime {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
    fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn fmt_manifest_11_two_sources_check_format_idempotence_and_direct_output() {
    let package = Package::new();
    package.manifest(true);
    package.write("unlisted.hir", "fn broken(\n");
    let prepared = hirsel_driver::prepare_format_path(&package.path("hirsel.json")).unwrap();
    assert_eq!(prepared.len(), 2);
    assert!(prepared[0].path.ends_with("second.hir"));
    assert!(prepared[1].path.ends_with("first.hir"));
    assert_eq!(prepared[1].original, INPUT);
    assert_eq!(prepared[1].formatted, EXPECTED);
    let before = package.snapshot();
    let output = package.run(true, "hirsel.json");
    exit(&output, 1);
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<_> = stderr.lines().collect();
    assert_eq!(lines.len(), 2, "canonical duplicates must be reported once");
    assert!(lines[0].ends_with("second.hir: formatting differs"));
    assert!(lines[1].ends_with("first.hir: formatting differs"));
    assert_eq!(package.snapshot(), before, "--check never writes");

    // Golden output and the existing single-source facade must agree exactly.
    assert_eq!(
        hirsel_driver::format_path(&package.path("first.hir")).unwrap(),
        EXPECTED
    );
    exit(&package.run(false, "hirsel.json"), 0);
    assert_eq!(
        fs::read(package.path("first.hir")).unwrap(),
        EXPECTED.as_bytes()
    );
    assert_eq!(
        fs::read(package.path("second.hir")).unwrap(),
        SECOND.as_bytes()
    );
    for (path, bytes) in &before {
        if !path.ends_with("first.hir") && !path.ends_with("second.hir") {
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
    }
    let first_time = set_old_mtime(&package.path("first.hir"));
    let second_time = set_old_mtime(&package.path("second.hir"));
    let formatted = package.snapshot();
    exit(&package.run(true, "hirsel.json"), 0);
    exit(&package.run(false, "hirsel.json"), 0);
    assert_eq!(package.snapshot(), formatted);
    assert_eq!(
        fs::metadata(package.path("first.hir"))
            .unwrap()
            .modified()
            .unwrap(),
        first_time
    );
    assert_eq!(
        fs::metadata(package.path("second.hir"))
            .unwrap()
            .modified()
            .unwrap(),
        second_time
    );
}

#[test]
fn fmt_manifest_11_library_needs_no_entry_or_dependency_loading() {
    let package = Package::new();
    // Even an unreadable/invalid dependency manifest and source do not take
    // part in formatting the library's direct sources.
    package.write("dep.json", "not JSON");
    package.write("dep.hir", "fn broken(\n");
    let dependency = fs::read(package.path("dep.hir")).unwrap();
    exit(&package.run(false, "hirsel.json"), 0);
    assert_eq!(
        fs::read_to_string(package.path("first.hir")).unwrap(),
        EXPECTED
    );
    assert_eq!(fs::read(package.path("dep.hir")).unwrap(), dependency);
    assert_eq!(
        fs::read_to_string(package.path("dep.json")).unwrap(),
        "not JSON"
    );
}

#[test]
fn fmt_manifest_11_preparation_errors_never_write_any_source() {
    for failure in ["syntax", "utf8", "missing", "directory"] {
        let package = Package::new();
        // second.hir is first in manifest order; the later first.hir fails
        // after the earlier output would already have been prepared.
        match failure {
            "syntax" => package.write("first.hir", "fn broken(\n"),
            "utf8" => package.write("first.hir", [0xff]),
            "missing" => fs::remove_file(package.path("first.hir")).unwrap(),
            "directory" => {
                fs::remove_file(package.path("first.hir")).unwrap();
                fs::create_dir(package.path("first.hir")).unwrap();
            }
            _ => unreachable!(),
        }
        let paths = ["second.hir", "hirsel.json", "dep.hir", "dep.json"];
        let before: Vec<_> = paths
            .iter()
            .map(|path| fs::read(package.path(path)).unwrap())
            .collect();
        for check in [false, true] {
            let output = package.run(check, "hirsel.json");
            assert!(!output.status.success(), "{failure}");
            assert!(!output.stderr.is_empty(), "{failure}");
            for (path, bytes) in paths.iter().zip(&before) {
                assert_eq!(
                    &fs::read(package.path(path)).unwrap(),
                    bytes,
                    "{failure}: {path}"
                );
            }
        }
    }
}

#[test]
fn fmt_manifest_11_invalid_manifests_do_not_write() {
    let package = Package::new();
    let valid: serde_json::Value =
        serde_json::from_slice(&fs::read(package.path("hirsel.json")).unwrap()).unwrap();
    let mut invalid = vec!["{".into(), r#"{"schema":1,"schema":1}"#.into()];
    for (field, value) in [
        ("schema", serde_json::json!(2)),
        ("sources", serde_json::json!([])),
        (
            "sources",
            serde_json::json!([{"path":"first.hir","module":["fn"]}]),
        ),
        (
            "dependencies",
            serde_json::json!([{"manifest":"dep.json","package":{"name":"dep","version":"1"},"expose":[{"head":"fn","module":[]}]}]),
        ),
        ("entry", serde_json::json!({"module":[],"name":"fn"})),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut manifest = valid.clone();
        manifest[field] = value;
        invalid.push(manifest.to_string());
    }
    for manifest in invalid {
        package.write("hirsel.json", manifest);
        let before = package.snapshot();
        for check in [false, true] {
            let output = package.run(check, "hirsel.json");
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("package manifest:"));
            assert_eq!(package.snapshot(), before);
        }
    }
}

#[test]
fn fmt_manifest_11_source_ignores_sidecar_and_skips_unchanged_file() {
    let package = Package::new();
    package.write("first.hir.json", "invalid sidecar");
    let before = package.snapshot();
    exit(&package.run(true, "first.hir"), 1);
    assert_eq!(package.snapshot(), before);
    exit(&package.run(false, "first.hir"), 0);
    assert_eq!(
        fs::read_to_string(package.path("first.hir")).unwrap(),
        EXPECTED
    );
    for (path, bytes) in before {
        if !path.ends_with("first.hir") {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
    let time = set_old_mtime(&package.path("first.hir"));
    exit(&package.run(true, "first.hir"), 0);
    exit(&package.run(false, "first.hir"), 0);
    assert_eq!(
        fs::metadata(package.path("first.hir"))
            .unwrap()
            .modified()
            .unwrap(),
        time
    );
}

#[cfg(unix)]
#[test]
fn fmt_manifest_11_symlink_and_absolute_source_aliases_deduplicate() {
    let package = Package::new();
    std::os::unix::fs::symlink("first.hir", package.path("alias.hir")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(package.path("hirsel.json")).unwrap()).unwrap();
    manifest["sources"] = serde_json::json!([
        {"path":"alias.hir","module":[]},
        {"path":package.path("first.hir"),"module":[]}
    ]);
    package.write("hirsel.json", manifest.to_string());
    let before = package.snapshot();
    let output = package.run(true, "hirsel.json");
    exit(&output, 1);
    assert_eq!(String::from_utf8(output.stderr).unwrap().lines().count(), 1);
    assert_eq!(package.snapshot(), before);
    exit(&package.run(false, "hirsel.json"), 0);
    assert!(
        fs::symlink_metadata(package.path("alias.hir"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(package.path("first.hir")).unwrap(),
        EXPECTED
    );
    exit(&package.run(true, "hirsel.json"), 0);
}
