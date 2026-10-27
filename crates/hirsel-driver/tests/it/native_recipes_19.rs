use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_rc::instance::{Actuals, InstanceRole};
use native::queries::{
    declarations::NativeDeclarations, recipes::SourceRecipe, source_inputs::NativeSourceProvider,
};
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
        let inputs = crate::native_front_fixture::inputs(&mut first, &[]);
        let declarations = &inputs.declarations;
        let mut source = inputs.provider();
        let records = NativeRecords::new(declarations.program.types.clone(), store.clone());
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
        let main_body = source.original_source(&main).unwrap().0;
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
        let outer_body = source.original_source(&outer).unwrap().0;
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
                let bytes = recipe.encode(&records, &source).unwrap();
                let abi = keys.fn_abi(&table.fn_abis[instances.abis[id.index()].0 as usize]);
                (key, bytes, abi)
            })
            .collect();
        drop(instances);
        drop(source);
        drop(inputs);
        drop(first);
        assert!(
            original.upgrade().is_none(),
            "recipes retain no original Definition"
        );

        let mut fresh = session();
        fresh.adapters.lowering = None;
        let inputs = crate::native_front_fixture::inputs(&mut fresh, &[]);
        let declarations = &inputs.declarations;
        let mut source = inputs.provider();
        let records = NativeRecords::new(declarations.program.types.clone(), store.clone());
        assert_eq!(source.body_demands, 0);
        assert!(declarations.program.definitions.is_empty());
        for (key, bytes, expected_abi) in recipes {
            let recipe = SourceRecipe::decode(&records, &source, &bytes).unwrap();
            assert_eq!(recipe.key().unwrap(), key);
            let body = source.original_source(key.definition()).unwrap().0;
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
            let mut imported = hirsel_rc::import::import_instance(
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
            let operations = source_operations(&declarations, &instances, id);
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
            hirsel_rc::import::prune_pretransfer(
                std::slice::from_mut(&mut imported.unit),
                &table,
                &operations.pretransfer_adapters,
            );
            hirsel_lir::validate::validate_value(imported.unit, &inputs).unwrap();
        }
    });
}

#[test]
fn library_recipe_rebinds_its_environment_without_reconstructing_the_requiring_value() {
    use native::queries::recipes::LibraryRecipe;
    with_compiler_stack(|| {
        const SOURCE: &str = "fn step(mut n:u64)->Option[u64] {if n==2u64 {None} else {let x=n;n+=1u64;Some(x)}}\npub fn main()->u64 {let state=0u64;let mut it=iter.from(take state,step);let _=iter.next(mut it);0u64}";
        let store = Arc::new(store::MemStore::default());
        let session = || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("library-recipe.hir", SOURCE.into())
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            engine
        };
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let mut first = session();
        let inputs = crate::native_front_fixture::inputs(&mut first, &[]);
        let declarations = &inputs.declarations;
        let mut source = inputs.provider();
        let records = NativeRecords::new(declarations.program.types.clone(), store.clone());
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
        let main = entry_path(&default_context());
        let id = instances
            .find(&declarations.program, &main, &Actuals::none())
            .unwrap();
        let body = source.original_source(&main).unwrap().0;
        let (mut instances, _) = hirsel_rc::instance::discover_instance(
            &declarations.program,
            &declarations.context,
            &mut layouts,
            instances,
            id,
            body,
        )
        .unwrap();
        let mut imported = hirsel_rc::import::import_instance(
            &declarations.program,
            &mut instances,
            id,
            None,
            &mut layouts,
        )
        .unwrap();
        for request in imported.required {
            hirsel_rc::library::instantiate(&mut layouts, &mut instances, request).unwrap();
        }
        let library = instances
            .ids()
            .find(|id| {
                instances.roles[id.index()].is_library()
                    && instances.environments[id.index()].is_some()
            })
            .expect("iter.from has a closed environment thunk");
        let table = layouts.finish();
        let recipe = LibraryRecipe::capture(&instances, library, &table).unwrap();
        let recipe_bytes = recipe.encode(&records, &source).unwrap();
        let expected_key = recipe.key().unwrap();
        let keys = hirsel_lir::LayoutKeys::new(&table);
        let expected_abi = keys.fn_abi(&table.fn_abis[instances.abis[library.index()].0 as usize]);
        let operations = source_operations(&declarations, &instances, id);
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
        hirsel_rc::import::prune_pretransfer(
            std::slice::from_mut(&mut imported.unit),
            &table,
            &operations.pretransfer_adapters,
        );
        let parent = hirsel_lir::validate::validate_value(imported.unit, &inputs).unwrap();
        let (closure, _) = hirsel_lir::products::LayoutClosure::capture(
            &parent,
            &inputs,
            &[instances.abis[library.index()]],
        )
        .unwrap();
        let closure_bytes = hirsel_codec::to_text(&closure);
        // No requiring Value, original Definition or instantiated registry is
        // available to the fresh library miss below.
        drop(parent);
        drop(instances);
        drop(source);
        drop(inputs);
        drop(first);

        let mut fresh = session();
        fresh.adapters.lowering = None;
        let inputs = crate::native_front_fixture::inputs(&mut fresh, &[]);
        let declarations = &inputs.declarations;
        let mut source = inputs.provider();
        let records = NativeRecords::new(declarations.program.types.clone(), store.clone());
        assert_eq!(source.body_demands, 0);
        let recipe = LibraryRecipe::decode(&records, &source, &recipe_bytes).unwrap();
        assert_eq!(recipe.key().unwrap(), expected_key);
        assert!(
            LibraryRecipe::decode(&records, &source, &recipe_bytes[..recipe_bytes.len() / 2])
                .is_err()
        );
        let closure: hirsel_lir::products::LayoutClosure =
            hirsel_codec::from_text(&closure_bytes).unwrap();
        let owners = hirsel_rc::library::owners(
            &declarations.program.types,
            &declarations.program.signatures,
        )
        .over(&declarations.program.owners);
        let mut missing = hirsel_layout::layout::LayoutBuilder::new(&target, &owners);
        assert!(recipe.restore(&declarations.program, &mut missing).is_err());
        assert!(
            hirsel_layout::layout::LayoutBuilder::from_products(
                &hirsel_lir::TargetSpec::aarch64_unknown_linux_gnu(),
                &owners,
                &[&closure]
            )
            .is_err()
        );
        let mut layouts =
            hirsel_layout::layout::LayoutBuilder::from_products(&target, &owners, &[&closure])
                .unwrap();
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let (mut instances, id) = recipe.restore(&declarations.program, &mut layouts).unwrap();
        assert_eq!(
            trace.totals(),
            hirsel_lir::construction::ValueCounts::default()
        );
        let mut imported = hirsel_rc::import::import_instance(
            &declarations.program,
            &mut instances,
            id,
            None,
            &mut layouts,
        )
        .unwrap();
        assert_eq!(trace.totals().created, 1);
        assert!(
            trace
                .snapshot()
                .keys()
                .all(|owner| owner == &(expected_key.definition().clone(), expected_key.digest()))
        );
        // The callback's parameter and the previously bound capture must share
        // the same id, as well as canonical content. Strict C checks that link.
        let state_layout = layouts.table().layouts
            [instances.environments[id.index()].unwrap().0 as usize]
            .fields[0]
            .layout;
        let hirsel_types::GenericArgument::Type(state_type) =
            instances.actuals[id.index()].arguments[0]
        else {
            panic!("iterator state is a type actual")
        };
        assert_eq!(
            layouts
                .layout(&declarations.program.types, state_type)
                .unwrap(),
            state_layout
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
        hirsel_rc::import::prune_pretransfer(
            std::slice::from_mut(&mut imported.unit),
            &table,
            &operations.pretransfer_adapters,
        );
        hirsel_lir::validate::validate_value(imported.unit, &inputs).unwrap();
    });
}

// Independent source selection is supplied by the restored instance recipe;
// neither the expected body ordinal nor call inventory comes from candidate LIR.
fn source_operations(
    declarations: &NativeDeclarations,
    instances: &hirsel_rc::instance::Instances,
    id: hirsel_rc::instance::InstanceId,
) -> hirsel_layout::operation::OperationRegistry {
    let view = instances.view(id);
    let selected = hirsel_lir::ir::Symbol {
        kind: hirsel_lir::ir::SymbolKind::Function,
        definition: view.key.definition().clone(),
        instance: Some(view.key.digest()),
    };
    declarations
        .source_operations(view.key.definition(), &selected, view.body)
        .unwrap()
}

#[test]
fn library_recipes_share_a_key_across_erased_actual_modes() {
    use hirsel_driver::native::queries::recipes::LibraryRecipe;
    use hirsel_types::{GenericArgument, Primitive, Type, TypeKind, Uniqueness};
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(Arc::new(store::MemStore::default()));
        let file = engine
            .sources
            .add("keyed-recipes.hir", "pub fn main()->u64 {0u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let inputs = crate::native_front_fixture::inputs(&mut engine, &[]);
        let program = &inputs.declarations.program;
        let source = inputs.provider();
        let records =
            NativeRecords::new(program.types.clone(), Arc::new(store::MemStore::default()));
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let path = hirsel_syntax::DefPath::core("list.push");
        let body = hirsel_rc::library::body_index(&path).unwrap();
        let mut captures = Vec::new();
        for unique in [false, true] {
            let mut string = Type::with_kind(TypeKind::Primitive(Primitive::String));
            if unique {
                string.modes.uniqueness = Uniqueness::Unique;
            }
            let string = program.types.intern(string);
            let owners = hirsel_rc::library::owners(&program.types, &program.signatures)
                .over(&program.owners);
            let mut layouts = hirsel_layout::layout::LayoutBuilder::new(&target, &owners);
            layouts.prepare_native_controls();
            let mut instances =
                hirsel_rc::instance::Instances::new(program.types.clone(), program.memo.clone());
            let request = hirsel_rc::library::Request::operation(
                program,
                body,
                &path,
                vec![GenericArgument::Type(string)],
            )
            .unwrap();
            let id =
                hirsel_rc::library::instantiate(&mut layouts, &mut instances, request).unwrap();
            let table = layouts.finish();
            let recipe = LibraryRecipe::capture(&instances, id, &table).unwrap();
            let key = recipe.key().unwrap();
            let bytes = recipe.encode(&records, &source).unwrap();
            let abi = hirsel_lir::LayoutKeys::new(&table)
                .fn_abi(&table.fn_abis[instances.abis[id.index()].0 as usize]);
            captures.push((key, abi, bytes));
        }
        assert_eq!(captures[0].0, captures[1].0);
        assert_eq!(captures[0].1, captures[1].1);
        assert_eq!(
            captures[0].2, captures[1].2,
            "one native key must register one recipe"
        );
    });
}
