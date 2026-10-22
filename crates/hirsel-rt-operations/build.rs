//! Compile the independently reviewed native inventory into a lookup table.
use std::{env, fs, path::PathBuf};
fn main() {
    let path = "../../native-tests/library/operations.tsv";
    println!("cargo:rerun-if-changed={path}");
    let text = fs::read_to_string(path).unwrap();
    let mut out = String::from(
        "fn reviewed(name: &str) -> Option<([u8; 32], &'static str)> { Some(match name {\n",
    );
    let mut names = std::collections::BTreeSet::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        let name = fields.next().unwrap();
        let digest = fields.next().unwrap();
        let ownership = fields.next().unwrap();
        assert!(fields.next().is_none() && names.insert(name));
        assert_eq!(digest.len(), 64);
        let bytes: Vec<u8> = (0..64)
            .step_by(2)
            .map(|i| u8::from_str_radix(&digest[i..i + 2], 16).unwrap())
            .collect();
        out.push_str(&format!("{name:?} => ({bytes:?}, {ownership:?}),\n"));
    }
    out.push_str("_ => return None, }) }\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("reviewed.rs"),
        out,
    )
    .unwrap();
}
