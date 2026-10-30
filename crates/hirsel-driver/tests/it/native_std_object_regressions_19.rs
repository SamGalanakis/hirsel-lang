//! Original private std projection and singleton callable ABI vectors from
//! native/seed_parity_tests.rs, executed through source-owned native tasks.
use hirsel_driver::{COMPILER, native};
use hirsel_engine::{
    store::{self, ResultStore, VerifyStore},
    *,
};
use std::sync::Arc;

fn manifest() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json")
}
fn root(package: hirsel_syntax::PackageId, module: &str, name: &str) -> DefPath {
    DefPath(hirsel_syntax::ItemPath {
        package,
        modules: vec![module.into()],
        name: name.into(),
    })
}
#[test]
fn http_reject_lowering_has_valid_projections() {
    with_compiler_stack(|| {
        let mut package = COMPILER.load_package_declarations(&manifest()).unwrap();
        let selected = root(package.package.clone(), "http", "http_server_reject");
        // This private root is absent from ordinary program reachability.
        package.engine.lower_to_tir(&selected).unwrap();
    });
}
#[test]
fn ordinary_store_delete_has_one_consistent_entry_abi() {
    with_compiler_stack(|| {
        let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
        let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
            Arc::new(store::MemStore::default()),
            budget.reserve(64 << 20).unwrap(),
        ));
        let mut config = COMPILER.configuration();
        config.store = store.clone();
        let mut package = COMPILER
            .with_config(config)
            .with_owned_engines()
            .load_package_declarations(&manifest())
            .unwrap();
        let selected = root(package.package.clone(), "store", "tx_delete");
        // Resolve only this original library root, as in the old singleton test.
        let _validated = package
            .engine
            .native_input_for(std::slice::from_ref(&selected))
            .unwrap();
        for backend in [native::Backend::Cranelift, native::Backend::Llvm] {
            for opt in [
                native::OptLevel::None,
                native::OptLevel::Default,
                native::OptLevel::Speed,
            ] {
                let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
                let (actual, ()) = crate::native_object_products_19::compile_roots(
                    &mut package.engine,
                    store.clone(),
                    std::slice::from_ref(&selected),
                    std::slice::from_ref(&selected),
                    backend,
                    opt,
                    options,
                    false,
                    hirsel_intern::jobs::Jobs::ONE,
                    VerifyStore::Off,
                    |_, _| (),
                );
                assert!(!actual.objects.is_empty(), "{backend:?} {opt:?}");
                assert_eq!(actual.objects.len(), actual.summaries.len());
            }
        }
    });
}
