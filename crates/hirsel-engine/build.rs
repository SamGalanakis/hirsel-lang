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

#[path = "src/registry/products.rs"]
mod products;

fn core_products(out: &Path) {
    use hirsel_artifact_store::{Contents, Domain, Memory, Store};
    use hirsel_product_codec::{BaseIndex, TypeRecords};
    use hirsel_types::{TypeInterner, TypeKind};
    use std::sync::Arc;
    let types = TypeInterner::new(None);
    let signatures = registry::build::registry(&types);
    let revision = hirsel_syntax::ContentHash::of_parts(
        "hirsel.core.registry.v9",
        signatures.values().map(|s| s.fingerprint.0.as_slice()),
    );
    let roots: Vec<_> = types.ids().collect();
    let mut variants = std::collections::BTreeSet::<String>::new();
    for &id in &roots {
        if let TypeKind::Variant(cases) = &types.row(id).kind {
            variants.extend(cases.iter().map(|v| v.name.as_str().to_string()));
        }
    }
    let memory = Arc::new(Memory::default());
    let records = TypeRecords::new(types, memory.clone());
    let base = BaseIndex::capture(&records, &roots).expect("core type index");
    let signatures = signatures
        .into_iter()
        .map(|(d, signature)| {
            let bytes = records
                .encode_record(&signature, |_| None)
                .expect("core signature record");
            let reference = memory
                .put_content(Domain { kind: 3, schema: 1 }, &bytes)
                .expect("bounded core signature");
            (d, reference)
        })
        .collect();
    let index = products::CoreProducts {
        revision,
        base,
        signatures,
        variants,
    };
    fs::write(
        out.join("core-products.index"),
        bincode::serialize(&index).unwrap(),
    )
    .unwrap();
    // Embed ordinary Store containers, including their normal checked header.
    // There is no second image framing, codec or admission implementation.
    let store = Store::new(out.join("core-store"));
    let mut entries = memory.entries();
    entries.sort_unstable_by_key(|(r, _)| r.output.0);
    let mut source = String::from("hirsel_artifact_store::Embedded(&[\n");
    for (reference, bytes) in entries {
        assert_eq!(store.put(reference.domain, &bytes).unwrap(), reference);
        let relative = store
            .output_path(reference)
            .strip_prefix(out)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        source.push_str(&format!(
            "(hirsel_artifact_store::Reference {{ output: hirsel_artifact_store::Output(hirsel_syntax::ContentHash({:?})), domain: hirsel_artifact_store::Domain {{ kind: {}, schema: {} }}, len: {} }}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{}\"))),\n",
            reference.output.0.0, reference.domain.kind, reference.domain.schema, reference.len, relative
        ));
    }
    source.push_str("])\n");
    fs::write(out.join("core-products.rs"), source).unwrap();
}

fn main() {
    core_products(&PathBuf::from(env::var_os("OUT_DIR").unwrap()));
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
