use hirsel_driver::*;
use hirsel_engine::*;

fn path(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}

const EDGES: &str = "resource type Holder[T: copyable] { v: T }\ndrop(x) { keep(copy x.v) }\ntype Pair[T: copyable] { first: Holder[(T,T)] }\ntype Wrap[T] { v: T }\nfn keep[T: copyable](x:T) {}\nfn total[n: size](xs: Inline[u32, n]) -> u64 { 0u64 }\nfn sizes[m: size](xs: Inline[u32, m]) -> u64 { total(xs) }\nfn outer[T: copyable](x:T) { let f = fn(y: T) { keep(y) }; f(x) }\nfn pairs[T: copyable](x:T) { let p = Pair(first: Holder(v: (copy x, copy x))) }\nconst W: Wrap[u32] = Wrap(v: 1u32)\n";
#[test]
fn native_discovery_cache_reuses_requirements_without_demanding_source_bodies() {
    use std::sync::Arc;
    // Compare portable observations in each owning session, including the
    // exact owner-relative source range. Raw anchors and TypeIds relocate.
    fn observations(engine: &QueryEngine, graph: &InstanceGraph) -> Vec<Vec<u8>> {
        graph
            .edges
            .iter()
            .map(|edge| {
                let mut table = hirsel_types::TableBuilder::new(engine.types().clone());
                let payload = table
                    .with_encoding(|| {
                        serde_json::to_vec(&(
                            &edge.owner,
                            edge.owner_binder,
                            edge.site.start,
                            edge.site.end,
                            engine.sources.slice(edge.site).unwrap(),
                            edge.kind,
                            &edge.callee,
                            edge.binder,
                            &edge.arguments,
                        ))
                    })
                    .unwrap();
                serde_json::to_vec(&(table.finish(), payload)).unwrap()
            })
            .collect()
    }
    fn parameter_contract(engine: &QueryEngine, contracts: &NativeBodyContracts) -> Vec<u8> {
        let mut table = hirsel_types::TableBuilder::new(engine.types().clone());
        let payload = table
            .with_encoding(|| serde_json::to_vec(&contracts.entry_parameters))
            .unwrap();
        serde_json::to_vec(&(table.finish(), payload)).unwrap()
    }
    with_compiler_stack(|| {
        let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
        let store = Arc::new(store::RequestStore::new(
            Arc::new(store::MemStore::default()),
            budget.reserve(64 << 20).unwrap(),
        ));
        let session = || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("discovery.hir", EDGES.into()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            engine
        };
        let mut first = session();
        let selected: std::collections::BTreeSet<_> = first
            .native_source_roots(&[])
            .unwrap()
            .definitions
            .into_iter()
            .collect();
        let mut queries = NativeDiscoveryCache::new(first.types().clone(), store.clone());
        let seed = queries
            .definition(&mut first, &path("outer"), &selected)
            .unwrap();
        assert!(seed.product.is_some());
        let contracts = seed.contracts.as_ref().unwrap();
        assert_eq!(contracts.entry_parameters.len(), 1);
        let expected_parameters = parameter_contract(&first, contracts);
        let body = first.native_definition_body(&seed.source).unwrap();
        assert_eq!(contracts.source_anchors, hirsel_tir::source_anchors(&body));
        let source_positions = |engine: &QueryEngine, anchors: &[hirsel_span::Anchor]| {
            anchors
                .iter()
                .map(|anchor| {
                    let (file, base) = engine.sources.anchor(*anchor).unwrap();
                    (engine.sources.file(file).unwrap().path.clone(), base)
                })
                .collect::<Vec<_>>()
        };
        let expected_positions = source_positions(&first, &contracts.source_anchors);
        let owning_codec = NativeRecords::new(first.types().clone(), store.clone());
        let recipe = owning_codec.encode(&first, contracts).unwrap();
        drop(body);
        assert!(!seed.requirements.edges.is_empty());
        assert_eq!(queries.counts.body_demands, 1);
        let expected = observations(&first, &seed.requirements);
        drop(queries);
        drop(first);
        let mut fresh = session();
        fresh.adapters.lowering = None;
        assert!(
            owning_codec
                .decode::<NativeBodyContracts>(&fresh, &recipe)
                .is_none(),
            "a codec cannot bind TypeIds into another session's interner"
        );
        let relocated_codec = NativeRecords::new(fresh.types().clone(), store.clone());
        let relocated: NativeBodyContracts = relocated_codec.decode(&fresh, &recipe).unwrap();
        assert_ne!(relocated.source_anchors, contracts.source_anchors);
        assert_eq!(
            source_positions(&fresh, &relocated.source_anchors),
            expected_positions
        );
        let mut queries = NativeDiscoveryCache::new(fresh.types().clone(), store);
        let read = queries
            .definition(&mut fresh, &path("outer"), &selected)
            .unwrap();
        assert_eq!(queries.counts.hits, 1);
        assert_eq!(queries.counts.body_demands, 0);
        assert_eq!(
            source_positions(&fresh, &read.contracts.as_ref().unwrap().source_anchors),
            expected_positions
        );
        assert_eq!(
            seed.product
                .unwrap()
                .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .unwrap()
                .reference(),
            read.product
                .unwrap()
                .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .unwrap()
                .reference()
        );
        assert_eq!(expected, observations(&fresh, &read.requirements));
        assert_eq!(
            expected_parameters,
            parameter_contract(&fresh, read.contracts.as_ref().unwrap())
        );
        // Selection of a previously observed callee is a dependency even
        // though its declaration and our owner's body did not change.
        fresh.adapters = compiler_adapters();
        let mut removed = selected.clone();
        removed.remove(&path("keep"));
        let changed = queries
            .definition(&mut fresh, &path("outer"), &removed)
            .unwrap();
        assert_eq!(queries.counts.misses, 1);
        assert!(
            changed
                .requirements
                .edges
                .iter()
                .all(|e| e.callee != path("keep"))
        );
    });
}

#[test]
fn native_discovery_cache_type_only_respects_forced_recomputation() {
    use std::sync::Arc;
    with_compiler_stack(|| {
        let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
        let store = Arc::new(store::RequestStore::new(
            Arc::new(store::MemStore::default()),
            budget.reserve(64 << 20).unwrap(),
        ));
        let session = || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("type-discovery.hir", "type Pair[T] { v:T }".into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            engine
        };
        let mut seed = session();
        let selected = seed
            .native_source_roots(&[])
            .unwrap()
            .definitions
            .into_iter()
            .collect();
        let mut queries = NativeDiscoveryCache::new(seed.types().clone(), store.clone());
        let expected = queries
            .definition(&mut seed, &path("Pair"), &selected)
            .unwrap();
        assert!(expected.product.is_some());
        let mut fresh = session();
        let mut queries = NativeDiscoveryCache::new(fresh.types().clone(), store.clone());
        queries
            .definition(&mut fresh, &path("Pair"), &selected)
            .unwrap();
        assert_eq!(queries.counts.hits, 1);
        let mut recompute = session().with_verify_store(store::VerifyStore::Recompute);
        let mut queries = NativeDiscoveryCache::new(recompute.types().clone(), store);
        let actual = queries
            .definition(&mut recompute, &path("Pair"), &selected)
            .unwrap();
        assert_eq!(queries.counts.hits, 0);
        assert_eq!(queries.counts.misses, 1);
        assert_eq!(
            expected
                .product
                .unwrap()
                .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .unwrap()
                .reference(),
            actual
                .product
                .unwrap()
                .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .unwrap()
                .reference()
        );
    });
}

#[test]
fn native_discovery_cache_forced_lower_uses_fresh_verifier_output() {
    use std::sync::Arc;
    with_compiler_stack(|| {
        let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
        let store = Arc::new(store::RequestStore::new(
            Arc::new(store::MemStore::default()),
            budget.reserve(64 << 20).unwrap(),
        ));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone())
            .with_verify_store(store::VerifyStore::Recompute);
        let file = engine
            .sources
            .add("forced.hir", "pub fn main()->u64{1u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let main = path("main");
        let selected = engine
            .native_source_roots(&[])
            .unwrap()
            .definitions
            .into_iter()
            .collect();
        assert!(engine.verified_lower_output(&main).unwrap().is_none());
        let mut queries = NativeDiscoveryCache::new(engine.types().clone(), store);
        let result = queries.definition(&mut engine, &main, &selected).unwrap();
        assert_eq!(queries.counts.hits, 0);
        assert_eq!(queries.counts.body_demands, 1);
        assert_eq!(engine.recompute_count(&main, QueryKind::Lower), 1);
        let output = result
            .source
            .lower
            .as_ref()
            .expect("actual Lower returns V output");
        assert!(engine.materialize_verified_lower_output(output).is_some());
        assert!(engine.verified_lower_output(&main).unwrap().is_none());
        assert!(result.product.is_some());
    });
}
