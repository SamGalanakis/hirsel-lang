//! Freeze the conservative shared execution dependency bundle at compiler build
//! time. Launch never reads source files or host paths to identify its program.
use hirsel_run_schema::{ProgramNode, ProgramRevision};
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

#[allow(dead_code)]
#[path = "src/registry/declarations.rs"]
mod registry;

struct Encoded<T>(T);
impl<T: serde::Serialize + Send> hirsel_image::Encode for Encoded<T> {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), String> {
        hirsel_image::encode_value(&self.0, out)
    }
}
fn core_image(out: &Path) {
    use hirsel_types::{TypeBase, TypeInterner, TypeKind};
    let scratch = TypeInterner::new(None);
    let signatures = registry::build::registry(&scratch);
    let revision = hirsel_syntax::ContentHash::of_parts(
        "hirsel.core.registry.v9",
        signatures.values().map(|s| s.fingerprint.0.as_slice()),
    );
    let mut variants = std::collections::BTreeSet::<String>::new();
    let base = TypeBase::build(scratch.len(), |ids| {
        ids.iter()
            .map(|id| {
                let row = scratch.row(*id);
                if let TypeKind::Variant(cases) = &row.kind {
                    variants.extend(cases.iter().map(|v| v.name.as_str().to_string()));
                }
                row.clone()
            })
            .collect()
    });
    let mut writer = hirsel_image::ImageWriter::new(revision, TypeInterner::new(Some(base)));
    for (d, signature) in signatures {
        writer.entry(hirsel_image::FIRST_KIND, d, Box::new(Encoded(signature)));
    }
    writer.blob(hirsel_image::FIRST_KIND + 1, Box::new(Encoded(variants)));
    let written = writer.finish().expect("encode core image");
    assert!(
        written.dropped.is_empty(),
        "core declarations must all encode"
    );
    fs::write(out.join("core.image"), written.bytes).unwrap();
}

fn main() {
    core_image(&PathBuf::from(env::var_os("OUT_DIR").unwrap()));
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
        "crates/hirsel-rt-operations/src",
        "crates/hirsel-rt-operations/Cargo.toml",
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
