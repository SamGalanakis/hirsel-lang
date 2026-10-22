use std::{path::PathBuf, process::Command};

struct Fixture(PathBuf);

fn literal(bytes: &serde_json::Value) -> String {
    let parts: Vec<_> = bytes
        .as_array()
        .unwrap()
        .iter()
        .map(|byte| format!("{}u8", byte.as_u64().unwrap()))
        .collect();
    format!("[{}]", parts.join(", "))
}

impl Fixture {
    fn new(engine: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let dir = std::env::temp_dir().join(format!(
            "la-ngram-upsert-14-{}-{engine}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let mut source = include_str!("../../../../../bench/langarena/distance.hir").to_owned();
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("vectors.json")).unwrap();
        assert_eq!(vectors.as_array().unwrap().len(), 56);
        for (i, v) in vectors.as_array().unwrap().iter().enumerate() {
            let a = literal(&v["a"]);
            let b = literal(&v["b"]);
            let numerator = v["intersection"].as_u64().unwrap();
            let denominator = v["distinct_total"].as_u64().unwrap();
            let expected = if denominator == 0 {
                "0.0".to_owned()
            } else {
                format!("{numerator}.0 / {denominator}.0")
            };
            source.push_str(&format!("\n@test\nfn vector_{i}(t: Test) {{\n  let a: List[u8] = {a}\n  let b: List[u8] = {b}\n  test.check(t, ngram(a, b) == {expected})\n  test.check(t, ngram(b, a) == {expected})\n}}\n"));
        }
        source.push_str("\n@test\nfn packed_bytes(t: Test) {\n  test.check(t, gram([0u8, 128u8, 255u8, 1u8], 0) == 8453889u32)\n}\n");
        // Invoke actual execute/step with the independent, unchanged test.js controls.
        source.push_str("\npub fn main(rt: Io) -> Result[(), String] {\n");
        let controls: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../bench/langarena/test.js")).unwrap();
        let mut selected = 0;
        for task in controls
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["name"].as_str().unwrap().starts_with("Distance::"))
        {
            source.push_str("{\nlet mut config: harness.Config = map.empty()\n");
            for key in ["iterations", "count", "size", "checksum"] {
                source.push_str(&format!(
                    "map.insert(mut config, \"{key}\", Number(\"{}\"))\n",
                    task[key]
                ));
            }
            source.push_str(&format!("let report = execute(rt, {}, config)?\nfailure.check(report.actual == {}u32, Bounds())\n}}\n", task["name"], task["checksum"]));
            selected += 1;
        }
        assert_eq!(selected, 2);
        source.push_str("Ok(())\n}\n");
        std::fs::write(dir.join("distance.hir"), source).unwrap();
        std::fs::write(
            dir.join("harness.hir"),
            include_str!("../../../../../bench/langarena/harness.hir"),
        )
        .unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "package": {"name":"la_ngram_upsert_14", "version":"1"},
            "entry": {"module":["distance"], "name":"main"},
            "sources": [{"path":"distance.hir", "module":["distance"]}, {"path":"harness.hir", "module":["harness"]}],
            "dependencies": [{"manifest":root.join("stdlib/std@1/hirsel.json"), "package":{"name":"std", "version":"1"}, "expose":[]}]
        });
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        std::fs::write(dir.join("grants.json"), serde_json::json!({"io":{"monotonic_clock":"clock.monotonic.ns@1", "dirs":[]}, "args":[]}).to_string()).unwrap();
        Self(dir)
    }

    fn check(&self, engine: &str) {
        let mut test = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        test.args(["test", "--json"])
            .arg(self.0.join("hirsel.json"));
        if engine != "h" {
            test.args(["--native", "--backend", engine]);
        }
        let output = test.output().unwrap();
        assert!(output.status.success(), "{engine}: {output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["summary"],
            serde_json::json!({"passed":57,"failed":0,"total":57}),
            "{engine}: {report}"
        );
        let mut run = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        run.arg("run")
            .arg(self.0.join("hirsel.json"))
            .arg("--grants")
            .arg(self.0.join("grants.json"));
        if engine != "h" {
            run.args(["--native", "--backend", engine]);
        }
        let output = run.output().unwrap();
        assert!(
            output.status.success(),
            "{engine} reduced checksums: {output:?}"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn vectors_and_distance_checksums_h() {
    Fixture::new("h").check("h");
}
#[test]
fn vectors_and_distance_checksums_llvm() {
    Fixture::new("llvm").check("llvm");
}
#[test]
fn vectors_and_distance_checksums_cranelift() {
    Fixture::new("cranelift").check("cranelift");
}
