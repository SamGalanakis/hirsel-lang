//! The private metadata layout is schema, not executable source authority.
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::layout_types::*;
use hirsel_lir::layout_validate::{Subject, validate_table};
use hirsel_lir::operation::{NativeType, Primitive};

fn fixture() -> (TargetSpec, LayoutTable, LayoutId) {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let id = builder.payload_receipt(&NativeType::Primitive(Primitive::Bytes));
    let table = builder.finish();
    validate_table(&table, &target).unwrap();
    (target, table, id)
}
#[test]
fn receipt_layout_is_pointer_sized_noncopyable_metadata_without_language_credits() {
    let (target, table, id) = fixture();
    let record = &table.layouts[id.0 as usize];
    assert_eq!(
        record.size,
        SizeSpec::Fixed(target.scalar_size(ScalarKind::Pointer))
    );
    assert_eq!(record.align, target.scalar_align(ScalarKind::Pointer));
    assert!(record.fields.is_empty() && record.scan.is_empty() && record.finalizer.is_none());
    assert_eq!(record.drop, [DropStep::ReleaseReceipt { offset: 0 }]);
    assert_eq!(record.copy, CopyRecipe::NotCopyable);
    assert_eq!(hirsel_lir::layout::logical_size(&table, id), None);
}
#[test]
fn receipt_layout_memo_and_content_bind_the_original_root_without_changing_ordinary_rows() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&target, &owners);
    let mut types = crate::support::ty();
    let ordinary_type = types.p(hirsel_types::Primitive::U64);
    let ordinary = b.layout(&types.store, ordinary_type).unwrap();
    let before = b.record(ordinary).clone();
    let root = NativeType::Primitive(Primitive::Bytes);
    let one = b.payload_receipt(&root);
    assert_eq!(b.payload_receipt(&root), one);
    let other = b.payload_receipt(&NativeType::Primitive(Primitive::String));
    assert_ne!(one, other);
    assert_ne!(b.record(one).key, b.record(other).key);
    assert_eq!(b.layout(&types.store, ordinary_type).unwrap(), ordinary);
    assert_eq!(b.record(ordinary), &before);
    validate_table(b.table(), &target).unwrap();
}
#[test]
fn receipt_layout_schema_roundtrips_and_rebases_its_private_drop_offset() {
    let (target, table, id) = fixture();
    let text = hirsel_codec::to_text(&table);
    let restored: LayoutTable = hirsel_codec::from_text(&text).unwrap();
    assert_eq!(restored, table);
    validate_table(&restored, &target).unwrap();
    assert_eq!(
        hirsel_lir::layout::shift_drop(&table.layouts[id.0 as usize].drop[0], 24),
        DropStep::ReleaseReceipt { offset: 24 }
    );
}
#[test]
fn receipt_layout_validator_refuses_language_credit_copy_size_and_field_forgeries() {
    let (target, good, id) = fixture();
    for mutation in 0..7 {
        let mut table = good.clone();
        let row = &mut table.layouts[id.0 as usize];
        match mutation {
            0 => row.scan = vec![ScanStep::Counted { offset: 0 }],
            1 => row.drop = vec![DropStep::ReleaseRoot { offset: 0 }],
            2 => row.copy = CopyRecipe::Bits,
            3 => row.size = SizeSpec::Fixed(16),
            4 => row.align = 4,
            5 => {
                row.finalizer = Some(FinalizerRef {
                    definition: crate::support::path("forged"),
                    body: 0,
                    unsafe_ffi_package: false,
                })
            }
            6 => row.fields.push(FieldLayout {
                ordinal: 0,
                offset: 0,
                layout: id,
                unaligned: false,
            }),
            _ => unreachable!(),
        }
        let errors = validate_table(&table, &target).unwrap_err();
        assert!(
            errors.iter().any(|e| e.subject == Subject::Layout(id)),
            "mutation {mutation}: {errors:?}"
        );
    }
}
#[test]
fn receipt_layout_runtime_descriptor_uses_a_distinct_metadata_release_without_a_scan() {
    let (_, table, id) = fixture();
    let descriptor = hirsel_lir::rt_program::descriptor(&table, id).unwrap();
    assert_eq!(descriptor.steps.len(), 1);
    let step = &descriptor.steps[0];
    assert_eq!(step.kind, hirsel_lir::rt_program::StepKind::ReleaseReceipt);
    assert_eq!(
        step.kind.code(),
        hirsel_lir::runtime_abi::STEP_RELEASE_RECEIPT
    );
    assert_eq!(step.offset, 0);
    assert_eq!(step.field, hirsel_lir::runtime_abi::WHOLE_VALUE);
    assert!(step.layout.is_none());
    assert_eq!(
        descriptor.logical,
        hirsel_lir::runtime_abi::LOGICAL_UNSUPPORTED
    );
    descriptor.check_finalizers(&table).unwrap();
}
#[test]
fn unused_receipt_layout_does_not_refuse_an_ordinary_unit() {
    use crate::support::{ENTRY, Lir};
    use hirsel_lir::stage::Value;
    for metadata in [false, true] {
        let mut t = Lir::<Value>::new(&[], None);
        t.builder.ret(ENTRY, None);
        if metadata {
            let (_, table, id) = fixture();
            t.table.layouts.push(table.layouts[id.0 as usize].clone());
        }
        let errors = t.check();
        assert!(errors.is_empty(), "{errors:?}");
    }
}
