use hirsel_driver::*;
use hirsel_engine::*;
use std::sync::Arc;

#[test]
fn settled_lower_binds_without_eager_bodies_and_independently_refuses_corruption() {
    with_compiler_stack(|| {
        let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "settled.hir",
                "fn helper()->u64 {9u64}\npub fn main()->u64 {7u64}".into(),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let main = entry_path(&default_context());
        let mut helper = main.clone();
        helper.0.name = "helper".into();
        let context = engine
            .native_original_context(&[main.clone(), helper.clone()])
            .unwrap();
        assert_eq!(engine.recompute_count(&main, QueryKind::Lower), 0);
        assert_eq!(engine.recompute_count(&helper, QueryKind::Lower), 0);
        let product = engine.native_definition_product(&main).unwrap();
        let (body, verified) = engine.native_definition_body_with_output(&product).unwrap();
        let records = NativeRecords::new(engine.types().clone(), store.clone());
        let publish = |body: &hirsel_tir::DefinitionRef| {
            let bytes = records.encode_tir(&engine, body).unwrap();
            store.put_content(NATIVE_ORIGINAL_BODY, &bytes).unwrap()
        };
        let ordinary = publish(&body);
        let input = |revision, signature, verified| NativeOriginalProduct {
            definition: main.clone(),
            implementation_revision: revision,
            signature,
            body: ordinary,
            verified,
        };
        let revision = product.source.implementation_revision;
        let signature = product.signature.fingerprint;
        let reader = context
            .reader([input(revision, signature, None)], store.clone())
            .unwrap();
        assert!(reader.input(&helper).is_none());
        assert!(reader.has_ordinary());
        assert!(!reader.query_policy().reuse);
        assert_eq!(engine.recompute_count(&helper, QueryKind::Lower), 0);
        assert!(
            context
                .reader([input(Default::default(), signature, None)], store.clone())
                .is_err()
        );
        assert!(
            context
                .reader([input(revision, Default::default(), None)], store.clone())
                .is_err()
        );
        assert!(
            context
                .reader(
                    [
                        input(revision, signature, None),
                        input(revision, signature, None)
                    ],
                    store.clone()
                )
                .is_err()
        );
        let mut invalid = body.as_ref().clone();
        invalid.bodies[invalid.entry.0 as usize].entry.0 = u32::MAX;
        let invalid = publish(&Arc::new(invalid));
        let mut corrupt = input(revision, signature, None);
        corrupt.body = invalid;
        let corrupt = context.reader([corrupt], store.clone()).unwrap();
        assert!(corrupt.read(&main).is_none());
        let verified = context
            .reader([input(revision, signature, verified)], store.clone())
            .unwrap();
        let expected = body.signature_hash;
        let archived = reader.archive(&main).expect("selected indexed source rows");
        assert_eq!(archived.reference(), ordinary);
        assert_eq!(archived.header().unwrap().signature_hash, expected);
        let entry = body.entry.0;
        let block = body.bodies[entry as usize].entry.0;
        let instruction =
            body.bodies[entry as usize].blocks[block as usize].instructions[0].clone();
        drop(body);
        drop(engine);
        assert_eq!(
            archived
                .body(entry)
                .unwrap()
                .block(block)
                .unwrap()
                .instruction(0),
            Some(instruction)
        );
        assert!(archived.body(archived.body_count()).is_none());
        assert_eq!(
            reader.read(&main).unwrap().definition().signature_hash,
            expected
        );
        assert_eq!(
            verified.read(&main).unwrap().definition().signature_hash,
            expected
        );
        assert!(reader.read(&helper).is_none());
    });
}
