use hirsel_driver::*;
use hirsel_engine::*;
use native::queries::declarations::NativeDeclarations;
use std::sync::Arc;

#[test]
fn settled_native_declaration_assembly_runs_without_a_source_controller() {
    with_compiler_stack(|| {
        let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine.sources.add("settled-declarations.hir", "resource type R {n:u64} drop(x) {let _=x.n}\nfn keep[T](take x:T)->T{x}\npub fn main()->u64 {let r=R(n:1u64);let f=fn(x:u64)->u64{keep(x)};f(2u64)}".into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let facts = crate::native_front_fixture::products(&mut engine, &[]).unwrap();
        let roots = facts.roots.clone();
        let catalogue = facts
            .catalogue
            .iter()
            .map(|product| (**product).clone())
            .collect::<Vec<_>>();
        let statics = facts.statics.clone();
        let extra = facts.extra.clone();
        let signatures = facts.signatures.clone();
        let expected = NativeDeclarations::from_front(facts).unwrap();
        let mut missing = catalogue.clone();
        missing.pop();
        assert!(
            NativeDeclarations::from_settled(
                &signatures,
                roots.clone(),
                missing,
                statics.clone(),
                extra.clone()
            )
            .is_err()
        );
        let mut duplicate = catalogue.clone();
        duplicate.push(catalogue[0].clone());
        assert!(
            NativeDeclarations::from_settled(
                &signatures,
                roots.clone(),
                duplicate,
                statics.clone(),
                extra.clone()
            )
            .is_err()
        );
        let mut crossed = catalogue.clone();
        Arc::make_mut(&mut crossed[0].source).source.syntax =
            catalogue.last().unwrap().source.source.syntax.clone();
        assert!(
            NativeDeclarations::from_settled(
                &signatures,
                roots.clone(),
                crossed,
                statics.clone(),
                extra.clone()
            )
            .is_err()
        );
        drop(engine);
        let actual =
            NativeDeclarations::from_settled(&signatures, roots, catalogue, statics, extra)
                .unwrap();
        assert!(actual.program.definitions.is_empty());
        assert_eq!(actual.program.binders, expected.program.binders);
        assert_eq!(actual.program.revisions, expected.program.revisions);
        assert_eq!(
            actual.program.instance_bodies,
            expected.program.instance_bodies
        );
        assert_eq!(
            actual.context.sources.keys().collect::<Vec<_>>(),
            expected.context.sources.keys().collect::<Vec<_>>()
        );
        assert_eq!(actual.source_anchors, expected.source_anchors);
        for (path, source) in &actual.context.sources {
            assert_eq!(source.entry, expected.context.sources[path].entry);
            assert_eq!(
                *source.declaration,
                *expected.context.sources[path].declaration
            );
        }
    });
}

#[test]
fn native_declaration_metadata_bound_tracks_copied_buffers_without_bodies() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("native-metadata.hir", "pub fn main()->u64{1u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
        engine = engine.with_result_store(store);
        let facts = crate::native_front_fixture::products(&mut engine, &[]).unwrap();
        let mut declarations = NativeDeclarations::from_front(facts).unwrap();
        assert!(declarations.program.definitions.is_empty());
        drop(engine);
        let before = declarations.retained_metadata_bytes().unwrap();
        let old = declarations.constant_definitions.capacity();
        declarations.constant_definitions.reserve_exact(old + 64);
        let added = declarations.constant_definitions.capacity() - old;
        let after = declarations.retained_metadata_bytes().unwrap();
        assert_eq!(
            after - before,
            (added * size_of::<hirsel_syntax::DefPath>()) as u64
        );
        assert!(declarations.program.definitions.is_empty());
    });
}
