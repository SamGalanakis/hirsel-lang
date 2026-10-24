//! Declaration-derived descriptor controls, separate from source-event authority.
use hirsel_layout::layout::LayoutBuilder;
use hirsel_lir::layout_types::*;
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations, Primitive};
use hirsel_lir::sealed::{SealDescriptor, StorageAccess};
use hirsel_syntax::ContentHash;

enum Fixture {
    Scalar,
    Bytes,
    List,
    Record,
    Shared,
}
fn fixture(
    kind: Fixture,
) -> (
    hirsel_layout::operation::OperationRegistry,
    TargetSpec,
    LayoutTable,
    SealDescriptor,
) {
    let registry = crate::support::registry();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut types = hirsel_types::testing::Types {
        store: registry.types.clone(),
    };
    let scalar = types.p(hirsel_types::Primitive::U64);
    let byte_type = types.p(hirsel_types::Primitive::Bytes);
    let (source, root) = match kind {
        Fixture::Scalar => (NativeType::Primitive(Primitive::U64), scalar),
        Fixture::Bytes => (bytes(), byte_type),
        Fixture::List => (
            NativeType::Builtin(
                hirsel_lir::operation::BuiltinType::List,
                vec![OperationArgument::Type(bytes())],
            ),
            types.list(byte_type),
        ),
        Fixture::Record => (
            NativeType::Record(vec![
                (false, NativeType::Primitive(Primitive::U64)),
                (false, bytes()),
            ]),
            types.record(&[scalar, byte_type]),
        ),
        Fixture::Shared => (
            NativeType::Builtin(
                hirsel_lir::operation::BuiltinType::Shared,
                vec![OperationArgument::Type(bytes())],
            ),
            types.builtin(
                hirsel_types::BuiltinType::Shared,
                vec![hirsel_types::GenericArgument::Type(byte_type)],
            ),
        ),
    };
    let mut builder = LayoutBuilder::new(&target, &registry.owners);
    builder.layout(&registry.types, root).unwrap();
    let table = builder.finish();
    hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
    let descriptor = registry.seal_descriptor(&source, &table, &target).unwrap();
    descriptor.check(&table).unwrap();
    descriptor.validate(&registry, &table, &target).unwrap();
    (registry, target, table, descriptor)
}
fn bytes() -> NativeType {
    NativeType::Primitive(Primitive::Bytes)
}
#[test]
fn seal_descriptor_reconstructs_scalar_bytes_collections_records_and_shared_boundaries() {
    for kind in [
        Fixture::Scalar,
        Fixture::Bytes,
        Fixture::List,
        Fixture::Record,
        Fixture::Shared,
    ] {
        fixture(kind);
    }
}
#[test]
fn seal_descriptor_rejects_missing_duplicate_and_altered_access_programs() {
    let (_, _, table, good) = fixture(Fixture::Bytes);
    for mutation in 0..3 {
        let mut bad = good.clone();
        match mutation {
            0 => {
                bad.access.pop();
            }
            1 => bad.access.push(good.access[0].clone()),
            2 => bad.access[0].scan.push(ScanStep::Counted { offset: 0 }),
            _ => unreachable!(),
        }
        assert!(bad.check(&table).is_err(), "mutation {mutation}");
    }
}
#[test]
fn seal_descriptor_logical_charge_claim_needs_independent_declaration_reconstruction() {
    let (registry, target, table, mut candidate) = fixture(Fixture::Scalar);
    candidate.logical.facts[0].bytes = hirsel_lir::logical_storage::LogicalBytes::Bytes(999);
    // It is a locally well-formed graph with the wrong declared logical size.
    candidate.check(&table).unwrap();
    let error = candidate.validate(&registry, &table, &target).unwrap_err();
    assert!(error.contains("independent declarations"), "{error}");
}
#[test]
fn seal_descriptor_type_declarations_do_not_replace_selected_original_source() {
    let (registry, target, table, descriptor) = fixture(Fixture::Scalar);
    descriptor.validate(&registry, &table, &target).unwrap();
    let error = descriptor
        .validate_at(
            &registry,
            &crate::support::path("selected_source"),
            ContentHash::of_bytes(b"original instance"),
            &table,
            &target,
        )
        .unwrap_err();
    assert!(
        error.contains("missing selected sealed source declarations"),
        "{error}"
    );
    assert!(descriptor.validate(&(), &table, &target).is_err());
}
#[test]
fn seal_descriptor_private_metadata_is_never_an_ordinary_payload_access_program() {
    let (registry, target, mut table, mut descriptor) = fixture(Fixture::Scalar);
    let mut builder = LayoutBuilder::new(&target, &registry.owners);
    let id = builder.payload_receipt(&bytes());
    let record = builder.record(id).clone();
    let id = LayoutId(table.layouts.len() as u32);
    table.layouts.push(record);
    hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
    descriptor.access.push(StorageAccess {
        layout: id,
        scan: vec![],
    });
    let error = descriptor.check(&table).unwrap_err();
    assert!(
        error.contains("private receipt metadata is not sealed source payload"),
        "{error}"
    );
}
#[test]
fn seal_descriptor_roundtrip_preserves_complete_logical_and_physical_association() {
    let (registry, target, table, descriptor) = fixture(Fixture::Bytes);
    let text = hirsel_codec::to_text(&descriptor);
    let restored: SealDescriptor = hirsel_codec::from_text(&text).unwrap();
    assert_eq!(restored, descriptor);
    restored.validate(&registry, &table, &target).unwrap();
    assert!(
        restored
            .layouts()
            .all(|layout| table.layout(layout).is_some())
    );
}
