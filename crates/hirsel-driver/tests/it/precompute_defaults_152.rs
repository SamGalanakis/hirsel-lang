use hirsel_driver::{COMPILER, CompilerRequestsExt};
use hirsel_engine::{CompileConfig, DefinitionQueries, QueryKind, with_compiler_stack};
use hirsel_syntax::{DefPath, ItemPath, PackageId};

#[test]
fn library_defaults_read_installed_products_and_preserve_explicit_none() {
    const CHILD: &str = "HIRSEL_LIBRARY_PRODUCTS_152";
    if let Some(manifest) = std::env::var_os(CHILD) {
        with_compiler_stack(|| {
            let manifest = std::path::Path::new(&manifest);
            let definition = DefPath(ItemPath {
                package: PackageId { name: "library_defaults".into(), version: "1".into() },
                modules: vec![], name: "first".into(),
            });
            assert!(COMPILER.configuration().store.enabled());
            let mut installed = COMPILER.load_check_package(manifest).unwrap();
            installed.engine.check_definition(&definition).unwrap();
            assert_eq!(installed.engine.recompute_count(&definition, QueryKind::Check), 0);
            assert_eq!(installed.engine.recompute_count(&definition, QueryKind::Modes), 0);
            let explicit = COMPILER.with_config(CompileConfig::default());
            assert!(!explicit.configuration().store.enabled());
            let mut fresh = explicit.load_check_package(manifest).unwrap();
            fresh.engine.check_definition(&definition).unwrap();
            assert_eq!(fresh.engine.recompute_count(&definition, QueryKind::Check), 1);
        });
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("main.hir"), "pub fn first()->u64{7u64}\n").unwrap();
    let manifest = directory.path().join("hirsel.json");
    std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"library_defaults","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]}"#).unwrap();
    let products = directory.path().join("products");
    with_compiler_stack(|| COMPILER.precompute_package(&manifest, &products).unwrap());
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "precompute_defaults_152::library_defaults_read_installed_products_and_preserve_explicit_none", "--nocapture"])
        .env(CHILD, &manifest).env("HIRSEL_PRODUCTS", &products)
        .env_remove("HIRSEL_VERIFY_CACHE")
        .output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}
