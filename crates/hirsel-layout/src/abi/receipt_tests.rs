use super::*;
use crate::layout::Owners;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::sealed_payload_source::*;

fn contract() -> ReceiptContract {
    ReceiptContract::new(
        vec![ReceiptParameter {
            source: 0,
            ty: ReceiptType {
                source_type: ContentHash::of_bytes(b"original source type"),
                profile: ContentHash::of_bytes(b"closed original profile"),
            },
            mode: ReceiptMode::Borrow,
        }],
        None,
        vec![],
        vec![],
    )
    .unwrap()
}
fn base(builder: &mut LayoutBuilder<'_>) -> FnAbiId {
    let types = TypeInterner::new(None);
    let scalar_type = types.intern(hirsel_types::Type::with_kind(TypeKind::Primitive(
        Primitive::U64,
    )));
    let scalar = builder.layout(&types, scalar_type).unwrap();
    let abi = FnAbi {
        key: FnAbiKey {
            callable: ContentHash::of_bytes(b"ordinary callable"),
            abi_version: hirsel_lir::ABI_VERSION,
            receipt_contract: None,
        },
        shape: CallableShape::Definition,
        multiplicity: CallMultiplicity::Many,
        context: true,
        environment: false,
        caller_site: false,
        destination: false,
        result: None,
        diverges: false,
        result_arena: None,
        parameters: vec![AbiParameter {
            source: 0,
            convention: Passing::Borrow,
            passing: ParameterPassing::ByValue,
            layout: scalar,
            slots: vec![slot(builder.target(), ScalarKind::U64)],
            extension: ScalarExtension::None,
        }],
        version: hirsel_lir::ABI_VERSION,
        nonretaining: vec![],
        nonretaining_captures: vec![],
        scoped_contract: hirsel_lir::abi::scoped_contract_digest(&[], &[]),
        scoped_entries: vec![],
        receipts: None,
    };
    let id = FnAbiId(builder.table().fn_abis.len() as u32);
    builder.abi_memo.insert((abi.key.callable, abi.shape), id);
    builder.table_mut_fn_abis().push(abi);
    id
}

#[test]
fn present_empty_demand_has_its_own_namespace_without_carriers() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let ordinary = base(&mut builder);
    let old = builder.table().fn_abis[ordinary.0 as usize].clone();
    let empty = ReceiptContract::new(vec![], None, vec![], vec![]).unwrap();
    let aware = builder.receipt_fn_abi(ordinary, &empty).unwrap();
    assert_ne!(aware, ordinary);
    assert_eq!(builder.receipt_fn_abi(ordinary, &empty).unwrap(), aware);
    assert_eq!(builder.table().fn_abis[ordinary.0 as usize], old);
    let new = &builder.table().fn_abis[aware.0 as usize];
    assert_eq!(new.receipts, Some(empty));
    assert_eq!(new.parameters, old.parameters);
    assert_eq!(new.result, old.result);
    hirsel_lir::layout_validate::validate_table(builder.table(), &target).unwrap();
}

#[test]
fn receipt_namespace_memoizes_without_changing_ordinary_prefix_or_request() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let ordinary = base(&mut builder);
    let old = builder.table().fn_abis[ordinary.0 as usize].clone();
    let aware = builder.receipt_fn_abi(ordinary, &contract()).unwrap();
    assert_ne!(aware, ordinary);
    assert_eq!(
        builder.receipt_fn_abi(ordinary, &contract()).unwrap(),
        aware
    );
    assert_eq!(
        builder
            .ordinary_cached(old.key.callable, old.shape)
            .unwrap(),
        Some(ordinary)
    );
    assert_eq!(builder.table().fn_abis[ordinary.0 as usize], old);
    let new = &builder.table().fn_abis[aware.0 as usize];
    assert_eq!(new.key.callable, old.key.callable);
    assert_eq!(new.parameters, old.parameters);
    assert_eq!(new.result, old.result);
    assert_eq!(new.receipts, Some(contract()));
    assert_ne!(
        hirsel_lir::abi::abi_digest(new),
        hirsel_lir::abi::abi_digest(&old)
    );
    assert_ne!(
        hirsel_lir::LayoutKeys::new(builder.table()).fn_abi(new),
        hirsel_lir::LayoutKeys::new(builder.table()).fn_abi(&old)
    );
    hirsel_lir::layout_validate::validate_table(builder.table(), &target).unwrap();
}

#[test]
fn aware_rows_cannot_poison_the_ordinary_memo_or_reenter_ordinary_decoration() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let ordinary = base(&mut builder);
    let old = builder.table().fn_abis[ordinary.0 as usize].clone();
    let aware = builder.receipt_fn_abi(ordinary, &contract()).unwrap();
    builder
        .abi_memo
        .insert((old.key.callable, old.shape), aware);
    assert!(
        builder
            .ordinary_cached(old.key.callable, old.shape)
            .is_err()
    );
    assert!(builder.with_caller_site(aware).is_err());
    assert!(builder.with_scoped_contract(aware, vec![], vec![]).is_err());
    assert!(builder.with_capture_parameters(aware, &[], false).is_err());
}

#[test]
fn zero_size_result_can_demand_receipts_but_never_cannot_return_one() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let ordinary = base(&mut builder);
    let result = ReceiptContract::new(
        vec![],
        Some(ReceiptResult {
            ty: contract().parameters[0].ty.clone(),
        }),
        vec![],
        vec![],
    )
    .unwrap();
    let aware = builder.receipt_fn_abi(ordinary, &result).unwrap();
    assert!(builder.table().fn_abis[aware.0 as usize].result.is_none());
    assert_eq!(
        builder.table().fn_abis[aware.0 as usize].receipts,
        Some(result.clone())
    );
    hirsel_lir::layout_validate::validate_table(builder.table(), &target).unwrap();
    let never = base(&mut builder);
    builder.table_mut_fn_abis()[never.0 as usize].diverges = true;
    assert!(builder.receipt_fn_abi(never, &result).is_err());
}

#[test]
fn foreign_entries_refuse_even_a_present_empty_receipt_contract() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let ordinary = base(&mut builder);
    let abi = &mut builder.table_mut_fn_abis()[ordinary.0 as usize];
    abi.shape = CallableShape::Foreign;
    abi.context = false;
    let empty = ReceiptContract::new(vec![], None, vec![], vec![]).unwrap();
    assert!(builder.receipt_fn_abi(ordinary, &empty).is_err());
}
