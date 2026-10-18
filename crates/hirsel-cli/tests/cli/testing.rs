use super::*;

fn suite(source: &str) -> std::path::PathBuf {
    let path = file(source);
    let manifest = path.with_extension("json");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "schema":1,"package":{"name":"suite","version":"1"},
            "sources":[{"path":path,"module":[]}],"dependencies":[]
        })
        .to_string(),
    )
    .unwrap();
    manifest
}

// Rust libtest filter_tests vectors, adapted to versioned Hirsel definition paths.
#[test]
fn rust_substring_and_multiple_filter_vectors() {
    let path = suite(
        "@test\nfn base() {}\n@test\nfn base_test() {}\n@test\nfn base_test1() {}\n@test\nfn base_test2() {}\n",
    );
    for (filters, count) in [
        (vec!["base"], 4),
        (vec!["bas"], 4),
        (vec!["_test"], 3),
        (vec!["base_test"], 3),
        (vec!["test1", "test2"], 2),
        (vec!["absent"], 0),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["test", "--json"])
            .arg(&path)
            .args(filters)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["summary"]["total"], count);
    }
}

#[test]
fn selection_is_explicit_filtered_and_failure_isolated() {
    let path = suite(
        "@test\nfn a_check(t: Test) { test.check(t, false) }\n@test\nfn b_error() -> Result[(), String] { Error(\"failed\") }\n@test\nfn c_pass(t: Test) { test.check(t, true) }\nfn not_a_test(t: Test) { test.check(t, false) }\n@test\nfn d_overflow() { let n: u8 = 255; let _ = n + 1 }\n@test\nfn e_unit() {}\n",
    );
    let all = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("test")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(all.status.code(), Some(1), "{all:?}");
    let text = String::from_utf8(all.stdout).unwrap();
    assert!(
        text.contains("3 failed; 5 total"),
        "{text}; stderr: {}",
        String::from_utf8_lossy(&all.stderr)
    );
    assert!(text.contains("suite@1/c_pass ... ok"), "{text}");
    assert!(!text.contains("not_a_test"), "{text}");
    let selected = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("test")
        .arg(&path)
        .args(["c_pass", "e_unit"])
        .output()
        .unwrap();
    assert_eq!(selected.status.code(), Some(0), "{selected:?}");
    assert!(
        String::from_utf8(selected.stdout)
            .unwrap()
            .contains("2 passed; 0 failed; 2 total")
    );
    let empty = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["test", "--json"])
        .arg(&path)
        .arg("absent")
        .output()
        .unwrap();
    assert_eq!(empty.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(json["summary"]["total"], 0);
}

#[test]
fn json_seeds_are_stable_filter_independent_and_overridable() {
    let path = suite(
        "@test\nfn a(t: Test) { test.check(t, true) }\n@test\nfn b() -> Result[(), ()] { Ok(()) }\n",
    );
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["test", "--json"])
            .arg(&path)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let all = run(&[]);
    assert_eq!(all["schema"], 1);
    assert_eq!(all["summary"]["passed"], 2);
    assert_eq!(all["tests"][0]["seed"], run(&["/a"])["tests"][0]["seed"]);
    assert_ne!(all["tests"][0]["seed"], all["tests"][1]["seed"]);
    for test in run(&["--seed", "42"])["tests"].as_array().unwrap() {
        assert_eq!(test["seed"], 42);
        assert_eq!(test["status"], "passed");
        assert!(test["failure"].is_null());
    }
}

#[test]
fn native_test_engines_match_interpreted_selection_failures_and_seeds() {
    let path = suite(
        "resource type Watch { n: u8 }\n@noalloc drop(w) { failure.check(w.n == 7, Arithmetic()) }\n@test\nfn a(t: Test) { test.check(t, false) }\n@test\nfn b() -> Result[(), String] { Error(\"bad\") }\n@test\nfn c() {}\n@test\nfn d_cleanup() { let _ = Watch(n: 0) }\n@test\nfn e_after_cleanup(t: Test) { test.check(t, true) }\n@test\nfn f_property(t: Test) { test.check_case(t, false, 7) }\n",
    );
    let run = |backend: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.args(["test", "--json", "--seed", "731"]).arg(&path);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let expected = run(None);
    assert_eq!(expected["summary"]["total"], 6);
    assert_eq!(
        expected["tests"][3]["status"], "failed",
        "finalizer must run: {expected}"
    );
    assert_eq!(
        expected["tests"][4]["status"], "passed",
        "later test must run: {expected}"
    );
    assert_eq!(run(Some("llvm")), expected);
    assert_eq!(run(Some("cranelift")), expected);
}

#[test]
fn test_selection_excludes_dependencies_and_seeds_survive_file_moves() {
    let dependency = suite("@test\nfn dependency_failure(t: Test) { test.check(t, false) }\n");
    let mut dep: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&dependency).unwrap()).unwrap();
    dep["package"]["name"] = "dependency".into();
    std::fs::write(&dependency, dep.to_string()).unwrap();
    let path = suite(
        "@test\nfn t(context: Test) { test.check(context, true) }\nfn hirsel_test_entry_0() {}\n",
    );
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["dependencies"] = serde_json::json!([{"manifest":dependency,"package":{"name":"dependency","version":"1"},"expose":[]}]);
    std::fs::write(&path, manifest.to_string()).unwrap();
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["test", "--json"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let original = run();
    assert_eq!(original["summary"]["total"], 1);
    let old = std::path::PathBuf::from(manifest["sources"][0]["path"].as_str().unwrap());
    let moved = old.with_extension("moved.hir");
    std::fs::rename(&old, &moved).unwrap();
    manifest["sources"][0]["path"] = serde_json::json!(moved);
    std::fs::write(&path, manifest.to_string()).unwrap();
    assert_eq!(run(), original);
}

#[test]
fn tracked_test_entries_have_no_synthetic_caller() {
    let path =
        suite("@test\n@track_caller\nfn broken() {\n  let n: u8 = 255\n  let _ = n + 1\n}\n");
    let run = |backend: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.args(["test", "--json"]).arg(&path);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let expected = run(None);
    let failure = expected["tests"][0]["failure"].as_str().unwrap();
    assert!(failure.contains("suite@1/broken"), "{failure}");
    assert!(failure.contains(":5:11)"), "{failure}");
    assert_eq!(run(Some("llvm")), expected);
    assert_eq!(run(Some("cranelift")), expected);
}
