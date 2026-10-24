//! Local entry schema controls; source authority and executable custody remain closed.
use crate::support::{ENTRY, Lir, ROOT, l};
use hirsel_core_contract::Passing;
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_lir::sealed_payload_source::*;
use hirsel_lir::stage::Value;
use hirsel_syntax::ContentHash;

fn fixture() -> (Lir<Value>, ReceiptEntry) {
    let mut t = Lir::<Value>::new(
        &[
            (Passing::Borrow, l::BLK),
            (Passing::Take, l::BLK),
            (Passing::Mut, l::BLK),
        ],
        None,
    );
    for take in [false, true, false] {
        let value = if take {
            t.owned(l::BLK)
        } else {
            t.plain(l::BLK)
        };
        t.param(value);
    }
    let capture = t.plain(l::BLK);
    t.builder.unit.captures.push(capture);
    t.builder.ret(ENTRY, None);
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let id = builder.payload_receipt(&NativeType::Primitive(Primitive::Bytes));
    let layout = LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(builder.record(id).clone());
    let meta = t.ty(ValueRepr::PayloadReceipt, layout);
    let mut bind = |source, kind| {
        let slot = SlotId(t.builder.unit.slots.len() as u32);
        t.builder.unit.slots.push(SlotInfo {
            ty: meta,
            kind,
            scope: ROOT,
        });
        let version = ValueId(t.builder.unit.values.len() as u32);
        t.builder.unit.values.push(ValueInfo {
            ty: meta,
            holding: Holding::Plain,
        });
        ReceiptEntrySlot {
            source,
            slot,
            version,
        }
    };
    let parameters = (0..3)
        .map(|source| bind(source, SlotKind::ReceiptParameter(source)))
        .collect();
    let captures = vec![bind(0, SlotKind::ReceiptCapture(0))];
    let result = SlotId(t.builder.unit.slots.len() as u32);
    t.builder.unit.slots.push(SlotInfo {
        ty: meta,
        kind: SlotKind::ReceiptResult,
        scope: ROOT,
    });
    let ty = ReceiptType {
        source_type: ContentHash::of_bytes(b"full original type"),
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
        Some(ReceiptResult { ty: ty.clone() }),
        vec![ReceiptCapture {
            ordinal: 0,
            ty,
            mode: ReceiptMode::Take,
        }],
        vec![],
    )
    .unwrap();
    let entry = ReceiptEntry {
        contract: contract.content().unwrap(),
        parameters,
        captures,
        result: Some(result),
    };
    let abi = &mut t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
    abi.shape = CallableShape::Closure;
    abi.environment = true;
    abi.caller_site = true;
    abi.key.receipt_contract = Some(entry.contract);
    abi.receipts = Some(contract);
    entry
        .check(
            &t.table.fn_abis[t.builder.unit.header.abi.0 as usize],
            &t.builder.unit,
            &t.table,
        )
        .unwrap();
    (t, entry)
}

#[test]
fn receipt_entry_covers_original_parameters_captures_and_a_receipt_only_unit_result() {
    let (t, entry) = fixture();
    let abi = &t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
    assert!(abi.result.is_none());
    assert_eq!(entry.versions().count(), 4);
    assert!(entry.result.is_some());
    entry.check(abi, &t.builder.unit, &t.table).unwrap();
    let text = hirsel_codec::to_text(&entry);
    let restored: ReceiptEntry = hirsel_codec::from_text(&text).unwrap();
    assert_eq!(restored, entry);
}
#[test]
fn receipt_entry_rejects_forged_contract_missing_coverage_and_wrong_namespace() {
    let (t, good) = fixture();
    let abi = &t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
    for mutation in 0..6 {
        let mut entry = good.clone();
        let mut unit = t.builder.unit.clone();
        match mutation {
            0 => entry.contract = ContentHash::of_bytes(b"forged"),
            1 => {
                entry.parameters.pop();
            }
            2 => entry.captures[0].source = 1,
            3 => entry.result = None,
            4 => unit.slots[entry.parameters[0].slot.index()].kind = SlotKind::Local,
            5 => entry.parameters.swap(0, 1),
            _ => unreachable!(),
        }
        assert!(
            entry.check(abi, &unit, &t.table).is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn receipt_entry_refuses_double_holder_versions_wrong_types_and_missing_original_values() {
    let (t, entry) = fixture();
    let abi = &t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
    for mutation in 0..6 {
        let mut unit = t.builder.unit.clone();
        let mut entry = entry.clone();
        match mutation {
            0 => unit.values[entry.parameters[1].version.index()].holding = Holding::Owned,
            1 => entry.parameters[1].version = entry.parameters[0].version,
            2 => unit.values[entry.parameters[0].version.index()].ty = unit.values[0].ty,
            3 => unit.parameters.clear(),
            4 => unit.captures.clear(),
            5 => {
                unit.types[unit.slots[entry.parameters[0].slot.index()].ty.index()].repr =
                    ValueRepr::Scalar(ScalarKind::Pointer)
            }
            _ => unreachable!(),
        }
        assert!(
            entry.check(abi, &unit, &t.table).is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn receipt_entry_references_participate_in_common_schema_bounds_checks() {
    for mutation in 0..3 {
        let (t, mut entry) = fixture();
        let (mut unit, table, _) = t.finish();
        unit.tables.receipt_entry = Some(entry.clone());
        unit.check_identity(&table).unwrap();
        match mutation {
            0 => entry.parameters[0].slot = SlotId(u32::MAX),
            1 => entry.captures[0].version = ValueId(u32::MAX),
            2 => entry.result = Some(SlotId(u32::MAX)),
            _ => unreachable!(),
        }
        unit.tables.receipt_entry = Some(entry);
        assert!(unit.check_identity(&table).is_err(), "mutation {mutation}");
    }
}
#[test]
fn receipt_entry_absence_preserves_legacy_side_table_encoding_and_hash() {
    use std::hash::{Hash, Hasher};
    let tables = SideTables::default();
    let legacy = format!(
        "(SideTables{} 0{})",
        "\n  (list)".repeat(13),
        "\n  (list)".repeat(5)
    );
    assert_eq!(hirsel_codec::to_text(&tables), legacy);
    let decoded: SideTables = hirsel_codec::from_text(&legacy).unwrap();
    assert_eq!(decoded, tables);
    let mut old = std::hash::DefaultHasher::new();
    for _ in 0..13 {
        0usize.hash(&mut old);
    }
    0u32.hash(&mut old);
    for _ in 0..5 {
        0usize.hash(&mut old);
    }
    let mut actual = std::hash::DefaultHasher::new();
    tables.hash(&mut actual);
    assert_eq!(old.finish(), actual.finish());
    let empty = ReceiptContract::new(vec![], None, vec![], vec![]).unwrap();
    let mut demanded = tables;
    demanded.receipt_entry = Some(ReceiptEntry {
        contract: empty.content().unwrap(),
        parameters: vec![],
        captures: vec![],
        result: None,
    });
    assert_ne!(hirsel_codec::to_text(&demanded), legacy);
    let restored: SideTables = hirsel_codec::from_text(&hirsel_codec::to_text(&demanded)).unwrap();
    assert_eq!(restored, demanded);
}
#[test]
fn receipt_entry_declaration_alone_is_not_executable_c_authority() {
    let (mut t, entry) = fixture();
    t.builder.unit.tables.receipt_entry = Some(entry);
    let errors = t.check();
    assert!(errors.iter().any(|e| matches!(e, hirsel_lir::validate::ViolationKind::Schema(s) if s.contains("receipt entry requires independent source and custody"))), "{errors:?}");
}
