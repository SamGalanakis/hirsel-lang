//! Producer reference/liveness controls, not independently accepted C transport.
use hirsel_lir::abi_types::FnAbiId;
use hirsel_lir::effect::Effects;
use hirsel_lir::ir::*;
use hirsel_lir::receipt_transport::{ReceiptArgument, ReceiptCall};
use hirsel_lir::stage::{StageKind, Value};
use hirsel_syntax::ContentHash;

fn unit() -> LirUnit<Value> {
    let mut unit = LirUnit::new(UnitHeader {
        source_count: 1,
        track_caller: false,
        stage: StageKind::Value,
        lir_version: hirsel_lir::LIR_VERSION,
        origin: UnitOrigin::Source,
        key: hirsel_lir::objects::UnitKey(ContentHash([0; 32])),
        definition: hirsel_syntax::DefPath::core("receipt_transport_test"),
        implementation_revision: ContentHash([1; 32]),
        instance: ContentHash([2; 32]),
        target: ContentHash([3; 32]),
        abi: FnAbiId(0),
    });
    let arguments = PoolRange::push(
        &mut unit.pools.call_arguments,
        [
            CallArgument {
                parameter: 0,
                passing: hirsel_core_contract::Passing::Borrow,
                value: ArgumentValue::Value(ValueId(1)),
            },
            CallArgument {
                parameter: 1,
                passing: hirsel_core_contract::Passing::Mut,
                value: ArgumentValue::Address {
                    address: AddressId(0),
                    version: ValueId(3),
                },
            },
        ],
    );
    unit.tables.calls.push(CallSite {
        pretransfer_failure: Some(FailureId(5)),
        forward_caller: false,
        callee: Callee::Bound {
            code: SymbolId(0),
            closure: ValueId(0),
        },
        operation_arguments: vec![],
        abi: FnAbiId(0),
        arguments,
        result: Some(ValueId(7)),
        destination: None,
        declared: Effects::MAY_FAIL,
        anchor: SourceAnchorId(0),
        receipts: Some(ReceiptCall {
            contract: ContentHash([4; 32]),
            arguments: vec![
                ReceiptArgument::Borrow {
                    parameter: 0,
                    receipt: ValueId(10),
                },
                ReceiptArgument::Take {
                    parameter: 1,
                    receipt: ValueId(11),
                },
                ReceiptArgument::Mut {
                    parameter: 2,
                    address: AddressId(6),
                    version: ValueId(12),
                },
            ],
            result: Some(ValueId(13)),
        }),
    });
    unit
}

#[test]
fn liveness_reads_receipt_inputs_and_defines_result_and_mutable_writeback() {
    let unit = unit();
    let term = Terminator::Call {
        call: CallId(0),
        normal: Some(EdgeId(0)),
        failure: FailureId(0),
    };
    assert_eq!(
        crate::liveness::terminator_defs(&unit, &term),
        vec![ValueId(7), ValueId(13), ValueId(3), ValueId(12)]
    );
    assert_eq!(
        crate::liveness::terminator_uses(&unit, &term),
        vec![ValueId(0), ValueId(10), ValueId(11), ValueId(1)]
    );
}

#[test]
fn rc_value_remapping_preserves_metadata_pairing_selectors_and_contract_identity() {
    let mut unit = unit();
    let contract = unit.tables.calls[0].receipts.as_ref().unwrap().contract;
    crate::insert::remap_values(&mut unit, &|value| ValueId(value.0 + 20));
    let call = &unit.tables.calls[0];
    assert_eq!(
        call.normal_results().collect::<Vec<_>>(),
        vec![ValueId(27), ValueId(33)]
    );
    assert_eq!(
        call.writebacks(&unit.pools).collect::<Vec<_>>(),
        vec![(AddressId(0), ValueId(23)), (AddressId(6), ValueId(32))]
    );
    let receipt = call.receipts.as_ref().unwrap();
    assert_eq!(
        receipt.inputs().collect::<Vec<_>>(),
        vec![ValueId(30), ValueId(31)]
    );
    assert_eq!(
        receipt
            .arguments
            .iter()
            .map(|argument| argument.parameter())
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(receipt.contract, contract);
    assert_eq!(call.pretransfer_failure, Some(FailureId(5)));
}

#[test]
fn compaction_keeps_entry_versions_without_minting_owning_values() {
    let mut unit = unit();
    unit.tables.calls.clear();
    unit.pools.call_arguments.clear();
    unit.values = (0..14)
        .map(|_| ValueInfo {
            ty: LirTypeId(0),
            holding: Holding::Plain,
        })
        .collect();
    unit.parameters = vec![ValueId(1)];
    unit.captures = vec![ValueId(2)];
    let contract = ContentHash::of_bytes(b"entry contract");
    unit.tables.receipt_entry = Some(ReceiptEntry {
        environment: None,
        contract,
        parameters: vec![ReceiptEntrySlot {
            source: 0,
            slot: SlotId(5),
            version: ValueId(10),
        }],
        captures: vec![ReceiptEntrySlot {
            source: 0,
            slot: SlotId(6),
            version: ValueId(12),
        }],
        result: Some(SlotId(7)),
    });
    crate::insert::compact_values(&mut unit);
    assert_eq!(unit.values.len(), 4);
    assert_eq!(unit.parameters, [ValueId(0)]);
    assert_eq!(unit.captures, [ValueId(1)]);
    let entry = unit.tables.receipt_entry.as_ref().unwrap();
    assert_eq!(
        entry.versions().collect::<Vec<_>>(),
        [ValueId(2), ValueId(3)]
    );
    assert_eq!(entry.parameters[0].slot, SlotId(5));
    assert_eq!(entry.captures[0].slot, SlotId(6));
    assert_eq!(entry.result, Some(SlotId(7)));
    assert_eq!(entry.contract, contract);
    assert!(unit.values.iter().all(|v| v.holding == Holding::Plain));
}

#[test]
fn local_producer_33_liveness_uses_metadata_without_rereading_consumed_originals() {
    let mut unit = unit();
    let inputs = PoolRange::push(
        &mut unit.pools.receipt_inputs,
        [
            ReceiptInput {
                original: ValueId(1),
                receipt: Some(ValueId(10)),
            },
            ReceiptInput {
                original: ValueId(3),
                receipt: None,
            },
        ],
    );
    let inst = Inst::PayloadReceipt {
        arena: hirsel_lir::ir::OptionalValueId::NONE,
        result: ValueId(14),
        value: ValueId(4),
        shape: SealDescriptorId(2),
        binding: PayloadBindingId(3),
        inputs,
    };
    assert_eq!(Inst::defs(&inst), vec![ValueId(14)]);
    let mut uses = crate::liveness::inst_uses(&unit, &inst);
    uses.sort();
    assert_eq!(uses, vec![ValueId(4), ValueId(10)]);
}

#[test]
fn local_producer_33_remap_preserves_binding_and_all_pooled_pairs() {
    let mut unit = unit();
    let inputs = PoolRange::push(
        &mut unit.pools.receipt_inputs,
        [
            ReceiptInput {
                original: ValueId(1),
                receipt: Some(ValueId(10)),
            },
            ReceiptInput {
                original: ValueId(3),
                receipt: None,
            },
        ],
    );
    unit.instructions.push(Inst::PayloadReceipt {
        arena: hirsel_lir::ir::OptionalValueId::NONE,
        result: ValueId(14),
        value: ValueId(4),
        shape: SealDescriptorId(2),
        binding: PayloadBindingId(3),
        inputs,
    });
    crate::insert::remap_values(&mut unit, &|v| ValueId(v.0 + 20));
    assert_eq!(
        unit.instructions[0],
        Inst::PayloadReceipt {
            arena: hirsel_lir::ir::OptionalValueId::NONE,
            result: ValueId(34),
            value: ValueId(24),
            shape: SealDescriptorId(2),
            binding: PayloadBindingId(3),
            inputs
        }
    );
    assert_eq!(
        unit.pools.receipt_inputs,
        vec![
            ReceiptInput {
                original: ValueId(21),
                receipt: Some(ValueId(30))
            },
            ReceiptInput {
                original: ValueId(23),
                receipt: None
            },
        ]
    );
}

#[test]
fn borrowed_receipt_closure_33_liveness_and_remap_keep_metadata_in_its_own_namespace() {
    let mut unit = unit();
    let captures = PoolRange::push(
        &mut unit.pools.captures,
        [CaptureValue {
            value: ValueId(1),
            usage: CaptureUse::Borrow,
        }],
    );
    unit.tables.closures.push(ClosureSite {
        scoped: true,
        code: None,
        environment: hirsel_lir::LayoutId(0),
        captures,
        destination: DestinationClass::Heap,
        placement: None,
        anchor: SourceAnchorId(0),
        receipts: Some(ReceiptClosure {
            contract: ContentHash([5; 32]),
            captures: vec![ReceiptClosureCapture {
                ordinal: 0,
                receipt: ValueId(10),
            }],
        }),
    });
    let inst = Inst::Closure {
        result: ValueId(14),
        closure: ClosureId(0),
        failure: FailureId(0),
    };
    let mut uses = crate::liveness::inst_uses(&unit, &inst);
    uses.sort_unstable();
    assert_eq!(uses, [ValueId(1), ValueId(10)]);
    crate::insert::remap_values(&mut unit, &|v| ValueId(v.0 + 20));
    assert_eq!(unit.pools.captures[0].value, ValueId(21));
    let receipts = unit.tables.closures[0].receipts.as_ref().unwrap();
    assert_eq!(receipts.captures[0].ordinal, 0);
    assert_eq!(receipts.captures[0].receipt, ValueId(30));
    assert_eq!(receipts.contract, ContentHash([5; 32]));
}
