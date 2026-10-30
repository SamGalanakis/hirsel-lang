//! Ordinary local library donor vectors from the former plan/seed controls.
use hirsel_driver::native::{self, Backend};
use hirsel_engine::{
    runtime,
    store::{DirStore, RequestStore, ResultStore, VerifyStore},
    *,
};
use std::sync::Arc;
fn compile(
    engine: &mut QueryEngine,
    store: Arc<dyn ResultStore>,
    selected: &[DefPath],
    requested: &[DefPath],
    backend: Backend,
    verification: VerifyStore,
) -> crate::native_object_products_19::Objects {
    let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = if verification == VerifyStore::Recompute {
        native::Validation::EveryStage
    } else {
        native::Validation::Ready
    };
    crate::native_object_products_19::compile_roots(
        engine,
        store,
        selected,
        requested,
        backend,
        native::OptLevel::None,
        options,
        false,
        hirsel_intern::jobs::Jobs::ONE,
        verification,
        |_, _| (),
    )
    .0
}
fn ordinary_provider(backend: Backend) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("library.hir"), "type Payload { n:u64 }\npub fn aaa()->(bool,u32) { (true,0u32) }\npub fn answer(x:u64)->u64 { let p=Payload(n:x); let get=fn()->u64 { p.n }; get()+1u64 }\n").unwrap();
        let library = root.join("library.json");
        std::fs::write(
            &library,
            serde_json::json!({
                "schema":1,"package":{"name":"ordinary","version":"1"},
                "sources":[{"path":"library.hir","module":[]}],"dependencies":[]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(root.join("main.hir"), "pub fn main()->Result[(),()] { failure.check(lib.answer(3u64)==4u64, Arithmetic()); Ok(()) }\n").unwrap();
        let application = root.join("application.json");
        std::fs::write(&application, serde_json::json!({
            "schema":1,"package":{"name":"consumer","version":"1"},
            "sources":[{"path":"main.hir","module":[]}],
            "dependencies":[{"manifest":"library.json","package":{"name":"ordinary","version":"1"},"expose":[{"head":"lib","module":[]}]}],
            "entry":{"module":[],"name":"main"}
        }).to_string()).unwrap();
        let cache = root.join("objects");
        let open_store = || -> Arc<dyn ResultStore> { Arc::new(DirStore::new(cache.clone())) };
        let donor = {
            let budget = runtime::ProcessBudget::new(24 << 20);
            let store: Arc<dyn ResultStore> = Arc::new(RequestStore::new(
                open_store(),
                budget.reserve(24 << 20).unwrap(),
            ));
            let mut config = hirsel_driver::COMPILER.configuration();
            config.store = store.clone();
            config.verify = Verify::On;
            config.verify_store = VerifyStore::Off;
            let mut package = hirsel_driver::COMPILER
                .with_config(config)
                .with_owned_engines()
                .load_package_declarations(&library)
                .unwrap();
            let roots = ["aaa", "answer"].map(|name| {
                DefPath(hirsel_syntax::ItemPath {
                    package: package.package.clone(),
                    modules: vec![],
                    name: name.into(),
                })
            });
            let _validated = package.engine.native_input_for(&roots).unwrap();
            compile(
                &mut package.engine,
                store,
                &roots,
                &roots,
                backend,
                VerifyStore::Off,
            )
        };
        assert!(!donor.objects.is_empty());
        let application_compile = |store: Arc<dyn ResultStore>, verification| {
            let transport_budget = runtime::ProcessBudget::new(24 << 20);
            let store: Arc<dyn ResultStore> = Arc::new(RequestStore::new(
                store,
                transport_budget.reserve(24 << 20).unwrap(),
            ));
            let mut config = hirsel_driver::COMPILER.configuration();
            config.store = store.clone();
            config.verify = Verify::On;
            config.verify_store = VerifyStore::Off;
            let mut loaded = hirsel_driver::COMPILER
                .with_config(config)
                .with_owned_engines()
                .load_program(&application)
                .unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let _validated = loaded.engine.import_native_input(input).unwrap();
            let entry = loaded.entry.clone();
            compile(
                &mut loaded.engine,
                store,
                &[entry],
                &[],
                backend,
                verification,
            )
        };
        let first = application_compile(open_store(), VerifyStore::Off);
        assert!(first.hits > 0, "{backend:?}: no ordinary donor hit");
        let expected = first.objects.clone();
        let summaries = first.summaries.clone();
        drop(first);
        let verified = application_compile(open_store(), VerifyStore::Recompute);
        assert_eq!(expected, verified.objects);
        assert_eq!(summaries, verified.summaries);
        drop(verified);
        let warm = application_compile(open_store(), VerifyStore::Off);
        assert_eq!(
            warm.hits,
            warm.summaries.len(),
            "{backend:?}: incomplete warm hit"
        );
        assert_eq!(expected, warm.objects);
        drop(warm);
        struct Imported(DirStore);
        impl ResultStore for Imported {
            fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
                self.0.get(key)
            }
            fn put(&self, _: &store::EntryKey, _: &[u8]) {}
            fn get_content(&self, reference: store::Reference) -> Option<store::Bytes> {
                self.0.get_content(reference)
            }
            fn put_content(&self, _: store::Domain, _: &[u8]) -> Option<store::Reference> {
                None
            }
            fn lookup_product(&self, address: store::Address) -> Option<store::Association> {
                self.0.lookup_product(address)
            }
            fn name(&self) -> &'static str {
                "ordinary-imported-bytes"
            }
        }
        let imported: Arc<dyn ResultStore> = Arc::new(Imported(DirStore::new(cache)));
        assert!(imported.verifier().is_none());
        assert!(
            imported.local_product_store().is_none(),
            "copied bytes gained native authority"
        );
        let imported = application_compile(imported, VerifyStore::Off);
        assert_eq!(expected, imported.objects);
        let fresh = application_compile(Arc::new(store::NoneStore), VerifyStore::Off);
        assert_eq!(expected, fresh.objects);
    });
}

#[test]
fn ordinary_provider_cranelift() {
    ordinary_provider(Backend::Cranelift);
}
#[test]
fn ordinary_provider_llvm() {
    ordinary_provider(Backend::Llvm);
}
