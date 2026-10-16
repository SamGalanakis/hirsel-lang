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
        let path = registry::core_path(&format!("{name}.order"));
        let declared = registry.get(&path).unwrap();
        let SignatureKind::Const(root) = declared.kind else {
            panic!()
        };
        let value = engine.materialize_value(&path).unwrap();
        assert!(semantic_equal(&value.types, value.root.ty, &declared.types, root).unwrap());
        let ValueKind::Construct {
            constructor: hirsel_tir::Constructor::Record(owner),
            fields,
        } = &value.root.kind
        else {
            panic!("expected declared record")
        };
        assert_eq!(*owner, registry::core_path("Order"));
        let [compare] = fields.as_slice() else {
            panic!()
        };
        let TypeKind::Function(function) = &value.types.get(compare.ty).unwrap().kind else {
            panic!()
        };
        assert_eq!(function.parameters.len(), 2);
        assert_eq!(function.allocation, Allocation::MayAllocate);
        assert_eq!(function.uses, Uses::Only(AuthoritySummary::inert()));
        assert!(matches!(
            value.types.get(function.result).unwrap().kind,
            TypeKind::Primitive(Primitive::I32)
        ));
        let ValueKind::FunctionReference(helper) = &compare.kind else {
            panic!()
        };
        assert_eq!(
            *helper,
            registry::core_path(&format!("$intrinsic.OrderCompare_{name}"))
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
    let mut foreign = registry::core_path("u32.order");
    foreign.0.package.name = "foreign".into();
    assert!(engine.materialize_value(&foreign).is_err());
}
