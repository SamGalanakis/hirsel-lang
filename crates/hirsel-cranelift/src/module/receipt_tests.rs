use super::*;
use hirsel_lir::abi_types::*;
use hirsel_lir::layout_types::{LayoutId, ScalarKind};
use hirsel_lir::sealed_payload_source::*;
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
            abi_version: hirsel_lir::ABI_VERSION,
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
        version: hirsel_lir::ABI_VERSION,
        nonretaining: vec![],
        nonretaining_captures: vec![],
        scoped_contract: hirsel_lir::abi::scoped_contract_digest(&[], &[]),
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
fn receipt_signature_appends_after_ordinary_parameters_and_preserves_results() {
    for (has_result, register) in [(false, None), (true, Some(ScalarKind::U64)), (true, None)] {
        let ordinary = receipt_abi(has_result);
        let before = signature(CallConv::SystemV, &ordinary, register).unwrap();
        let mut aware = ordinary.clone();
        demand(&mut aware);
        let after = signature(CallConv::SystemV, &aware, register).unwrap();
        assert_eq!(&after.ir.params[..before.ir.params.len()], before.ir.params);
        assert_eq!(
            &after.ir.params[before.ir.params.len()..],
            &[AbiParam::new(PTR); 4]
        );
        assert_eq!(after.first, before.first);
        assert_eq!(after.hidden, before.hidden);
        assert_eq!(after.ir.returns, before.ir.returns);
        assert_eq!(after.value, before.value);
        let index = hirsel_lir::abi::receipt_slots(&aware, before.ir.params.len()).unwrap();
        assert_eq!(index.result, Some(before.ir.params.len() + 3));
    }
}
#[test]
fn receipt_signature_present_empty_preserves_the_entire_ordinary_function_type() {
    let ordinary = receipt_abi(true);
    let before = signature(CallConv::SystemV, &ordinary, Some(ScalarKind::U64)).unwrap();
    let mut aware = ordinary.clone();
    let empty = ReceiptContract::new(vec![], None, vec![], vec![]).unwrap();
    aware.key.receipt_contract = Some(empty.content().unwrap());
    aware.receipts = Some(empty);
    let after = signature(CallConv::SystemV, &aware, Some(ScalarKind::U64)).unwrap();
    assert_eq!(after.ir, before.ir);
    assert_eq!(after.first, before.first);
}
