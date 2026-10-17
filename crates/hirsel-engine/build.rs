//! Freeze the conservative shared execution dependency bundle at compiler build
//! time. Launch never reads source files or host paths to identify its program.
use hirsel_run_config::{ProgramNode, ProgramRevision};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn files(path: &Path, result: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("semantic dependency directory") {
            files(&entry.expect("semantic dependency entry").path(), result);
        }
    } else {
        result.push(path.to_owned());
    }
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap().parent().unwrap();
    let mut selected = Vec::new();
    for input in [
        "crates/hirsel-driver/src/network.rs",
        "crates/hirsel-interp/src",
        "crates/hirsel-interp/Cargo.toml",
        "crates/hirsel-rt/src",
        "crates/hirsel-rt/include",
        "crates/hirsel-rt/Cargo.toml",
        "crates/hirsel-kernels/src",
        "crates/hirsel-kernels/Cargo.toml",
        "crates/hirsel-rc/src/library",
        "crates/hirsel-engine/src/registry/section22.txt",
        "crates/hirsel-engine/src/registry/dictionaries.rs",
        "crates/hirsel-engine/src/registry/loops.rs",
        "crates/hirsel-engine/src/registry/heap.rs",
        "native-tests/library/operations.tsv",
    ] {
        files(&root.join(input), &mut selected);
    }
    let mut nodes = Vec::new();
    for path in selected {
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_str().unwrap())
            .collect::<Vec<_>>()
            .join("/");
        nodes.push(ProgramNode {
            identity: format!("$hirsel/runtime@1/{relative}"),
            content: vec![fs::read(path).unwrap()],
            dependencies: vec![],
        });
    }
    // These are the external implementation dependencies of the frozen runtime
    // and kernels. Bind exact resolved versions/checksums without compiler-only
    // packages, artifact paths, build profile, target or Cargo's own provenance.
    let lock = root.join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let lock = fs::read_to_string(lock).unwrap();
    for name in ["libc"] {
        let marker = format!("name = {name:?}\n");
        let mut found = false;
        for package in lock
            .split("[[package]]\n")
            .filter(|p| p.starts_with(&marker))
        {
            found = true;
            nodes.push(ProgramNode {
                identity: format!("$hirsel/runtime@1/external/{name}"),
                content: vec![package.trim().as_bytes().to_vec()],
                dependencies: vec![],
            });
        }
        assert!(found, "missing runtime dependency {name}");
    }
    let dependencies = nodes.iter().map(|n| n.identity.clone()).collect();
    nodes.push(ProgramNode {
        identity: "$hirsel/runtime@1".into(),
        content: vec![],
        dependencies,
    });
    let revision = ProgramRevision::encode("$hirsel/runtime@1", nodes).unwrap();
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("semantic_runtime.rs");
    fs::write(
        output,
        format!(
            "const RUNTIME_REVISION: [u8; 32] = {:?};\n",
            revision.digest()
        ),
    )
    .unwrap();
}
