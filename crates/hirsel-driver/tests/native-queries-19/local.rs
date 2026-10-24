use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use native::queries::{
    arena::{ArenaQueryCounts, EquationProduct},
    declarations::NativeDeclarations,
    demand::DemandInstances,
    graph::{ArenaNode, settle_arena_queries},
    local::LocalReadyQueries,
    ready::EMISSION_CONTEXT_DOMAIN,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[test]
fn demand_ready_graph_cuts_off_before_unchanged_values_and_propagates_arena_edits() {
    with_compiler_stack(|| {
        const BEFORE: &str = "type Item { n: u64 }\nfn first(a: Arena, n: u64) -> (box Item) in a {\n  if n == 0u64 { box Item(n:6u64) }\n  else { second(a,n-1u64) }\n}\nfn second(a: Arena, n: u64) -> (box Item) in a { first(a,n) }\nfn wrapper(a: Arena) -> (box Item) in a { second(a,2u64) }\nfn main() { arena.with(fn(a: Arena) { let item = wrapper(a); }); }\n";
        const AFTER: &str = "type Item { n: u64 }\nfn first(a: Arena, n: u64) -> (box Item) in a {\n  if n == 0u64 { let h: (box Item) in heap = box Item(n:6u64); h }\n  else { second(a,n-1u64) }\n}\nfn second(a: Arena, n: u64) -> (box Item) in a { first(a,n) }\nfn wrapper(a: Arena) -> (box Item) in a { second(a,2u64) }\nfn main() { arena.with(fn(a: Arena) { let item = wrapper(a); }); }\n";
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let policy = ContentHash::of_bytes(b"actual local Ready graph control");
        use hirsel_engine::store::ResultStore;
        let emission = store.put_content(EMISSION_CONTEXT_DOMAIN, b"{}").unwrap();
        let stable_edit = BEFORE.replace("6u64", "7u64");
        let mut previous = BTreeMap::new();
        for (iteration, (source, fresh)) in [
            (BEFORE, false),
            (BEFORE, true),
            (stable_edit.as_str(), false),
            (AFTER, false),
            (AFTER, true),
        ]
        .into_iter()
        .enumerate()
        {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("local-demand.hir", source.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if fresh {
                engine.adapters.lowering = None;
            }
            let mut source_queries =
                NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let mut declarations =
                NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
            if fresh {
                assert_eq!(source_queries.counts.body_demands, 0);
            }
            assert!(declarations.program.definitions.is_empty());
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let mut instances =
                DemandInstances::new(&engine, store.clone(), target.clone(), policy);
            let mut pending = instances.roots(&engine, &declarations).unwrap();
            let mut seen = BTreeSet::new();
            let mut completed = Vec::new();
            while let Some(key) = pending.pop() {
                if !seen.insert(key.instance.encoding().to_vec()) {
                    continue;
                }
                let output = instances
                    .instance(&mut engine, &mut declarations, &key, &options)
                    .unwrap();
                pending.extend(output.dependencies.iter().cloned());
                completed.push((key, output));
            }
            completed.sort_by(|a, b| a.0.canonical_cmp(&b.0));
            let nodes = completed
                .iter()
                .map(|(key, output)| ArenaNode {
                    equations: EquationProduct {
                        owner: key.clone(),
                        reference: output.product.local_equations,
                    },
                    callees: output.callees.clone(),
                })
                .collect::<Vec<_>>();
            let mut arena_counts = ArenaQueryCounts::default();
            let arena =
                settle_arena_queries(store.as_ref(), &nodes, &[], 0, &mut arena_counts).unwrap();
            let operations = declarations.program.operation_registry();
            let manifest = hirsel_core_contract::Manifest::skeleton(
                declarations.program.operations.values().cloned(),
            );
            let environment = hirsel_lir::validate::NativeEnvironment {
                target: &target,
                operations: &operations,
                manifest: &manifest,
                discharge: Some(&options.discharge),
            };
            let mut ready = LocalReadyQueries::new(store.clone());
            let mut links = Vec::new();
            let mut actual = BTreeMap::new();
            for (key, instance) in &completed {
                let product = ready
                    .ready(
                        &instances,
                        key,
                        &arena,
                        &options,
                        emission,
                        true,
                        &environment,
                    )
                    .unwrap();
                links.push(
                    ready
                        .queries
                        .linkage(key, product.product.action, &environment)
                        .unwrap(),
                );
                actual.insert(
                    (key.instance.definition().clone(), key.instance.digest()),
                    (instance.product.product.record, product.product.record),
                );
            }
            hirsel_lir::validate::check_arena_scoped_links(&links).unwrap();
            if fresh {
                assert_eq!(actual, previous);
                assert_eq!(
                    (
                        instances.imports,
                        instances.recipe_decodes,
                        ready.queries.counts.transformations,
                        arena_counts.misses
                    ),
                    (0, 0, 0, 0)
                );
                assert_eq!(
                    trace.totals(),
                    hirsel_lir::construction::ValueCounts::default()
                );
            } else if iteration == 2 {
                assert_eq!(
                    (
                        instances.imports,
                        instances.recipe_decodes,
                        ready.queries.counts.transformations
                    ),
                    (1, 1, 1)
                );
                assert!(
                    trace
                        .snapshot()
                        .keys()
                        .all(|(path, _)| path.0.name == "first"),
                    "summary-preserving edit reconstructed an unchanged Value"
                );
                for (function, products) in &actual {
                    if function.0.0.name != "first" {
                        assert_eq!(previous[function], *products);
                    }
                }
            } else if iteration == 3 {
                assert_eq!((instances.imports, instances.recipe_decodes), (1, 1));
                assert!(ready.queries.counts.transformations >= 3);
                for (function, products) in &actual {
                    if ["second", "wrapper"].contains(&function.0.0.name.as_str()) {
                        assert_eq!(previous[function].0, products.0, "unchanged base Value");
                        assert_ne!(
                            previous[function].1, products.1,
                            "callee arena edit must invalidate Ready"
                        );
                    }
                }
            }
            previous = actual;
        }
    });
}

#[test]
fn current_materialized_static_is_read_before_an_unchanged_source_value_hit() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "static-demand.hir",
                "const SECOND: u64 = comptime 7u64\npub fn main()->u64 {SECOND}".into(),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let policy = ContentHash::of_bytes(b"static read independence");
        let mut source_queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let mut declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        let mut instances = DemandInstances::new(&engine, store.clone(), target.clone(), policy);
        let key = instances
            .roots(&engine, &declarations)
            .unwrap()
            .into_iter()
            .find(|key| key.instance.definition() == &entry_path(&default_context()))
            .unwrap();
        let original = instances
            .instance(&mut engine, &mut declarations, &key, &options)
            .unwrap();
        assert!(original.callees.is_empty());
        let settle = |instance: &native::queries::demand::DiscoveredInstance| {
            settle_arena_queries(
                store.as_ref(),
                &[ArenaNode {
                    equations: EquationProduct {
                        owner: key.clone(),
                        reference: instance.product.local_equations,
                    },
                    callees: instance.callees.clone(),
                }],
                &[],
                0,
                &mut ArenaQueryCounts::default(),
            )
            .unwrap()
        };
        use hirsel_engine::store::ResultStore;
        let emission = store.put_content(EMISSION_CONTEXT_DOMAIN, b"{}").unwrap();
        let operations = declarations.program.operation_registry();
        let manifest = hirsel_core_contract::Manifest::skeleton(
            declarations.program.operations.values().cloned(),
        );
        let environment = hirsel_lir::validate::NativeEnvironment {
            target: &target,
            operations: &operations,
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        let mut ready = LocalReadyQueries::new(store.clone());
        let original_ready = ready
            .ready(
                &instances,
                &key,
                &settle(&original),
                &options,
                emission,
                true,
                &environment,
            )
            .unwrap();
        let mut constant = entry_path(&default_context());
        constant.0.name = "SECOND".into();
        let current = declarations.program.constants.get_mut(&constant).unwrap();
        assert_eq!(
            current.root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(7))
        );
        current.root.kind = hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(8));
        let source_v = declarations.sources[key.instance.definition()]
            .lower
            .as_ref()
            .unwrap()
            .reference();
        let changed = instances
            .instance(&mut engine, &mut declarations, &key, &options)
            .unwrap();
        assert_eq!(
            declarations.sources[key.instance.definition()]
                .lower
                .as_ref()
                .unwrap()
                .reference(),
            source_v
        );
        assert_ne!(
            changed.product.product.action, original.product.product.action,
            "current source materialized data must invalidate the native action"
        );
        assert_ne!(
            changed.product.statics_finalizers,
            original.product.statics_finalizers
        );
        assert_eq!(
            changed.product.value, original.product.value,
            "the caller Value body is unchanged"
        );
        let changed_ready = ready
            .ready(
                &instances,
                &key,
                &settle(&changed),
                &options,
                emission,
                true,
                &environment,
            )
            .unwrap();
        assert_eq!(
            changed_ready.ready, original_ready.ready,
            "canonical Ready body is unchanged"
        );
        assert_ne!(
            changed_ready.emission_context, original_ready.emission_context,
            "codegen must observe new static bytes even when the Ready body is equal"
        );
        assert_ne!(changed_ready.product.record, original_ready.product.record);
        let bytes = store.get_content(changed_ready.emission_context).unwrap();
        let context: native::queries::local::UnitEmissionContext =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            context.statics_finalizers,
            changed.product.statics_finalizers
        );
        assert_eq!(context.request, emission);
        declarations.program.constants.remove(&constant);
        assert!(
            instances
                .instance(&mut engine, &mut declarations, &key, &options)
                .is_err(),
            "a missing current static must refuse the prior native output"
        );
    });
}

#[test]
fn local_ready_queries_bind_reached_finalizers_and_library_environments_on_misses_only() {
    with_compiler_stack(|| {
        for source in [
            "resource type R[T:copyable] {n:T} drop(x) {let _=x.n}\nfn outer[T](take x:T)->T {let f:once fn()->T=fn()->T{take x};f()}\npub fn main()->u64 {let r=R(n:1u64);outer(take 7u64)}",
            "fn step(mut n:u64)->Option[u64] {if n==2u64 {None} else {let x=n;n+=1u64;Some(x)}}\npub fn main()->u64 {let state=0u64;let mut it=iter.from(take state,step);let _=iter.next(mut it);0u64}",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let store = Arc::new(store::DirStore::new(directory.path().join("products")));
            let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
            let options = native::Options::new(target.clone());
            let policy = ContentHash::of_bytes(b"local finalizer and library environments");
            use hirsel_engine::store::ResultStore;
            let emission = store.put_content(EMISSION_CONTEXT_DOMAIN, b"{}").unwrap();
            let mut expected = BTreeMap::new();
            for fresh in [false, true] {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                    .with_result_store(store.clone());
                let file = engine
                    .sources
                    .add("local-environments.hir", source.into())
                    .unwrap();
                engine.parse_file(file, &default_context()).unwrap();
                if fresh {
                    engine.adapters.lowering = None;
                }
                let mut source_queries =
                    NativeDiscoveryCache::new(engine.types().clone(), store.clone());
                let mut declarations =
                    NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
                if fresh {
                    assert_eq!(source_queries.counts.body_demands, 0);
                }
                let trace = hirsel_lir::construction::ValueTrace::default();
                let _observe = trace.enter();
                let mut instances =
                    DemandInstances::new(&engine, store.clone(), target.clone(), policy);
                let mut pending = instances.roots(&engine, &declarations).unwrap();
                let mut seen = BTreeSet::new();
                let mut completed = Vec::new();
                while let Some(key) = pending.pop() {
                    if !seen.insert(key.instance.encoding().to_vec()) {
                        continue;
                    }
                    let output = instances
                        .instance(&mut engine, &mut declarations, &key, &options)
                        .unwrap();
                    pending.extend(output.dependencies.iter().cloned());
                    completed.push((key, output));
                }
                completed.sort_by(|a, b| a.0.canonical_cmp(&b.0));
                let nodes = completed
                    .iter()
                    .map(|(key, output)| ArenaNode {
                        equations: EquationProduct {
                            owner: key.clone(),
                            reference: output.product.local_equations,
                        },
                        callees: output.callees.clone(),
                    })
                    .collect::<Vec<_>>();
                let arena = settle_arena_queries(
                    store.as_ref(),
                    &nodes,
                    &[],
                    0,
                    &mut ArenaQueryCounts::default(),
                )
                .unwrap();
                let operations = declarations.program.operation_registry();
                let manifest = hirsel_core_contract::Manifest::skeleton(
                    declarations.program.operations.values().cloned(),
                );
                let environment = hirsel_lir::validate::NativeEnvironment {
                    target: &target,
                    operations: &operations,
                    manifest: &manifest,
                    discharge: Some(&options.discharge),
                };
                let mut ready = LocalReadyQueries::new(store.clone());
                let mut actual = BTreeMap::new();
                let mut links = Vec::new();
                for (key, _) in &completed {
                    let product = ready
                        .ready(
                            &instances,
                            key,
                            &arena,
                            &options,
                            emission,
                            true,
                            &environment,
                        )
                        .unwrap();
                    links.push(
                        ready
                            .queries
                            .linkage(key, product.product.action, &environment)
                            .unwrap(),
                    );
                    actual.insert(key.instance.encoding().to_vec(), product.product.record);
                }
                hirsel_lir::validate::check_arena_scoped_links(&links).unwrap();
                if fresh {
                    assert_eq!(actual, expected);
                    assert_eq!(
                        (
                            instances.imports,
                            instances.recipe_decodes,
                            ready.queries.counts.transformations
                        ),
                        (0, 0, 0)
                    );
                    assert_eq!(
                        trace.totals(),
                        hirsel_lir::construction::ValueCounts::default()
                    );
                } else {
                    assert!(ready.queries.counts.transformations >= 3);
                    expected = actual;
                }
            }
        }
    });
}
