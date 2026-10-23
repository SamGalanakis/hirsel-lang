use hirsel_layout::{logical_storage, structural};
use hirsel_lir::logical_storage::{LogicalBytes, StorageClass};
use hirsel_types::*;

fn builtin(types: &TypeInterner, kind: BuiltinType, element: TypeId) -> TypeId {
    types.intern(Type::with_kind(TypeKind::Builtin {
        kind,
        arguments: vec![GenericArgument::Type(element)].into(),
    }))
}

#[test]
fn storage_facts_reuse_the_structural_graph_and_keep_shared_boundaries() {
    let types = TypeInterner::new(None);
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let shared = builtin(&types, BuiltinType::Shared, bytes);
    let list = builtin(&types, BuiltinType::List, shared);
    let mut lookup = |_| Ok(None);
    let typed = structural::derive_typed(&types, list, &mut lookup)
        .unwrap()
        .unwrap();
    assert!(std::ptr::eq(typed.types, &*types));
    assert_eq!(typed.node_types, vec![list, shared, bytes]);
    let storage = logical_storage::derive(&types, list, &mut lookup)
        .unwrap()
        .unwrap();
    assert_eq!(storage.shape, typed.graph);
    assert_eq!(
        storage.facts.iter().map(|f| f.class).collect::<Vec<_>>(),
        vec![
            StorageClass::Block,
            StorageClass::Shared,
            StorageClass::Bytes
        ]
    );
    assert!(
        storage
            .facts
            .iter()
            .all(|f| f.bytes == LogicalBytes::Bytes(8))
    );
    assert!(logical_storage::validate(&storage, &types, list, &mut lookup).unwrap());
}

#[test]
fn missing_child_fact_wrong_size_and_wrong_boundary_reject_independently() {
    let types = TypeInterner::new(None);
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let shared = builtin(&types, BuiltinType::Shared, bytes);
    let mut lookup = |_| Ok(None);
    let good = logical_storage::derive(&types, shared, &mut lookup)
        .unwrap()
        .unwrap();
    assert!(logical_storage::validate(&good, &types, shared, &mut lookup).unwrap());
    let mut missing = good.clone();
    missing.facts.pop();
    assert!(!logical_storage::validate(&missing, &types, shared, &mut lookup).unwrap());
    let mut wrong = good.clone();
    wrong.facts[0].bytes = LogicalBytes::Bytes(16);
    assert!(!logical_storage::validate(&wrong, &types, shared, &mut lookup).unwrap());
    let mut erased = good.clone();
    erased.facts[0].class = StorageClass::Transparent;
    assert!(erased.check().is_ok()); // schema alone cannot prove MEM-11 custody
    assert!(!logical_storage::validate(&erased, &types, shared, &mut lookup).unwrap());
}

#[test]
fn inline_and_allocated_sequences_have_equal_shapes_and_distinct_storage() {
    let types = TypeInterner::new(None);
    let word = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let list = builtin(&types, BuiltinType::List, word);
    let inline = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Inline,
        arguments: vec![
            GenericArgument::Type(word),
            GenericArgument::Size(Size::Constant(2)),
        ]
        .into(),
    }));
    let mut lookup = |_| Ok(None);
    let a = logical_storage::derive(&types, list, &mut lookup)
        .unwrap()
        .unwrap();
    let b = logical_storage::derive(&types, inline, &mut lookup)
        .unwrap()
        .unwrap();
    assert_eq!(a.shape, b.shape);
    assert_eq!(a.facts[0].class, StorageClass::Block);
    assert_eq!(b.facts[0].class, StorageClass::InlineSequence { count: 2 });
    assert_eq!(b.facts[0].bytes, LogicalBytes::Bytes(16));
    assert!(!logical_storage::validate(&a, &types, inline, &mut lookup).unwrap());
    let mut changed_count = b.clone();
    changed_count.facts[0].class = StorageClass::InlineSequence { count: 3 };
    assert!(!logical_storage::validate(&changed_count, &types, inline, &mut lookup).unwrap());
}

#[test]
fn exact_storage_width_preserves_overflow() {
    let types = TypeInterner::new(None);
    let word = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let inline = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Inline,
        arguments: vec![
            GenericArgument::Type(word),
            GenericArgument::Size(Size::Constant(u64::MAX)),
        ]
        .into(),
    }));
    let mut lookup = |_| Ok(None);
    let good = logical_storage::derive(&types, inline, &mut lookup)
        .unwrap()
        .unwrap();
    assert_eq!(good.facts[0].bytes, LogicalBytes::Overflow);
    assert!(logical_storage::validate(&good, &types, inline, &mut lookup).unwrap());
    let mut saturated = good.clone();
    saturated.facts[0].bytes = LogicalBytes::Bytes(u64::MAX);
    assert!(!logical_storage::validate(&saturated, &types, inline, &mut lookup).unwrap());
}

#[test]
fn wrong_edges_cannot_be_repaired_by_matching_storage_facts() {
    let types = TypeInterner::new(None);
    let word = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let list = builtin(&types, BuiltinType::List, word);
    let mut lookup = |_| Ok(None);
    let good = logical_storage::derive(&types, list, &mut lookup)
        .unwrap()
        .unwrap();
    let mut wrong = good.clone();
    wrong.shape.nodes[0] = hirsel_lir::structural::Shape::Sequence(0);
    assert!(wrong.check().is_ok());
    assert!(!logical_storage::validate(&wrong, &types, list, &mut lookup).unwrap());
}

#[test]
fn logical_storage_graph_roundtrips_without_session_type_ids() {
    let types = TypeInterner::new(None);
    let byte = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8)));
    let list = builtin(&types, BuiltinType::List, byte);
    let graph = logical_storage::derive(&types, list, &mut |_| Ok(None))
        .unwrap()
        .unwrap();
    let encoded = hirsel_codec::to_text(&graph);
    let decoded = hirsel_codec::from_text(&encoded).unwrap();
    assert_eq!(graph, decoded);
}
