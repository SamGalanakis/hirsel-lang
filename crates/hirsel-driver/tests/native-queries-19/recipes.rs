use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_rc::instance::{Actuals, InstanceRole};
use native::queries::{declarations::NativeDeclarations, recipes::SourceRecipe};
use std::sync::Arc;

#[test]
fn source_recipes_restore_generic_closures_and_finalizers_without_ancestor_values() {
    with_compiler_stack(|| {
        const SOURCE: &str = "resource type R[T:copyable] {n:T} drop(x) {let _=x.n}\nfn outer[T](take x:T)->T {let f:once fn()->T=fn()->T{take x};f()}\npub fn main()->u64 {let r=R(n:1u64);outer(take 7u64)}";
        let store = Arc::new(store::MemStore::default());
        let session = || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("recipes.hir", SOURCE.into()).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            engine
        };
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let mut first = session();
        let records = NativeRecords::new(first.types().clone(), store.clone());
        let mut cache = NativeDiscoveryCache::new(first.types().clone(), store.clone());
        let mut declarations = NativeDeclarations::collect(&mut first, &mut cache, &[]).unwrap();
        let main = entry_path(&default_context());
        let mut outer = main.clone();
        outer.0.name = "outer".into();
        let owners = hirsel_rc::library::owners(
            &declarations.program.types,
            &declarations.program.signatures,
        )
        .over(&declarations.program.owners);
        let mut layouts = hirsel_layout::layout::LayoutBuilder::new(&target, &owners);
        layouts.prepare_native_controls();
        let (instances, _) = hirsel_rc::instance::seed_instances(
            &declarations.program,
            &declarations.context,
            &mut layouts,
        )
        .unwrap();
        let main_id = instances
            .find(&declarations.program, &main, &Actuals::none())
            .unwrap();
        let main_body = declarations.body(&mut first, &main).unwrap();
        let (instances, _) = hirsel_rc::instance::discover_instance(
            &declarations.program,
            &declarations.context,
            &mut layouts,
            instances,
            main_id,
            main_body,
        )
        .unwrap();
        let outer_id = instances
            .ids()
            .find(|id| {
                instances.view(*id).key.definition() == &outer
                    && instances.roles[id.index()] == InstanceRole::Function
            })
            .unwrap();
        assert!(!instances.actuals[outer_id.index()].arguments.is_empty());
        let outer_body = declarations.body(&mut first, &outer).unwrap();
        let original = Arc::downgrade(&outer_body);
        let (instances, _) = hirsel_rc::instance::discover_instance(
            &declarations.program,
            &declarations.context,
            &mut layouts,
            instances,
            outer_id,
            outer_body,
        )
        .unwrap();
        let closure = instances
            .ids()
            .find(|id| instances.roles[id.index()] == (InstanceRole::Closure { parent: outer_id }))
            .unwrap();
        let finalizer = instances
            .ids()
            .find(|id| instances.roles[id.index()] == InstanceRole::Finalizer)
            .unwrap();
        let table = layouts.finish();
        let keys = hirsel_lir::LayoutKeys::new(&table);
        let recipes: Vec<_> = [outer_id, closure, finalizer]
            .into_iter()
            .map(|id| {
                let recipe = SourceRecipe::capture(&instances, id).unwrap();
                let key = recipe.key().unwrap();
                let bytes = recipe.encode(&records, &first).unwrap();
                let abi = keys.fn_abi(&table.fn_abis[instances.abis[id.index()].0 as usize]);
                (key, bytes, abi)
            })
            .collect();
        drop(instances);
        drop(declarations);
        drop(cache);
        drop(first);
        assert!(
            original.upgrade().is_none(),
            "recipes retain no original Definition"
        );

        let mut fresh = session();
        fresh.adapters.lowering = None;
        let records = NativeRecords::new(fresh.types().clone(), store.clone());
        let mut cache = NativeDiscoveryCache::new(fresh.types().clone(), store);
        let mut declarations = NativeDeclarations::collect(&mut fresh, &mut cache, &[]).unwrap();
        assert_eq!(cache.counts.body_demands, 0);
        assert!(declarations.program.definitions.is_empty());
        for (key, bytes, expected_abi) in recipes {
            let recipe = SourceRecipe::decode(&records, &fresh, &bytes).unwrap();
            assert_eq!(recipe.key().unwrap(), key);
            let body = declarations.body(&mut fresh, key.definition()).unwrap();
            let owners = hirsel_rc::library::owners(
                &declarations.program.types,
                &declarations.program.signatures,
            )
            .over(&declarations.program.owners);
            let mut layouts = hirsel_layout::layout::LayoutBuilder::new(&target, &owners);
            layouts.prepare_native_controls();
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let (mut instances, id) = recipe
                .restore(
                    &declarations.program,
                    &declarations.context,
                    body,
                    &mut layouts,
                )
                .unwrap();
            assert_eq!(
                trace.totals(),
                hirsel_lir::construction::ValueCounts::default(),
                "restoring lexical metadata constructs/decodes/clones/rebinds no Value"
            );
            assert_eq!(instances.view(id).key, key);
            let imported = hirsel_rc::import::import_instance(
                &declarations.program,
                &mut instances,
                id,
                None,
                &mut layouts,
            )
            .unwrap();
            assert_eq!(
                trace.totals().created,
                1,
                "only the demanded Value is constructed"
            );
            assert!(
                trace
                    .snapshot()
                    .keys()
                    .all(|owner| owner == &(key.definition().clone(), key.digest()))
            );
            let table = layouts.finish();
            let keys = hirsel_lir::LayoutKeys::new(&table);
            assert_eq!(
                keys.fn_abi(&table.fn_abis[imported.unit.header.abi.0 as usize]),
                expected_abi
            );
            let operations = declarations.program.operation_registry();
            let manifest = hirsel_core_contract::Manifest::skeleton(
                declarations.program.operations.values().cloned(),
            );
            let inputs = hirsel_lir::validate::Inputs {
                memo: Default::default(),
                target: &target,
                layouts: &table,
                operations: &operations,
                manifest: &manifest,
                discharge: None,
            };
            hirsel_lir::validate::validate_value(imported.unit, &inputs).unwrap();
        }
    });
}
