//! Independent full-byte LZW oracle, ported from LangArena 3c9f84d
//! rust/src/benchmarks/compress.rs:890-919,963-998 (MIT).
//! Copyright (c) 2026 Konstantin Makarchev. See the license in the lane report.
//! This is a Cargo-discovered standalone harness; Buck2 sync registers it.
hirsel_guard::install!();

use std::{collections::HashMap, path::PathBuf, process::Command};

fn reference_encode(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut dict = HashMap::with_capacity(4096);
    for i in 0..256 {
        dict.insert(vec![i as u8], i);
    }
    let mut next_code = 256;
    let mut result = Vec::with_capacity(input.len() * 2);
    let mut current = vec![input[0]];
    for &byte in &input[1..] {
        current.push(byte);
        if !dict.contains_key(&current) {
            let code = dict[&current[..current.len() - 1]];
            result.push(((code >> 8) & 0xff) as u8);
            result.push((code & 0xff) as u8);
            dict.insert(current, next_code);
            next_code += 1;
            current = vec![byte];
        }
    }
    let code = dict[&current];
    result.push(((code >> 8) & 0xff) as u8);
    result.push((code & 0xff) as u8);
    result
}

fn literal(bytes: &[u8]) -> String {
    format!(
        "[{}]",
        bytes
            .iter()
            .map(|b| format!("{b}u8"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

struct Fixture(PathBuf);
impl Fixture {
    fn new(backend: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let dir = std::env::temp_dir().join(format!("la-lzw-10-{}-{backend}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut source = include_str!("../../../bench/langarena/compress.hir").to_owned();
        let pattern = b"ABRACADABRA";
        let vectors = [
            Vec::new(),
            vec![0],
            vec![255],
            b"ABABABA".to_vec(),
            vec![65; 257],
            (0..=255).collect(),
            vec![0, 255, 0, 1, 255, 128, 0, 255],
            (0..60).map(|i| pattern[i % 11]).collect(),
            (0..61).map(|i| pattern[i % 11]).collect(),
            (0..1024).map(|i| ((i * 73 + i / 7) % 256) as u8).collect(),
        ];
        for (index, input) in vectors.iter().enumerate() {
            let encoded = reference_encode(input);
            source.push_str(&format!("\n@test\nfn la_lzw_10_vector_{index}(t: Test) {{\n  let input: List[u8] = {}\n  let encoded: List[u8] = {}\n  test.check(t, lzw_encode(input) == encoded)\n  test.check(t, lzw_decode(encoded) == input)\n}}\n",literal(input),literal(&encoded)));
        }
        source.push_str("\n@test\nfn la_lzw_10_codes(t: Test) {\n  test.check(t, lzw_encode([65u8, 66u8, 65u8, 66u8, 65u8, 66u8, 65u8]) == [0u8, 65u8, 0u8, 66u8, 1u8, 0u8, 1u8, 2u8])\n  test.check(t, lzw_decode([0u8, 65u8, 1u8, 0u8]) == [65u8, 65u8, 65u8])\n  test.check(t, lzw_decode([0u8, 65u8, 1u8, 1u8]) == [])\n}\n");
        std::fs::write(dir.join("compress.hir"), source).unwrap();
        std::fs::write(dir.join("main.hir"), "pub fn main() {}\n").unwrap();
        std::fs::write(
            dir.join("harness.hir"),
            include_str!("../../../bench/langarena/harness.hir"),
        )
        .unwrap();
        let manifest = serde_json::json!({"schema":1,"package":{"name":"la_lzw_10","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]},{"path":"compress.hir","module":["compress"]},{"path":"harness.hir","module":["harness"]}],"dependencies":[{"manifest":root.join("stdlib/std@1/hirsel.json"),"package":{"name":"std","version":"1"},"expose":[]}]});
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        Self(dir)
    }
    fn check(&self, backend: &str) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        cmd.arg("test").arg(self.0.join("hirsel.json"));
        if backend != "h" {
            cmd.args(["--native", "--backend", backend]);
        }
        let output = cmd.output().unwrap();
        assert!(output.status.success(), "{backend}: {output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("13 passed; 0 failed; 13 total"),
            "{backend}: {stdout}"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn la_lzw_10_h() {
    Fixture::new("h").check("h");
}
#[test]
fn la_lzw_10_llvm() {
    Fixture::new("llvm").check("llvm");
}
#[test]
fn la_lzw_10_cranelift() {
    Fixture::new("cranelift").check("cranelift");
}
