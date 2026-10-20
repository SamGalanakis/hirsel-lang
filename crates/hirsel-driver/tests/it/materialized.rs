//! Reified compile-time values keep their checked types and identities.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::Value;
use hirsel_span::Span;
use hirsel_tir::{Constant, Constructor, ValueKind};
use hirsel_types::{GenericArgument, TypeKind, TypeStore};
use hirsel_types::{Primitive, Type};
fn ty(kind: TypeKind) -> Type {
    Type::with_kind(kind)
}
#[test]
fn nominal_reification_uses_declared_generic_fields_and_preserves_identity() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("types.hir", "type Wrapper[T] { value: T }\n".into())
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    let mut path = entry_path(&default_context());
    path.0.name = "Wrapper".into();
    let mut types = TypeStore::default();
    let mut number = ty(TypeKind::Primitive(Primitive::U32));
    number.modes.sendability = hirsel_types::Sendability::Sendable;
    let number = types.push(number);
    let wrapper = types.push(ty(TypeKind::Named {
        definition: path.clone(),
        arguments: vec![GenericArgument::Type(number)].into(),
    }));
    let plan = engine
        .reify_value(
            &default_context().package,
            Value::Record {
                definition: (&path).into(),
                fields: vec![Some(Value::Integer(
                    hirsel_interp::Integer::new(Primitive::U32, 7).unwrap(),
                ))],
            },
            &types,
            wrapper,
            Span::default(),
        )
        .unwrap();
    let ValueKind::Construct {
        constructor: Constructor::Record(identity),
        fields,
    } = plan.root.kind
    else {
        panic!("expected record")
    };
    assert_eq!(identity, path);
    assert!(hirsel_types::semantic_equal(&plan.types, fields[0].ty, &types, number).unwrap());
}
#[test]
fn capability_materialization_is_a_language_error_without_hiding_malformed_values() {
    let mut types = TypeStore::default();
    let capability = types.push(ty(TypeKind::Named {
        definition: hirsel_syntax::DefPath::core("Io"),
        arguments: vec![].into(),
    }));
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let site = Span::default();
    let result = engine.reify_value(
        &default_context().package,
        Value::Handle {
            kind: "Io".into(),
            id: 0,
        },
        &types,
        capability,
        site,
    );
    let Err(QueryError::Diagnostics(diagnostics)) = result else {
        panic!("{result:?}")
    };
    assert_eq!(diagnostics[0].rule.as_str(), "CT-2");
    assert_eq!(diagnostics[0].primary, site);
    assert!(matches!(
        engine.reify_value(
            &default_context().package,
            Value::Tuple(vec![]),
            &types,
            capability,
            site
        ),
        Err(QueryError::Operational(_))
    ));
}
#[test]
fn partial_and_non_data_values_fail_closed() {
    let mut types = TypeStore::default();
    let boolean = types.push(ty(TypeKind::Primitive(Primitive::Bool)));
    let tuple = types.push(ty(TypeKind::Tuple(vec![boolean].into())));
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    assert!(matches!(
        engine.reify_value(
            &default_context().package,
            Value::Tuple(vec![None]),
            &types,
            tuple,
            Span::default()
        ),
        Err(QueryError::Operational(_))
    ));
    assert!(matches!(
        engine.reify_value(
            &default_context().package,
            Value::Handle {
                kind: "Io".into(),
                id: 0
            },
            &types,
            boolean,
            Span::default()
        ),
        Err(QueryError::Operational(_))
    ));
}
#[test]
fn finite_value_trees_materialize_at_and_past_the_old_32_level_guard() {
    with_compiler_stack(|| {
        for depth in [31, 32, 33, 200] {
            let mut types = TypeStore::default();
            let mut root = types.push(ty(TypeKind::Primitive(Primitive::U32)));
            let mut value = Value::Integer(hirsel_interp::Integer::new(Primitive::U32, 7).unwrap());
            for _ in 0..depth {
                root = types.push(ty(TypeKind::Tuple(vec![root].into())));
                value = Value::Tuple(vec![Some(value)]);
            }
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let materialized = engine
                .reify_value(
                    &default_context().package,
                    value,
                    &types,
                    root,
                    Span::default(),
                )
                .unwrap();
            let mut node = &materialized.root;
            for _ in 0..depth {
                let ValueKind::Construct {
                    constructor: Constructor::Tuple,
                    fields,
                } = &node.kind
                else {
                    panic!("expected tuple")
                };
                assert_eq!(fields.len(), 1);
                node = &fields[0];
            }
            assert_eq!(node.kind, ValueKind::Constant(Constant::Integer(7)));
        }
    });
}
