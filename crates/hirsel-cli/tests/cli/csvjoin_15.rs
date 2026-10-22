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
