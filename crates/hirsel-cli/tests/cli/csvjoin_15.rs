//! Independent Python csv oracle, three engines, bounded-input counters and replay.
use std::{fs, path::Path, process::Command};

#[test]
fn csvjoin_15_oracle_engines_limits_and_replay() {
    let evidence = std::env::var_os("CSVJOIN_15_EVIDENCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let scratch = tempfile::Builder::new()
        .prefix("csvjoin-15-")
        .tempdir_in(evidence)
        .unwrap()
        .keep();
    eprintln!("csvjoin_15 retained evidence: {}", scratch.display());
    fs::write(
        scratch.join("main.hir"),
        include_str!("../../../../examples/csvjoin/main.hir"),
    )
    .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/csvjoin/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    fs::write(scratch.join("hirsel.json"), manifest.to_string()).unwrap();
    let oracle = scratch.join("oracle.py");
    fs::write(&oracle, include_str!("../support/csvjoin_15/oracle.py")).unwrap();
    let output = Command::new("python3")
        .arg(oracle)
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(&scratch)
        .env("HIRSEL_DAEMON", "off")
        .output()
        .unwrap();
    fs::write(scratch.join("oracle.stdout"), &output.stdout).unwrap();
    fs::write(scratch.join("oracle.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "evidence {}: {}\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["engines"], 3);
    let tail_only = report["phase"] == "tail-replay";
    assert_eq!(report["oracle_cells"], if tail_only { 0 } else { 171 });
    assert_eq!(report["replay_cells"], 9);
    assert_eq!(report["streaming_cells"], if tail_only { 0 } else { 6 });
    for engine in ["H", "llvm", "cranelift"] {
        let tape =
            hirsel_rt::launch::read_recording_text(scratch.join(format!("{engine}-bounded.tape")))
                .unwrap();
        fs::write(scratch.join(format!("{engine}-bounded.txt")), &tape).unwrap();
        let reads: Vec<_> = tape
            .lines()
            .filter(|line| line.contains("file_read "))
            .collect();
        assert_eq!(reads.len(), 3, "{engine}: {reads:?}");
        for read in &reads {
            let request = read.split_once(" -> ").unwrap().0;
            assert_eq!(request.split_whitespace().last(), Some("4096"), "{read}");
        }
        eprintln!(
            "csvjoin_15 {engine}: {} buffered reads for a >4MiB left input, at most 12288 requested bytes",
            reads.len()
        );
    }
    eprintln!("csvjoin_15 executed: {report}");
}

#[test]
fn csvjoin_15_shared_nested_bucket_handback() {
    let scratch = tempfile::tempdir().unwrap();
    let source = format!(
        "{}\n{}",
        include_str!("../../../../examples/csvjoin/main.hir"),
        r#"
fn stop_after_append(mut index: Map[String, List[List[String]]]) -> Result[(), u64] {
  index_row(mut index, ["x", "R3"], 0)
  Error(7u64)
}

fn propagate(mut index: Map[String, List[List[String]]]) -> Result[(), u64] {
  stop_after_append(mut index)?
  index_row(mut index, ["never", "unreachable"], 0)
  Ok(())
}

@test
fn shared_nested_buckets(t: Test) {
  let key = f"x"
  let mut row = [copy key, f"R1"]
  let mut index: Map[String, List[List[String]]] = map.empty()
  index_row(mut index, row, 0)
  let saved = copy index
  row[1] = "changed"
  index_row(mut index, ["y", "Y1"], 0)
  index_row(mut index, [copy key, "R2"], 0)
  index_row(mut index, ["z", "Z1"], 0)
  index_row(mut index, ["w", "W1"], 0)
  test.check(t, propagate(mut index) == Error(7u64))
  test.check(t, map.len(index) == 4 && !map.contains(index, "never"))
  test.check(t, key == "x" && row == ["x", "changed"])
  test.check(t, map.get(saved, key) == Some([["x", "R1"]]))
  test.check(t, map.get(index, key) == Some([["x", "R1"], ["x", "R2"], ["x", "R3"]]))
  let mut keys: List[String] = []
  for (stored_key, _) in index {
    list.push(mut keys, copy stored_key)
  }
  test.check(t, keys == ["x", "y", "z", "w"])
  let array: Array[String] = ["array"]
  let inline: Inline[String, 1] = ["inline"]
  test.check(t, array[0] == "array" && inline[0] == "inline")
}
"#
    );
    fs::write(scratch.path().join("main.hir"), source).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/csvjoin/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let manifest_path = scratch.path().join("hirsel.json");
    fs::write(&manifest_path, manifest.to_string()).unwrap();
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.env("HIRSEL_DAEMON", "off").args(["test", "--json"]);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.arg(&manifest_path).output().unwrap();
        assert!(output.status.success(), "{backend:?}: {output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["summary"]["total"], 1, "{backend:?}: {report}");
        assert_eq!(report["summary"]["passed"], 1, "{backend:?}: {report}");
    }
}
