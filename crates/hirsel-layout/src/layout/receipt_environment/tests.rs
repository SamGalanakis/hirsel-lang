use super::*;
use hirsel_lir::sealed_payload_source::{ReceiptCapture, ReceiptType};

fn demand(profile: &[u8], ordinal: u32, mode: ReceiptMode) -> ReceiptContract {
    ReceiptContract::new(
        vec![],
        None,
        vec![ReceiptCapture {
            ordinal,
            mode,
            ty: ReceiptType {
                source_type: ContentHash::of_bytes(b"full capture type"),
                profile: ContentHash::of_bytes(profile),
            },
        }],
        vec![],
    )
    .unwrap()
}
fn base(builder: &mut LayoutBuilder<'_>, movable: bool) -> LayoutId {
    let scalar = builder.scalar(ScalarKind::U64);
    builder
        .environment_data(
            &DefPath::core("receipt_environment_probe"),
            0,
            &[
                (scalar, Passing::Take),
                (scalar, Passing::Borrow),
                (scalar, Passing::Take),
            ],
            if movable { &[0, 2] } else { &[] },
        )
        .unwrap()
}

#[test]
fn borrowed_receipt_environment_appends_after_the_complete_base_and_once_live_word() {
    for target in TargetSpec::executed() {
        for movable in [false, true] {
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&target, &owners);
            let base = base(&mut builder, movable);
            let before = builder.record(base).clone();
            let contract = demand(b"profile", 1, ReceiptMode::Borrow);
            let wrapper = builder
                .borrowed_receipt_environment(base, &contract)
                .unwrap();
            let (actual, companions) = hirsel_lir::receipt_closure::borrowed_environment(
                builder.table(),
                wrapper,
                &contract,
            )
            .unwrap();
            assert_eq!(actual, base);
            assert_eq!(companions.len(), 1);
            assert!(companions[0].offset >= before.size.storage().unwrap());
            assert_eq!(builder.record(base), &before);
            assert_eq!(builder.record(wrapper).fields[0].offset, 0);
            hirsel_lir::layout_validate::validate_table(builder.table(), &target).unwrap();
        }
    }
}
#[test]
fn borrowed_receipt_environment_empty_capture_demand_keeps_the_ordinary_layout() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let base = base(&mut builder, true);
    let count = builder.table().layouts.len();
    let empty = ReceiptContract::new(vec![], None, vec![], vec![]).unwrap();
    assert_eq!(
        builder.borrowed_receipt_environment(base, &empty).unwrap(),
        base
    );
    assert_eq!(builder.table().layouts.len(), count);
}
#[test]
fn borrowed_receipt_environment_keeps_complete_profiles_distinct_and_memoizes_only_exact_contracts()
{
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let base = base(&mut builder, false);
    let a = demand(b"a", 1, ReceiptMode::Borrow);
    let b = demand(b"b", 1, ReceiptMode::Borrow);
    let first = builder.borrowed_receipt_environment(base, &a).unwrap();
    let second = builder.borrowed_receipt_environment(base, &b).unwrap();
    assert_ne!(first, second);
    assert_eq!(
        builder.borrowed_receipt_environment(base, &a).unwrap(),
        first
    );
    assert!(hirsel_lir::receipt_closure::borrowed_environment(builder.table(), first, &b).is_err());
}
#[test]
fn borrowed_receipt_environment_refuses_foreign_selectors_and_unsupported_custody_without_partial_rows()
 {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let base = base(&mut builder, true);
    let before = builder.table().layouts.len();
    for (ordinal, mode) in [
        (3, ReceiptMode::Borrow),
        (1, ReceiptMode::Take),
        (1, ReceiptMode::MutSlot),
    ] {
        assert!(
            builder
                .borrowed_receipt_environment(base, &demand(b"profile", ordinal, mode))
                .is_err()
        );
        assert_eq!(builder.table().layouts.len(), before);
    }
}
#[test]
fn borrowed_receipt_environment_independently_rejects_corrupt_wrapper_geometry() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let base = base(&mut builder, true);
    let contract = demand(b"profile", 1, ReceiptMode::Borrow);
    let wrapper = builder
        .borrowed_receipt_environment(base, &contract)
        .unwrap();
    for mutation in 0..5 {
        let mut table = builder.table().clone();
        let r = &mut table.layouts[wrapper.0 as usize];
        match mutation {
            0 => r.fields[0].offset = 8,
            1 => r.fields[1].offset = 0,
            2 => r.fields[1].layout = builder.record(base).fields[0].layout,
            3 => r.key.instance = Some(ContentHash::of_bytes(b"another full profile")),
            4 => r.size = SizeSpec::Fixed(0),
            _ => unreachable!(),
        }
        assert!(
            hirsel_lir::receipt_closure::borrowed_environment(&table, wrapper, &contract).is_err(),
            "mutation {mutation}"
        );
    }
}
