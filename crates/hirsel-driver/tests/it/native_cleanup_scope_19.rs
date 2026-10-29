//! An ordinary direct call must not consume a peer cleanup-body observation.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    store::{self, ResultStore},
    *,
};
use std::sync::Arc;

#[test]
fn same_source_call_receipt_survives_the_complete_source_inventory() {
    with_compiler_stack(|| {
        let store: Arc<dyn ResultStore> = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "plan.hir",
                "fn helper(x:u64)->u64{x+1u64}\nfn main()->u64{helper(7u64)}".into(),
            )
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let _validated = engine.import_native_input(input).unwrap();
        let result = crate::native_object_products_19::compile_front(
            crate::native_front_fixture::products(&mut engine, &[]).unwrap(),
            store,
            &[entry_path(&default_context())],
            native::Backend::Cranelift,
            native::OptLevel::None,
            native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            false,
            hirsel_intern::jobs::Jobs::ONE,
            store::VerifyStore::Off,
            false,
            |_, _, _| (),
            |_, _| (),
        )
        .0;
        assert!(!result.objects.is_empty());
        assert_eq!(result.objects.len(), result.summaries.len());
    });
}
