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
