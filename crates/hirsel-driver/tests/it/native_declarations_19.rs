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
fn native_declarations_match_snapshot_inputs_and_refuse_implicit_growth() {
    use std::sync::Arc;
    with_compiler_stack(|| {
        let budget = runtime::ProcessBudget::new(64 << 20);
        let store = Arc::new(store::RequestStore::new(
            Arc::new(store::MemStore::default()),
            budget.reserve(64 << 20).unwrap(),
        ));
        let mut engine = native_engine(EDGES).with_result_store(store.clone());
        let snapshot = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(snapshot).unwrap();
        let expected = native::program(&validated);
        let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let actual = native::queries::declarations::NativeDeclarations::collect(
            &mut engine,
            &mut cache,
            &[],
        )
        .unwrap();
        assert!(actual.program.definitions.is_empty());
        assert_eq!(actual.program.binders, expected.binders);
        assert_eq!(actual.program.revisions, expected.revisions);
        assert_eq!(actual.program.instance_bodies, expected.instance_bodies);
        assert_eq!(
            actual.program.signatures.keys().collect::<Vec<_>>(),
            expected.signatures.keys().collect::<Vec<_>>()
        );
        for (path, signature) in &expected.signatures {
            assert_eq!(
                actual.program.signatures[path].fingerprint,
                signature.fingerprint
            );
        }
        assert_eq!(
            actual.program.constants.keys().collect::<Vec<_>>(),
            expected.constants.keys().collect::<Vec<_>>()
        );
        // Same session, so Debug compares the exact typed/anchor fields without
        // introducing another type or source codec. Order is per source owner.
        let mut expected_requirements: Vec<_> = expected
            .requirements
            .iter()
            .map(|edge| format!("{edge:?}"))
            .collect();
        let mut actual_requirements: Vec<_> = actual
            .context
            .requirements
            .values()
            .flatten()
            .map(|edge| format!("{edge:?}"))
            .collect();
        expected_requirements.sort();
        actual_requirements.sort();
        assert_eq!(actual_requirements, expected_requirements);
        assert_eq!(
            format!("{:?}", actual.program.owners),
            format!("{:?}", expected.owners)
        );

        let mut growing = native_engine(FINALIZER_GROWING).with_result_store(store.clone());
        let mut cache = NativeDiscoveryCache::new(growing.types().clone(), store);
        let error = native::queries::declarations::NativeDeclarations::collect(
            &mut growing,
            &mut cache,
            &[],
        )
        .err()
        .expect("native implicit finalizer growth must still refuse");
        assert!(
            format!("{error:?}").contains("instance expansion grows through an implicit instance")
        );
    });
}
