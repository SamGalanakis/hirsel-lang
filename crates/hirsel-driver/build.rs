//! The compiler identity a prechecked package is checked under: a digest of every
//! source and manifest of this crate and of each workspace crate it depends on,
//! with the lockfile and toolchain pin. Any edit to them changes it, so a precheck
//! written by a different compiler is never used (docs/building.md).
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

/// The workspace crates a manifest's `[dependencies]` name by path.
fn path_dependencies(manifest: &str) -> Vec<String> {
    let mut section = "";
    let mut result = vec![];
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        } else if section == "[dependencies]"
            && let Some((_, rest)) = line.split_once("path = \"../")
        {
            result.push(rest.split('"').next().unwrap().to_owned());
        }
    }
    result
}

fn files(path: &Path, result: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("compiler source directory") {
            files(&entry.expect("compiler source entry").path(), result);
        }
    } else if path.exists() {
        result.push(path.to_owned());
    }
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let crates = manifest.parent().unwrap();
    let root = crates.parent().unwrap();
    let mut closure = BTreeSet::new();
    let mut todo = vec!["hirsel-driver".to_owned()];
    while let Some(name) = todo.pop() {
        if closure.insert(name.clone()) {
            let text = fs::read_to_string(crates.join(&name).join("Cargo.toml"))
                .expect("workspace crate manifest");
            todo.extend(path_dependencies(&text));
        }
    }
    let mut selected = vec![];
    for name in &closure {
        let dir = crates.join(name);
        for part in ["Cargo.toml", "build.rs", "src", "include", "bridge"] {
            files(&dir.join(part), &mut selected);
        }
    }
    for part in [
        "Cargo.lock",
        "rust-toolchain.toml",
        "native-tests/library/operations.tsv",
        "tools/native-toolchain/llvm-23.lock",
        "tools/buck2/llvm-cxx-headers-lock.json",
    ] {
        files(&root.join(part), &mut selected);
    }
    let mut entries: Vec<_> = selected
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_str().unwrap())
                .collect::<Vec<_>>()
                .join("/");
            (relative, path)
        })
        .collect();
    entries.sort();
    let mut digest = Sha256::new();
    digest.update(b"hirsel.compiler-identity.v1");
    for (relative, path) in entries {
        let content = fs::read(path).unwrap();
        for part in [relative.as_bytes(), &content] {
            digest.update((part.len() as u64).to_le_bytes());
            digest.update(part);
        }
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("compiler_identity.rs"),
        format!(
            "/// See build.rs.\npub const COMPILER_IDENTITY: [u8; 32] = {:?};\n",
            digest.finalize().as_slice()
        ),
    )
    .unwrap();
}
