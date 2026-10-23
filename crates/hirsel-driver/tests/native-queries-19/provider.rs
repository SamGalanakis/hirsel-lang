use hirsel_driver::native::queries::request_store;
use hirsel_engine::store::{Domain, MemStore, ResultStore};
use hirsel_engine::{PhaseAdapters, QueryEngine, default_environment, runtime::ProcessBudget};
use std::sync::Arc;

#[test]
fn admitted_native_request_selects_one_transport_and_only_selected_authority() {
    let budget = ProcessBudget::new(16384);
    let engine = QueryEngine::new(default_environment(), PhaseAdapters::default());
    let selected = request_store(&engine, budget.reserve(8192).unwrap());
    assert!(selected.verifier().is_none());
    assert!(selected.local_product_store().is_none());
    let native = selected.clone();
    let object = selected.clone();
    let reference = native
        .put_content(
            Domain {
                kind: 0x4e00001f,
                schema: 1,
            },
            b"request",
        )
        .unwrap();
    assert_eq!(&*object.get_content(reference).unwrap(), b"request");
    assert!(!selected.exhausted());
    let backend = Arc::new(MemStore::default());
    let authority = backend.verifier().unwrap();
    let engine = QueryEngine::new(default_environment(), PhaseAdapters::default())
        .with_result_store(backend);
    let selected = request_store(&engine, budget.reserve(8192).unwrap());
    assert!(Arc::ptr_eq(&selected.verifier().unwrap(), &authority));
}
