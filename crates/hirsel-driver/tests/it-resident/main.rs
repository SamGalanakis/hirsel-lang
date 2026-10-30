//! Shared-runtime request lifecycle controls, independent of native campaigns.
hirsel_guard::install!();

fn fixture_product(
    store: &dyn hirsel_engine::store::ResultStore,
    address: hirsel_engine::store::Address,
    action: hirsel_engine::store::Action,
    reference: hirsel_engine::store::Reference,
) -> hirsel_engine::runtime::ProductRef {
    let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
    let mut allowance = budget.reserve(64 << 20).unwrap();
    let archive =
        hirsel_engine::runtime::ProductArchive::open(store, reference, &mut allowance).unwrap();
    hirsel_engine::runtime::ProductRef::import_archive(address, action, archive)
}

fn fixture_product_bytes(
    address: hirsel_engine::store::Address,
    action: hirsel_engine::store::Action,
    domain: hirsel_engine::store::Domain,
    bytes: &[u8],
) -> hirsel_engine::runtime::ProductRef {
    use hirsel_engine::store::ResultStore;
    let store = hirsel_engine::store::MemStore::default();
    let reference = store.put_content(domain, bytes).unwrap();
    fixture_product(&store, address, action, reference)
}

include!("modules.rs");
