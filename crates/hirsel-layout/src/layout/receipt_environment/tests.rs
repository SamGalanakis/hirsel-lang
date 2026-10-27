use super::*;
use hirsel_lir::sealed_payload_source::{ReceiptCapture, ReceiptType};

fn owning_source(modes: &[ReceiptMode]) -> ReceiptSource {
    use hirsel_lir::sealed_payload_source::{
        ClosedStorageProfile, ClosedStorageType, ReceiptProfile,
    };
    let profile = ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: ClosedStorageType {
            ty: hirsel_lir::operation::NativeType::Primitive(hirsel_lir::operation::Primitive::U64),
            source_type: ContentHash::of_bytes(b"original u64 capture"),
        },
        functions: vec![],
    };
    let hash = ContentHash::of_bytes(b"owning closure source");
    ReceiptSource {
        original: hirsel_lir::invocations::ReferenceBody {
            definition: DefPath::core("owning_receipt_capture"),
            body: 0,
            instance: hash,
        },
        instance: hash,
        source: hash,
        implementation_revision: hash,
        contract: ReceiptContract::new(
            vec![],
            None,
            modes
                .iter()
                .enumerate()
                .map(|(ordinal, &mode)| ReceiptCapture {
                    ordinal: ordinal as u32,
                    mode,
                    ty: ReceiptType {
                        source_type: profile.root.source_type,
                        profile: profile.content().unwrap(),
                    },
                })
                .collect(),
            vec![],
        )
        .unwrap(),
        profiles: modes
            .iter()
            .enumerate()
            .map(|(ordinal, _)| ReceiptProfile {
                port: ReceiptPort::Capture(ordinal as u32),
                profile: profile.clone(),
            })
            .collect(),
    }
}

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

#[test]
fn owning_receipt_environment_keeps_base_live_word_and_metadata_only_custody() {
    for target in TargetSpec::executed() {
        for movable in [false, true] {
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&target, &owners);
            let base = base(&mut builder, movable);
            let before = builder.record(base).clone();
            let source =
                owning_source(&[ReceiptMode::Take, ReceiptMode::Borrow, ReceiptMode::Take]);
            let wrapper = builder.owning_receipt_environment(base, &source).unwrap();
            let (actual, suffix) =
                hirsel_lir::receipt_closure::owned_environment(builder.table(), wrapper, &source)
                    .unwrap();
            assert_eq!(actual, base);
            assert_eq!(builder.record(base), &before);
            assert_eq!(suffix.len(), 3);
            assert!(suffix[0].offset >= before.size.storage().unwrap());
            assert_eq!(builder.record(wrapper).scan, before.scan);
            let mut drops = vec![
                DropStep::ReleaseReceipt {
                    offset: suffix[2].offset,
                },
                DropStep::ReleaseReceipt {
                    offset: suffix[0].offset,
                },
            ];
            drops.extend(before.drop.clone());
            assert_eq!(builder.record(wrapper).drop, drops);
            assert_eq!(builder.record(wrapper).copy, CopyRecipe::NotCopyable);
            hirsel_lir::layout_validate::validate_table(builder.table(), &target).unwrap();
            assert_eq!(
                builder.owning_receipt_environment(base, &source).unwrap(),
                wrapper
            );
        }
    }
}

#[test]
fn owning_receipt_environment_rejects_forged_recipes_types_and_geometry() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let base = base(&mut builder, true);
    let source = owning_source(&[ReceiptMode::Take, ReceiptMode::Borrow, ReceiptMode::Take]);
    let wrapper = builder.owning_receipt_environment(base, &source).unwrap();
    hirsel_lir::receipt_closure::owned_environment(builder.table(), wrapper, &source).unwrap();
    for mutation in 0..11 {
        let mut table = builder.table().clone();
        let companion = table.layouts[wrapper.0 as usize].fields[1].layout;
        let pointer = table.layouts[wrapper.0 as usize].fields[2].layout;
        match mutation {
            0 => table.layouts[wrapper.0 as usize].fields[0].offset = 8,
            1 => table.layouts[wrapper.0 as usize].fields[1].offset = 0,
            2 => table.layouts[wrapper.0 as usize].fields[1].layout = pointer,
            3 => {
                table.layouts[companion.0 as usize].kind = LayoutKind::PayloadReceipt {
                    root: hirsel_lir::operation::NativeType::Primitive(
                        hirsel_lir::operation::Primitive::Bool,
                    ),
                }
            }
            4 => table.layouts[wrapper.0 as usize].drop.clear(),
            5 => table.layouts[wrapper.0 as usize].drop.reverse(),
            6 => {
                let offset = table.layouts[wrapper.0 as usize].fields[1].offset;
                table.layouts[wrapper.0 as usize]
                    .scan
                    .push(ScanStep::Counted { offset });
            }
            7 => table.layouts[wrapper.0 as usize].copy = CopyRecipe::Bits,
            8 => table.layouts[wrapper.0 as usize].size = SizeSpec::Fixed(0),
            9 => {
                table.layouts[wrapper.0 as usize].key.instance =
                    Some(ContentHash::of_bytes(b"forged capture contract"))
            }
            10 => table.layouts[companion.0 as usize].drop = vec![DropStep::Release { offset: 0 }],
            _ => unreachable!(),
        }
        assert!(
            hirsel_lir::receipt_closure::owned_environment(&table, wrapper, &source).is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn owning_receipt_environment_rolls_back_incomplete_profiles_and_invalid_capture_modes() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let base = base(&mut builder, true);
    let count = builder.table().layouts.len();
    for modes in [
        vec![ReceiptMode::Borrow],
        vec![ReceiptMode::Take, ReceiptMode::MutSlot],
        vec![ReceiptMode::Take; 4],
    ] {
        assert!(
            builder
                .owning_receipt_environment(base, &owning_source(&modes))
                .is_err()
        );
        assert_eq!(builder.table().layouts.len(), count);
    }
    let mut source = owning_source(&[ReceiptMode::Take]);
    source.profiles.clear();
    assert!(builder.owning_receipt_environment(base, &source).is_err());
    assert_eq!(builder.table().layouts.len(), count);
}

#[test]
fn owning_receipt_environment_identity_retains_the_complete_original_capture_profile() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let base = base(&mut builder, false);
    let a = owning_source(&[ReceiptMode::Take]);
    let mut b = a.clone();
    b.profiles[0].profile.root.source_type =
        ContentHash::of_bytes(b"distinct original capture type");
    let mut capture = b.contract.captures[0].clone();
    capture.ty.source_type = b.profiles[0].profile.root.source_type;
    capture.ty.profile = b.profiles[0].profile.content().unwrap();
    b.contract = ReceiptContract::new(vec![], None, vec![capture], vec![]).unwrap();
    let first = builder.owning_receipt_environment(base, &a).unwrap();
    let second = builder.owning_receipt_environment(base, &b).unwrap();
    assert_ne!(first, second);
    assert_eq!(builder.record(first).fields, builder.record(second).fields);
    assert!(hirsel_lir::receipt_closure::owned_environment(builder.table(), first, &b).is_err());
}
