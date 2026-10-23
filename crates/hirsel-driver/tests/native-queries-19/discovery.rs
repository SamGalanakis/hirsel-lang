use hirsel_driver::*;
use hirsel_engine::*;

fn native_engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("graph.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    engine
}
fn path(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}

const EDGES: &str = "resource type Holder[T: copyable] { v: T }\ndrop(x) { keep(copy x.v) }\ntype Pair[T: copyable] { first: Holder[(T,T)] }\ntype Wrap[T] { v: T }\nfn keep[T: copyable](x:T) {}\nfn total[n: size](xs: Inline[u32, n]) -> u64 { 0u64 }\nfn sizes[m: size](xs: Inline[u32, m]) -> u64 { total(xs) }\nfn outer[T: copyable](x:T) { let f = fn(y: T) { keep(y) }; f(x) }\nfn pairs[T: copyable](x:T) { let p = Pair(first: Holder(v: (copy x, copy x))) }\nconst W: Wrap[u32] = Wrap(v: 1u32)\n";
const FINALIZER_GROWING: &str = "resource type Holder[T: copyable] { v: T }\ndrop(x) { grow(copy x.v) }\nfn grow[T: copyable](x:T) { let h = Holder(v: (copy x, copy x)) }";

#[test]
fn native_demand_expansion_matches_implicit_growth_and_size_edges() {
    with_compiler_stack(|| {
        for source in [EDGES, FINALIZER_GROWING] {
            let mut engine = native_engine(source);
            let old = engine.export_native_input().unwrap();
            let roots = engine.native_source_roots(&[]).unwrap();
            let selected = roots.definitions.iter().cloned().collect();
            let mut graph = InstanceGraph::default();
            for path in &roots.definitions {
                let product = engine.native_definition_product(path).unwrap();
                let expansion = engine
                    .native_definition_expansion(&product, &selected)
                    .unwrap();
                let expected = InstanceGraph {
                    edges: old
                        .instances
                        .edges
                        .iter()
                        .filter(|e| &e.owner == path)
                        .cloned()
                        .collect(),
                };
                assert!(
                    expansion.requirements.same(&expected, engine.types()),
                    "{path}: {:?} != {:?}",
                    expansion.requirements,
                    expected
                );
                graph.edges.extend(expansion.requirements.edges);
            }
            assert_eq!(
                graph.growing_sites(engine.types()),
                old.instances.growing_sites(engine.types())
            );
        }
    });
}

#[test]
fn native_demand_source_descriptor_reuses_v_envelope_and_refuses_stale_body() {
    use std::sync::Arc;
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let source = "fn helper(x:u64)->u64{x}\npub fn main()->u64{helper(1u64)}\n";
        let session = |adapters| {
            let mut engine =
                QueryEngine::new(default_environment(), adapters).with_result_store(store.clone());
            let file = engine.sources.add("demand.hir", source.into()).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            engine
        };
        let main = path("main");
        let mut seed = session(compiler_adapters());
        let product = seed.native_definition_product(&main).unwrap();
        assert!(product.lower.is_none());
        let body = seed.native_definition_body(&product).unwrap();
        let settled = seed.native_definition_product(&main).unwrap();
        assert!(settled.lower.is_some());
        let mut adapters = compiler_adapters();
        adapters.lowering = None;
        let mut fresh = session(adapters);
        let descriptor = fresh.native_definition_product(&main).unwrap();
        assert_eq!(descriptor.lower, settled.lower);
        assert_eq!(fresh.recompute_count(&main, QueryKind::Lower), 0);
        let rebound = fresh.native_definition_body(&descriptor).unwrap();
        assert_eq!(rebound.signature_hash, body.signature_hash);
        assert_eq!(fresh.recompute_count(&main, QueryKind::Lower), 0);
        let changed = fresh
            .sources
            .add("demand.hir", source.replace("helper(1u64)", "helper(2u64)"))
            .unwrap();
        fresh.parse_file(changed, &default_context()).unwrap();
        assert!(fresh.native_definition_body(&descriptor).is_err());
        let mut off = session(compiler_adapters());
        off.verify = Verify::Off;
        assert!(
            off.native_definition_product(&main)
                .unwrap()
                .lower
                .is_none()
        );
    });
}

#[test]
fn native_demand_type_only_expansion_refuses_stale_representation() {
    with_compiler_stack(|| {
        let mut engine = native_engine("type Pair[T] { v:T }\n");
        let roots = engine.native_source_roots(&[]).unwrap();
        let selected = roots.definitions.into_iter().collect();
        let product = engine.native_definition_product(&path("Pair")).unwrap();
        assert!(product.lower.is_none());
        assert!(
            engine
                .native_definition_expansion(&product, &selected)
                .unwrap()
                .body
                .is_none()
        );
        let file = engine
            .sources
            .add("graph.hir", "type Pair[T] { v:(T,T) }\n".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        assert!(
            engine
                .native_definition_expansion(&product, &selected)
                .is_err()
        );
    });
}

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
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let session = || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("discovery.hir", EDGES.into()).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
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
        assert!(seed.contracts.is_some());
        assert!(!seed.requirements.edges.is_empty());
        assert_eq!(queries.counts.body_demands, 1);
        let expected = observations(&first, &seed.requirements);
        drop(queries);
        drop(first);
        let mut fresh = session();
        fresh.adapters.lowering = None;
        let mut queries = NativeDiscoveryCache::new(fresh.types().clone(), store);
        let read = queries
            .definition(&mut fresh, &path("outer"), &selected)
            .unwrap();
        assert_eq!(queries.counts.hits, 1);
        assert_eq!(queries.counts.body_demands, 0);
        assert_eq!(seed.product.unwrap().record, read.product.unwrap().record);
        assert_eq!(expected, observations(&fresh, &read.requirements));
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
        let store = Arc::new(store::MemStore::default());
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
            expected.product.unwrap().record,
            actual.product.unwrap().record
        );
    });
}
