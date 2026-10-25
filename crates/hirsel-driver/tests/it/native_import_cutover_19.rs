//! Import-cache vectors exercised through the demand producer before the old
//! whole-program replay provider is removed.
use hirsel_driver::*;
use hirsel_engine::{store::ResultStore, *};
use native::queries::{
    declarations::NativeDeclarations, demand::DemandInstances, source_inputs::*,
};
use std::{collections::BTreeMap, sync::Arc};

type Bodies = BTreeMap<Vec<u8>, (Vec<u8>, Vec<u8>)>;

fn import(source: &str, store: Arc<dyn ResultStore>) -> (Bodies, u64, u64) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine.sources.add("imports.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
    let declarations = NativeDeclarations::collect(&mut engine, &mut discovery, &[]).unwrap();
    let paths = declarations
        .context
        .sources
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let context = engine.native_original_context(&paths).unwrap();
    let records = NativeRecords::new(engine.types().clone(), store.clone());
    let mut originals = Vec::new();
    for path in &paths {
        let source = &declarations.sources[path];
        let (body, verified) = engine.native_definition_body_with_output(source).unwrap();
        let bytes = records.encode(&engine, &body).unwrap();
        originals.push(NativeOriginalProduct {
            definition: path.clone(),
            implementation_revision: source.source.implementation_revision,
            signature: source.signature.fingerprint,
            body: store.put_content(NATIVE_ORIGINAL_BODY, &bytes).unwrap(),
            verified,
        });
    }
    let reader = context.reader(originals, store.clone()).unwrap();
    let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
    drop(discovery);
    drop(engine);
    let mut source = inputs.provider();
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let policy = hirsel_syntax::ContentHash::of_bytes(b"native import cutover control");
    let mut producer =
        DemandInstances::new_from_source(&source, store.clone(), target.clone(), policy);
    let mut pending = producer.roots_from(&source).unwrap();
    let mut bodies = BTreeMap::new();
    let mut options = native::Options::new(target);
    options.validation = native::Validation::Ready;
    while let Some(key) = pending.pop() {
        if bodies.contains_key(key.instance.encoding()) {
            continue;
        }
        let output = producer.instance_from(&mut source, &key, &options).unwrap();
        let value = store.get_content(output.product.value).unwrap().to_vec();
        let layouts = store
            .get_content(output.product.layout_closure)
            .unwrap()
            .to_vec();
        pending.extend(output.dependencies.iter().cloned());
        bodies.insert(key.instance.encoding().to_vec(), (value, layouts));
    }
    assert!(!bodies.is_empty());
    (bodies, producer.imports, producer.counts().hits)
}

fn fresh_import(source: &str) -> (Bodies, u64, u64) {
    let dir = tempfile::tempdir().unwrap();
    import(
        source,
        Arc::new(store::DirStore::new(dir.path().join("products"))),
    )
}

#[test]
fn demand_imported_closure_symbols_match_fresh_after_layout_numbering_changes() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(dir.path().join("products")));
        let source = "type Captured { n:u64 }\nfn compute(x:Captured)->u64 { let f=fn()->u64 { x.n }; f() }\nfn main()->u64 { compute(Captured(n:3u64)) }\n";
        let donor = format!("fn aaa()->(bool,u32) {{ (true,0u32) }}\n{source}");
        assert!(import(&donor, store.clone()).1 > 0);
        let cached = import(source, store);
        let fresh = fresh_import(source);
        assert!(
            cached.2 > 0,
            "the distinct consumer must exercise stored imports"
        );
        assert_eq!(
            cached.0, fresh.0,
            "canonical symbols, bodies and reached layouts must match fresh imports"
        );
    });
}

#[test]
fn demand_imports_replay_across_sessions_unrelated_layouts_and_body_edits() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(dir.path().join("products")));
        let source = "fn helper(x: u64) -> u64 { x + 1u64 }\nfn main() -> u64 { helper(3u64) }\n";
        let cold = import(source, store.clone());
        assert_eq!(cold.2, 0);
        assert_eq!(cold.1, cold.0.len() as u64);
        let warm = import(source, store.clone());
        assert_eq!(warm.0, cold.0);
        assert_eq!(warm.1, 0);
        assert_eq!(warm.2, cold.0.len() as u64);
        let unrelated = format!(
            "type Other {{ text: String }}\nfn unrelated() -> Other {{ Other(text: \"x\") }}\n{source}"
        );
        let shifted = import(&unrelated, store.clone());
        for (key, body) in &cold.0 {
            assert_eq!(
                shifted.0.get(key),
                Some(body),
                "unrelated layouts changed an existing import"
            );
        }
        assert!(shifted.2 >= cold.0.len() as u64);
        let edited = unrelated.replace("x + 1u64", "x + 2u64");
        let edit = import(&edited, store.clone());
        assert_eq!(edit.1, 1, "only the helper's body changed");
        assert_eq!(edit.0.len(), shifted.0.len());
        assert_eq!(edit.0, fresh_import(&edited).0);
        let lists = "fn main() -> u64 { let mut xs: List[u64] = []; list.push(mut xs, 1u64); list.len(xs) }";
        let list_cold = import(lists, store.clone());
        assert!(list_cold.1 > 0);
        let list_warm = import(lists, store);
        assert_eq!(list_warm.0, list_cold.0);
        assert_eq!(list_warm.1, 0);
        assert_eq!(list_warm.2, list_warm.0.len() as u64);
    });
}
