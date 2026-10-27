//! Original private plan missing/corrupt/publication-failure vectors over Codegen.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    runtime::*,
    store::{self, ResultStore},
    *,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Faults {
    disk: store::DirStore,
    hidden: Mutex<Vec<store::Reference>>,
    substituted: Mutex<Vec<(store::Address, store::Association)>>,
    drop_writes: AtomicBool,
    fragment_puts: AtomicUsize,
}
impl ResultStore for Faults {
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.disk.get(key)
    }
    fn put(&self, key: &store::EntryKey, bytes: &[u8]) {
        self.disk.put(key, bytes);
    }
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.disk.local_product_store()
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.disk.verifier()
    }
    fn get_content(&self, reference: store::Reference) -> Option<store::Bytes> {
        if self.hidden.lock().unwrap().contains(&reference) {
            None
        } else {
            self.disk.get_content(reference)
        }
    }
    fn put_content(&self, domain: store::Domain, bytes: &[u8]) -> Option<store::Reference> {
        if domain.kind == 0x4f00_0001 {
            self.fragment_puts.fetch_add(1, Ordering::Relaxed);
            if self.drop_writes.load(Ordering::Relaxed) {
                return None;
            }
        }
        let written = self.disk.put_content(domain, bytes);
        if let Some(reference) = written {
            self.hidden
                .lock()
                .unwrap()
                .retain(|hidden| *hidden != reference);
        }
        written
    }
    fn lookup_product(&self, address: store::Address) -> Option<store::Association> {
        self.substituted
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| *key == address)
            .map(|(_, row)| *row)
            .or_else(|| self.disk.lookup_product(address))
    }
    fn associate_product(
        &self,
        address: store::Address,
        action: store::Action,
        reference: store::Reference,
    ) {
        if !(reference.domain.kind == 0x4f00_0001 && self.drop_writes.load(Ordering::Relaxed)) {
            self.disk.associate_product(address, action, reference);
        }
    }
    fn name(&self) -> &'static str {
        "native-publication-fault-controls"
    }
}
fn compile(faults: Arc<Faults>, verify: Verify) -> crate::native_object_products_19::Objects {
    let physical = faults.disk.local_product_store().unwrap();
    let publication = physical.request();
    let budget = ProcessBudget::new(24 << 20);
    let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
        faults,
        budget.reserve(24 << 20).unwrap(),
    ));
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    engine.verify = verify;
    let file = engine
        .sources
        .add(
            "plan.hir",
            "fn helper(x:u64)->u64{x+1u64}\nfn main()->u64{helper(7u64)}".into(),
        )
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = native::Validation::Ready;
    let result = crate::native_object_products_19::compile_front(
        crate::native_front_fixture::products(&mut engine, &[]).unwrap(),
        store,
        &[entry_path(&default_context())],
        native::Backend::Cranelift,
        native::OptLevel::None,
        options,
        false,
        hirsel_intern::jobs::Jobs::ONE,
        store::VerifyStore::Off,
        |_, _, _| (),
        |_, _| (),
    )
    .0;
    drop(engine);
    publication.finish().unwrap();
    result
}
#[test]
fn original_missing_corrupt_metadata_and_failed_publication_recompute_without_losing_output() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Faults {
            disk: store::DirStore::new(directory.path().join("products")),
            hidden: Mutex::new(vec![]),
            substituted: Mutex::new(vec![]),
            drop_writes: AtomicBool::new(false),
            fragment_puts: AtomicUsize::new(0),
        });
        let cold = compile(store.clone(), Verify::On);
        assert!(!cold.objects.is_empty());
        let (key, record) = cold.products.first().unwrap();
        store.hidden.lock().unwrap().push(record.record);
        let before = store.fragment_puts.load(Ordering::Relaxed);
        let missing = compile(store.clone(), Verify::On);
        assert!(
            store.fragment_puts.load(Ordering::Relaxed) > before,
            "missing record did not recompute"
        );
        assert_eq!(cold.objects, missing.objects);
        store.hidden.lock().unwrap().clear();
        let truncated = store
            .disk
            .put_content(record.record.domain, b"truncated")
            .unwrap();
        store.substituted.lock().unwrap().push((
            key.address(),
            store::Association {
                action: record.action,
                reference: truncated,
            },
        ));
        let before = store.fragment_puts.load(Ordering::Relaxed);
        let corrupt = compile(store.clone(), Verify::On);
        assert!(
            store.fragment_puts.load(Ordering::Relaxed) > before,
            "malformed record did not recompute"
        );
        assert_eq!(cold.objects, corrupt.objects);
        store.substituted.lock().unwrap().clear();
        // A fresh durable store refuses every fragment publication. The
        // bounded request transport still owns each actual produced output.
        let denied = Arc::new(Faults {
            disk: store::DirStore::new(directory.path().join("denied")),
            hidden: Mutex::new(vec![]),
            substituted: Mutex::new(vec![]),
            drop_writes: AtomicBool::new(true),
            fragment_puts: AtomicUsize::new(0),
        });
        let unpublished = compile(denied.clone(), Verify::On);
        assert!(denied.fragment_puts.load(Ordering::Relaxed) > 0);
        assert_eq!(cold.objects, unpublished.objects);
        for (key, _) in &unpublished.products {
            assert!(
                denied.lookup_product(key.address()).is_none(),
                "failed durable write published an association"
            );
        }
        // Verification off still uses independent native C, never an object
        // byte association as source or native validation authority.
        let unverified = compile(denied.clone(), Verify::Off);
        assert_eq!(cold.objects, unverified.objects);
    });
}
