use hirsel_driver::*;
use hirsel_engine::*;
use native::queries::declarations::NativeDeclarations;
use std::sync::Arc;

#[test]
fn actual_original_lower_capture_keeps_v_observation_before_native_rewrite() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add("original-source.hir", "pub fn main()->u64 {7u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut queries = NativeDiscoveryCache::new(engine.types().clone(), store);
        let mut declarations = NativeDeclarations::collect(&mut engine, &mut queries, &[]).unwrap();
        let path = entry_path(&default_context());
        let (mut definition, source) = declarations.original_source(&mut engine, &path).unwrap();
        let captured = source.expect("actual V observation must be available");
        let output = declarations.sources[&path].lower.as_ref().unwrap();
        let key = hirsel_layout::instance::InstanceKey::new(
            &path,
            declarations.context.sources[&path].entry.0,
            &[],
            engine.types(),
            None,
            None,
        )
        .unwrap();
        let expected = captured.derive(&key, engine.types(), &[], Ok).unwrap();
        assert_eq!(expected.witness().source, output.observation_identity());
        assert_eq!(
            expected.witness().implementation_revision,
            declarations.sources[&path].source.implementation_revision
        );
        let original = captured.demand_body(&key).unwrap().clone();
        let weak = Arc::downgrade(&definition);
        let body = &mut Arc::make_mut(&mut definition).bodies[key.body() as usize];
        let literal = body
            .rvalues
            .iter_mut()
            .find(|rvalue| {
                matches!(
                    rvalue,
                    hirsel_tir::Rvalue::Constant(hirsel_tir::Constant::Integer(7))
                )
            })
            .unwrap();
        *literal = hirsel_tir::Rvalue::Constant(hirsel_tir::Constant::Integer(9));
        assert_ne!(body, &original);
        assert_eq!(captured.demand_body(&key).unwrap(), &original);
        assert!(declarations.program.definitions.is_empty());
        drop(definition);
        drop(declarations);
        drop(engine);
        assert!(
            weak.upgrade().is_some(),
            "the sole captured input retains original Lower"
        );
        drop(captured);
        assert!(
            weak.upgrade().is_none(),
            "the snapshot is released after its local consumer"
        );
    });
}
