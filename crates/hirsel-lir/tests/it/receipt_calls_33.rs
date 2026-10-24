use crate::support::{ENTRY, Lir};
use hirsel_lir::ir::*;
use hirsel_lir::sealed_payload_source::*;
use hirsel_lir::stage::Value;
use hirsel_lir::validate::ViolationKind;
use hirsel_syntax::ContentHash;

fn ty() -> ReceiptType {
    ReceiptType {
        source_type: ContentHash::of_bytes(b"original source bytes"),
        profile: ContentHash::of_bytes(b"closed storage profile"),
    }
}
fn contract(modes: &[ReceiptMode], result: bool) -> ReceiptContract {
    ReceiptContract::new(
        modes
            .iter()
            .enumerate()
            .map(|(source, mode)| ReceiptParameter {
                source: source as u32,
                ty: ty(),
                mode: *mode,
            })
            .collect(),
        result.then(|| ReceiptResult { ty: ty() }),
        vec![],
        vec![],
    )
    .unwrap()
}
fn receipt_call() -> (ReceiptContract, ReceiptCall) {
    let contract = contract(
        &[ReceiptMode::Borrow, ReceiptMode::Take, ReceiptMode::MutSlot],
        true,
    );
    let call = ReceiptCall {
        contract: contract.content().unwrap(),
        arguments: vec![
            ReceiptArgument::Borrow {
                parameter: 0,
                receipt: ValueId(1),
            },
            ReceiptArgument::Take {
                parameter: 1,
                receipt: ValueId(2),
            },
            ReceiptArgument::Mut {
                parameter: 2,
                address: AddressId(0),
                version: ValueId(3),
            },
        ],
        result: Some(ValueId(4)),
    };
    (contract, call)
}
fn ordinary_call() -> CallSite {
    CallSite {
        pretransfer_failure: None,
        forward_caller: false,
        callee: Callee::Direct(SymbolId(3)),
        operation_arguments: vec![],
        abi: hirsel_lir::FnAbiId(7),
        arguments: PoolRange::EMPTY,
        result: None,
        destination: None,
        declared: hirsel_lir::Effects::NONE,
        anchor: SourceAnchorId(0),
        receipts: None,
    }
}

#[test]
fn receipt_call_binds_complete_contract_and_all_canonical_carriers() {
    let (contract, call) = receipt_call();
    call.check_contract(&contract).unwrap();
    let text = hirsel_codec::to_text(&call);
    let restored: ReceiptCall = hirsel_codec::from_text(&text).unwrap();
    assert_eq!(restored, call);
    restored.check_contract(&contract).unwrap();
}

#[test]
fn receipt_call_refuses_changed_hash_mode_order_selector_and_result() {
    let (contract, good) = receipt_call();
    good.check_contract(&contract).unwrap();
    for mutation in 0..7 {
        let mut bad = good.clone();
        match mutation {
            0 => bad.contract = ContentHash::of_bytes(b"forged contract"),
            1 => bad.arguments.swap(0, 1),
            2 => {
                bad.arguments[1] = ReceiptArgument::Borrow {
                    parameter: 1,
                    receipt: ValueId(2),
                }
            }
            3 => {
                bad.arguments.pop();
            }
            4 => {
                bad.arguments[2] = ReceiptArgument::Mut {
                    parameter: 3,
                    address: AddressId(0),
                    version: ValueId(3),
                }
            }
            5 => bad.result = None,
            6 => bad.arguments.push(good.arguments[0]),
            _ => unreachable!(),
        }
        assert!(
            bad.check_contract(&contract).is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn receipt_call_rejects_duplicate_taken_holders_and_exclusive_slot_addresses() {
    for mode in [ReceiptMode::Take, ReceiptMode::MutSlot] {
        let contract = contract(&[mode, mode], false);
        let argument = |parameter, id| match mode {
            ReceiptMode::Take => ReceiptArgument::Take {
                parameter,
                receipt: ValueId(id),
            },
            ReceiptMode::MutSlot => ReceiptArgument::Mut {
                parameter,
                address: AddressId(id),
                version: ValueId(id),
            },
            _ => unreachable!(),
        };
        let mut call = ReceiptCall {
            contract: contract.content().unwrap(),
            arguments: vec![argument(0, 0), argument(1, 1)],
            result: None,
        };
        call.check_contract(&contract).unwrap();
        call.arguments[1] = argument(1, 0);
        assert!(call.check_contract(&contract).is_err());
    }
}

#[test]
fn receipt_call_absence_keeps_legacy_encoding_and_hash() {
    use std::hash::{Hash, Hasher};
    let call = ordinary_call();
    let legacy = "(CallSite\n  (none) 0\n  (Direct 3)\n  (list) 7\n  (range 0 0)\n  (none)\n  (none)\n  (Effects) 0)";
    assert_eq!(hirsel_codec::to_text(&call), legacy);
    let restored: CallSite = hirsel_codec::from_text(legacy).unwrap();
    assert_eq!(restored, call);
    let mut before = std::hash::DefaultHasher::new();
    (
        call.pretransfer_failure,
        call.forward_caller,
        &call.callee,
        &call.operation_arguments,
        call.abi,
        call.arguments,
        call.result,
        &call.destination,
        call.declared,
        call.anchor,
    )
        .hash(&mut before);
    let mut after = std::hash::DefaultHasher::new();
    call.hash(&mut after);
    assert_eq!(before.finish(), after.finish());
}

#[test]
fn receipt_call_present_empty_is_distinct_and_keeps_pretransfer_failure() {
    let mut call = ordinary_call();
    call.pretransfer_failure = Some(FailureId(5));
    let ordinary = hirsel_codec::to_text(&call);
    let empty = contract(&[], false);
    call.receipts = Some(ReceiptCall {
        contract: empty.content().unwrap(),
        arguments: vec![],
        result: None,
    });
    call.receipts
        .as_ref()
        .unwrap()
        .check_contract(&empty)
        .unwrap();
    let text = hirsel_codec::to_text(&call);
    assert_ne!(text, ordinary);
    let restored: CallSite = hirsel_codec::from_text(&text).unwrap();
    assert_eq!(restored, call);
    assert_eq!(restored.pretransfer_failure, Some(FailureId(5)));
}

#[test]
fn receipt_call_data_cannot_enter_an_ordinary_callee() {
    for aware in [false, true] {
        let mut t = Lir::<Value>::new(&[], None);
        let next = t.builder.block(BlockKind::Body, &[]);
        let failure = t.builder.propagate();
        t.call(ENTRY, &[], None, &[], (next, &[]), failure);
        t.builder.ret(next, None);
        if aware {
            let empty = contract(&[], false);
            t.builder.unit.tables.calls[0].receipts = Some(ReceiptCall {
                contract: empty.content().unwrap(),
                arguments: vec![],
                result: None,
            });
        }
        let errors = t.check();
        if aware {
            assert!(
                errors.iter().any(
                    |e| matches!(e, ViolationKind::Schema(s) if s.contains("contract presence"))
                ),
                "{errors:?}"
            );
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}

#[test]
fn receipt_call_schema_checks_all_handle_address_version_and_result_indices() {
    use crate::support::{ROOT, l};
    for mutation in 0..4 {
        let mut t = Lir::<Value>::new(&[], None);
        let value = t.plain(l::PTR);
        let slot = t.slot(l::PTR, SlotKind::Local, ROOT);
        let address = t.at(ENTRY, slot, &[], &[]);
        let next = t.builder.block(BlockKind::Body, &[]);
        let failure = t.builder.propagate();
        t.call(ENTRY, &[], None, &[], (next, &[]), failure);
        t.builder.ret(next, None);
        let (mut unit, table, _) = t.finish();
        let (_, mut call) = receipt_call();
        call.arguments = vec![
            ReceiptArgument::Borrow {
                parameter: 0,
                receipt: value,
            },
            ReceiptArgument::Mut {
                parameter: 2,
                address,
                version: value,
            },
        ];
        call.result = Some(value);
        unit.tables.calls[0].receipts = Some(call.clone());
        assert!(unit.check_identity(&table).is_ok());
        let foreign = u32::MAX;
        match mutation {
            0 => {
                call.arguments[0] = ReceiptArgument::Borrow {
                    parameter: 0,
                    receipt: ValueId(foreign),
                }
            }
            1 => {
                call.arguments[1] = ReceiptArgument::Mut {
                    parameter: 2,
                    address: AddressId(foreign),
                    version: value,
                }
            }
            2 => {
                call.arguments[1] = ReceiptArgument::Mut {
                    parameter: 2,
                    address,
                    version: ValueId(foreign),
                }
            }
            3 => call.result = Some(ValueId(foreign)),
            _ => unreachable!(),
        }
        unit.tables.calls[0].receipts = Some(call);
        assert!(unit.check_identity(&table).is_err(), "mutation {mutation}");
    }
}
