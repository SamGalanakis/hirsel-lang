use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use native::queries::{
    NativeProducts,
    arena::{ArenaQueryCounts, EquationProduct},
    declarations::NativeDeclarations,
    demand::DemandInstances,
    graph::{ArenaNode, settle_arena_queries},
    products::{DemandProducts, EmissionOptions, NativeSnapshot},
    ready::ReadyQueries,
};
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn owned_snapshot_shares_only_actual_c_acceptance_between_task_local_views() {
    with_compiler_stack(|| {
        use hirsel_engine::store::ResultStore;
        let store = Arc::new(store::MemStore::default());
        assert!(store.local_product_store().is_none());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add("owned-products.hir", "pub fn main()->u64 {7u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut source_queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let mut declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        let main = entry_path(&default_context());
        let original = declarations.body(&mut engine, &main).unwrap();
        let weak = Arc::downgrade(&original);
        drop(original);
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let mut instances = DemandInstances::new(
            &engine,
            store.clone(),
            target.clone(),
            ContentHash::of_bytes(b"owned native snapshot"),
        );
        let mut pending = instances.roots(&engine, &declarations).unwrap();
        let mut seen = BTreeSet::new();
        let mut nodes = Vec::new();
        while let Some(key) = pending.pop() {
            if !seen.insert(key.instance.encoding().to_vec()) {
                continue;
            }
            let product = instances
                .instance(&mut engine, &mut declarations, &key, &options)
                .unwrap();
            pending.extend(product.dependencies.iter().cloned());
            nodes.push(ArenaNode {
                equations: EquationProduct {
                    owner: key,
                    reference: product.product.local_equations,
                },
                callees: product.callees.clone(),
            });
        }
        nodes.sort_by(|a, b| a.equations.owner.canonical_cmp(&b.equations.owner));
        let key = nodes
            .iter()
            .find(|n| n.equations.owner.instance.definition() == &main)
            .unwrap()
            .equations
            .owner
            .clone();
        let arena = settle_arena_queries(
            store.as_ref(),
            &nodes,
            &[],
            0,
            &mut ArenaQueryCounts::default(),
        )
        .unwrap();
        let operations = Arc::new(declarations.program.operation_registry());
        let manifest = hirsel_core_contract::Manifest::skeleton(
            declarations.program.operations.values().cloned(),
        );
        let emission = EmissionOptions {
            input: declarations.program.input.clone(),
            opt: hirsel_lir::emission::OptLevel::None,
            debug: true,
            callbacks: false,
            tracking: false,
            foreign: Default::default(),
            inline_instances: Default::default(),
            verify_backend: true,
            measurement: false,
            fuel: false,
        };
        let snapshot = NativeSnapshot::new(
            store.clone(),
            instances,
            arena,
            options.clone(),
            emission,
            manifest.clone(),
            operations.clone(),
        )
        .unwrap();
        fn send_sync<T: Send + Sync>(_: &T) {}
        send_sync(snapshot.as_ref());
        drop(declarations);
        drop(engine);
        assert!(
            weak.upgrade().is_none(),
            "owned input snapshot retains no original Definition"
        );
        let mut first = DemandProducts::new(snapshot.clone());
        let produced = first.ready(&key).unwrap();
        assert_eq!(first.local.queries.counts.transformations, 1);
        drop(first);
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let mut next = DemandProducts::new(snapshot);
        let hit = next.ready(&key).unwrap();
        assert_eq!(hit.product, produced.product);
        assert_eq!(next.local.queries.counts.transformations, 0);
        assert_eq!(next.local.queries.counts.hits, 1);
        assert_eq!(
            trace.totals(),
            hirsel_lir::construction::ValueCounts::default()
        );
        let environment = hirsel_lir::validate::NativeEnvironment {
            target: &target,
            operations: operations.as_ref(),
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        let mut unrelated = ReadyQueries::new(store);
        assert!(
            unrelated
                .lookup(&key, hit.product.action, &environment)
                .is_none(),
            "ordinary memory-store products do not confer the shared snapshot's private C acceptance"
        );
    });
}
