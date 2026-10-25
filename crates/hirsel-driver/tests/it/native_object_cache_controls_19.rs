//! Original unit-cache warm/recompute vectors using admitted native tasks.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    runtime::ProductRef,
    store::{self, Address, Association, Bytes, EntryKey, ResultStore, VerifyStore},
    *,
};
use hirsel_intern::{
    jobs::Jobs,
    timings::{self, Count, Phase},
};
use std::sync::{Arc, Mutex};

const SOURCE: &str = "fn helper(x:u64)->u64{x+1u64}\nfn main()->u64{helper(7u64)}";
struct Store {
    inner: store::DirStore,
    substituted: Mutex<Option<(Address, Association)>>,
}
impl ResultStore for Store {
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.inner.get(key)
    }
    fn put(&self, key: &EntryKey, bytes: &[u8]) {
        self.inner.put(key, bytes);
    }
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.inner.local_product_store()
    }
    fn get_content(&self, reference: store::Reference) -> Option<Bytes> {
        self.inner.get_content(reference)
    }
    fn put_content(&self, domain: store::Domain, bytes: &[u8]) -> Option<store::Reference> {
        self.inner.put_content(domain, bytes)
    }
    fn associate_product(
        &self,
        address: Address,
        action: store::Action,
        reference: store::Reference,
    ) {
        self.inner.associate_product(address, action, reference);
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.inner.verifier()
    }
    fn lookup_product(&self, address: Address) -> Option<Association> {
        if let Some((selected, association)) = *self.substituted.lock().unwrap()
            && selected == address
        {
            return Some(association);
        }
        self.inner.lookup_product(address)
    }
    fn name(&self) -> &'static str {
        "original-object-cache-controls"
    }
}
fn compile(
    store: Arc<Store>,
    jobs: usize,
    backend: native::Backend,
    verification: VerifyStore,
) -> crate::native_object_products_19::Objects {
    let transport_budget = runtime::ProcessBudget::new(24 << 20);
    let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
        store,
        transport_budget.reserve(24 << 20).unwrap(),
    ));
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add("warm-object.hir", SOURCE.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let input = engine.export_native_input().unwrap();
    let _validated = engine.import_native_input(input).unwrap();
    let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = if verification == VerifyStore::Recompute {
        native::Validation::EveryStage
    } else {
        native::Validation::Ready
    };
    crate::native_object_products_19::compile_with_context(
        &mut engine,
        store,
        &entry_path(&default_context()),
        backend,
        native::OptLevel::None,
        options,
        false,
        Jobs::new(jobs),
        verification,
        |_, _| (),
    )
    .0
}
fn calls() -> ([u64; 6], u64) {
    let report = timings::report();
    let phases = [
        Phase::Place,
        Phase::Rc,
        Phase::Reuse,
        Phase::Ready,
        Phase::Codegen,
        Phase::LirValidate,
    ];
    (
        phases.map(|phase| {
            report
                .phases
                .iter()
                .find(|p| p.phase == phase)
                .unwrap()
                .calls
        }),
        report
            .counts
            .iter()
            .find(|(count, _)| *count == Count::Units)
            .unwrap()
            .1,
    )
}
#[test]
fn original_warm_ready_skips_work_and_recompute_compares_actual_fragments() {
    with_compiler_stack(|| {
        timings::enable();
        for workers in [1, 2, 8] {
            let directory = tempfile::tempdir().unwrap();
            let store = Arc::new(Store {
                inner: store::DirStore::new(directory.path().join("products")),
                substituted: Mutex::new(None),
            });
            let before = calls();
            let cold = compile(
                store.clone(),
                workers,
                native::Backend::Cranelift,
                VerifyStore::Off,
            );
            let after = calls();
            let cold_calls: [_; 6] = std::array::from_fn(|i| after.0[i] - before.0[i]);
            let units = after.1 - before.1;
            assert!(units > 0);
            assert!(cold_calls.iter().all(|calls| *calls > 0));
            assert!(!cold.objects.is_empty());
            assert_eq!(cold.hits, 0);
            assert!(cold.ready_transformations > 0);
            let warm = compile(
                store.clone(),
                workers,
                native::Backend::Cranelift,
                VerifyStore::Off,
            );
            let after_warm = calls();
            assert_eq!(warm.hits, cold.objects.len());
            assert_eq!(warm.objects, cold.objects);
            assert_eq!(warm.ready_transformations, 0);
            assert_eq!(after_warm.1, after.1, "Ready hit lowered a unit");
            assert_eq!(
                &after_warm.0[..5],
                &after.0[..5],
                "Ready hit ran the pipeline"
            );
            let verified = compile(
                store.clone(),
                workers,
                native::Backend::Cranelift,
                VerifyStore::Recompute,
            );
            let after_verified = calls();
            assert_eq!(verified.objects, cold.objects);
            assert_eq!(verified.hits, cold.objects.len());
            assert_eq!(verified.ready_transformations, cold.ready_transformations);
            let checked_calls: [_; 6] =
                std::array::from_fn(|i| after_verified.0[i] - after_warm.0[i]);
            assert_eq!(after_verified.1 - after_warm.1, units);
            assert_eq!(&checked_calls[..5], &cold_calls[..5]);
            assert_eq!(
                checked_calls[5] - cold_calls[5],
                4 * units,
                "warm EveryStage did not restore all four intermediate checkpoints"
            );
            // Coherent bytes from another actual backend cannot acquire the
            // selected Codegen action by forging its ordinary association.
            let other = compile(
                store.clone(),
                workers,
                native::Backend::Llvm,
                VerifyStore::Off,
            );
            let (key, original): &(_, ProductRef) = cold
                .products
                .iter()
                .find(|(key, _)| key.owner.instance.definition().0.name == "main")
                .unwrap();
            let (_, replacement) = other
                .products
                .iter()
                .find(|(other, _)| other.owner == key.owner)
                .unwrap();
            assert_ne!(original.record, replacement.record);
            *store.substituted.lock().unwrap() = Some((
                key.address(),
                Association {
                    action: original.action,
                    reference: replacement.record,
                },
            ));
            let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                compile(
                    store,
                    workers,
                    native::Backend::Cranelift,
                    VerifyStore::Recompute,
                )
            }));
            let message = refused
                .err()
                .expect("recomputation accepted changed cached object");
            let text = message
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| message.downcast_ref::<&str>().copied())
                .unwrap_or("");
            assert!(
                text.contains("recomputed object fragment differs from cached output"),
                "{text}"
            );
        }
    });
}
