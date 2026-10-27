//! Original plan forgery vectors applied to actual root and Ready descriptors.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    runtime::*,
    store::{self, ResultStore, VerifyStore},
    *,
};
use native::queries::{self, NativeRoots};
use std::sync::Arc;

#[test]
fn bodyless_keyed_inventory_survives_cross_open_and_refuses_missing_metadata() {
    with_compiler_stack(|| {
        let source = "fn main()->u64 {let m:Map[u64,u64]=map.empty();let get:fn(Map[u64,u64],u64)->Option[u64]=map.get;match get(m,1u64) {Some(x)=>x,None=>0u64}}";
        let directory = tempfile::tempdir().unwrap();
        let mut expected = None;
        for warm in [false, true] {
            let selected = Arc::new(store::DirStore::new(directory.path().join("products")));
            let publication = selected.local_product_store().unwrap().request();
            let budget = ProcessBudget::new(24 << 20);
            let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
                selected,
                budget.reserve(24 << 20).unwrap(),
            ));
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("plan.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
            options.validation = native::Validation::Ready;
            let (actual, ()) = crate::native_object_products_19::compile_front(
                crate::native_front_fixture::products(&mut engine, &[]).unwrap(),
                store.clone(),
                &[entry_path(&default_context())],
                native::Backend::Cranelift,
                native::OptLevel::None,
                options,
                false,
                hirsel_intern::jobs::Jobs::ONE,
                VerifyStore::Off,
                |view, root, readies| {
                    let inventory = view
                        .with_link_context(root, readies, |program| {
                            assert!(program.units.is_empty());
                            assert_eq!(program.operations.pretransfer_adapters.len(), 1);
                            assert!(!program.operations.invocation_bodies.is_empty());
                            Ok((
                                program.operations.pretransfer_adapters.clone(),
                                program.operations.invocation_bodies.clone(),
                            ))
                        })
                        .unwrap();
                    if let Some(expected) = &expected {
                        assert_eq!(&inventory, expected);
                    } else {
                        expected = Some(inventory);
                    }
                    // The source/adapter inventory is immutable admitted input.
                    // A caller cannot replace the body or its typed layout
                    // closure while retaining only the old product identity.
                    for body in [false, true] {
                        let mut supplied = readies.to_vec();
                        let mut forged = (*supplied[0]).clone();
                        let previous = if body {
                            forged.ready
                        } else {
                            forged.layout_closure
                        };
                        let reference = store
                            .put_content(
                                previous.domain,
                                b"missing independently planned inventory",
                            )
                            .unwrap();
                        if body {
                            forged.ready = reference;
                        } else {
                            forged.layout_closure = reference;
                        }
                        supplied[0] = Arc::new(forged);
                        assert!(view.with_link_context(root, &supplied, |_| Ok(())).is_err());
                    }
                    let mut legacy = (**root).clone();
                    legacy.product.record = store
                        .put_content(root.product.record.domain, b"HIRNPL02")
                        .unwrap();
                    assert!(
                        view.with_link_context(&Arc::new(legacy), readies, |_| Ok(()))
                            .is_err()
                    );
                },
                |_, _| (),
            );
            if warm {
                assert_eq!(actual.hits, actual.objects.len());
            }
            publication.finish().unwrap();
        }
    });
}
