//! The image tests' fixture: a temporary directory of packages, std named
//! absolutely, and loading with a chosen set of std images.
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
            "hirsel-image-test-{}-{}",
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
pub fn std_manifest() -> PathBuf {
    // Manifests are written into a temporary directory: name std absolutely.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
}
pub fn load(path: &Path, images: &StdImages) -> LoadedProgram {
    load_program_source_with_images(path, None, images).unwrap()
}
/// The toolchain's std image: Buck2 writes it once for all tests; Cargo users
/// write it with the documented `hirsel image` command and set HIRSEL_IMAGE.
pub fn std_image() -> std::sync::Arc<StdImage> {
    let path =
        std::env::var_os("HIRSEL_IMAGE").expect("set HIRSEL_IMAGE to the toolchain's std image");
    StdImage::open(Path::new(&path)).unwrap()
}

pub const STD_PROGRAM: &str = "pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  out.print(output, string.trim(\" hello \")) ? Output
  out.print(output, u64.to_string(42u64)) ? Output
  Ok(())
}
";
