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
