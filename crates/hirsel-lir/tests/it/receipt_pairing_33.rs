//! Local call binding controls; executable receipt custody remains refused.
use crate::support::{Lir, l};
use hirsel_core_contract::Passing;
use hirsel_lir::FnAbi;
use hirsel_lir::ir::*;
use hirsel_lir::receipt_transport::check_call_contract;
use hirsel_lir::sealed_payload_source::*;
use hirsel_lir::stage::Value;
use hirsel_syntax::ContentHash;

fn fixture(result: bool) -> (FnAbi, CallSite, Pools) {
    let t = Lir::<Value>::new(
        &[
            (Passing::Borrow, l::PTR),
            (Passing::Borrow, l::UNIT),
            (Passing::Mut, l::PTR),
        ],
        result.then_some(l::U64),
    );
    let mut abi = t.table.fn_abis[t.builder.unit.header.abi.0 as usize].clone();
    let ty = ReceiptType {
        source_type: ContentHash::of_bytes(b"original source type"),
        profile: ContentHash::of_bytes(b"closed profile"),
    };
    let contract = ReceiptContract::new(
        vec![
            ReceiptParameter {
                source: 0,
                ty: ty.clone(),
                mode: ReceiptMode::Borrow,
            },
            ReceiptParameter {
                source: 2,
                ty: ty.clone(),
                mode: ReceiptMode::MutSlot,
            },
        ],
        Some(ReceiptResult { ty }),
        vec![],
        vec![],
    )
    .unwrap();
    abi.key.receipt_contract = Some(contract.content().unwrap());
    abi.receipts = Some(contract.clone());
    let mut pools = Pools::default();
    let arguments = PoolRange::push(
        &mut pools.call_arguments,
        [
            CallArgument {
                parameter: 0,
                passing: Passing::Borrow,
                value: ArgumentValue::Value(ValueId(0)),
            },
            CallArgument {
                parameter: 1,
                passing: Passing::Borrow,
                value: ArgumentValue::Value(ValueId(1)),
            },
            CallArgument {
                parameter: 2,
                passing: Passing::Mut,
                value: ArgumentValue::Address {
                    address: AddressId(0),
                    version: ValueId(2),
                },
            },
        ],
    );
    let call = CallSite {
        pretransfer_failure: Some(FailureId(1)),
        forward_caller: false,
        callee: Callee::Direct(SymbolId(0)),
        operation_arguments: vec![],
        abi: hirsel_lir::FnAbiId(0),
        arguments,
        result: result.then_some(ValueId(3)),
        destination: None,
        declared: hirsel_lir::Effects::ALL,
        anchor: SourceAnchorId(0),
        receipts: Some(ReceiptCall {
            contract: contract.content().unwrap(),
            arguments: vec![
                ReceiptArgument::Borrow {
                    parameter: 0,
                    receipt: ValueId(4),
                },
                ReceiptArgument::Mut {
                    parameter: 2,
                    address: AddressId(1),
                    version: ValueId(5),
                },
            ],
            result: Some(ValueId(6)),
        }),
    };
    (abi, call, pools)
}
#[test]
fn receipt_pairing_uses_original_selectors_without_requiring_unit_value_carriers() {
    for result in [false, true] {
        let (abi, call, pools) = fixture(result);
        check_call_contract(&abi, &call, &pools).unwrap();
        assert_eq!(call.pretransfer_failure, Some(FailureId(1)));
    }
}
#[test]
fn receipt_pairing_rejects_missing_duplicate_wrong_mode_and_nonaddress_actuals() {
    let (abi, good, pools) = fixture(true);
    check_call_contract(&abi, &good, &pools).unwrap();
    for mutation in 0..5 {
        let mut call = good.clone();
        let mut pools = pools.clone();
        match mutation {
            0 => pools.call_arguments[0].parameter = 1,
            1 => {
                let mut duplicate = pools.call_arguments[0];
                duplicate.value = ArgumentValue::Value(ValueId(9));
                pools.call_arguments.push(duplicate);
                call.arguments.len += 1;
            }
            2 => pools.call_arguments[0].passing = Passing::Take,
            3 => pools.call_arguments[2].value = ArgumentValue::Value(ValueId(2)),
            4 => call.arguments.start = u32::MAX,
            _ => unreachable!(),
        }
        assert!(
            check_call_contract(&abi, &call, &pools).is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn receipt_pairing_requires_the_ordinary_nonunit_result_but_allows_receipt_only_unit() {
    let (abi, mut call, pools) = fixture(true);
    check_call_contract(&abi, &call, &pools).unwrap();
    call.result = None;
    assert!(check_call_contract(&abi, &call, &pools).is_err());
    let (abi, call, pools) = fixture(false);
    assert!(call.result.is_none());
    check_call_contract(&abi, &call, &pools).unwrap();
}
#[test]
fn receipt_pairing_requires_complete_contract_content_and_presence() {
    let (abi, good, pools) = fixture(false);
    check_call_contract(&abi, &good, &pools).unwrap();
    for mutation in 0..4 {
        let mut abi = abi.clone();
        let mut call = good.clone();
        match mutation {
            0 => abi.key.receipt_contract = Some(ContentHash::of_bytes(b"forged key")),
            1 => abi.receipts = None,
            2 => call.receipts = None,
            3 => call.receipts.as_mut().unwrap().contract = ContentHash::of_bytes(b"wrong callee"),
            _ => unreachable!(),
        }
        assert!(
            check_call_contract(&abi, &call, &pools).is_err(),
            "mutation {mutation}"
        );
    }
}
