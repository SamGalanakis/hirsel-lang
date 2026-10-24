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
