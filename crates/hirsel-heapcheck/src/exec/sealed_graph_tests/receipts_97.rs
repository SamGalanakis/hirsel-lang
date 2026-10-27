use super::*;
use hirsel_lir::sealed_payload::*;

fn setup(full: bool) -> (Fixture, Config, LayoutId, ValueId) {
    let mut x = fixture();
    set_root(&mut x.p, 2);
    let root = x.p.units[0].tables.seal_descriptors[2].root_type.clone();
    let mut receipt = x.p.layouts.layouts[1].clone();
    receipt.kind = LayoutKind::PayloadReceipt { root: root.clone() };
    receipt.drop = vec![DropStep::ReleaseReceipt { offset: 0 }];
    receipt.scan.clear();
    let layout = LayoutId(x.p.layouts.layouts.len() as u32);
    x.p.layouts.layouts.push(receipt);
    let unit = &mut x.p.units[0];
    let ty = LirTypeId(unit.types.len() as u32);
    unit.types.push(LirType {
        layout,
        repr: ValueRepr::PayloadReceipt,
    });
    let result = ValueId(unit.values.len() as u32);
    unit.values.push(ValueInfo {
        ty,
        holding: Holding::Owned,
    });
    let scalar_ty = LirTypeId(unit.types.len() as u32);
    unit.types.push(LirType {
        layout: LayoutId(0),
        repr: ValueRepr::Scalar(ScalarKind::U64),
    });
    let values = [
        ValueId(unit.values.len() as u32),
        ValueId(unit.values.len() as u32 + 1),
    ];
    unit.values.extend(values.map(|_| ValueInfo {
        ty: scalar_ty,
        holding: Holding::Plain,
    }));
    let receipt_inputs = PoolRange::push(
        &mut unit.pools.receipt_inputs,
        values.map(|value| ReceiptInput {
            original: value,
            receipt: None,
        }),
    );
    assert_eq!(receipt_inputs.start, 0);
    let fields = PoolRange::push(
        &mut unit.pools.fields,
        values
            .into_iter()
            .enumerate()
            .map(|(field, value)| FieldValue {
                field: field as u32,
                value,
            }),
    );
    let recipe = PayloadRecipe {
        source: unit.tables.source_anchors[0].clone(),
        operation: PayloadOperation::Allocate {
            destination: PayloadDestination::Heap,
        },
        inputs: [11u128, 12]
            .into_iter()
            .enumerate()
            .map(|(place, value)| PayloadOperand {
                place: place as u32,
                ty: T::Primitive(P::U64),
                producer: PayloadProducer::Literal(PayloadLiteral::Integer(value)),
                capture_slot: None,
            })
            .collect(),
    };
    unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        tracked: true,
        constructor: ConstructorKind::Sequence,
        layout: x.lists[1],
        destination: DestinationClass::Heap,
        placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
        fields,
        anchor: SourceAnchorId(0),
    });
    unit.tables.seal_payloads.push(PayloadTable {
        descriptor: SealDescriptorId(2),
        bindings: vec![PayloadBinding {
            recipe: recipe.clone(),
            values: values.to_vec(),
        }],
    });
    let mut c = config(&mut x.p, 2, &[], full);
    c.seal_sources[0].events.clear();
    c.seal_sources[0].payloads = vec![SourcePayload {
        root_type: root,
        recipe,
    }];
    let source = &mut c.seal_sources[0];
    source.witness = SealSourceWitness::from_expectations(
        source.witness.source,
        source.reference.definition.clone(),
        source.witness.implementation_revision,
        source.reference.instance,
        &source.events,
        &source.payloads,
    );
    (x, c, layout, result)
}
fn snapshot(m: &mut Machine<'_, Rc>, f: &mut Frame, result: ValueId) -> Ptr {
    for (index, bits) in [11u128, 12].into_iter().enumerate() {
        let value = ValueId(result.0 + index as u32 + 1);
        m.define_fresh(
            f,
            value,
            Val::Bits {
                scalar: ScalarKind::U64,
                bits,
            },
        )
        .unwrap();
    }
    m.exec(
        f,
        &Inst::PayloadReceipt {
            result,
            value: ValueId(4),
            shape: SealDescriptorId(2),
            binding: PayloadBindingId(0),
            inputs: PoolRange::new(0, 2),
        },
    )
    .unwrap();
    let Val::Ptr(control) = m.read_use(f, result).unwrap() else {
        panic!()
    };
    control
}
fn release(m: &mut Machine<'_, Rc>, f: &mut Frame, value: ValueId, layout: LayoutId) {
    let moved = m.take_value(f, value).unwrap();
    let storage = m.ledger.alloc(Owner::Frame, Some(layout), 8, 8, 8, None);
    m.write_value(storage, layout, moved).unwrap();
    drain(
        m,
        vec![Work::DropAt {
            at: storage,
            layout,
            mask: None,
        }],
    );
    m.ledger.free(storage.alloc).unwrap();
}
#[test]
fn h_receipt_snapshots_do_not_retain_or_release_language_payloads() {
    for full in [false, true] {
        let (x, c, layout, result) = setup(full);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, grant) = prepare(&mut m, 100);
        let ptr = nums(&mut m, false, x.payloads[1], &[11, 12], 1);
        root(&mut m, &mut f, Val::Ptr(ptr));
        m.seal_allocations.insert(
            ptr.alloc,
            (0, x.p.units[0].tables.source_anchors[0].clone()),
        );
        let control = snapshot(&mut m, &mut f, result);
        recount(&mut m, &f);
        assert_eq!(m.ledger.get(ptr.alloc).count, Some(Count::Finite(1)));
        release(&mut m, &mut f, result, layout);
        assert_eq!(m.ledger.get(control.alloc).state, AllocState::Dead);
        assert_eq!(m.ledger.get(ptr.alloc).state, AllocState::Live);
        assert_eq!(m.ledger.get(ptr.alloc).count, Some(Count::Finite(1)));
        release(&mut m, &mut f, ValueId(4), x.lists[1]);
        m.exit_region(&mut f, 0).unwrap();
        m.ledger.free(grant.alloc).unwrap();
        m.audit().unwrap();
    }
}
#[test]
fn h_receipt_pairing_detects_changed_payload_cells_with_a_repaired_control() {
    let (x, c, layout, result) = setup(false);
    let mut ops = NoOperations;
    let mut m = Machine::new(&x.p, &c, &mut ops);
    let (mut f, _) = prepare(&mut m, 100);
    let ptr = nums(&mut m, false, x.payloads[1], &[11, 12], 1);
    root(&mut m, &mut f, Val::Ptr(ptr));
    m.seal_allocations.insert(
        ptr.alloc,
        (0, x.p.units[0].tables.source_anchors[0].clone()),
    );
    let control = snapshot(&mut m, &mut f, result);
    let root = &x.p.units[0].tables.seal_descriptors[2].root_type;
    m.check_receipt_pair(control, layout, root, Val::Ptr(ptr))
        .unwrap();
    m.ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(16, &99u64.to_le_bytes());
    assert_eq!(
        m.check_receipt_pair(control, layout, root, Val::Ptr(ptr))
            .unwrap_err()
            .kind,
        V::TypeConfusion
    );
    m.ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(16, &11u64.to_le_bytes());
    m.check_receipt_pair(control, layout, root, Val::Ptr(ptr))
        .unwrap();
}
#[test]
fn h_receipt_forged_bytes_and_wrong_construction_identity_are_rejected() {
    let (x, c, layout, result) = setup(false);
    let mut ops = NoOperations;
    let mut m = Machine::new(&x.p, &c, &mut ops);
    let (mut f, _) = prepare(&mut m, 100);
    let mut bytes = Extent::new(8);
    bytes.write_bytes(0, &1u64.to_le_bytes());
    assert_eq!(
        m.val_from(bytes, ValueRepr::PayloadReceipt, layout)
            .unwrap_err()
            .kind,
        V::ForgedPointer
    );
    let ptr = nums(&mut m, false, x.payloads[1], &[11, 12], 1);
    root(&mut m, &mut f, Val::Ptr(ptr));
    let instruction = Inst::PayloadReceipt {
        result,
        value: ValueId(4),
        shape: SealDescriptorId(2),
        binding: PayloadBindingId(0),
        inputs: PoolRange::new(0, 2),
    };
    for (index, bits) in [11u128, 12].into_iter().enumerate() {
        m.define_fresh(
            &mut f,
            ValueId(result.0 + index as u32 + 1),
            Val::Bits {
                scalar: ScalarKind::U64,
                bits,
            },
        )
        .unwrap();
    }
    assert_eq!(
        m.exec(&mut f, &instruction).err().unwrap().kind,
        V::Malformed
    );
    m.seal_allocations.insert(
        ptr.alloc,
        (0, x.p.units[0].tables.source_anchors[0].clone()),
    );
    m.exec(&mut f, &instruction).unwrap();
}

fn call_setup(take: bool) -> (Fixture, Config, LayoutId, ValueId, CallSite) {
    use hirsel_core_contract::Passing;
    use hirsel_lir::abi_types::{AbiParameter, FnAbiId, ParameterPassing, ScalarExtension};
    use hirsel_syntax::ContentHash;
    let (mut x, mut c, layout, result) = setup(false);
    let profile = ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: ClosedStorageType {
            ty: x.p.units[0].tables.seal_descriptors[2].root_type.clone(),
            source_type: ContentHash([7; 32]),
        },
        functions: vec![],
    };
    let contract = ReceiptContract::new(
        vec![ReceiptParameter {
            source: 0,
            ty: ReceiptType {
                source_type: profile.root.source_type,
                profile: profile.content().unwrap(),
            },
            mode: if take {
                ReceiptMode::Take
            } else {
                ReceiptMode::Borrow
            },
        }],
        None,
        vec![],
        vec![],
    )
    .unwrap();
    let mut abi = x.p.layouts.fn_abis[0].clone();
    abi.parameters = vec![AbiParameter {
        source: 0,
        convention: if take { Passing::Take } else { Passing::Borrow },
        passing: ParameterPassing::ByValue,
        layout: x.lists[1],
        slots: vec![],
        extension: ScalarExtension::None,
    }];
    abi.key.receipt_contract = Some(contract.content().unwrap());
    abi.receipts = Some(contract.clone());
    let callee_abi = FnAbiId(x.p.layouts.fn_abis.len() as u32);
    x.p.layouts.fn_abis.push(abi);
    let mut callee = x.p.units[0].clone();
    callee.header.definition = hirsel_syntax::DefPath::core("h_receipt_callee_97");
    callee.header.abi = callee_abi;
    callee.parameters = vec![ValueId(4)];
    callee.slots.clear();
    callee.slots.push(SlotInfo {
        ty: callee.values[result.index()].ty,
        kind: SlotKind::ReceiptParameter(0),
        scope: ScopeId(0),
    });
    let version = ValueId(callee.values.len() as u32);
    callee.values.push(ValueInfo {
        ty: callee.values[result.index()].ty,
        holding: Holding::Plain,
    });
    callee.tables.receipt_entry = Some(ReceiptEntry {
        contract: contract.content().unwrap(),
        parameters: vec![ReceiptEntrySlot {
            source: 0,
            slot: SlotId(0),
            version,
        }],
        captures: vec![],
        result: None,
        environment: None,
    });
    c.receipt_sources.push(ReceiptSource {
        original: hirsel_lir::invocations::ReferenceBody {
            definition: callee.header.definition.clone(),
            instance: ContentHash([9; 32]),
            body: 0,
        },
        instance: callee.header.instance,
        source: ContentHash([8; 32]),
        implementation_revision: callee.header.implementation_revision,
        contract: contract.clone(),
        profiles: vec![ReceiptProfile {
            port: ReceiptPort::Parameter(0),
            profile,
        }],
    });
    x.p.units.push(callee);
    let arguments = PoolRange::push(
        &mut x.p.units[0].pools.call_arguments,
        [CallArgument {
            parameter: 0,
            passing: if take { Passing::Take } else { Passing::Borrow },
            value: ArgumentValue::Value(ValueId(4)),
        }],
    );
    let site = CallSite {
        pretransfer_failure: None,
        forward_caller: false,
        callee: Callee::Indirect(ValueId(0)),
        operation_arguments: vec![],
        abi: callee_abi,
        arguments,
        result: None,
        destination: None,
        declared: hirsel_lir::effect::Effects::MAY_FAIL,
        anchor: SourceAnchorId(0),
        receipts: Some(ReceiptCall {
            contract: contract.content().unwrap(),
            arguments: vec![if take {
                ReceiptArgument::Take {
                    parameter: 0,
                    receipt: result,
                }
            } else {
                ReceiptArgument::Borrow {
                    parameter: 0,
                    receipt: result,
                }
            }],
            result: None,
        }),
    };
    (x, c, layout, result, site)
}
#[test]
fn h_receipt_call_borrow_and_take_bind_real_entry_slots_and_preserve_custody() {
    for take in [false, true] {
        let (x, c, layout, result, site) = call_setup(take);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, _) = prepare(&mut m, 100);
        let ptr = nums(&mut m, false, x.payloads[1], &[11, 12], 1);
        root(&mut m, &mut f, Val::Ptr(ptr));
        m.seal_allocations.insert(
            ptr.alloc,
            (0, x.p.units[0].tables.source_anchors[0].clone()),
        );
        let control = snapshot(&mut m, &mut f, result);
        let mut callee = m.frame(1, Role::Callee, None);
        m.bind_receipt_call(&mut f, &mut callee, &site).unwrap();
        assert_eq!(m.get(&f, result).unwrap().credits, u32::from(!take));
        let SlotStore::Own(storage) = callee.slots[0] else {
            panic!()
        };
        assert!(
            matches!(m.read_cell(storage).unwrap(), Cell::Data { ptr, counted, .. } if ptr == control && counted == take)
        );
        let version = x.p.units[1]
            .tables
            .receipt_entry
            .as_ref()
            .unwrap()
            .parameters[0]
            .version;
        assert_eq!(callee.versions[0], Some(version));
        assert!(matches!(m.get(&callee, version).unwrap().val, Val::Version));
        if take {
            m.release_receipt(storage, layout).unwrap();
            assert_eq!(m.ledger.get(control.alloc).state, AllocState::Dead);
        } else {
            assert_eq!(
                m.release_receipt(storage, layout).unwrap_err().kind,
                V::CreditUnderflow
            );
            assert_eq!(m.ledger.get(control.alloc).state, AllocState::Live);
            m.ledger
                .get_mut(storage.alloc)
                .extent
                .take(0, 8, FREE_POISON, None);
            release(&mut m, &mut f, result, layout);
        }
        assert_eq!(m.ledger.get(ptr.alloc).count, Some(Count::Finite(1)));
    }
}
#[test]
fn h_receipt_call_rejects_another_snapshot_before_taking_metadata() {
    let (x, c, _, result, site) = call_setup(true);
    let mut ops = NoOperations;
    let mut m = Machine::new(&x.p, &c, &mut ops);
    let (mut f, _) = prepare(&mut m, 100);
    let ptr = nums(&mut m, false, x.payloads[1], &[11, 12], 1);
    root(&mut m, &mut f, Val::Ptr(ptr));
    m.seal_allocations.insert(
        ptr.alloc,
        (0, x.p.units[0].tables.source_anchors[0].clone()),
    );
    snapshot(&mut m, &mut f, result);
    m.ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(16, &99u64.to_le_bytes());
    let mut callee = m.frame(1, Role::Callee, None);
    assert_eq!(
        m.bind_receipt_call(&mut f, &mut callee, &site)
            .unwrap_err()
            .kind,
        V::TypeConfusion
    );
    assert_eq!(m.get(&f, result).unwrap().credits, 1);
    assert!(matches!(callee.slots[0], SlotStore::Dead));
    m.ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(16, &11u64.to_le_bytes());
    m.bind_receipt_call(&mut f, &mut callee, &site).unwrap();
}

#[test]
fn h_borrowed_receipt_entry_cannot_be_taken_or_retired_before_return() {
    let (mut x, c, layout, result, site) = call_setup(false);
    let ty = x.p.units[1].values[result.index()].ty;
    x.p.units[1].values[result.index()].holding = Holding::Plain;
    x.p.units[1].addresses.push(AddressInfo {
        base: AddressBase::Slot(SlotId(0)),
        source_place: None,
        unchecked: vec![],
        projection: PoolRange::EMPTY,
        ty,
    });
    let address = AddressId(x.p.units[1].addresses.len() as u32 - 1);
    let mut ops = NoOperations;
    let mut m = Machine::new(&x.p, &c, &mut ops);
    let (mut f, _) = prepare(&mut m, 100);
    let ptr = nums(&mut m, false, x.payloads[1], &[11, 12], 1);
    root(&mut m, &mut f, Val::Ptr(ptr));
    m.seal_allocations.insert(
        ptr.alloc,
        (0, x.p.units[0].tables.source_anchors[0].clone()),
    );
    let control = snapshot(&mut m, &mut f, result);
    let mut callee = m.frame(1, Role::Callee, None);
    m.bind_receipt_call(&mut f, &mut callee, &site).unwrap();
    let version = x.p.units[1]
        .tables
        .receipt_entry
        .as_ref()
        .unwrap()
        .parameters[0]
        .version;
    let instruction = Inst::Read {
        result,
        address,
        reaching: version,
        mode: ReadMode::Take,
    };
    assert_eq!(
        m.exec(&mut callee, &instruction).err().unwrap().kind,
        V::WritePublished
    );
    assert_eq!(
        m.exec(
            &mut callee,
            &Inst::StorageDead {
                slot: SlotId(0),
                if_live: false
            }
        )
        .err()
        .unwrap()
        .kind,
        V::BorrowExpired
    );
    assert_eq!(
        m.exec(&mut callee, &Inst::StorageLive { slot: SlotId(0) })
            .err()
            .unwrap()
            .kind,
        V::BorrowExpired
    );
    m.check_receipt_cell(control, layout).unwrap();
    m.exec(
        &mut callee,
        &Inst::Read {
            result,
            address,
            reaching: version,
            mode: ReadMode::Borrow,
        },
    )
    .unwrap();
}

#[test]
fn h_receipt_source_operands_reject_a_changed_constructor_before_snapshot_publication() {
    let (x, c, _, result) = setup(false);
    let mut ops = NoOperations;
    let mut m = Machine::new(&x.p, &c, &mut ops);
    let (mut f, _) = prepare(&mut m, 100);
    let ptr = nums(&mut m, false, x.payloads[1], &[99, 12], 1);
    root(&mut m, &mut f, Val::Ptr(ptr));
    m.seal_allocations.insert(
        ptr.alloc,
        (0, x.p.units[0].tables.source_anchors[0].clone()),
    );
    for (index, bits) in [11u128, 12].into_iter().enumerate() {
        m.define_fresh(
            &mut f,
            ValueId(result.0 + index as u32 + 1),
            Val::Bits {
                scalar: ScalarKind::U64,
                bits,
            },
        )
        .unwrap();
    }
    let instruction = Inst::PayloadReceipt {
        result,
        value: ValueId(4),
        shape: SealDescriptorId(2),
        binding: PayloadBindingId(0),
        inputs: PoolRange::new(0, 2),
    };
    assert_eq!(
        m.exec(&mut f, &instruction).err().unwrap().kind,
        V::TypeConfusion
    );
    assert!(f.values[result.index()].is_none());
    m.ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(16, &11u64.to_le_bytes());
    m.exec(&mut f, &instruction).unwrap();
}

#[test]
fn h_historical_payload_charge_survives_physical_elision_without_keeping_holders() {
    for full in [false, true] {
        let (x, c, layout, result) = setup(full);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, grant) = prepare(&mut m, 100);
        let ptr = nums(&mut m, false, x.payloads[1], &[11, 12], 1);
        root(&mut m, &mut f, Val::Ptr(ptr));
        m.seal_allocations.insert(
            ptr.alloc,
            (0, x.p.units[0].tables.source_anchors[0].clone()),
        );
        let control = snapshot(&mut m, &mut f, result);
        let saved = m.receipt_control(control, layout).unwrap().clone();
        let descriptor = &x.p.units[0].tables.seal_descriptors[2];
        let charge = |m: &mut Machine<'_, Rc>| {
            m.receipt_occurrences(
                &saved,
                descriptor,
                0,
                &mut std::collections::BTreeSet::new(),
                &mut BTreeMap::new(),
            )
            .unwrap()
        };
        assert_eq!(charge(&mut m), Some(16));
        release(&mut m, &mut f, ValueId(4), x.lists[1]);
        assert_eq!(m.ledger.get(ptr.alloc).state, AllocState::Dead);
        let allocations = m.ledger.counters.allocations;
        assert_eq!(charge(&mut m), Some(16));
        assert_eq!(m.ledger.get(ptr.alloc).state, AllocState::Dead);
        assert_eq!(m.ledger.counters.allocations, allocations);
        assert_eq!(m.receipt_control(control, layout).unwrap(), &saved);
        recount(&mut m, &f);
        release(&mut m, &mut f, result, layout);
        m.exit_region(&mut f, 0).unwrap();
        m.ledger.free(grant.alloc).unwrap();
        m.audit().unwrap();
    }
}

#[test]
fn h_historical_charge_still_counts_repeated_heap_occurrences() {
    let mut x = fixture();
    let c = config(&mut x.p, 1, &[], false);
    let mut ops = NoOperations;
    let mut m = Machine::new(&x.p, &c, &mut ops);
    let (mut f, _) = prepare(&mut m, 100);
    let ptr = byte_list(&mut m, &x, true, 3, false, 2);
    let descriptor = &x.p.units[0].tables.seal_descriptors[1];
    let saved = m
        .receipt_snapshot(descriptor.root_type.clone(), Val::Ptr(ptr))
        .unwrap();
    let mut shared = BTreeMap::new();
    assert_eq!(
        m.receipt_occurrences(
            &saved,
            descriptor,
            0,
            &mut std::collections::BTreeSet::new(),
            &mut shared
        )
        .unwrap(),
        Some(6)
    );
    assert!(shared.is_empty());
    // Corrupting the later physical payload cannot rewrite a recorded source
    // snapshot. The normal physical seal proof still inspects current cells.
    m.ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(0, &0u64.to_le_bytes());
    assert_eq!(
        m.receipt_occurrences(
            &saved,
            descriptor,
            0,
            &mut std::collections::BTreeSet::new(),
            &mut shared
        )
        .unwrap(),
        Some(6)
    );
    assert_eq!(m.scan_scalar(ptr, 8).unwrap(), 0);
    f.values[4] = None;
}
