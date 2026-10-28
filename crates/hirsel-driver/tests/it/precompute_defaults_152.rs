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
            let corrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let compiler = if mode == "reuse" || mode == "tamper" {
                use hirsel_engine::store::ResultStore;
                let directory = hirsel_engine::store::DirStore::new(std::path::PathBuf::from(
                    std::env::var_os("HIRSEL_RESULTS_DIR").unwrap(),
                ))
                .with_read_only_root(products.clone());
                let selected = directory.for_compiler(COMPILER.identity()).unwrap();
                COMPILER.with_config(CompileConfig {
                    store: std::sync::Arc::new(GeneratorInputUnavailable {
                        store: selected,
                        corrupt_bound: mode == "tamper",
                        corrupted: corrupted.clone(),
                    }),
                    ..COMPILER.configuration()
                })
            } else {
                COMPILER.clone()
            };
            let mut requests = hirsel_driver::requests::CompilerRequests::for_host(
                compiler.clone(),
                compiler.configuration(),
            );
            let output = requests.execute(
                &manifest,
                hirsel_driver::requests::CompileOperation::ExportTir,
            );
            if mode == "tamper" {
                assert!(corrupted.load(std::sync::atomic::Ordering::Relaxed));
                let output = output.unwrap();
                let hirsel_driver::requests::CompileResult::ExportTir(tir) = &output.result else {
                    panic!("expected TIR")
                };
                assert_eq!(
                    tir.as_bytes(),
                    std::fs::read(manifest.with_file_name("registered.tir")).unwrap(),
                    "a producer-rebound output and decode limit must force fresh execution"
                );
                return;
            }
            let output = output.unwrap();
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
            std::fs::write(
                &source,
                format!("{original}\npub fn invalid()->u8{{false}}\n"),
            )
            .unwrap();
            let checked = COMPILER.check_path(&manifest).unwrap();
            assert!(
                checked
                    .diagnostics
                    .iter()
                    .any(|diagnostic| checked.sources.slice(diagnostic.primary) == Some("false"))
            );
            let diagnostics = checked
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.to_json(&checked.sources))
                .collect::<Vec<_>>();
            let output = std::path::PathBuf::from(
                std::env::var_os("HIRSEL_INSTALLATION_OUTPUT_214").unwrap(),
            );
            std::fs::write(
                output.with_extension("diagnostics"),
                serde_json::to_vec(&diagnostics).unwrap(),
            )
            .unwrap();
            std::fs::write(source, original).unwrap();
        });
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("hirsel.json");
    std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"registered_installation","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    std::fs::write(
        directory.path().join("main.hir"),
        r#"fn make()->Result[Body[fn()->u8 uses {}],String]{Ok(codegen.body("fn()->u8 uses {} {7u8}"))}
pub fn main()->u8 uses {} = comptime make()
"#,
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
    run("reuse", "registered.tir");
    run("tamper", "unused.tir");
    std::fs::remove_dir_all(directory.path().join("state")).unwrap();
    for cache in ["cache", "results"] {
        let _ = std::fs::remove_dir_all(directory.path().join(cache));
    }
    run("check", "fresh.tir");
    assert_eq!(
        std::fs::read(directory.path().join("registered.tir")).unwrap(),
        std::fs::read(directory.path().join("fresh.tir")).unwrap()
    );
    assert_eq!(
        std::fs::read(directory.path().join("registered.diagnostics")).unwrap(),
        std::fs::read(directory.path().join("fresh.diagnostics")).unwrap()
    );
}

// Refuse persisted private inputs and substitute an ordinary generator output.
// Request-local input transport remains available for sound recomputation.
struct GeneratorInputUnavailable {
    store: std::sync::Arc<dyn hirsel_engine::store::ResultStore>,
    corrupt_bound: bool,
    corrupted: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl hirsel_engine::store::ResultStore for GeneratorInputUnavailable {
    fn get(&self, key: &hirsel_engine::store::EntryKey) -> Option<hirsel_engine::store::Bytes> {
        use hirsel_engine::store::{Entry, VariantRef};
        if self.corrupt_bound && key.2 == QueryKind::Generate {
            let mut entry = Entry::load(&*self.store, key);
            for variant in &mut entry.variants {
                let bytes = self.store.get_content(variant.output)?;
                if bytes.len() != 56 {
                    continue;
                }
                // Schema 1 stores Reference (48 bytes), then the decode bound.
                if u32::from_le_bytes(bytes[32..36].try_into().unwrap()) != 0x4601_0006
                    || u32::from_le_bytes(bytes[36..40].try_into().unwrap()) != 1
                {
                    continue;
                }
                let original = hirsel_engine::store::Reference {
                    output: hirsel_engine::store::Output(hirsel_syntax::ContentHash(
                        bytes[..32].try_into().unwrap(),
                    )),
                    domain: hirsel_engine::store::Domain {
                        kind: 0x4601_0006,
                        schema: 1,
                    },
                    len: u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
                };
                let mut text = self.store.get_content(original)?.to_vec();
                let at = text.windows(5).position(|bytes| bytes == b"{7u8}")?;
                text[at + 1] = b'8';
                let replacement = self.store.put_content(original.domain, &text)?;
                let mut forged = bytes.to_vec();
                forged[..32].copy_from_slice(&replacement.output.0.0);
                forged[40..48].copy_from_slice(&replacement.len.to_le_bytes());
                forged[48..56].fill(0);
                *variant = VariantRef::publish(&*self.store, key, variant.reads.clone(), &forged)?;
                self.corrupted
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return Some(entry.encode_for(key).into());
        }
        self.store.get(key)
    }
    fn put(&self, key: &hirsel_engine::store::EntryKey, bytes: &[u8]) {
        self.store.put(key, bytes)
    }
    fn get_content(
        &self,
        reference: hirsel_engine::store::Reference,
    ) -> Option<hirsel_engine::store::Bytes> {
        if reference.domain.kind == 0x4601_0005 {
            None
        } else {
            self.store.get_content(reference)
        }
    }
    fn put_content(
        &self,
        domain: hirsel_engine::store::Domain,
        bytes: &[u8],
    ) -> Option<hirsel_engine::store::Reference> {
        self.store.put_content(domain, bytes)
    }
    fn lookup_product(
        &self,
        address: hirsel_engine::store::Address,
    ) -> Option<hirsel_engine::store::Association> {
        self.store.lookup_product(address)
    }
    fn associate_product(
        &self,
        address: hirsel_engine::store::Address,
        action: hirsel_engine::store::Action,
        reference: hirsel_engine::store::Reference,
    ) {
        self.store.associate_product(address, action, reference)
    }
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.store.local_product_store()
    }
    fn verifier(&self) -> Option<std::sync::Arc<hirsel_validate::VerifierCache>> {
        self.store.verifier()
    }
    fn name(&self) -> &'static str {
        "generator-input-unavailable"
    }
}
