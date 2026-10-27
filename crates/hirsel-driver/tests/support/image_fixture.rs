//! Temporary packages loaded through the ordinary product provider.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::PackageId;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub struct Fixture(pub PathBuf);
impl Fixture {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hirsel-products-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    pub fn write(&self, name: &str, source: &str) {
        std::fs::write(self.0.join(name), source).unwrap();
    }
    /// A root package `name` depending on the package at `manifest`.
    pub fn program(&self, name: &str, source: &str, manifest: &Path, dependency: &str) -> PathBuf {
        self.write(&format!("{name}.hir"), source);
        let manifest = serde_json::json!({
            "schema": 1,
            "package": {"name": name, "version": "1"},
            "sources": [{"path": format!("{name}.hir"), "module": []}],
            "dependencies": [{
                "manifest": manifest,
                "package": {"name": dependency, "version": "1"},
                "expose": [],
            }],
            "entry": {"module": [], "name": "main"},
        });
        let path = self.0.join(format!("{name}.json"));
        std::fs::write(&path, manifest.to_string()).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
pub fn package(name: &str) -> PackageId {
    PackageId {
        name: name.into(),
        version: "1".into(),
    }
}
/// std's manifest, named absolutely: manifests are written into a temporary directory.
/// The `it` binary's precheck tests use small libraries instead.
#[allow(dead_code)]
pub fn std_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
}
/// Load with an optional ordinary product root and a fresh local authority.
pub fn load(path: &Path, products: Option<&Path>) -> LoadedProgram {
    let store: std::sync::Arc<dyn store::ResultStore> = match products {
        Some(products) => {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let local = path
                .parent()
                .unwrap()
                .join(format!("reader-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
            std::sync::Arc::new(
                store::DirStore::new(local).with_read_only_root(products.to_path_buf()),
            )
        }
        None => std::sync::Arc::new(store::NoneStore),
    };
    COMPILER
        .clone()
        .with_config(CompileConfig {
            store,
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        })
        .load_program(path)
        .unwrap()
}
#[allow(dead_code)]
pub fn std_products() -> PathBuf {
    std::env::var_os("HIRSEL_TEST_PRODUCTS")
        .or_else(|| std::env::var_os("HIRSEL_PRODUCTS"))
        .map(PathBuf::from)
        .expect("the harness supplies ordinary std products")
}

/// Select only the source's core declarations for fixtures that use no std API.
#[allow(dead_code)]
pub fn core_manifest(source: &Path, manifest: &Path) {
    let package = serde_json::json!({
        "schema": 1,
        "package": {"name": "standalone", "version": "0"},
        "sources": [{"path": source.canonicalize().unwrap(), "module": []}],
        "dependencies": [],
        "entry": {"module": [], "name": "main"},
    });
    std::fs::write(manifest, package.to_string()).unwrap();
}
