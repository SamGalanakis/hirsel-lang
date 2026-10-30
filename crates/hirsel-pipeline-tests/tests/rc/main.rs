//! Pipeline tests of hirsel-rc, linked as one binary: each `rc_<file>` module holds
//! what was `crates/hirsel-rc/tests/it/<file>.rs`. They compile source through the
//! driver; the front-phase suites link separately (tests/front).

hirsel_guard::install!();

fn imported_fixture(
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

#[path = "support.rs"]
mod rc_support;

include!("modules.rs");
