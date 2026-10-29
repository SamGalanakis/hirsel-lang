//! Exact settled front descriptors for native source tasks. Query demand and
//! V/native C admission remain with their respective producers.
use crate::native::queries::source_inputs::{NativeFrontLower, NativeLowerProducts};
use hirsel_engine::{
    FrontLowerQueries, NativeOriginalContext, NativeOriginalProduct, runtime::ProductRef,
};
use hirsel_syntax::DefPath;
use std::{collections::BTreeMap, sync::Arc};

pub(super) fn inputs(
    lower: Arc<FrontLowerQueries>,
    context: Arc<NativeOriginalContext>,
    definitions: &[DefPath],
) -> Result<NativeFrontLower, String> {
    let mut bindings = BTreeMap::new();
    for path in definitions {
        let query = lower
            .query(path)
            .ok_or_else(|| format!("native source has no admitted Lower producer: {path}"))?;
        if bindings.insert(path.clone(), query).is_some() {
            return Err(format!("native source repeats a Lower binding: {path}"));
        }
    }
    Ok(NativeFrontLower {
        context,
        bindings,
        products: lower,
    })
}
impl NativeLowerProducts for FrontLowerQueries {
    fn lower_product(
        &self,
        definition: &DefPath,
        selected: ProductRef,
    ) -> Result<NativeOriginalProduct, String> {
        self.native_product(selected)
            .filter(|product| &product.definition == definition)
            .ok_or_else(|| {
                format!("settled native source differs from selected Lower: {definition}")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_engine::{
        runtime::*,
        store::{Domain, MemStore, Reference, ResultStore},
        *,
    };
    use hirsel_intern::jobs::Jobs;

    #[derive(Default)]
    struct EvictableVerifier {
        contents: MemStore,
        rows: std::sync::Mutex<BTreeMap<hirsel_syntax::ContentHash, Reference>>,
        missing: std::sync::Mutex<Option<Reference>>,
    }
    impl store::Contents for EvictableVerifier {
        fn get_content(&self, reference: Reference) -> Option<store::Bytes> {
            if *self.missing.lock().unwrap() == Some(reference) {
                return None;
            }
            self.contents.get_content(reference)
        }
        fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
            self.contents.put_content(domain, bytes)
        }
    }
    impl hirsel_validate::VerifierRows for EvictableVerifier {
        fn row(&self, key: hirsel_syntax::ContentHash) -> Option<Reference> {
            self.rows.lock().unwrap().get(&key).copied()
        }
        fn set_row(&self, key: hirsel_syntax::ContentHash, reference: Reference) {
            self.rows.lock().unwrap().insert(key, reference);
        }
    }
    struct SelectedStore {
        products: MemStore,
        verifier: Arc<hirsel_validate::VerifierCache>,
    }
    impl ResultStore for SelectedStore {
        fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
            self.products.get(key)
        }
        fn put(&self, key: &store::EntryKey, bytes: &[u8]) {
            self.products.put(key, bytes);
        }
        fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
            Some(self.verifier.clone())
        }
        fn name(&self) -> &'static str {
            "selected-native-source-test"
        }
    }
    #[test]
    fn native_lower_bridge_requires_the_exact_settled_product_without_running_a_controller() {
        with_compiler_stack(|| {
            let verifier = Arc::new(EvictableVerifier::default());
            let store: Arc<dyn ResultStore> = Arc::new(SelectedStore {
                products: MemStore::default(),
                verifier: Arc::new(hirsel_validate::VerifierCache::hosted(verifier.clone())),
            });
            let mut engine = QueryEngine::new(default_environment(), crate::compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add(
                    "native-front.hir",
                    "fn other()->u8{2u8}\npub fn main()->u8{1u8}".into(),
                )
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let paths = engine.indexed_definitions();
            let main = paths.iter().find(|p| p.0.name == "main").unwrap().clone();
            let other = paths.iter().find(|p| p.0.name == "other").unwrap().clone();
            let context = Arc::new(engine.native_original_context(&paths).unwrap());
            let failure = store
                .put_content(
                    Domain {
                        kind: 0x52000001,
                        schema: 1,
                    },
                    b"bridge refusal",
                )
                .unwrap();
            let diagnostic = |text: &[u8]| {
                store
                    .put_content(
                        Domain {
                            kind: 0x52000001,
                            schema: 1,
                        },
                        text,
                    )
                    .unwrap()
            };
            let errors = RuntimeErrors {
                resource: diagnostic(b"resource"),
                cycle: diagnostic(b"cycle"),
                cancelled: diagnostic(b"cancelled"),
                protocol: diagnostic(b"protocol"),
                panicked: diagnostic(b"panicked"),
            };
            let budget = ProcessBudget::new(256 << 20);
            let weight = MemoryWeight {
                retained: 1 << 20,
                scratch: 1 << 20,
                stack: 0,
            };
            let front = engine
                .front_body_queries(&paths, weight, budget.reserve(8 << 20).unwrap(), errors)
                .unwrap();
            let lower = engine
                .front_lower_queries(
                    &paths,
                    front.clone(),
                    weight,
                    paths.len().checked_mul(3).unwrap().min(4096),
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let bridge = inputs(lower.clone(), context, &paths).unwrap();
            assert!(
                inputs(
                    lower.clone(),
                    bridge.context.clone(),
                    &[main.clone(), main.clone()]
                )
                .is_err()
            );
            let query = bridge.bindings[&main];
            let absent = ProductRef {
                address: query.address,
                action: hirsel_artifact_store::Action(query.address.0),
                record: failure,
            };
            assert!(bridge.products.lower_product(&main, absent).is_err());
            drop(engine);
            let mut runtime =
                AdmittedRuntime::new(Jobs::ONE, budget.reserve(192 << 20).unwrap(), errors)
                    .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            let request = runtime.request(&[query]);
            runtime.completion().wait(request);
            let completed = match runtime.poll(request) {
                std::task::Poll::Ready(Ok(completed)) => completed,
                std::task::Poll::Ready(Err(reference)) => panic!(
                    "front Lower failed: lower={:?}, front={:?}, record={:?}, stats={:?}",
                    lower.failure(reference),
                    front
                        .outcome(reference)
                        .map(|output| output.outcome.result.err()),
                    store
                        .get_content(reference)
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                    runtime.stats(),
                ),
                std::task::Poll::Pending => {
                    panic!("front Lower completion returned before settlement")
                }
            };
            let selected = completed[0].clone();
            let polls = runtime.stats().polls;
            let product = bridge
                .products
                .lower_product(&main, selected.clone())
                .unwrap();
            assert!(
                bridge
                    .products
                    .lower_product(&other, selected.clone())
                    .is_err()
            );
            let mut crossed = selected;
            crossed.record = failure;
            assert!(bridge.products.lower_product(&main, crossed).is_err());
            let issued = product.verified.clone().expect("actual completed source V");
            let reader = bridge
                .context
                .reader([product.clone()], store.clone())
                .unwrap();
            assert!(reader.read(&main).is_some());
            *verifier.missing.lock().unwrap() = Some(issued.reference());
            // The host evicts only V's replay bytes; the admitted ordinary
            // Lower descriptor still names the exact original source payload.
            let restored = reader
                .read(&main)
                .expect("ordinary original survives V eviction");
            assert_eq!(restored.output(), Some(&issued));
            assert!(!restored.verification_ran());
            let mut changed = restored.definition().clone();
            Arc::make_mut(&mut changed).entry = hirsel_tir::BodyId(u32::MAX);
            let records = NativeRecords::new(
                reader.record_context().record_types().clone(),
                store.clone(),
            );
            let bytes = records
                .encode_tir(reader.record_context().as_ref(), &changed)
                .unwrap();
            let mut crossed_body = product;
            crossed_body.body = store.put_content(NATIVE_ORIGINAL_BODY, &bytes).unwrap();
            let crossed_reader = bridge.context.reader([crossed_body], store).unwrap();
            assert!(
                crossed_reader.read(&main).is_none(),
                "a V output cannot rebind to changed original bytes"
            );
            assert!(reader.input(&main).is_some());
            assert!(reader.input(&other).is_none());
            assert!(Arc::ptr_eq(reader.signatures(), &lower.signatures()));
            assert_eq!(
                runtime.stats().polls,
                polls,
                "descriptor binding executes no producer"
            );
        });
    }
}
