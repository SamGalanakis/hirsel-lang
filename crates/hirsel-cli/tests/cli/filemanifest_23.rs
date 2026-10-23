//! Go matching vectors, independent hashlib expectations, real read-only effects.
use std::{fs, path::PathBuf, process::Command};

#[test]
fn filemanifest_23_oracle_engines_bounds_and_read_only_effects() {
    let evidence = std::env::var_os("FILEMANIFEST_23_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let scratch = tempfile::Builder::new()
        .prefix("filemanifest-23-")
        .tempdir_in(evidence)
        .unwrap();
    let root = scratch.path();
    fs::create_dir_all(root.join("fixtures/release/docs")).unwrap();
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../../examples/filemanifest/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/filemanifest/model.hir"),
        ),
        (
            "grants.json",
            include_str!("../../../../examples/filemanifest/grants.json"),
        ),
        (
            "fixtures/release/abc.txt",
            include_str!("../../../../examples/filemanifest/fixtures/release/abc.txt"),
        ),
        (
            "fixtures/release/empty.txt",
            include_str!("../../../../examples/filemanifest/fixtures/release/empty.txt"),
        ),
        (
            "fixtures/release/docs/notes.txt",
            include_str!("../../../../examples/filemanifest/fixtures/release/docs/notes.txt"),
        ),
        (
            "fixtures/release/selected.json",
            include_str!("../../../../examples/filemanifest/fixtures/release/selected.json"),
        ),
        (
            "fixtures/release/invalid.json",
            include_str!("../../../../examples/filemanifest/fixtures/release/invalid.json"),
        ),
        (
            "fixtures/go-match.json",
            include_str!("../../../../examples/filemanifest/fixtures/go-match.json"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    let mut package: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../examples/filemanifest/hirsel.json"
    ))
    .unwrap();
    package["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    fs::write(root.join("hirsel.json"), package.to_string()).unwrap();
    let script = root.join("verify.py");
    fs::write(
        &script,
        include_str!("../support/filemanifest_23/verify.py"),
    )
    .unwrap();
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
    assert_eq!(result["upstream_vectors"], 30);
    assert!(result["cases"].as_u64().unwrap() >= 80, "{result}");
    assert_eq!(
        result["observations"],
        result["cases"].as_u64().unwrap() * 3
    );
    assert_eq!(result["cleanup"], true);
    eprintln!("filemanifest_23 executed: {result}");
    let removed = root.to_owned();
    scratch.close().unwrap();
    assert!(!removed.exists());
}

#[test]
fn filemanifest_23_checked_h_pure_model_and_upstream_vectors() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        fs::write(
            root.join("model.hir"),
            include_str!("../../../../examples/filemanifest/model.hir"),
        )
        .unwrap();
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../examples/filemanifest/fixtures/go-match.json"
        ))
        .unwrap();
        let mut source = String::from("pub fn main(t: Test) -> Result[(), model.InvalidInput] {\n");
        for vector in vectors.as_array().unwrap() {
            let pattern = serde_json::to_string(&vector["pattern"]).unwrap();
            let path = serde_json::to_string(&vector["path"]).unwrap();
            source.push_str(&format!(
                "{{ let Ok(compiled) = glob.compile({pattern}) else {{ return Error(model.invalid(\"pattern\", \"\")) }}; \
                 test.check(t, glob.is_match(compiled, {path}) == {}) }}\n", vector["matches"]));
        }
        source.push_str(r#"
  test.check(t, sha256.digest_hex(b"") == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
  let mut state = sha256.new()
  sha256.update(mut state, b"a")
  sha256.update(mut state, b"")
  sha256.update(mut state, b"bc")
  test.check(t, hex.encode(bytes.from_inline(sha256.finish(take state))) == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
  test.check(t, model.validate_path("docs/a.txt") == Ok(()))
  test.check(t, model.validate_path("../a") == Error(model.invalid("invalid_path", "../a")))
  test.check(t, model.validate_path("a//b") == Error(model.invalid("invalid_path", "a//b")))
  let entries = model.decode(b"{\"schema\":1,\"entries\":[{\"path\":\"z\",\"size\":0,\"sha256\":\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"},{\"path\":\"a\",\"size\":3,\"sha256\":\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\"}]}")?
  test.check(t, entries[0u64].path == "a" && entries[1u64].path == "z")
  let again = model.decode(model.encode(entries))?
  test.check(t, again == entries)
  test.check(t, model.decode(b"{") == Error(model.invalid("manifest_json", "")))
  test.check(t, model.verification_json(model.Verification(missing: ["a"], changed: ["z"])) == b"{\"schema\":1,\"missing\":[\"a\"],\"changed\":[\"z\"]}\n")
  Ok(())
}
"#);
        fs::write(root.join("main.hir"), source).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../examples/filemanifest/hirsel.json"
        ))
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
        eprintln!(
            "filemanifest_23: 30 Go match vectors, SHA-256 empty/abc and pure model controls execute under checked H"
        );
    });
}
