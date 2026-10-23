use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_rc::instance::{Actuals, Instances, Program};
use hirsel_syntax::ContentHash;
use std::sync::Arc;

#[test]
fn source_callable_registration_and_lookup_need_no_definition_collection() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("declaration.hir", "fn keep(x:u64)->u64{x}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "keep".into();
        let signature = engine.item_signature(&path).unwrap().value;
        let definition = engine.lower_to_tir(&path).unwrap().value;
        let declaration = Arc::new(hirsel_layout::instance::InstanceDeclaration::of(
            &definition,
        ));
        let entry = definition.entry;
        let weak = Arc::downgrade(&definition);
        let program = declaration_program(&engine, &path, signature);
        drop(definition);
        drop(engine);
        assert!(
            weak.upgrade().is_none(),
            "registry inputs retain declarations only"
        );
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let mut layouts = hirsel_layout::layout::LayoutBuilder::new(&target, &program.owners);
        let mut instances = Instances::new(program.types.clone(), program.memo.clone());
        let (id, fresh) = instances
            .register_function(
                &program,
                declaration.clone(),
                entry,
                Actuals::none(),
                &mut layouts,
            )
            .unwrap();
        assert!(fresh);
        assert!(instances.view(id).definition().is_err());
        assert_eq!(
            instances.find(&program, &path, &Actuals::none()).unwrap(),
            id
        );
        let (again, fresh) = instances
            .register_function(
                &program,
                declaration.clone(),
                entry,
                Actuals::none(),
                &mut layouts,
            )
            .unwrap();
        assert_eq!(again, id);
        assert!(!fresh);
        let mut stale = (*declaration).clone();
        stale.signature = ContentHash::of_bytes(b"changed");
        assert!(
            instances
                .register_function(
                    &program,
                    Arc::new(stale),
                    entry,
                    Actuals::none(),
                    &mut layouts
                )
                .is_err()
        );
    });
}

fn declaration_program(
    engine: &QueryEngine,
    path: &DefPath,
    signature: hirsel_types::SignatureRef,
) -> Program {
    Program {
        input: hirsel_lir::stage::InputIdentity {
            interface_version: 1,
            digest: ContentHash::of_bytes(b"declaration-only"),
        },
        types: engine.types().clone(),
        memo: Arc::new(hirsel_types::TypeMemo::default()),
        definitions: Default::default(),
        signatures: [(path.clone(), signature)].into(),
        revisions: Default::default(),
        binders: Default::default(),
        operations: Default::default(),
        requirements: Vec::new(),
        owners: Default::default(),
        constants: Default::default(),
        instance_bodies: Default::default(),
    }
}

#[test]
fn demanded_lexical_index_does_not_retain_a_released_source_body() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("lexical.hir", "fn keep(xs: Inline[u8, 3])->u64 { fn length[n:size](ys:Inline[u8,n])->u64{n} (fn(){length(xs)})() }".into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "keep".into();
        let signature = engine.item_signature(&path).unwrap().value;
        let definition = engine.lower_to_tir(&path).unwrap().value;
        let declaration = Arc::new(hirsel_layout::instance::InstanceDeclaration::of(
            &definition,
        ));
        let entry = definition.entry;
        let weak = Arc::downgrade(&definition);
        let program = declaration_program(&engine, &path, signature);
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let mut layouts = hirsel_layout::layout::LayoutBuilder::new(&target, &program.owners);
        let mut instances = Instances::new(program.types.clone(), program.memo.clone());
        let (id, _) = instances
            .register_function(&program, declaration, entry, Actuals::none(), &mut layouts)
            .unwrap();
        instances.attach_definition(id, definition).unwrap();
        drop(engine);
        assert!(weak.upgrade().is_some(), "the demanded view owns its body");
        instances.views[id.index()].release_definition();
        assert!(
            weak.upgrade().is_none(),
            "lexical producer metadata keeps no implementation body"
        );
        assert_eq!(
            instances.find(&program, &path, &Actuals::none()).unwrap(),
            id
        );
    });
}

#[test]
fn demanded_instance_discovery_registers_generic_callee_without_its_body() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(
                "single-instance.hir",
                "fn keep[T](take x:T)->T{x}\npub fn main()->u64{keep(take 1u64)}".into(),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let mut program = native::program(&input);
        let context = hirsel_rc::instance::DiscoveryContext::of(&program).unwrap();
        let main = entry_path(&default_context());
        let mut keep = main.clone();
        keep.0.name = "keep".into();
        let body = program.definitions[&main].clone();
        let weak_callee = Arc::downgrade(&program.definitions[&keep]);
        program.definitions.clear();
        drop(input);
        drop(engine);
        assert!(weak_callee.upgrade().is_none());
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let mut layouts = hirsel_layout::layout::LayoutBuilder::new(&target, &program.owners);
        let mut instances = Instances::new(program.types.clone(), program.memo.clone());
        let source = &context.sources[&main];
        let (id, _) = instances
            .register_function(
                &program,
                source.declaration.clone(),
                source.entry,
                Actuals::none(),
                &mut layouts,
            )
            .unwrap();
        let (instances, required) = hirsel_rc::instance::discover_instance(
            &program,
            &context,
            &mut layouts,
            instances,
            id,
            body,
        )
        .unwrap();
        assert!(program.definitions.is_empty());
        assert_eq!(required.len(), 1);
        let callee = instances.view(required[0]);
        assert_eq!(callee.key.definition(), &keep);
        assert_eq!(instances.actuals[required[0].index()].arguments.len(), 1);
        assert!(
            callee.definition().is_err(),
            "callee declaration lookup cannot demand its implementation"
        );
        assert!(weak_callee.upgrade().is_none());
        let mut instances = instances;
        let imported =
            hirsel_rc::import::import_instance(&program, &mut instances, id, None, &mut layouts)
                .unwrap();
        let table = layouts.finish();
        let manifest =
            hirsel_core_contract::Manifest::skeleton(program.operations.values().cloned());
        let operations = program.operation_registry();
        let inputs = hirsel_lir::validate::Inputs {
            target: &target,
            layouts: &table,
            manifest: &manifest,
            operations: &operations,
            discharge: None,
            memo: Default::default(),
        };
        hirsel_lir::validate::validate(imported.unit, &inputs)
            .unwrap_or_else(|rejected| panic!("local caller C refusal: {:?}", rejected.violations));
        assert!(instances.view(required[0]).definition().is_err());
        assert!(weak_callee.upgrade().is_none());
    });
}

#[test]
fn demanded_instance_discovery_registers_finalizer_without_its_body() {
    use hirsel_rc::instance::InstanceRole;
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("finalizer-instance.hir", "resource type R[T:copyable] {n:T} drop(x) {let _=x.n}\npub fn main()->u64{let r=R(n:1u64);r.n}".into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let mut program = native::program(&input);
        let context = hirsel_rc::instance::DiscoveryContext::of(&program).unwrap();
        let main = entry_path(&default_context());
        let mut resource = main.clone();
        resource.0.name = "R".into();
        let hirsel_types::SignatureKind::Type(signature) = &program.signatures[&resource].kind
        else {
            panic!("source resource signature")
        };
        let finalizer = signature.drop.clone().unwrap();
        let weak_finalizer = Arc::downgrade(&program.definitions[&finalizer]);
        let body = program.definitions[&main].clone();
        program.definitions.clear();
        drop(input);
        drop(engine);
        assert!(weak_finalizer.upgrade().is_none());
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let mut layouts = hirsel_layout::layout::LayoutBuilder::new(&target, &program.owners);
        let mut instances = Instances::new(program.types.clone(), program.memo.clone());
        let source = &context.sources[&main];
        let (id, _) = instances
            .register_function(
                &program,
                source.declaration.clone(),
                source.entry,
                Actuals::none(),
                &mut layouts,
            )
            .unwrap();
        let (instances, required) = hirsel_rc::instance::discover_instance(
            &program,
            &context,
            &mut layouts,
            instances,
            id,
            body,
        )
        .unwrap();
        assert_eq!(required.len(), 1);
        let id = required[0];
        assert_eq!(instances.roles[id.index()], InstanceRole::Finalizer);
        assert_eq!(instances.view(id).key.definition(), &finalizer);
        assert!(instances.view(id).definition().is_err());
        let table = layouts.finish();
        let abi = &table.fn_abis[instances.abis[id.index()].0 as usize];
        assert_eq!(abi.parameters.len(), 1);
        assert_eq!(
            abi.parameters[0].convention,
            hirsel_core_contract::Passing::Borrow
        );
        assert!(abi.result.is_none());
        assert!(weak_finalizer.upgrade().is_none());
    });
}
