//! Logical representation controls; these do not qualify public seal execution.
use hirsel_interp::inventory::{
    BlockPlacement, Charge, ChargeRoot, LogicalBlockId, PlacementReceipt, heap_charge,
};
use hirsel_interp::{
    Value,
    aggregate::{Nominal, RuntimeTypes},
    value::{ArenaStorage, Environment, NamedFunction},
};
use hirsel_syntax::{DefId, DefPath};
use hirsel_types::{BuiltinType, GenericArgument, Multiplicity, Primitive, TypeId, testing::Types};

fn number(value: u64) -> Value {
    Value::Integer(hirsel_interp::Integer::new(Primitive::U64, u128::from(value)).unwrap())
}

fn sorted(capture: bool) -> (RuntimeTypes, TypeId, TypeId, Value, PlacementReceipt) {
    let mut types = Types::default();
    let word = types.p(Primitive::U64);
    let bytes = types.p(Primitive::Bytes);
    let callable = types.function(&[], word, Multiplicity::Many, 0);
    let sorted = types.builtin(
        BuiltinType::SortedMap,
        vec![
            GenericArgument::Type(word),
            GenericArgument::Type(bytes),
            GenericArgument::Type(callable),
        ],
    );
    let pair = types.tuple(&[word, bytes]);
    let array = types.builtin(BuiltinType::Array, vec![GenericArgument::Type(pair)]);
    let comparator = if capture {
        Value::Closure {
            invoked: Default::default(),
            definition: DefId::intern(&DefPath::core("seal_test_comparator")),
            body: hirsel_tir::BodyId(0),
            captures: vec![Some(Value::Bytes(b"xy".to_vec()))],
            environment: Box::new(Environment {
                storage: None,
                capture_types: vec![bytes],
                owned_captures: vec![true],
                sizes: Default::default(),
                type_arguments: Default::default(),
            }),
        }
    } else {
        Value::FunctionReference(Box::new(NamedFunction {
            definition: DefPath::core("seal_test_comparator"),
            arguments: vec![],
            types: types.store.clone(),
            ty: callable,
        }))
    };
    let entries = Value::Sequence {
        kind: BuiltinType::Array,
        elements: vec![
            Some(Value::Tuple(vec![
                Some(number(7)),
                Some(Value::Bytes(b"abc".to_vec())),
            ])),
            Some(Value::Tuple(vec![
                Some(number(9)),
                Some(Value::Bytes(b"12345".to_vec())),
            ])),
        ]
        .into(),
        arena: None,
    };
    let value = Value::Record {
        definition: DefId::intern(&DefPath::core("SortedMap")),
        fields: vec![Some(comparator), Some(entries)],
    };
    let mut receipt = PlacementReceipt::default();
    for path in [vec![1], vec![1, 0, 1], vec![1, 1, 1]] {
        receipt.blocks.insert(path, BlockPlacement::Heap);
    }
    if capture {
        receipt.blocks.insert(vec![0], BlockPlacement::Heap);
        receipt.blocks.insert(vec![0, 0], BlockPlacement::Heap);
        receipt.captures.insert(vec![0], vec![bytes]);
    }
    (
        RuntimeTypes::new(types.store),
        sorted,
        array,
        value,
        receipt,
    )
}

#[test]
fn sorted_map_entry_array_and_owned_comparator_are_distinct_occurrences() {
    for (capture, expected) in [(false, 40), (true, 50)] {
        let (runtime, ty, _, value, receipt) = sorted(capture);
        assert_eq!(
            heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).unwrap(),
            Charge::Bytes(expected)
        );
        let mut incomplete = receipt;
        incomplete.blocks.remove(&vec![1, 1, 1]);
        assert!(heap_charge(&runtime, ty, &value, 7, &incomplete, ChargeRoot::Value).is_err());
    }
}

#[test]
fn arena_sorted_entries_do_not_hide_heap_comparator_or_double_charge_array() {
    let (runtime, ty, array, mut value, mut receipt) = sorted(true);
    let Value::Record { fields, .. } = &mut value else {
        unreachable!()
    };
    let Value::Sequence { arena, .. } = fields[1].as_mut().unwrap() else {
        unreachable!()
    };
    let mut storage = ArenaStorage::fresh(7, 16);
    storage.logical_block = Some(LogicalBlockId(9));
    *arena = Some(storage);
    receipt.blocks.insert(
        vec![1],
        BlockPlacement::Arena {
            arena: 7,
            block: LogicalBlockId(9),
        },
    );
    assert_eq!(
        heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).unwrap(),
        Charge::Bytes(10)
    );
    let Value::Record { fields, .. } = &value else {
        unreachable!()
    };
    let entries = fields[1].as_ref().unwrap();
    let mut retained = PlacementReceipt::default();
    retained.blocks.insert(
        vec![],
        BlockPlacement::Arena {
            arena: 7,
            block: LogicalBlockId(9),
        },
    );
    retained.blocks.insert(vec![0, 1], BlockPlacement::Heap);
    retained.blocks.insert(vec![1, 1], BlockPlacement::Heap);
    assert_eq!(
        heap_charge(
            &runtime,
            array,
            entries,
            7,
            &retained,
            ChargeRoot::RetainedPayload
        )
        .unwrap(),
        Charge::Bytes(8)
    );
    let mut foreign = receipt;
    foreign.blocks.insert(
        vec![0],
        BlockPlacement::Arena {
            arena: 8,
            block: LogicalBlockId(10),
        },
    );
    assert!(heap_charge(&runtime, ty, &value, 7, &foreign, ChargeRoot::Value).is_err());
}

#[test]
fn closed_nominal_representation_charges_children_without_physical_framing() {
    let mut types = Types::default();
    let word = types.p(Primitive::U64);
    let text = types.p(Primitive::String);
    let parameter = types.param(0);
    let list = types.list(parameter);
    let shape = types.record(&[text, list]);
    let definition = DefPath::core("seal_test_FlatRepresentation");
    let ty = types.named(&definition, vec![GenericArgument::Type(word)]);
    let mut runtime = RuntimeTypes::new(types.store);
    // The reference engine retains logical fields, including for a declared
    // Flat owner; its encoding's byte prefixes and framing are native facts.
    runtime.nominals.push(Nominal {
        definition: definition.clone(),
        resource: false,
        distinct: false,
        finalizer: None,
        fields: 2,
        representation: Some(shape),
    });
    let value = Value::Record {
        definition: DefId::intern(&definition),
        fields: vec![
            Some(Value::String("abc".into())),
            Some(Value::Sequence {
                kind: BuiltinType::List,
                elements: vec![Some(number(7)), Some(number(9))].into(),
                arena: None,
            }),
        ],
    };
    let mut receipt = PlacementReceipt::default();
    receipt.blocks.insert(vec![0], BlockPlacement::Heap);
    receipt.blocks.insert(vec![1], BlockPlacement::Heap);
    assert_eq!(
        heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).unwrap(),
        Charge::Bytes(19)
    );
    // The inline nominal is not another semantic allocated block.
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
    receipt.blocks.insert(
        vec![1],
        BlockPlacement::Arena {
            arena: 7,
            block: LogicalBlockId(4),
        },
    );
    assert_eq!(
        heap_charge(&runtime, ty, &value, 7, &receipt, ChargeRoot::Value).unwrap(),
        Charge::Bytes(3)
    );
}
