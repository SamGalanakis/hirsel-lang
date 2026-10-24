use super::*;
use crate::abi_types::*;
use crate::layout_types::{LayoutId, ScalarKind};
use crate::sealed_payload_source::*;
use hirsel_syntax::ContentHash;

fn receipt_abi(result: bool) -> FnAbi {
    let pointer = PhysicalSlot {
        ty: PhysicalType::Pointer,
        size: 8,
        align: 8,
    };
    FnAbi {
        key: FnAbiKey {
            callable: ContentHash::of_bytes(b"receipt signature fixture"),
            abi_version: crate::ABI_VERSION,
            receipt_contract: None,
        },
        shape: CallableShape::Closure,
        multiplicity: CallMultiplicity::Many,
        context: true,
        environment: true,
        caller_site: true,
        destination: true,
        result: result.then(|| AbiResult {
            borrowed_parameters: vec![],
            layout: LayoutId(0),
            slots: vec![pointer],
            register: Some(ScalarKind::U64),
        }),
        diverges: false,
        result_arena: None,
        parameters: [
            hirsel_core_contract::Passing::Borrow,
            hirsel_core_contract::Passing::Take,
            hirsel_core_contract::Passing::Mut,
        ]
        .into_iter()
        .enumerate()
        .map(|(source, convention)| AbiParameter {
            source: source as u32,
            convention,
            passing: if convention == hirsel_core_contract::Passing::Mut {
                ParameterPassing::ExclusiveAddress
            } else {
                ParameterPassing::ByValue
            },
            layout: LayoutId(0),
            slots: vec![pointer],
            extension: ScalarExtension::None,
        })
        .collect(),
        version: crate::ABI_VERSION,
        nonretaining: vec![],
        nonretaining_captures: vec![],
        scoped_contract: crate::abi::scoped_contract_digest(&[], &[]),
        scoped_entries: vec![],
        receipts: None,
    }
}
fn demand(abi: &mut FnAbi) {
    let ty = ReceiptType {
        source_type: ContentHash::of_bytes(b"original full type"),
        profile: ContentHash::of_bytes(b"closed profile"),
    };
    let contract = ReceiptContract::new(
        [ReceiptMode::Borrow, ReceiptMode::Take, ReceiptMode::MutSlot]
            .into_iter()
            .enumerate()
            .map(|(source, mode)| ReceiptParameter {
                source: source as u32,
                ty: ty.clone(),
                mode,
            })
            .collect(),
        Some(ReceiptResult { ty }),
        vec![],
        vec![],
    )
    .unwrap();
    abi.key.receipt_contract = Some(contract.content().unwrap());
    abi.receipts = Some(contract);
}

#[test]
fn receipt_slots_follow_existing_arguments_in_source_ordinal_order() {
    let mut abi = receipt_abi(true);
    demand(&mut abi);
    let slots = receipt_slots(&abi, 9).unwrap();
    assert_eq!(
        slots
            .parameters
            .iter()
            .map(|p| (p.source, p.index, p.mode))
            .collect::<Vec<_>>(),
        vec![
            (0, 9, ReceiptMode::Borrow),
            (1, 10, ReceiptMode::Take),
            (2, 11, ReceiptMode::MutSlot)
        ]
    );
    assert_eq!(slots.result, Some(12));
    assert_eq!(slots.len(), 4);
}
#[test]
fn receipt_slots_absence_and_present_empty_append_nothing() {
    let mut abi = receipt_abi(false);
    let before = receipt_slots(&abi, 8).unwrap();
    assert!(before.is_empty());
    let empty = ReceiptContract::new(vec![], None, vec![], vec![]).unwrap();
    abi.key.receipt_contract = Some(empty.content().unwrap());
    abi.receipts = Some(empty);
    assert_eq!(receipt_slots(&abi, 8).unwrap(), before);
}
#[test]
fn receipt_slots_refuse_wrong_contract_foreign_divergence_and_overflow() {
    let mut good = receipt_abi(false);
    demand(&mut good);
    receipt_slots(&good, 8).unwrap();
    for mutation in 0..5 {
        let mut bad = good.clone();
        let mut first = 8;
        match mutation {
            0 => bad.key.receipt_contract = None,
            1 => bad.receipts = None,
            2 => bad.shape = CallableShape::Foreign,
            3 => bad.diverges = true,
            4 => first = usize::MAX,
            _ => unreachable!(),
        }
        assert!(receipt_slots(&bad, first).is_err(), "mutation {mutation}");
    }
}
