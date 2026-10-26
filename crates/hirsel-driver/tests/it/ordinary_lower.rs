//! Program checking must consume ordinary Lower products through current V.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use std::sync::Arc;

#[test]
fn program_check_reuses_current_ordinary_lower_products_without_worker_relowering() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            directory.path().join("main.hir"),
            "pub fn first()->u64 { 7u64 }\npub fn second()->u64 { 9u64 }\n",
        )
        .unwrap();
        std::fs::write(
            &manifest,
            serde_json::json!({
                "schema":1, "package":{"name":"cached", "version":"1"},
                "sources":[{"path":"main.hir", "module":[]}], "dependencies":[]
            })
            .to_string(),
        )
        .unwrap();
        let seed = directory.path().join("products");
        COMPILER.precompute_package(&manifest, &seed).unwrap();
        let path = |name: &str| {
            DefPath(ItemPath {
                package: PackageId {
                    name: "cached".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: name.into(),
            })
        };
        for jobs in [1, 2, 8] {
            // This positive reuse test retains only V's original local issuer;
            // ordinary read-only installations still need independent first V.
            let compiler = COMPILER.clone().with_config(CompileConfig {
                store: Arc::new(store::DirStore::new(seed.clone())),
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                hold_bytes: 0,
                ..CompileConfig::for_memory_cap(None)
            });
            let mut program = compiler.load_check_package(&manifest).unwrap();
            let checked = program.check(LoweringPolicy::Reachable).unwrap();
            assert!(checked.diagnostics.is_empty());
            assert_eq!(checked.completion, CheckCompletion::Complete);
            for name in ["first", "second"] {
                assert_eq!(
                    program.engine.lower_adapter_calls(&path(name)),
                    0,
                    "jobs={jobs}: current ordinary Lower was ignored during program check"
                );
                assert_eq!(
                    program
                        .engine
                        .recompute_count(&path(name), QueryKind::Lower),
                    0
                );
            }
            // A cache candidate later than the first failed root must remain
            // unadopted. Metadata/snapshot reads do not authorize Lower work.
            let mut failed = compiler.load_check_package(&manifest).unwrap();
            let output = failed.engine.lower_definition_prefix(
                vec![path("missing"), path("second")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert_eq!(output.len(), 1);
            assert!(output[0].result.is_err());
            assert!(
                failed
                    .engine
                    .memo_reads(&path("second"), QueryKind::Lower)
                    .is_none()
            );
        }
    });
}

#[test]
fn readonly_comptime_products_reuse_only_the_consumers_local_verifier() {
    with_compiler_stack(|| {
        struct ReadOnly {
            products: store::DirStore,
            verifier: Arc<hirsel_validate::VerifierCache>,
        }
        impl store::ResultStore for ReadOnly {
            fn read_scope(&self) -> Option<Box<dyn Send + Sync>> {
                self.products.read_scope()
            }
            fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
                self.products.get(key)
            }
            fn put(&self, _: &store::EntryKey, _: &[u8]) {}
            fn get_content(&self, reference: store::Reference) -> Option<store::Bytes> {
                self.products.get_content(reference)
            }
            fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
                Some(self.verifier.clone())
            }
            fn name(&self) -> &'static str {
                "none@1"
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let library = root.join("library.json");
        std::fs::write(root.join("library.hir"),
            "const LEAF:u64=7u64\nconst CHAIN:u64=comptime LEAF\npub fn value()->u64 { comptime CHAIN }\npub const VALUE:u64=comptime value()\n").unwrap();
        std::fs::write(
            &library,
            serde_json::json!({
                "schema":1, "package":{"name":"library", "version":"1"},
                "sources":[{"path":"library.hir", "module":[]}], "dependencies":[]
            })
            .to_string(),
        )
        .unwrap();
        let seed = root.join("products");
        COMPILER.precompute_package(&library, &seed).unwrap();
        let manifest = root.join("hirsel.json");
        std::fs::write(
            root.join("main.hir"),
            "pub fn main()->Result[(),()] { Ok(()) }\n",
        )
        .unwrap();
        std::fs::write(&manifest, serde_json::json!({
            "schema":1, "package":{"name":"client", "version":"1"},
            "sources":[{"path":"main.hir", "module":[]}],
            "dependencies":[{"manifest":library,"package":{"name":"library","version":"1"},"expose":[]}],
            "entry":{"module":[],"name":"main"}
        }).to_string()).unwrap();
        let store = Arc::new(ReadOnly {
            products: store::DirStore::new(root.join("unused")).with_read_only_root(seed),
            verifier: Arc::new(hirsel_validate::VerifierCache::directory(
                root.join("local-v"),
            )),
        });
        let compiler = COMPILER.clone().with_config(CompileConfig {
            store: store.clone(),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        let mut first = compiler.load_check_package(&manifest).unwrap();
        assert!(
            first
                .check(LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        drop(first);
        let mut warm = compiler.load_check_package(&manifest).unwrap();
        assert!(
            warm.check(LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let calls: u64 = warm
            .engine
            .indexed_definitions()
            .iter()
            .filter(|path| path.0.package.name == "library")
            .map(|path| warm.engine.lower_adapter_calls(path))
            .sum();
        assert_eq!(
            calls, 0,
            "warm consumer re-lowered CT products despite its own V cache"
        );
        assert!(
            !root.join("unused").exists(),
            "read-only provider wrote query results"
        );
    });
}
