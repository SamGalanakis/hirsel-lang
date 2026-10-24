use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use native::queries::{
    declarations::NativeDeclarations, demand::DemandInstances, source_inputs::NativeInstanceInputs,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

struct ResultGraph {
    products: BTreeMap<Vec<u8>, native::queries::ProductRef>,
    imports: u64,
    decodes: u64,
    body_demands: u64,
    values: hirsel_lir::construction::ValueCounts,
    owners: BTreeSet<String>,
}
fn run(source: &str, store: Arc<store::DirStore>, frozen: bool, fresh: bool) -> ResultGraph {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add("worker-demand.hir", source.into())
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    if fresh {
        engine.adapters.lowering = None;
    }
    let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
    let mut declarations = NativeDeclarations::collect(&mut engine, &mut cache, &[]).unwrap();
    if fresh {
        assert_eq!(cache.counts.body_demands, 0);
    }
    // Source production is explicitly completed before transporting its V-owned
    // envelopes. This control covers cold *native* work, not cold source tasks.
    let mut originals = Vec::new();
    if !fresh {
        for path in declarations
            .context
            .sources
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            let body = declarations.body(&mut engine, &path).unwrap();
            originals.push(Arc::downgrade(&body));
        }
    }
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let options = native::Options::new(target.clone());
    let policy = ContentHash::of_bytes(b"immutable demand source control");
    let trace = hirsel_lir::construction::ValueTrace::default();
    let _observe = trace.enter();
    let mut graph;
    let mut pending;
    let mut provider = None;
    let mut live = Some((engine, declarations));
    if frozen {
        let (mut engine, declarations) = live.take().unwrap();
        let inputs = NativeInstanceInputs::capture(&mut engine, declarations)
            .unwrap()
            .unwrap();
        fn send_sync<T: Send + Sync>(_: &T) {}
        send_sync(&inputs);
        drop(engine);
        assert!(
            originals.iter().all(|body| body.upgrade().is_none()),
            "worker transport must retain no original Definition"
        );
        let source = inputs.provider();
        graph = DemandInstances::new_from_source(&source, store, target, policy);
        pending = graph.roots_from(&source).unwrap();
        provider = Some(source);
    } else {
        let (engine, declarations) = live.as_ref().unwrap();
        graph = DemandInstances::new(engine, store, target, policy);
        pending = graph.roots(engine, declarations).unwrap();
    }
    assert_eq!(trace.totals(), Default::default());
    let mut products = BTreeMap::new();
    let mut seen = BTreeSet::new();
    while let Some(key) = pending.pop() {
        if !seen.insert(key.instance.encoding().to_vec()) {
            continue;
        }
        let output = if let Some(source) = provider.as_mut() {
            graph.instance_from(source, &key, &options).unwrap()
        } else {
            let (engine, declarations) = live.as_mut().unwrap();
            graph
                .instance(engine, declarations, &key, &options)
                .unwrap()
        };
        pending.extend(output.dependencies.iter().cloned());
        products.insert(key.instance.encoding().to_vec(), output.product.product);
    }
    ResultGraph {
        products,
        imports: graph.imports,
        decodes: graph.recipe_decodes,
        body_demands: provider.map_or(0, |source| source.body_demands),
        values: trace.totals(),
        owners: trace
            .snapshot()
            .keys()
            .map(|(path, _)| path.0.name.to_string())
            .collect(),
    }
}

#[test]
fn frozen_demand_source_preserves_live_identity_and_imports_only_the_changed_owner() {
    with_compiler_stack(|| {
        const SOURCE: &str = "resource type R[T:copyable] {n:T} drop(x) {let _=x.n}\nfn outer[T](take x:T)->T {let f:once fn()->T=fn()->T{take x};f()}\nfn unrelated()->u64{13u64}\npub fn main()->u64 {let r=R(n:1u64);outer(take 7u64)}";
        let directory = tempfile::tempdir().unwrap();
        let live = Arc::new(store::DirStore::new(directory.path().join("live")));
        let frozen = Arc::new(store::DirStore::new(directory.path().join("frozen")));
        let baseline = run(SOURCE, live, false, false);
        let cold = run(SOURCE, frozen.clone(), true, false);
        assert_eq!(
            cold.products, baseline.products,
            "transport changes no action or output identity"
        );
        assert!(
            cold.imports >= 5,
            "source, generic, closure and finalizer imports"
        );
        assert_eq!(cold.imports, cold.decodes);
        assert!(cold.body_demands > 0);
        let fresh = run(SOURCE, frozen.clone(), true, true);
        assert_eq!(fresh.products, cold.products);
        assert_eq!(
            (fresh.imports, fresh.decodes, fresh.body_demands),
            (0, 0, 0)
        );
        assert_eq!(fresh.values, Default::default());
        let edit = run(
            &SOURCE.replace("13u64", "14u64"),
            frozen.clone(),
            true,
            false,
        );
        assert_eq!((edit.imports, edit.decodes, edit.body_demands), (1, 1, 1));
        let changed = cold
            .products
            .iter()
            .filter(|(key, old)| edit.products[*key] != **old)
            .count();
        assert_eq!(changed, 1);
        assert_eq!(edit.owners, BTreeSet::from(["unrelated".into()]));
        let fresh = run(&SOURCE.replace("13u64", "14u64"), frozen, true, true);
        assert_eq!(fresh.products, edit.products);
        assert_eq!(
            (fresh.imports, fresh.decodes, fresh.body_demands),
            (0, 0, 0)
        );
        assert_eq!(fresh.values, Default::default());
    });
}

#[test]
fn frozen_demand_source_restores_actual_library_environment_without_an_engine() {
    with_compiler_stack(|| {
        const SOURCE: &str = "fn step(mut n:u64)->Option[u64] {if n==2u64 {None} else {let x=n;n+=1u64;Some(x)}}\npub fn main()->u64 {let state=0u64;let mut it=iter.from(take state,step);let _=iter.next(mut it);0u64}";
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let cold = run(SOURCE, store.clone(), true, false);
        assert!(cold.imports > 2);
        assert!(cold.owners.contains("iter_from_step"));
        let fresh = run(SOURCE, store, true, true);
        assert_eq!(fresh.products, cold.products);
        assert_eq!(
            (fresh.imports, fresh.decodes, fresh.body_demands),
            (0, 0, 0)
        );
        assert_eq!(fresh.values, Default::default());
    });
}
