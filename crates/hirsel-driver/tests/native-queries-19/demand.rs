use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use native::queries::{declarations::NativeDeclarations, demand::DemandInstances};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[test]
fn actual_demand_instance_graph_hits_before_recipe_or_value_construction() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let source = "resource type R[T:copyable] {n:T} drop(x) {let _=x.n}\nfn outer[T](take x:T)->T {let f:once fn()->T=fn()->T{take x};f()}\nfn unrelated()->u64{13u64}\npub fn main()->u64 {let r=R(n:1u64);outer(take 7u64)}";
        let session = |source: &str| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("demand.hir", source.into()).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            engine
        };
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let policy = ContentHash::of_bytes(b"actual demand graph control");
        let run = |engine: &mut QueryEngine, disable_lowering: bool| {
            if disable_lowering {
                engine.adapters.lowering = None;
            }
            let mut source = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let mut declarations = NativeDeclarations::collect(engine, &mut source, &[]).unwrap();
            if disable_lowering {
                assert_eq!(source.counts.body_demands, 0);
            }
            assert!(declarations.program.definitions.is_empty());
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let mut graph = DemandInstances::new(engine, store.clone(), target.clone(), policy);
            let mut pending = graph.roots(engine, &declarations).unwrap();
            assert_eq!(
                trace.totals(),
                hirsel_lir::construction::ValueCounts::default()
            );
            let mut seen = BTreeSet::new();
            let mut results = BTreeMap::new();
            while let Some(key) = pending.pop() {
                if !seen.insert(key.instance.encoding().to_vec()) {
                    continue;
                }
                let output = graph
                    .instance(engine, &mut declarations, &key, &options)
                    .unwrap();
                assert_eq!(
                    output.product.product.address,
                    key.address(native::queries::INSTANCE_KIND)
                );
                pending.extend(output.dependencies.iter().cloned());
                results.insert(
                    key.instance.encoding().to_vec(),
                    output.product.product.record,
                );
            }
            (
                results,
                graph.imports,
                graph.recipe_decodes,
                trace.snapshot(),
                trace.totals(),
            )
        };
        let mut first = session(source);
        let (expected, imports, decodes, _, _) = run(&mut first, false);
        assert!(
            imports >= 5,
            "source, generic, closure and finalizer misses ran"
        );
        assert_eq!(imports, decodes);
        drop(first);
        let mut fresh = session(source);
        let (hits, imports, decodes, _, counts) = run(&mut fresh, true);
        assert_eq!(hits, expected);
        assert_eq!((imports, decodes), (0, 0));
        assert_eq!(counts, hirsel_lir::construction::ValueCounts::default());
        drop(fresh);
        let changed = source.replace("13u64", "14u64");
        let mut edit = session(&changed);
        let (edited, imports, decodes, owners, _) = run(&mut edit, false);
        assert_eq!((imports, decodes), (1, 1));
        let mut changed_outputs = 0;
        for (key, old) in expected {
            if edited[&key] != old {
                changed_outputs += 1;
            }
        }
        assert_eq!(changed_outputs, 1);
        assert!(owners.keys().all(|(path, _)| path.0.name == "unrelated"));
    });
}

#[test]
fn actual_library_graph_uses_admitted_environment_products_on_fresh_hits() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        const SOURCE: &str = "fn step(mut n:u64)->Option[u64] {if n==2u64 {None} else {let x=n;n+=1u64;Some(x)}}\npub fn main()->u64 {let state=0u64;let mut it=iter.from(take state,step);let _=iter.next(mut it);0u64}";
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let policy = ContentHash::of_bytes(b"library demand graph control");
        let mut expected = BTreeMap::new();
        for fresh in [false, true] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("library-demand.hir", SOURCE.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if fresh {
                engine.adapters.lowering = None;
            }
            let mut source = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let mut declarations =
                NativeDeclarations::collect(&mut engine, &mut source, &[]).unwrap();
            if fresh {
                assert_eq!(source.counts.body_demands, 0);
            }
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let mut graph = DemandInstances::new(&engine, store.clone(), target.clone(), policy);
            let mut pending = graph.roots(&engine, &declarations).unwrap();
            let mut seen = BTreeSet::new();
            let mut actual = BTreeMap::new();
            while let Some(key) = pending.pop() {
                if !seen.insert(key.instance.encoding().to_vec()) {
                    continue;
                }
                let output = graph
                    .instance(&mut engine, &mut declarations, &key, &options)
                    .unwrap();
                pending.extend(output.dependencies.iter().cloned());
                actual.insert(
                    key.instance.encoding().to_vec(),
                    output.product.product.record,
                );
            }
            if fresh {
                assert_eq!(actual, expected);
                assert_eq!((graph.imports, graph.recipe_decodes), (0, 0));
                assert_eq!(
                    trace.totals(),
                    hirsel_lir::construction::ValueCounts::default()
                );
            } else {
                assert!(
                    trace
                        .snapshot()
                        .keys()
                        .any(|(path, _)| path.0.name == "iter_from_step")
                );
                assert!(graph.imports > 2);
                expected = actual;
            }
        }
    });
}
