use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_tir::ValueKind;
use hirsel_types::*;

#[test]
fn named_integer_dictionaries_materialize_declared_typed_records() {
    hirsel_engine::with_compiler_stack(
        named_integer_dictionaries_materialize_declared_typed_records_on_compiler_thread,
    );
}
fn named_integer_dictionaries_materialize_declared_typed_records_on_compiler_thread() {
    let registry = registry::BuiltinRegistry::new();
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    for name in [
        "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128",
    ] {
        let path = hirsel_syntax::DefPath::core(&format!("{name}.order"));
        let declared = registry.get(&path).unwrap();
        let SignatureKind::Const { ty: root, .. } = declared.kind else {
            panic!()
        };
        let value = engine.materialize_value(&path).unwrap();
        assert!(engine.types().semantic_eq(value.root.ty, root));
        let ValueKind::Construct {
            constructor: hirsel_tir::Constructor::Record(owner),
            fields,
        } = &value.root.kind
        else {
            panic!("expected declared record")
        };
        assert_eq!(*owner, hirsel_syntax::DefPath::core("Order"));
        let [compare] = fields.as_slice() else {
            panic!()
        };
        let TypeKind::Function(function) = &engine.types().get(compare.ty).unwrap().kind else {
            panic!()
        };
        assert_eq!(function.parameters.len(), 2);
        assert_eq!(function.allocation, Allocation::MayAllocate);
        assert_eq!(function.uses, Uses::Only(AuthoritySummary::inert()));
        assert!(matches!(
            engine.types().get(function.result).unwrap().kind,
            TypeKind::Primitive(Primitive::I32)
        ));
        let ValueKind::FunctionReference {
            definition: helper,
            arguments,
        } = &compare.kind
        else {
            panic!()
        };
        assert!(arguments.is_empty());
        assert_eq!(
            *helper,
            hirsel_syntax::DefPath::core(&format!("$intrinsic.OrderCompare_{name}"))
        );
        let helper_signature = registry.get(helper).unwrap();
        let SignatureKind::Function(helper_function) = &helper_signature.kind else {
            panic!()
        };
        assert_eq!(helper_function.allocation, Allocation::NoAlloc);
        assert_eq!(
            helper_signature.metadata.visibility,
            hirsel_syntax::Visibility::Private
        );
    }
    // Same spelling in another package is never an implicitly derived dictionary.
    let mut foreign = hirsel_syntax::DefPath::core("u32.order");
    foreign.0.package.name = "foreign".into();
    assert!(engine.materialize_value(&foreign).is_err());
}

#[test]
fn dictionary_graph_import_preserves_actuals_and_rejects_forgery() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let text = "type Op { run: fn(u64) -> u64 uses {}, tags: List[String] }\n\
            fn identity[T: plain](value: T) -> T uses {} { value }\n\
            fn wrong[T: plain](value: T) -> T uses {} { value }\n\
            const OP: Op = Op(run: identity, tags: [\"one\", \"two\"])\n\
            const ALIAS: Op = OP\n\
            pub fn main() -> Result[(), String] { let op = ALIAS; failure.check(op.run(9) == 9 && wrong(0u64) == 0, Bounds()); Ok(()) }\n";
        let file = engine.sources.add("dictionary.hir", text.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let text = engine.export_tir(&entry_path(&default_context())).unwrap();
        let imported = engine.import_tir(&text).unwrap();
        assert_eq!(hirsel_proof_export::export(&imported.to_input()), text);
        let mut forged_export = imported.clone();
        let section = forged_export
            .constants
            .iter_mut()
            .find(|c| c.path.0.name == "OP")
            .unwrap();
        let ValueKind::Construct { fields, .. } = &mut section.value.root.kind else {
            panic!()
        };
        let ValueKind::FunctionReference { arguments, .. } = &mut fields[0].kind else {
            panic!()
        };
        arguments.clear();
        assert!(
            engine
                .import_tir(&hirsel_proof_export::export(&forged_export.to_input()))
                .is_err()
        );
        let mut forged_export = imported.clone();
        let section = forged_export
            .constants
            .iter_mut()
            .find(|c| c.path.0.name == "OP")
            .unwrap();
        let ValueKind::Construct { fields, .. } = &mut section.value.root.kind else {
            panic!()
        };
        let ValueKind::FunctionReference { definition, .. } = &mut fields[0].kind else {
            panic!()
        };
        definition.0.name = "wrong".into();
        assert!(
            engine
                .import_tir(&hirsel_proof_export::export(&forged_export.to_input()))
                .is_err(),
            "same-shaped proof target substitution"
        );
        let base = engine.export_native_input().unwrap();
        engine.import_native_input(base.clone()).unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "OP".into();
        let ValueKind::Construct { fields, .. } = &base.constants[&path].root.kind else {
            panic!()
        };
        let ValueKind::FunctionReference { arguments, .. } = &fields[0].kind else {
            panic!()
        };
        assert_eq!(
            arguments.len(),
            1,
            "expected-type instantiation is retained"
        );
        let mut forged = base.clone();
        let ValueKind::Construct { fields, .. } =
            &mut forged.constants.get_mut(&path).unwrap().root.kind
        else {
            panic!()
        };
        let ValueKind::FunctionReference { arguments, .. } = &mut fields[0].kind else {
            panic!()
        };
        arguments.clear();
        assert!(
            engine.import_native_input(forged).is_err(),
            "missing generic actuals"
        );
        let mut forged = base.clone();
        let ValueKind::Construct { fields, .. } =
            &mut forged.constants.get_mut(&path).unwrap().root.kind
        else {
            panic!()
        };
        let ValueKind::FunctionReference { definition, .. } = &mut fields[0].kind else {
            panic!()
        };
        definition.0.name = "wrong".into();
        assert!(
            engine.import_native_input(forged).is_err(),
            "same-shaped target substitution"
        );
        let mut forged = base;
        path.0.name = "ALIAS".into();
        std::sync::Arc::make_mut(forged.signatures.get_mut(&path).unwrap())
            .metadata
            .constant_alias = None;
        assert!(
            engine.import_native_input(forged).is_err(),
            "forged alias identity"
        );
    });
}

#[test]
fn dictionary_alias_edits_invalidate_declared_brand_identity() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn compare(a:u32,b:u32)->i32 uses {} { 0 }\n\
            const FIRST: Order[u32] = Order(compare: compare)\n\
            const SECOND: Order[u32] = Order(compare: compare)\n\
            const ALIAS: Order[u32] = FIRST\n";
        let file = engine.sources.add("alias.hir", source.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "ALIAS".into();
        let before = engine.item_signature(&path).unwrap().value;
        let file = engine
            .sources
            .add("alias.hir", source.replace("= FIRST", "= SECOND"))
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let after = engine.item_signature(&path).unwrap().value;
        assert_ne!(before.fingerprint, after.fingerprint);
        assert_eq!(
            before.metadata.constant_alias.as_ref().unwrap().0.name,
            "FIRST"
        );
        assert_eq!(
            after.metadata.constant_alias.as_ref().unwrap().0.name,
            "SECOND"
        );
    });
}
