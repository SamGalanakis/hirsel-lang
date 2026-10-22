//! Full-output controls for LangArena's stable BWT inverse.
//! Goldens: unchanged LangArena 3c9f84d compress.rs:32-123,159-197 (MIT).
//! Copyright (c) 2026 Konstantin Makarchev; see bench/langarena/LICENSE.
hirsel_guard::install!();

use std::{path::PathBuf, process::Command};

struct Fixture(PathBuf);

fn literal(bytes: &serde_json::Value) -> String {
    let bytes: Vec<String> = bytes
        .as_array()
        .unwrap()
        .iter()
        .map(|byte| format!("{}u8", byte.as_u64().unwrap()))
        .collect();
    format!("[{}]", bytes.join(", "))
}

impl Fixture {
    fn new(engine: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let dir =
            std::env::temp_dir().join(format!("la-bwt-inverse-11-{}-{engine}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut source = include_str!("../../../bench/langarena/compress.hir").to_owned();
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("support/la_bwt_inverse_11/vectors.json")).unwrap();
        assert_eq!(vectors.as_array().unwrap().len(), 61);
        for (index, vector) in vectors.as_array().unwrap().iter().enumerate() {
            let input = literal(&vector["input"]);
            let encoded = literal(&vector["encoded"]);
            let primary = vector["primary"].as_u64().unwrap();
            source.push_str(&format!(
                "\n@test\nfn inverse_vector_{index}(t: Test) {{\n\
                 let input: List[u8] = {input}\n\
                 let expected: List[u8] = {encoded}\n\
                 let encoded = bwt_encode(input)\n\
                 test.check(t, encoded.data == expected)\n\
                 test.check(t, encoded.original == {primary})\n\
                 test.check(t, bwt_decode(encoded) == input)\n\
                 test.check(t, bwt_decode(Bwt(data: expected, original: {primary})) == input)\n\
                 }}\n"
            ));
        }
        source.push_str(include_str!("support/la_bwt_inverse_11/controls.hir"));
        // A separate entry exercises the historical checked-index failure.
        source.push_str("\npub fn main() -> Result[(), String] { let _ = bwt_decode(Bwt(data: [65u8], original: 1)); Ok(()) }\n");
        std::fs::write(dir.join("compress.hir"), source).unwrap();
        std::fs::write(
            dir.join("harness.hir"),
            include_str!("../../../bench/langarena/harness.hir"),
        )
        .unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "package": {"name": "la_bwt_inverse_11", "version": "1"},
            "entry": {"module": ["compress"], "name": "main"},
            "sources": [{"path": "compress.hir", "module": ["compress"]},
                        {"path": "harness.hir", "module": ["harness"]}],
            "dependencies": [{"manifest": root.join("stdlib/std@1/hirsel.json"),
                              "package": {"name": "std", "version": "1"}, "expose": []}]
        });
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        Self(dir)
    }

    fn check(&self, engine: &str) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .args(["test", "--json"])
            .arg(self.0.join("hirsel.json"));
        if engine != "h" {
            command.args(["--native", "--backend", engine]);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{engine}: {output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        // The imported compression module contributes its own codec controls.
        // Pin membership as well as the count so a missing inverse vector cannot
        // be hidden by another test added to that module.
        let mut expected: Vec<String> = [
            "codec_vectors",
            "huff_decode_cursor_vectors",
            "inverse_empty_ignores_primary",
            "inverse_reduced_checksums",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain((0..61).map(|index| format!("inverse_vector_{index}")))
        .map(|name| format!("la_bwt_inverse_11@1/compress/{name}"))
        .collect();
        expected.sort();
        let mut actual = Vec::new();
        for test in report["tests"].as_array().unwrap() {
            assert_eq!(test["status"], "passed", "{engine}: {test}");
            assert_eq!(test["failure"], serde_json::Value::Null, "{engine}: {test}");
            actual.push(test["name"].as_str().unwrap().to_owned());
        }
        actual.sort();
        assert_eq!(actual, expected, "{engine}: {report}");
        let total = expected.len();
        assert_eq!(
            report["summary"],
            serde_json::json!({"passed": total, "failed": 0, "total": total}),
            "{engine}: {report}"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn full_outputs_h() {
    Fixture::new("h").check("h");
}

#[test]
fn full_outputs_llvm() {
    Fixture::new("llvm").check("llvm");
}

#[test]
fn full_outputs_cranelift() {
    Fixture::new("cranelift").check("cranelift");
}

#[test]
fn invalid_primary_is_checked() {
    let fixture = Fixture::new("invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(fixture.0.join("hirsel.json"))
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("MEM-14"),
        "{output:?}"
    );
}
