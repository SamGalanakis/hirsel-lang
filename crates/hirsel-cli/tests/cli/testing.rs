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

/// A suite with std in its own directory, so its `snapshots/` directory is private.
fn std_suite(name: &str, source: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "hirsel-cli-{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("suite.hir");
    std::fs::write(&path, source).unwrap();
    let manifest = directory.join("suite.json");
    let std = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    std::fs::write(
        &manifest,
        serde_json::json!({
            "schema":1,"package":{"name":"suite","version":"1"},
            "sources":[{"path":path,"module":[]}],
            "dependencies":[{"manifest":std,"package":{"name":"std","version":"1"},"expose":[]}]
        })
        .to_string(),
    )
    .unwrap();
    manifest
}

fn hirsel_test(manifest: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("test")
        .arg(manifest)
        .args(args)
        .output()
        .unwrap()
}

// As with golden files and an -update flag: a missing or different snapshot fails its
// test after it finishes, with a deterministic unified diff; --bless writes the files instead.
#[test]
fn snapshots_compare_with_committed_files_and_bless_rewrites_them() {
    let manifest = std_suite(
        "snapshots",
        "@test\nfn render(t: Test) {\n  test.expect_snapshot(t, \"greeting\", \"hello\\nworld\\n\")\n  test.expect_snapshot(t, \"tail\", \"no newline\")\n  test.log(t, \"after the snapshots\")\n}\n\n@test\nfn twice(t: Test) {\n  test.expect_snapshot(t, \"same\", \"a\")\n  test.expect_snapshot(t, \"same\", \"b\")\n}\n",
    );
    let directory = manifest.parent().unwrap();
    let greeting = directory.join("snapshots/suite.render.greeting.snap");
    let missing = hirsel_test(&manifest, &["render", "--seed", "9"]);
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    assert_eq!(
        String::from_utf8(missing.stdout).unwrap(),
        concat!(
            "test suite@1/render ... FAILED (seed=9): snapshot greeting has no committed snapshots/suite.render.greeting.snap; snapshot tail has no committed snapshots/suite.render.tail.snap\n    after the snapshots\n    diff snapshots/suite.render.greeting.snap actual\n    --- snapshots/suite.render.greeting.snap\n    +++ actual\n    @@ -0,0 +1,2 @@\n    +hello\n    +world\n    diff snapshots/suite.render.tail.snap actual\n    --- snapshots/suite.render.tail.snap\n    +++ actual\n    @@ -0,0 +1,1 @@\n    +no newline\n    \\ No newline at end of file\n    run `hirsel test --bless` to accept the actual snapshots\ntest result: FAILED. 0 passed; 1 failed; 1 total\n"
        )
    );
    assert!(!greeting.exists());
    let blessed = hirsel_test(&manifest, &["render", "--bless"]);
    assert_eq!(blessed.status.code(), Some(0), "{blessed:?}");
    assert_eq!(
        std::fs::read_to_string(&greeting).unwrap(),
        "hello\nworld\n"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("snapshots/suite.render.tail.snap")).unwrap(),
        "no newline"
    );
    assert_eq!(hirsel_test(&manifest, &["render"]).status.code(), Some(0));
    std::fs::write(&greeting, "hello\nthere\n").unwrap();
    let changed = hirsel_test(&manifest, &["render", "--json"]);
    assert_eq!(changed.status.code(), Some(1), "{changed:?}");
    let json: serde_json::Value = serde_json::from_slice(&changed.stdout).unwrap();
    assert_eq!(
        json["tests"][0]["failure"],
        "snapshot greeting differs from snapshots/suite.render.greeting.snap"
    );
    assert_eq!(
        json["tests"][0]["report"],
        "after the snapshots\ndiff snapshots/suite.render.greeting.snap actual\n--- snapshots/suite.render.greeting.snap\n+++ actual\n@@ -1,2 +1,2 @@\n hello\n-there\n+world\nrun `hirsel test --bless` to accept the actual snapshots\n"
    );
    let twice = hirsel_test(&manifest, &["twice", "--bless", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&twice.stdout).unwrap();
    assert_eq!(json["tests"][0]["failure"], "snapshot same recorded twice");
}

// Hypothesis and minithesis report the shrunk counterexample; the reported seed replays it.
#[test]
fn property_failures_report_the_shrunk_counterexample_and_the_seed_replays_it() {
    let manifest = std_suite(
        "property",
        "fn small(mut s: test.Source) -> u64 {\n  test.range_u64(mut s, 0, 1000)\n}\n\nfn below(n: u64) -> bool uses {} {\n  n < 500\n}\n\n@test\nfn bounded(t: Test) {\n  test.property(t, small, below, u64.to_string)\n}\n\n@test\nfn equal(t: Test) {\n  test.assert_eq(t, 2u64 + 2, 5, u64.to_string)\n}\n",
    );
    let output = hirsel_test(&manifest, &["--json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let property = &json["tests"][0];
    let seed = property["seed"].as_u64().unwrap();
    let report = property["report"].as_str().unwrap();
    assert!(
        report.starts_with("counterexample: 500\ntest failure: seed="),
        "{report}"
    );
    assert!(report.contains(&format!("seed={seed} case=")), "{report}");
    assert_eq!(
        json["tests"][1]["report"],
        "assertion failed: actual == expected\n  actual: 4\nexpected: 5\n"
    );
    let replay = hirsel_test(
        &manifest,
        &["bounded", "--json", "--seed", &seed.to_string()],
    );
    let replayed: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replayed["tests"][0], *property);
    // A different seed fails at a case of its own, still shrinking to the boundary.
    let other = hirsel_test(&manifest, &["bounded", "--json", "--seed", "7"]);
    let other: serde_json::Value = serde_json::from_slice(&other.stdout).unwrap();
    assert!(
        other["tests"][0]["report"]
            .as_str()
            .unwrap()
            .starts_with("counterexample: 500\ntest failure: seed=7 case=")
    );
}

#[test]
fn test_step_budgets_are_explicit_and_interpreted_only() {
    let manifest = std_suite(
        "steps",
        "@test\nfn spin() {\n  let mut i: u64 = 0\n  while i < 100000 {\n    i += 1\n  }\n}\n",
    );
    let limited = hirsel_test(&manifest, &["--json", "--max-steps", "1000"]);
    assert_eq!(limited.status.code(), Some(1), "{limited:?}");
    let json: serde_json::Value = serde_json::from_slice(&limited.stdout).unwrap();
    assert!(
        json["tests"][0]["failure"]
            .as_str()
            .unwrap()
            .contains("ToolingLimit"),
        "{json}"
    );
    assert_eq!(hirsel_test(&manifest, &[]).status.code(), Some(0));
    let native = hirsel_test(&manifest, &["--native", "--max-steps", "1"]);
    assert_eq!(native.status.code(), Some(2), "{native:?}");
}

// Go testing.B, adapted: benches run only under --bench, calibrated by powers of ten.
#[test]
fn benches_run_only_under_bench_with_a_stable_report() {
    let manifest = std_suite(
        "bench",
        "@bench\nfn add() {\n  let x: u64 = 2\n  let _ = x + 2\n}\n\n@bench\nfn broken(t: Test) {\n  test.check(t, false)\n}\n\n@test\nfn ordinary() {}\n",
    );
    let tests = hirsel_test(&manifest, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&tests.stdout).unwrap();
    assert_eq!(json["summary"]["total"], 1, "{json}");
    let benches = hirsel_test(&manifest, &["--bench", "--bench-time", "20"]);
    assert_eq!(benches.status.code(), Some(1), "{benches:?}");
    let text = String::from_utf8(benches.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    let measured = lines[0]
        .strip_prefix("bench suite@1/add ... ")
        .and_then(|rest| rest.strip_suffix(" ns/iter"))
        .and_then(|rest| rest.split_once(" iterations, "))
        .unwrap_or_else(|| panic!("{text}"));
    let iterations: u64 = measured.0.parse().unwrap();
    assert!(
        iterations
            .to_string()
            .trim_start_matches('1')
            .bytes()
            .all(|b| b == b'0'),
        "{text}"
    );
    assert!(measured.1.parse::<f64>().unwrap() >= 0.0, "{text}");
    assert!(
        lines[1].starts_with("bench suite@1/broken ... FAILED (seed="),
        "{text}"
    );
    assert_eq!(
        lines[2],
        "bench result: FAILED. 1 measured; 1 failed; 2 total"
    );
    let json = hirsel_test(
        &manifest,
        &["--bench", "--bench-time", "20", "--json", "add"],
    );
    assert_eq!(json.status.code(), Some(0), "{json:?}");
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(json["benches"][0]["status"], "measured");
    assert!(json["benches"][0]["iterations"].as_u64().unwrap() >= 1);
    for invalid in [
        &["--bench", "--bless"][..],
        &["--bench-time", "5"],
        &["--bench", "--bench-time", "0"],
    ] {
        assert_eq!(
            hirsel_test(&manifest, invalid).status.code(),
            Some(2),
            "{invalid:?}"
        );
    }
}

#[test]
fn native_reports_snapshots_properties_and_benches_match_interpreted() {
    let manifest = std_suite(
        "native",
        "fn small(mut s: test.Source) -> u64 {\n  test.range_u64(mut s, 0, 1000)\n}\n\nfn below(n: u64) -> bool uses {} {\n  n < 500\n}\n\n@test\nfn a_property(t: Test) {\n  test.property(t, small, below, u64.to_string)\n}\n\n@test\nfn b_report(t: Test) {\n  test.log(t, \"line\\none\")\n  test.check(t, test.seed(t) == 731)\n  test.expect_snapshot(t, \"text\", \"snap\\n\")\n  test.assert_eq(t, \"a\", \"b\", test.quoted)\n}\n\n@test\nfn c_bad_name(t: Test) {\n  test.expect_snapshot(t, \"a b\", \"x\")\n}\n\n@bench\nfn d_bench() {}\n",
    );
    let run = |args: &[&str]| {
        let mut all = vec!["--json", "--seed", "731"];
        all.extend_from_slice(args);
        let output = hirsel_test(&manifest, &all);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let expected = run(&[]);
    assert_eq!(expected["summary"]["failed"], 3, "{expected}");
    assert!(
        expected["tests"][1]["report"]
            .as_str()
            .unwrap()
            .starts_with("line\none\nassertion failed: actual == expected\n  actual: \"a\"\nexpected: \"b\"\n"),
        "{expected}"
    );
    for backend in ["llvm", "cranelift"] {
        assert_eq!(
            run(&["--native", "--backend", backend]),
            expected,
            "{backend}"
        );
        let bench = hirsel_test(
            &manifest,
            &[
                "--bench",
                "--bench-time",
                "5",
                "--native",
                "--backend",
                backend,
                "--json",
            ],
        );
        assert_eq!(bench.status.code(), Some(0), "{bench:?}");
        let bench: serde_json::Value = serde_json::from_slice(&bench.stdout).unwrap();
        assert_eq!(bench["benches"][0]["status"], "measured", "{bench}");
    }
}

// Corpus checks language behavior; this checks runner failure isolation, filtering,
// seed replay and interpreter/native report parity, which corpus headers cannot express.
#[test]
fn simulated_tasks_and_clock_replay_in_isolated_tests() {
    let source = [
        include_str!("../../../../conformance/LANG/LANG-9-test-tasks-mutex.hir"),
        include_str!("../../../../conformance/LANG/LANG-9-test-tasks-ping-pong.hir"),
        include_str!("../../../../conformance/LANG/LANG-9-test-clock.hir"),
        include_str!("../../../../conformance/LANG/LANG-9-test-tasks-interleaving.hir"),
        include_str!("../../../../conformance/LANG/LANG-9-test-tasks-deadlock.hir"),
    ]
    .join("\n");
    let manifest = std_suite("simulated-tasks", &source);
    let run = |backend: Option<&str>, seed: &str, filters: &[&str]| {
        let scheduler = format!("random@1:{seed}");
        let mut args = vec!["--json", "--seed", seed, "--scheduler", &scheduler];
        if let Some(backend) = backend {
            args.extend(["--native", "--backend", backend]);
        }
        args.extend(filters);
        let output = hirsel_test(&manifest, &args);
        assert_eq!(
            output.status.code(),
            Some(if filters.is_empty() { 1 } else { 0 }),
            "{output:?}"
        );
        assert!(output.stderr.is_empty(), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let expected = run(None, "731", &[]);
    assert_eq!(expected["summary"]["total"], 6, "{expected}");
    assert_eq!(expected["summary"]["passed"], 5, "{expected}");
    let tests = expected["tests"].as_array().unwrap();
    let deadlock = tests
        .iter()
        .find(|t| t["name"].as_str().unwrap().ends_with("/deadlock"))
        .unwrap();
    let failure = deadlock["failure"].as_str().unwrap();
    assert!(failure.contains("blocked"), "{failure}");
    assert!(failure.contains("task []: task.await"), "{failure}");
    assert!(failure.contains("task [0]: channel.receive"), "{failure}");
    let interleaving = tests
        .iter()
        .find(|t| t["name"].as_str().unwrap().ends_with("/interleaving"))
        .unwrap();
    assert_eq!(
        run(None, "731", &["interleaving"])["tests"][0],
        *interleaving
    );
    let different = run(None, "42", &["interleaving"]);
    assert_ne!(
        different["tests"][0]["report"], interleaving["report"],
        "seeds should exercise distinct schedules"
    );
    for backend in ["llvm", "cranelift"] {
        let mut native_expected = expected.clone();
        for test in native_expected["tests"].as_array_mut().unwrap() {
            if let Some(replay) = test["replay"].as_str() {
                let suffix = if backend == "llvm" {
                    " --native"
                } else {
                    " --native --backend cranelift"
                };
                test["replay"] = format!("{replay}{suffix}").into();
            }
        }
        assert_eq!(run(Some(backend), "731", &[]), native_expected, "{backend}");
        assert_eq!(
            run(Some(backend), "42", &["interleaving"]),
            different,
            "{backend}"
        );
    }
}

#[test]
fn seeded_race_reports_name_both_operations_and_replay_the_same_candidates() {
    let manifest = std_suite(
        "race-names",
        include_str!("../../../../examples/race/main.hir"),
    );
    for scheduler in ["random@1:1", "pct@1:7"] {
        let args = ["--json", "--race", "--scheduler", scheduler];
        let output = hirsel_test(&manifest, &args);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let test = &json["tests"][0];
        let report = test["report"].as_str().unwrap();
        for name in [
            "race candidate on Atomic #",
            "task [0]",
            "task [1]",
            "atomic.load",
            "atomic.store",
            ":7:",
            ":8:",
        ] {
            assert!(report.contains(name), "{scheduler}: {report}");
        }
        let replay = test["replay"].as_str().unwrap();
        assert!(replay.contains("--race"), "{replay}");
        let replayed = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(replay.split(' ').skip(1))
            .arg("--json")
            .output()
            .unwrap();
        let replayed: serde_json::Value = serde_json::from_slice(&replayed.stdout).unwrap();
        assert_eq!(replayed["tests"][0]["report"], test["report"]);
        let plain = hirsel_test(&manifest, &["--json", "--scheduler", scheduler]);
        let plain: serde_json::Value = serde_json::from_slice(&plain.stdout).unwrap();
        assert_eq!(plain["tests"][0]["failure"], test["failure"]);
        assert!(
            !plain["tests"][0]["report"]
                .as_str()
                .unwrap()
                .contains("race candidate")
        );
    }
}

#[test]
fn extra_simulator_points_are_reproducible_and_replay_preserves_the_options() {
    let manifest = std_suite(
        "race-points",
        include_str!("../../../../examples/race/main.hir"),
    );
    for policy in ["random@1", "pct@1"] {
        let args = [
            "--json",
            "--race",
            "--sim-points",
            "root-release,atomic-store",
            "--explore",
            "20",
            "--scheduler",
            policy,
        ];
        let output = hirsel_test(&manifest, &args);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let test = &json["tests"][0];
        let replay = test["replay"].as_str().unwrap();
        assert!(
            replay.contains("--sim-points root-release,atomic-store"),
            "{replay}"
        );
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(replay.split(' ').skip(1))
            .arg("--json")
            .output()
            .unwrap();
        let replayed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        for field in ["status", "failure", "report", "scheduler"] {
            assert_eq!(
                replayed["tests"][0][field], test[field],
                "{replay}: {field}"
            );
        }
    }
    for flags in [
        &["--race", "--native"][..],
        &["--race", "--bench"],
        &["--sim-points", "atomic-store", "--native"],
        &["--sim-points", "bogus"],
        &["--sim-points", "root-release,root-release"],
    ] {
        let output = hirsel_test(&manifest, flags);
        assert!(!output.status.success(), "{flags:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("--sim-points"),
            "{output:?}"
        );
    }
}
