//! Pure inventory witnesses. Public execution is qualified separately.
use hirsel_interp::inventory::{ArenaInventory, Charge, LogicalBlockId};
use hirsel_interp::inventory::{BlockPlacement, ChargeRoot, PlacementReceipt, heap_charge};
use hirsel_interp::{Value, aggregate::RuntimeTypes};
use hirsel_types::{BuiltinType, GenericArgument, Primitive, Type, TypeKind};

fn bytes_list() -> (RuntimeTypes, hirsel_types::TypeId, Value) {
    let types = hirsel_types::TypeInterner::new(None);
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let list = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(bytes)].into(),
    }));
    (
        RuntimeTypes::new(types),
        list,
        Value::Sequence {
            kind: BuiltinType::List,
            elements: vec![
                Some(Value::Bytes(b"abc".to_vec())),
                Some(Value::Bytes(b"abc".to_vec())),
            ]
            .into(),
            arena: None,
        },
    )
}

#[test]
fn typed_heap_root_and_retained_payload_follow_logical_occurrences() {
    let (runtime, ty, value) = bytes_list();
    let mut receipt = PlacementReceipt::default();
    for path in [vec![], vec![0], vec![1]] {
        receipt.blocks.insert(path, BlockPlacement::Heap);
    }
    assert_eq!(
        heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).unwrap(),
        Charge::Bytes(22)
    );
    receipt.blocks.insert(
        vec![],
        BlockPlacement::Arena {
            arena: 7,
            block: LogicalBlockId(0),
        },
    );
    assert_eq!(
        heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).unwrap(),
        Charge::ZERO
    );
    assert_eq!(
        heap_charge(
            &runtime,
            ty,
            &value,
            7,
            &receipt,
            ChargeRoot::RetainedPayload
        )
        .unwrap(),
        Charge::Bytes(6)
    );
}

#[test]
fn missing_placement_and_foreign_arena_are_rejected() {
    let (runtime, ty, value) = bytes_list();
    let mut receipt = PlacementReceipt::default();
    assert!(heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).is_err());
    receipt.blocks.insert(vec![], BlockPlacement::Heap);
    receipt.blocks.insert(vec![0], BlockPlacement::Heap);
    receipt.blocks.insert(
        vec![1],
        BlockPlacement::Arena {
            arena: 8,
            block: LogicalBlockId(0),
        },
    );
    assert!(heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).is_err());
    assert!(
        heap_charge(
            &runtime,
            ty,
            &value,
            7,
            &receipt,
            ChargeRoot::RetainedPayload
        )
        .is_err()
    );
}

#[test]
fn collection_receipt_must_match_its_semantic_identity() {
    let (runtime, ty, mut value) = bytes_list();
    let Value::Sequence { arena, .. } = &mut value else {
        unreachable!()
    };
    let mut tag = hirsel_interp::value::ArenaStorage::fresh(7, 8);
    tag.logical_block = Some(LogicalBlockId(4));
    *arena = Some(tag);
    let mut receipt = PlacementReceipt::default();
    receipt.blocks.insert(
        vec![],
        BlockPlacement::Arena {
            arena: 7,
            block: LogicalBlockId(5),
        },
    );
    assert!(heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).is_err());
    receipt.blocks.insert(
        vec![],
        BlockPlacement::Arena {
            arena: 7,
            block: LogicalBlockId(4),
        },
    );
    assert_eq!(
        heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).unwrap(),
        Charge::ZERO
    );
}

#[test]
fn inline_root_sums_heap_fields_and_visits_only_active_variant() {
    let types = hirsel_types::TypeInterner::new(None);
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let either = types.intern(Type::with_kind(TypeKind::Variant(
        vec![
            hirsel_types::Variant {
                name: "Empty".into(),
                fields: vec![].into(),
            },
            hirsel_types::Variant {
                name: "Data".into(),
                fields: vec![(None, bytes)].into(),
            },
        ]
        .into(),
    )));
    let runtime = RuntimeTypes::new(types);
    let empty = Value::Variant {
        variant: 0,
        fields: vec![],
    };
    let data = Value::Variant {
        variant: 1,
        fields: vec![Some(Value::Bytes(b"abc".to_vec()))],
    };
    let mut receipt = PlacementReceipt::default();
    assert_eq!(
        heap_charge(&runtime, either, &empty, 7, &receipt, ChargeRoot::Value).unwrap(),
        Charge::ZERO
    );
    receipt.blocks.insert(vec![0], BlockPlacement::Heap);
    assert_eq!(
        heap_charge(&runtime, either, &data, 7, &receipt, ChargeRoot::Value).unwrap(),
        Charge::Bytes(3)
    );
    let malformed = Value::Variant {
        variant: 2,
        fields: vec![],
    };
    assert!(heap_charge(&runtime, either, &malformed, 7, &receipt, ChargeRoot::Value).is_err());
}

fn accepts(cap: u64, construction: u64, seal: Charge) -> bool {
    cap.checked_sub(construction)
        .is_some_and(|remaining| seal.fits(remaining))
}

#[test]
fn equal_distinct_and_shared_leaves_charge_per_occurrence() {
    for bytes in [1, 3] {
        for leaves in [[17, 17], [17, 18]] {
            let mut inventory = ArenaInventory::default();
            inventory.allocate(leaves, Charge::Bytes(bytes).times(2));
            let charge = inventory.seal_charge(Charge::ZERO);
            let total = 16 + 2 * bytes;
            assert!(!accepts(total - 1, 16, charge));
            assert!(accepts(total, 16, charge));
        }
    }
}

#[test]
fn growth_replaces_payload_but_cow_retains_both_allocations() {
    let mut grown = ArenaInventory::default();
    let id = grown.allocate(vec![17], Charge::Bytes(3));
    assert_eq!(
        grown.replace(id, vec![17, 17], Charge::Bytes(6)),
        Ok(vec![17])
    );
    assert_eq!(grown.entries().len(), 1);
    assert_eq!(grown.seal_charge(Charge::ZERO), Charge::Bytes(6));
    assert!(!accepts(21, 16, grown.seal_charge(Charge::ZERO)));
    assert!(accepts(22, 16, grown.seal_charge(Charge::ZERO)));

    let mut copied = ArenaInventory::default();
    let old = copied.allocate(vec![17], Charge::Bytes(3));
    let new = copied.allocate(vec![17, 17], Charge::Bytes(6));
    assert_ne!(old, new);
    // Original 8, COW 8, push 8; the dead original retains its heap edge.
    assert!(!accepts(32, 24, copied.seal_charge(Charge::ZERO)));
    assert!(accepts(33, 24, copied.seal_charge(Charge::ZERO)));
}

#[test]
fn heap_root_is_added_once_and_arena_children_are_separate_entries() {
    let mut inventory = ArenaInventory::default();
    // Parent -> arena child stops; child -> heap leaf owns the obligation.
    inventory.allocate("parent", Charge::ZERO);
    inventory.allocate("child", Charge::Bytes(8));
    assert_eq!(inventory.seal_charge(Charge::ZERO), Charge::Bytes(8));
    assert_eq!(inventory.seal_charge(Charge::Bytes(16)), Charge::Bytes(24));
    assert_eq!(inventory.entries().len(), 2);
}

#[test]
fn replacement_retires_removed_edges_and_shared_boundaries_add_zero() {
    let mut inventory = ArenaInventory::default();
    let id = inventory.allocate("ordinary heap edge", Charge::Bytes(3));
    assert_eq!(
        inventory.replace(id, "Shared root", Charge::ZERO),
        Ok("ordinary heap edge")
    );
    assert_eq!(inventory.seal_charge(Charge::ZERO), Charge::ZERO);
    assert!(accepts(0, 0, inventory.seal_charge(Charge::ZERO)));
    assert_eq!(inventory.entries().len(), 1);
}

#[test]
fn missing_replacement_keeps_the_inventory_and_returns_custody() {
    let mut inventory = ArenaInventory::default();
    let id = inventory.allocate("retained", Charge::Bytes(3));
    assert_eq!(
        inventory.replace(LogicalBlockId(id.0 + 1), "new", Charge::Bytes(99)),
        Err("new")
    );
    assert_eq!(inventory.entries()[&id].payload, "retained");
    assert_eq!(inventory.seal_charge(Charge::ZERO), Charge::Bytes(3));
}

#[test]
fn overflow_is_over_capacity_even_at_max_and_replacement_recovers() {
    let mut inventory = ArenaInventory::default();
    let first = inventory.allocate((), Charge::Bytes(u64::MAX));
    let second = inventory.allocate((), Charge::Bytes(1));
    assert_eq!(inventory.seal_charge(Charge::ZERO), Charge::Overflow);
    assert!(!inventory.seal_charge(Charge::ZERO).fits(u64::MAX));
    assert_eq!(inventory.replace(second, (), Charge::ZERO), Ok(()));
    assert_eq!(inventory.seal_charge(Charge::ZERO), Charge::Bytes(u64::MAX));
    assert!(inventory.seal_charge(Charge::ZERO).fits(u64::MAX));
    assert_eq!(inventory.seal_charge(Charge::Bytes(1)), Charge::Overflow);
    assert_eq!(inventory.replace(first, (), Charge::Overflow), Ok(()));
    assert_eq!(inventory.seal_charge(Charge::ZERO), Charge::Overflow);
    assert_eq!(inventory.replace(first, (), Charge::Bytes(3)), Ok(()));
    assert_eq!(inventory.seal_charge(Charge::ZERO), Charge::Bytes(3));
    assert_eq!(Charge::Overflow.times(0), Charge::ZERO);
    assert_eq!(Charge::Bytes(u64::MAX).times(2), Charge::Overflow);
}
