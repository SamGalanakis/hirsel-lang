//! Independent CPython vectors and application boundaries through the actual CLI.
use std::{fs, path::PathBuf, process::Command};

#[test]
fn argvplan_23_oracle_engines_bounds_and_read_only_effects() {
    let evidence = std::env::var_os("ARGVPLAN_23_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let scratch = tempfile::Builder::new()
        .prefix("argvplan-23-")
        .tempdir_in(evidence)
        .unwrap();
    let root = scratch.path();
    fs::create_dir(root.join("fixtures")).unwrap();
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../../examples/argvplan/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/argvplan/model.hir"),
        ),
        (
            "fixtures/review.txt",
            include_str!("../../../../examples/argvplan/fixtures/review.txt"),
        ),
        (
            "fixtures/invalid.txt",
            include_str!("../../../../examples/argvplan/fixtures/invalid.txt"),
        ),
        (
            "fixtures/cpython-posix.json",
            include_str!("../../../../examples/argvplan/fixtures/cpython-posix.json"),
        ),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/argvplan/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    fs::write(root.join("hirsel.json"), manifest.to_string()).unwrap();
    let script = root.join("verify.py");
    fs::write(&script, include_str!("../support/argvplan_23/verify.py")).unwrap();
    let output = Command::new("python3")
        .arg(script)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(root)
        .arg("--output")
        .arg(root.join("evidence"))
        .output()
        .unwrap();
    if !output.status.success() {
        let retained = scratch.keep();
        panic!(
            "evidence {}: stdout={}\nstderr={}",
            retained.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["upstream_vectors"], 31);
    assert!(result["cases"].as_u64().unwrap() >= 40, "{result}");
    assert_eq!(
        result["observations"],
        result["cases"].as_u64().unwrap() * 3
    );
    assert_eq!(result["cleanup"], true);
    eprintln!("argvplan_23 executed: {result}");
    let removed = root.to_owned();
    scratch.close().unwrap();
    assert!(!removed.exists());
}

#[test]
fn argvplan_23_checked_h_pure_model_vectors() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        fs::write(
            root.join("model.hir"),
            include_str!("../../../../examples/argvplan/model.hir"),
        )
        .unwrap();
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../examples/argvplan/fixtures/cpython-posix.json"
        ))
        .unwrap();
        let mut source = String::from("pub fn main(t: Test) -> Result[(), model.InvalidInput] {\n");
        for vector in vectors.as_array().unwrap() {
            let line = serde_json::to_string(&vector["source"]).unwrap();
            let argv = serde_json::to_string(&vector["argv"]).unwrap();
            source.push_str(&format!(
                "{{ let plan = model.decode(unique string.as_bytes({line}), false)?; \
                 let expected: List[String] = {argv}; \
                 test.check(t, list.len(plan.commands) == 1); \
                 test.check(t, plan.commands[0u64].argv == expected); \
                 let roundtrip = shlex.split(plan.commands[0u64].joined); \
                 test.check(t, roundtrip == Ok(expected)) }}\n"
            ));
        }
        source.push_str(r#"
  let blank = model.decode(b"\n \t\r\n", false)?
  test.check(t, list.len(blank.commands) == 0)
  let commented = model.decode(b"echo a#tail\n", true)?
  test.check(t, commented.commands[0u64].argv == ["echo", "a"])
  test.check(t, model.decode(b"echo 'broken", false) == Error(model.invalid("unclosed_quote", 1, 5)))
  test.check(t, model.decode(b"echo x\\", false) == Error(model.invalid("trailing_escape", 1, 6)))
  test.check(t, model.filename("review.txt") && !model.filename("../review.txt"))
  let wire = model.plan_json(commented)
  let Ok(rendered) = json.stringify(wire) else { return Error(model.invalid("json", 0, 0)) }
  test.check(t, rendered == "{\"schema\":1,\"commands\":[{\"line\":1,\"argv\":[\"echo\",\"a\"],\"joined\":\"echo a\"}]}")
  Ok(())
}
"#);
        fs::write(root.join("main.hir"), source).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../../examples/argvplan/hirsel.json"))
                .unwrap();
        manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .into();
        let selection = root.join("hirsel.json");
        fs::write(&selection, manifest.to_string()).unwrap();
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let plain = hirsel_engine::run_observed_snapshot(
            &mut loaded.engine,
            &config,
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                plain.outcome,
                Ok(hirsel_engine::RunOutcome::Returned(
                    hirsel_run_schema::Returned::Ok
                ))
            ),
            "{:?}",
            plain.outcome
        );
        let checked = hirsel_engine::run_observed_checked(
            &mut loaded.engine,
            &config,
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        assert_eq!(plain.observation, checked.observation);
        assert!(
            matches!(
                checked.outcome,
                Ok(hirsel_engine::RunOutcome::Returned(
                    hirsel_run_schema::Returned::Ok
                ))
            ),
            "{:?}",
            checked.outcome
        );
        eprintln!("argvplan_23: 31 upstream vectors and pure model controls agree under checked H");
    });
}
