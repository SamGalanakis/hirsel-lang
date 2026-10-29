use hirsel_driver::*;
use hirsel_engine::*;
use std::sync::Arc;
fn signature_session(
    store: Arc<dyn store::ResultStore>,
    source: &str,
    normalize: bool,
) -> QueryEngine {
    let mut adapters = compiler_adapters();
    if !normalize {
        adapters.signature = None;
    }
    let mut engine = QueryEngine::new(default_environment(), adapters).with_result_store(store);
    if !normalize {
        engine
            .sources
            .add("earlier.hir", "fn unrelated(){}\n".into())
            .unwrap();
    }
    let file = engine.sources.add("signatures.hir", source.into()).unwrap();
    assert!(
        engine
            .parse_file(file, &default_context())
            .unwrap()
            .diagnostics
            .is_empty()
    );
    engine
}
fn definition(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}
#[test]
fn staged_lower_products_restore_own_declarations_before_admission_without_modes() {
    with_compiler_stack(|| {
        use crate::staged_product_fixture::StagedProducts;
        let source = "pub opaque type Hidden:plain uses {}{field:u32}\nfn value[n:size]()->u64{comptime (n*n)}\n";
        let path = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let mut seed_products = StagedProducts::capture(&mut seed, 1).unwrap();
        let request = seed_products
            .runtime
            .request(&[seed_products.lower.query(&path).unwrap()]);
        seed_products.runtime.wait(request).unwrap();
        let expected = seed_products.lower.settled_definition(&path).unwrap();
        let declaration = expected
            .bodies
            .iter()
            .flat_map(|body| &body.rvalues)
            .find_map(|value| match value {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        let signature = seed.item_signature(&declaration).unwrap().value.fingerprint;
        seed_products.runtime.shutdown();
        drop(seed_products);
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        let mut products = StagedProducts::capture(&mut fresh, 1).unwrap();
        let query = products.lower.query(&path).unwrap();
        let request = products.runtime.request(&[query]);
        products.runtime.wait(request).unwrap();
        let actual = products.lower.settled_definition(&path).unwrap();
        assert_eq!(actual.path, expected.path);
        assert_eq!(
            fresh
                .item_signature(&declaration)
                .unwrap()
                .value
                .fingerprint,
            signature
        );
        assert_eq!(products.lower.work(&path).lowerings, 0);
        assert!(
            !products
                .front
                .work_snapshot()
                .iter()
                .any(|(p, kind, count)| p == &path && *kind == QueryKind::Modes && *count != 0)
        );
        products.runtime.shutdown();
        drop(products);
        drop(fresh);
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        for (provider, text, normalize, message) in [
            (
                copied as Arc<dyn store::ResultStore>,
                source.to_owned(),
                false,
                "declaration bytes cannot transfer V completion",
            ),
            (
                store.clone(),
                source.replace("field:u32", "field:u64"),
                true,
                "private V universe changes invalidate completion",
            ),
        ] {
            let mut invalid = signature_session(provider, &text, normalize);
            invalid.adapters.modes = None;
            invalid.adapters.lowering = None;
            let accepted = match StagedProducts::capture(&mut invalid, 1) {
                Err(_) => false,
                Ok(mut products) => {
                    let request = products
                        .runtime
                        .request(&[products.lower.query(&path).unwrap()]);
                    let accepted = products.runtime.wait(request).is_ok();
                    products.runtime.shutdown();
                    accepted
                }
            };
            assert!(!accepted, "{message}");
        }
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        let mut products = StagedProducts::capture(&mut oracle, 1).unwrap();
        let request = products
            .runtime
            .request(&[products.lower.query(&path).unwrap()]);
        products.runtime.wait(request).unwrap();
        products.runtime.shutdown();
    });
}

#[test]
fn lower_hits_replay_exact_staged_values_and_preserve_completion_authority() {
    with_compiler_stack(|| {
        use crate::staged_product_fixture::StagedProducts;
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\nfn value()->u64{comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}}\n";
        let root = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let mut seed_products = StagedProducts::capture(&mut seed, 1).unwrap();
        let request = seed_products.runtime.request(&[
            seed_products.lower.query(&definition("scaled")).unwrap(),
            seed_products.lower.query(&root).unwrap(),
        ]);
        seed_products.runtime.wait(request).unwrap();
        let generic = seed_products
            .lower
            .settled_definition(&definition("scaled"))
            .unwrap();
        let hidden = generic.bodies[generic.entry.0 as usize]
            .rvalues
            .iter()
            .find_map(|r| match r {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        let expected = seed_products.lower.settled_definition(&root).unwrap();
        assert!(seed_products.budget.reserved() > 0);
        seed_products.runtime.shutdown();
        drop(seed_products);
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        let mut products = StagedProducts::capture(&mut fresh, 1).unwrap();
        let query = products.lower.query(&root).unwrap();
        for _ in 0..2 {
            let request = products.runtime.request(&[query]);
            products.runtime.wait(request).unwrap();
            let actual = products.lower.settled_definition(&root).unwrap();
            assert_eq!(actual.path, expected.path);
            assert_eq!(
                products.lower.work(&root).lowerings,
                0,
                "resident Lower replays the same ledger"
            );
            assert!(
                !products
                    .front
                    .work_snapshot()
                    .iter()
                    .any(|(p, kind, count)| p == &root && *kind == QueryKind::Modes && *count != 0)
            );
            assert_eq!(
                products
                    .lower
                    .settled_constant(&hidden, &[2])
                    .unwrap()
                    .root
                    .kind,
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4))
            );
            assert!(
                products.lower.settled_constant(&hidden, &[3]).is_err(),
                "unselected size stays absent"
            );
        }
        products.runtime.shutdown();
        drop(products);
        drop(fresh);
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, false);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        let accepted = match StagedProducts::capture(&mut remote, 1) {
            Err(_) => false,
            Ok(mut products) => {
                let request = products
                    .runtime
                    .request(&[products.lower.query(&root).unwrap()]);
                let accepted = products.runtime.wait(request).is_ok();
                products.runtime.shutdown();
                accepted
            }
        };
        assert!(!accepted, "producer ledger cannot transfer V completion");
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        let mut products = StagedProducts::capture(&mut oracle, 1).unwrap();
        let request = products
            .runtime
            .request(&[products.lower.query(&root).unwrap()]);
        products.runtime.wait(request).unwrap();
        products.runtime.shutdown();
    });
}
