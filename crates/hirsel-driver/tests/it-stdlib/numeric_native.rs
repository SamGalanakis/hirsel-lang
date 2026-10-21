//! One native executable shared by the numeric vector suites. Large products
//! exceed the corpus interpreter budget; the vector cases remain ordinary std source.
use hirsel_driver::native::{self, BuildProfile, TargetSpec};
use std::{path::PathBuf, process::Command, sync::OnceLock};

pub fn check(arguments: &[String], context: &str) {
    static CHECKER: OnceLock<PathBuf> = OnceLock::new();
    let checker = CHECKER.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("hirsel-numeric-vectors-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("vectors.hir");
        std::fs::write(&source, SOURCE).unwrap();
        let std_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "package": {"name": "numeric_vectors", "version": "1"},
            "entry": {"module": [], "name": "main"},
            "sources": [{"path": "vectors.hir", "module": []}],
            "dependencies": [{"manifest": std_manifest, "package": {"name": "std", "version": "1"}, "expose": []}]
        });
        std::fs::write(source.with_extension("hir.json"), manifest.to_string()).unwrap();
        std::fs::write(dir.join("grants.json"), "{}").unwrap();
        let binary = dir.join("vectors");
        native::build_path_native_backend_with_validation(&source, &binary, TargetSpec::x86_64_unknown_linux_gnu(),
            BuildProfile::Release, native::Backend::Llvm, None, false, hirsel_engine::Verify::On, native::Validation::EveryStage).unwrap().unwrap();
        binary
    });
    let result = Command::new(checker)
        .arg("--hirsel-grants")
        .arg(checker.parent().unwrap().join("grants.json"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{context}: {:?}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
}

const SOURCE: &str = include_str!("numeric-vectors.hir");
