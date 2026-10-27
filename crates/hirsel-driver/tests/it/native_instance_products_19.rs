use hirsel_artifact_store::Action;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_layout::layout::LayoutBuilder;
use hirsel_syntax::ContentHash;
use native::queries::{InstanceAddress, instance::*, source_inputs::NativeSourceProvider};
use std::sync::Arc;

#[test]
fn native_instance_product_hits_require_current_c_without_value_materialization() {
    with_compiler_stack(|| {
        let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "instance-products.hir",
                "fn helper(x:u64)->u64{x+1u64}\npub fn main()->u64{helper(1u64)}".into(),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let source_inputs = crate::native_front_fixture::inputs(&mut engine, &[]);
        let declarations = &source_inputs.declarations;
        let mut source = source_inputs.provider();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let owners = hirsel_rc::library::owners(
            &declarations.program.types,
            &declarations.program.signatures,
        )
        .over(&declarations.program.owners);
        let mut layouts = LayoutBuilder::new(&target, &owners);
        layouts.prepare_native_controls();
        let (instances, _) = hirsel_rc::instance::seed_instances(
            &declarations.program,
            &declarations.context,
            &mut layouts,
        )
        .unwrap();
        let main = entry_path(&default_context());
        let id = instances
            .find(
                &declarations.program,
                &main,
                &hirsel_rc::instance::Actuals::none(),
            )
            .unwrap();
        let original = source.original_source(&main).unwrap().0;
        let (mut instances, _) = hirsel_rc::instance::discover_instance(
            &declarations.program,
            &declarations.context,
            &mut layouts,
            instances,
            id,
            original,
        )
        .unwrap();
        let view = instances.view(id);
        let key = InstanceAddress {
            instance: view.key.clone(),
            target_abi: target.digest(),
            policy: ContentHash::of_parts("native-instance-test-policy", [b"on".as_slice()]),
        };
        let selected = hirsel_lir::ir::Symbol {
            kind: hirsel_lir::ir::SymbolKind::Function,
            definition: main.clone(),
            instance: Some(view.key.digest()),
        };
        let mut operations = declarations
            .source_operations(&main, &selected, view.body)
            .unwrap();
        let manifest = declarations.manifest().unwrap();
        let imported = hirsel_rc::import::import_instance(
            &declarations.program,
            &mut instances,
            id,
            None,
            &mut layouts,
        )
        .unwrap();
        assert!(imported.required.is_empty());
        let table = layouts.finish();
        let discovery = store
            .put_content(INSTANCE_DISCOVERY_DOMAIN, b"explicit discovery")
            .unwrap();
        let statics = store.put_content(STATICS_FINALIZERS_DOMAIN, b"[]").unwrap();
        let action = Action(ContentHash::of_parts(
            "native-instance-test-action",
            [b"current".as_slice()],
        ));
        let mut queries = InstanceQueries::new(store.clone());
        let inputs = hirsel_lir::validate::Inputs {
            target: &target,
            layouts: &table,
            manifest: &manifest,
            operations: &operations,
            discharge: None,
            memo: Default::default(),
        };
        let (checked, output) = queries
            .publish(
                &key,
                action,
                imported.unit,
                &inputs,
                &[],
                discovery,
                statics,
            )
            .unwrap();
        assert_eq!(queries.counts.validations, 1);
        drop(checked);
        drop(inputs);
        drop(instances);
        drop(source);
        drop(source_inputs);
        drop(engine);
        drop(table);
        // Source/type context survives as independent compact declarations;
        // no original Definition, Value or layout table survives this scope.
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        {
            let environment = hirsel_lir::validate::NativeEnvironment {
                target: &target,
                manifest: &manifest,
                operations: &operations,
                discharge: None,
            };
            let hit = queries.lookup(&key, action, &environment).unwrap();
            assert_eq!(hit.product, output.product);
            assert_eq!(
                queries
                    .linkage(&key, action, &environment)
                    .unwrap()
                    .definition(),
                &main
            );
            assert!(
                InstanceQueries::new_callable(store.clone())
                    .lookup(&key, action, &environment)
                    .is_none()
            );
            let wrong = Action(ContentHash::default());
            assert!(queries.lookup(&key, wrong, &environment).is_none());
        }
        assert_eq!(
            trace.totals(),
            hirsel_lir::construction::ValueCounts::default()
        );
        assert_eq!(queries.counts.validations, 1);
        operations.invocation_bodies.clear();
        let environment = hirsel_lir::validate::NativeEnvironment {
            target: &target,
            manifest: &manifest,
            operations: &operations,
            discharge: None,
        };
        assert!(
            queries.lookup(&key, action, &environment).is_none(),
            "ordinary descriptor cannot replace the independent source association"
        );
    });
}
