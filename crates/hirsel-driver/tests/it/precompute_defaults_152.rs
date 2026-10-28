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
                package: PackageId {
                    name: "library_defaults".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: "first".into(),
            });
            assert!(COMPILER.configuration().store.enabled());
            let mut installed = COMPILER.load_check_package(manifest).unwrap();
            installed.engine.check_definition(&definition).unwrap();
            assert_eq!(
                installed
                    .engine
                    .recompute_count(&definition, QueryKind::Check),
                0
            );
            assert_eq!(
                installed
                    .engine
                    .recompute_count(&definition, QueryKind::Modes),
                0
            );
            let explicit = COMPILER.with_config(CompileConfig::default());
            assert!(!explicit.configuration().store.enabled());
            let mut fresh = explicit.load_check_package(manifest).unwrap();
            fresh.engine.check_definition(&definition).unwrap();
            assert_eq!(
                fresh.engine.recompute_count(&definition, QueryKind::Check),
                1
            );
        });
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("main.hir"),
        "pub fn first()->u64{7u64}\n",
    )
    .unwrap();
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

#[test]
fn registered_installation_and_deleted_private_state_preserve_diagnostics_and_tir() {
    const CHILD: &str = "HIRSEL_INSTALLATION_214";
    if let Some(mode) = std::env::var_os(CHILD) {
        with_compiler_stack(|| {
            let manifest = std::path::PathBuf::from(
                std::env::var_os("HIRSEL_INSTALLATION_MANIFEST_214").unwrap(),
            );
            let products = std::path::PathBuf::from(std::env::var_os("HIRSEL_PRODUCTS").unwrap());
            if mode == "install" {
                COMPILER.precompute_package(&manifest, &products).unwrap();
                let state = hirsel_validate::Installation::default_state().unwrap();
                assert!(
                    hirsel_validate::Installation::registered(
                        &products,
                        &state,
                        COMPILER.identity(),
                        std::sync::Arc::default()
                    )
                    .is_some()
                );
                return;
            }
            let mut requests = hirsel_driver::requests::CompilerRequests::for_host(
                COMPILER.clone(),
                COMPILER.configuration(),
            );
            let output = requests
                .execute(
                    &manifest,
                    hirsel_driver::requests::CompileOperation::ExportTir,
                )
                .unwrap();
            let hirsel_driver::requests::CompileResult::ExportTir(tir) = &output.result else {
                panic!("expected TIR")
            };
            std::fs::write(
                std::env::var_os("HIRSEL_INSTALLATION_OUTPUT_214").unwrap(),
                tir,
            )
            .unwrap();
            let source = manifest.with_file_name("main.hir");
            let original = std::fs::read_to_string(&source).unwrap();
            std::fs::write(&source, original.replace("7u8", "false")).unwrap();
            let checked = COMPILER.check_path(&manifest).unwrap();
            assert!(
                checked
                    .diagnostics
                    .iter()
                    .any(|diagnostic| checked.sources.slice(diagnostic.primary) == Some("false"))
            );
            std::fs::write(source, original).unwrap();
        });
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("hirsel.json");
    std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"registered_installation","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    std::fs::write(
        directory.path().join("main.hir"),
        "pub fn main()->u8 uses {} {7u8}\n",
    )
    .unwrap();
    let run = |mode: &str, output: &str| {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "precompute_defaults_152::registered_installation_and_deleted_private_state_preserve_diagnostics_and_tir", "--nocapture"])
            .env(CHILD, mode).env("HIRSEL_INSTALLATION_MANIFEST_214", &manifest)
            .env("HIRSEL_PRODUCTS", directory.path().join("products"))
            .env("XDG_STATE_HOME", directory.path().join("state"))
            .env("XDG_CACHE_HOME", directory.path().join("cache"))
            .env("HIRSEL_RESULTS_DIR", directory.path().join("results"))
            .env("HIRSEL_INSTALLATION_OUTPUT_214", directory.path().join(output))
            .env("HIRSEL_RESULTS", "dir@1").env_remove("HIRSEL_VERIFY_CACHE")
            .output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    };
    run("install", "unused");
    run("check", "registered.tir");
    std::fs::remove_dir_all(directory.path().join("state")).unwrap();
    for cache in ["cache", "results"] {
        let _ = std::fs::remove_dir_all(directory.path().join(cache));
    }
    run("check", "fresh.tir");
    assert_eq!(
        std::fs::read(directory.path().join("registered.tir")).unwrap(),
        std::fs::read(directory.path().join("fresh.tir")).unwrap()
    );
}
