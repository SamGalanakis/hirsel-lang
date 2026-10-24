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
