use hirsel_driver::*;
use hirsel_engine::*;
use native::queries::{declarations::NativeDeclarations, source_inputs::*};
use std::sync::Arc;

#[test]
fn immutable_native_inputs_bind_only_the_selected_front_output_without_eager_bodies() {
    with_compiler_stack(|| {
        let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "worker-source.hir",
                "fn helper()->u64{9u64}\npub fn main()->u64{7u64}".into(),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let main = entry_path(&default_context());
        let mut helper = main.clone();
        helper.0.name = "helper".into();
        assert_eq!(engine.recompute_count(&main, QueryKind::Lower), 0);
        let facts = crate::native_front_fixture::products(&mut engine, &[]).unwrap();
        let context = facts.original_context.clone();
        let selected = facts
            .originals
            .iter()
            .find(|product| product.definition == main)
            .unwrap()
            .as_ref()
            .clone();
        let descriptor = facts
            .catalogue
            .iter()
            .find(|product| product.source.definition == main)
            .unwrap()
            .source
            .clone();
        let reader = context.reader([selected.clone()]).unwrap();
        let body = reader.read(&main).unwrap().definition().clone();
        let original = Arc::downgrade(&body);
        drop(reader);
        let declarations = NativeDeclarations::from_front(facts.clone()).unwrap();
        for product in &facts.catalogue {
            assert!(Arc::ptr_eq(
                &product.source,
                &declarations.sources[&product.source.definition],
            ));
        }
        let complete = context
            .reader(facts.originals.iter().map(|product| (**product).clone()))
            .unwrap();
        let shared = NativeInstanceInputs::from_products(
            NativeDeclarations::from_front(facts.clone()).unwrap(),
            complete,
        )
        .unwrap();
        for product in &facts.catalogue {
            assert!(Arc::ptr_eq(
                &product.source,
                &shared.declarations.sources[&product.source.definition],
            ));
        }
        let empty = context
            .reader(std::iter::empty::<NativeOriginalProduct>())
            .unwrap();
        let inputs = NativeInstanceInputs::from_products(declarations, empty).unwrap();
        let mut unavailable = inputs.provider();
        assert!(unavailable.source_output(&main).is_err());
        assert!(unavailable.original_source(&main).is_err());
        let mut wrong = selected.clone();
        wrong.signature = Default::default();
        assert!(inputs.provider_from_products(&context, [wrong]).is_err());
        let mut pinned = inputs
            .provider_from_products(&context, [selected.clone()])
            .unwrap();
        assert_eq!(pinned.body_demands, 0);
        assert_eq!(pinned.source_verifications, 0);
        assert!(pinned.source_observation(&helper).is_err());
        assert!(pinned.source_output(&main).unwrap().is_some());
        assert!(
            inputs.provider().source_output(&main).is_err(),
            "worker binding cannot change the shared declaration snapshot"
        );
        let mut ordinary = selected;
        ordinary.verified = None;
        let mut ordinary = inputs.provider_from_products(&context, [ordinary]).unwrap();
        assert!(ordinary.source_output(&main).unwrap().is_none());
        assert!(!ordinary.query_policy().reuse);
        drop(body);
        drop(facts);
        drop(engine);
        assert!(Arc::ptr_eq(
            &descriptor,
            &shared.declarations.sources[&main]
        ));
        assert!(
            original.upgrade().is_some(),
            "immutable worker inputs retain the exact shared original Definition"
        );
        assert_eq!(
            pinned.original_source(&main).unwrap().0.signature_hash,
            descriptor.signature.fingerprint
        );
        assert_eq!(pinned.body_demands, 1);
        assert_eq!(pinned.source_verifications, 0);
        let original = ordinary.original_source(&main).unwrap();
        assert!(
            original.1.is_none(),
            "ordinary transport cannot create a source witness"
        );
        assert_eq!(ordinary.body_demands, 1);
        assert!(ordinary.source_verifications <= 1);
    });
}
