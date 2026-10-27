//! Local borrowed-capture schema controls; no source/provider or C admission.
use hirsel_codec::snapshot_struct;
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::abi_types::*;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_lir::sealed_payload_source::*;
use hirsel_lir::stage::{StageKind, Value};
use hirsel_syntax::{ContentHash, DefPath};
use std::hash::{Hash, Hasher};

fn fixture() -> (
    LirUnit<Value>,
    LayoutTable,
    FnAbi,
    ClosureSite,
    ReceiptEntry,
) {
    fixture_with_once(false)
}
fn fixture_with_once(
    once: bool,
) -> (
    LirUnit<Value>,
    LayoutTable,
    FnAbi,
    ClosureSite,
    ReceiptEntry,
) {
    fixture_with_custody(once, false)
}
fn fixture_with_custody(
    once: bool,
    owned: bool,
) -> (
    LirUnit<Value>,
    LayoutTable,
    FnAbi,
    ClosureSite,
    ReceiptEntry,
) {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let types = hirsel_types::TypeInterner::new(None);
    let ty = types.intern(hirsel_types::Type::with_kind(
        hirsel_types::TypeKind::Primitive(hirsel_types::Primitive::U64),
    ));
    let mut builder = LayoutBuilder::new(&target, &owners);
    let scalar = builder.layout(&types, ty).unwrap();
    let mut ordinary = vec![(
        scalar,
        if owned {
            hirsel_core_contract::Passing::Take
        } else {
            hirsel_core_contract::Passing::Borrow
        },
    )];
    if once {
        ordinary.push((scalar, hirsel_core_contract::Passing::Take));
    }
    let base = builder
        .environment_data(
            &DefPath::core("receipt_closure_fixture"),
            0,
            &ordinary,
            if once { &[1] } else { &[] },
        )
        .unwrap();
    let hash = ContentHash::of_bytes(b"complete closure source profile");
    let profile = ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: ClosedStorageType {
            ty: NativeType::Primitive(Primitive::U64),
            source_type: hash,
        },
        functions: vec![],
    };
    let contract = ReceiptContract::new(
        vec![],
        None,
        vec![ReceiptCapture {
            ordinal: 0,
            ty: ReceiptType {
                source_type: hash,
                profile: profile.content().unwrap(),
            },
            mode: if owned {
                ReceiptMode::Take
            } else {
                ReceiptMode::Borrow
            },
        }],
        vec![],
    )
    .unwrap();
    let source = capture_source(contract.clone());
    let environment = if owned {
        builder.owning_receipt_environment(base, &source).unwrap()
    } else {
        builder
            .borrowed_receipt_environment(base, &contract)
            .unwrap()
    };
    let meta = builder.payload_receipt(&NativeType::Primitive(Primitive::U64));
    let mut table = builder.finish();
    let abi = FnAbi {
        receipts: Some(contract.clone()),
        key: FnAbiKey {
            callable: hash,
            abi_version: hirsel_lir::ABI_VERSION,
            receipt_contract: Some(contract.content().unwrap()),
        },
        shape: CallableShape::Closure,
        multiplicity: if once {
            CallMultiplicity::Once
        } else {
            CallMultiplicity::Many
        },
        context: true,
        environment: true,
        caller_site: false,
        destination: false,
        result: None,
        diverges: false,
        result_arena: None,
        parameters: vec![],
        version: hirsel_lir::ABI_VERSION,
        nonretaining: vec![],
        nonretaining_captures: vec![],
        scoped_contract: hirsel_lir::abi::scoped_contract_digest(&[], &[]),
        scoped_entries: vec![],
    };
    table.fn_abis.push(abi.clone());
    let mut unit = LirUnit::<Value>::new(UnitHeader {
        source_count: 1,
        track_caller: false,
        stage: StageKind::Value,
        lir_version: hirsel_lir::LIR_VERSION,
        origin: UnitOrigin::Source,
        key: hirsel_lir::objects::UnitKey(hash),
        definition: DefPath::core("receipt_closure_fixture"),
        implementation_revision: hash,
        instance: hash,
        target: table.target,
        abi: FnAbiId(0),
    });
    unit.scopes.push(ScopeInfo { parent: None });
    unit.types = vec![
        LirType {
            repr: ValueRepr::Scalar(ScalarKind::U64),
            layout: scalar,
        },
        LirType {
            repr: ValueRepr::PayloadReceipt,
            layout: meta,
        },
    ];
    unit.values = vec![
        ValueInfo {
            ty: LirTypeId(0),
            holding: Holding::Plain,
        },
        ValueInfo {
            ty: LirTypeId(1),
            holding: if owned {
                Holding::Owned
            } else {
                Holding::Borrowed(LoanId(0))
            },
        },
        ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Plain,
        },
    ];
    unit.captures.push(ValueId(0));
    unit.slots.push(SlotInfo {
        ty: LirTypeId(0),
        kind: SlotKind::Capture(0),
        scope: ScopeId(0),
    });
    unit.slots.push(SlotInfo {
        ty: LirTypeId(1),
        kind: SlotKind::ReceiptCapture(0),
        scope: ScopeId(0),
    });
    if once {
        let live = table.layouts[base.0 as usize].fields.last().unwrap().layout;
        unit.types.push(LirType {
            repr: ValueRepr::Scalar(ScalarKind::U64),
            layout: live,
        });
        for ordinal in [1, 2] {
            let ty = if ordinal == 2 {
                LirTypeId(2)
            } else {
                LirTypeId(0)
            };
            let value = ValueId(unit.values.len() as u32);
            unit.values.push(ValueInfo {
                ty,
                holding: Holding::Plain,
            });
            unit.captures.push(value);
            unit.slots.push(SlotInfo {
                ty,
                kind: SlotKind::Capture(ordinal),
                scope: ScopeId(0),
            });
        }
    }
    unit.addresses.push(AddressInfo {
        source_place: None,
        unchecked: vec![],
        base: AddressBase::Slot(SlotId(1)),
        projection: PoolRange::EMPTY,
        ty: LirTypeId(1),
    });
    unit.tables.loans.push(LoanInfo {
        address: AddressId(0),
        mutable: false,
    });
    unit.tables.source_anchors.push(SourceAnchor {
        body: 0,
        block: 0,
        statement: 0,
        site: SiteRange {
            source: 0,
            start: 0,
            end: 1,
        },
    });
    unit.tables.symbols.push(Symbol {
        kind: SymbolKind::Function,
        definition: unit.header.definition.clone(),
        instance: None,
    });
    unit.blocks.push(Block {
        kind: BlockKind::Body,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::EMPTY,
        anchor: SourceAnchorId(0),
    });
    unit.terminators.push(Terminator::Return(None));
    let mut ordinary_captures = vec![CaptureValue {
        value: ValueId(0),
        usage: if owned {
            CaptureUse::Copy
        } else {
            CaptureUse::Borrow
        },
    }];
    if once {
        ordinary_captures.push(CaptureValue {
            value: ValueId(3),
            usage: CaptureUse::Take,
        });
    }
    let captures = PoolRange::push(&mut unit.pools.captures, ordinary_captures);
    let site = ClosureSite {
        scoped: true,
        code: Some(ClosureCode {
            symbol: SymbolId(0),
            abi: FnAbiId(0),
        }),
        environment,
        captures,
        destination: DestinationClass::Heap,
        placement: None,
        anchor: SourceAnchorId(0),
        receipts: Some(ReceiptClosure {
            contract: contract.content().unwrap(),
            captures: vec![ReceiptClosureCapture {
                ordinal: 0,
                receipt: ValueId(1),
            }],
        }),
    };
    let entry = ReceiptEntry {
        contract: contract.content().unwrap(),
        parameters: vec![],
        captures: vec![ReceiptEntrySlot {
            source: 0,
            slot: SlotId(1),
            version: ValueId(2),
        }],
        result: None,
        environment: Some(environment),
    };
    unit.tables.closures.push(site.clone());
    unit.tables.receipt_entry = Some(entry.clone());
    (unit, table, abi, site, entry)
}

fn capture_source(contract: ReceiptContract) -> ReceiptSource {
    let hash = ContentHash::of_bytes(b"complete closure source profile");
    ReceiptSource {
        original: hirsel_lir::invocations::ReferenceBody {
            definition: DefPath::core("receipt_closure_fixture"),
            body: 0,
            instance: hash,
        },
        instance: hash,
        source: hash,
        implementation_revision: hash,
        contract,
        profiles: vec![ReceiptProfile {
            port: ReceiptPort::Capture(0),
            profile: ClosedStorageProfile {
                schema: ClosedStorageProfile::SCHEMA,
                root: ClosedStorageType {
                    ty: NativeType::Primitive(Primitive::U64),
                    source_type: hash,
                },
                functions: vec![],
            },
        }],
    }
}

#[test]
fn receipt_closure_129_owning_schema_preserves_copy_take_and_once_entry_bindings() {
    for once in [false, true] {
        let (unit, table, abi, site, entry) = fixture_with_custody(once, true);
        let source = capture_source(abi.receipts.clone().unwrap());
        entry
            .check_with_source(&abi, &unit, &table, &source)
            .unwrap();
        assert!(entry.check(&abi, &unit, &table).is_err());
        for usage in [CaptureUse::Copy, CaptureUse::Take] {
            let mut unit = unit.clone();
            unit.pools.captures[0].usage = usage;
            site.receipts
                .as_ref()
                .unwrap()
                .check_with_source(&site, &abi, &unit, &table, &source)
                .unwrap();
        }
        unit.check_identity(&table).unwrap();
        let restored: LirUnit<Value> =
            hirsel_codec::from_text(&hirsel_codec::to_text(&unit)).unwrap();
        assert_eq!(restored.tables.receipt_entry, unit.tables.receipt_entry);
        assert_eq!(restored.tables.closures, unit.tables.closures);
    }
}

#[test]
fn receipt_closure_129_owning_schema_rejects_wrong_custody_and_original_types() {
    let (unit, table, abi, site, entry) = fixture_with_custody(true, true);
    let source = capture_source(abi.receipts.clone().unwrap());
    site.receipts
        .as_ref()
        .unwrap()
        .check_with_source(&site, &abi, &unit, &table, &source)
        .unwrap();
    for mutation in 0..8 {
        let mut unit = unit.clone();
        let mut site = site.clone();
        let mut table = table.clone();
        let mut source = source.clone();
        match mutation {
            0 => unit.pools.captures[0].usage = CaptureUse::Borrow,
            1 => unit.values[1].holding = Holding::Borrowed(LoanId(0)),
            2 => site.receipts.as_mut().unwrap().captures[0].ordinal = 1,
            3 => source.profiles.clear(),
            4 => source.profiles[0].profile.root.ty = NativeType::Primitive(Primitive::Bool),
            5 => unit.types[1].layout = unit.types[0].layout,
            6 => table.layouts[site.environment.0 as usize].drop.clear(),
            7 => table.layouts[site.environment.0 as usize].copy = CopyRecipe::Bits,
            _ => unreachable!(),
        }
        assert!(
            site.receipts
                .as_ref()
                .unwrap()
                .check_with_source(&site, &abi, &unit, &table, &source)
                .is_err(),
            "mutation {mutation}"
        );
    }
    let mut missing = entry.clone();
    missing.environment = None;
    assert!(
        missing
            .check_with_source(&abi, &unit, &table, &source)
            .is_err()
    );
    let mut wrong = unit.clone();
    wrong.values[2].holding = Holding::Owned;
    assert!(
        entry
            .check_with_source(&abi, &wrong, &table, &source)
            .is_err()
    );
}
fn hashed(value: &impl Hash) -> u64 {
    let mut hash = std::hash::DefaultHasher::new();
    value.hash(&mut hash);
    hash.finish()
}

#[test]
fn receipt_closure_33_roundtrips_borrowed_companions_and_exact_entry_environment() {
    let (unit, table, abi, site, entry) = fixture();
    site.receipts
        .as_ref()
        .unwrap()
        .check(&site, &abi, &unit, &table)
        .unwrap();
    entry.check(&abi, &unit, &table).unwrap();
    unit.check_identity(&table).unwrap();
    let restored: LirUnit<Value> = hirsel_codec::from_text(&hirsel_codec::to_text(&unit)).unwrap();
    assert_eq!(restored.tables.closures, unit.tables.closures);
    assert_eq!(restored.tables.receipt_entry, unit.tables.receipt_entry);
}
#[test]
fn receipt_closure_33_rejects_wrong_profiles_selectors_modes_and_metadata_custody() {
    let (unit, table, abi, site, _) = fixture();
    for mutation in 0..7 {
        let mut unit = unit.clone();
        let mut site = site.clone();
        let mut abi = abi.clone();
        match mutation {
            0 => {
                site.receipts.as_mut().unwrap().contract =
                    ContentHash::of_bytes(b"another full profile")
            }
            1 => site.receipts.as_mut().unwrap().captures[0].ordinal = 1,
            2 => unit.pools.captures[0].usage = CaptureUse::Take,
            3 => unit.values[1].holding = Holding::Owned,
            4 => site.scoped = false,
            5 => abi.receipts.as_mut().unwrap().captures[0].mode = ReceiptMode::Take,
            6 => site.environment = LayoutId(0),
            _ => unreachable!(),
        }
        assert!(
            site.receipts
                .as_ref()
                .unwrap()
                .check(&site, &abi, &unit, &table)
                .is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn receipt_closure_33_references_cover_companions_and_entry_layout_and_remap_values() {
    let (unit, table, _, mut site, _) = fixture();
    for mutation in 0..2 {
        let mut bad = unit.clone();
        if mutation == 0 {
            bad.tables.closures[0].receipts.as_mut().unwrap().captures[0].receipt =
                ValueId(u32::MAX);
        } else {
            bad.tables.receipt_entry.as_mut().unwrap().environment = Some(LayoutId(u32::MAX));
        }
        assert!(bad.check_identity(&table).is_err());
    }
    site.receipts
        .as_mut()
        .unwrap()
        .remap_values(&|v| ValueId(v.0 + 9));
    assert_eq!(site.receipts.unwrap().captures[0].receipt, ValueId(10));
}
#[test]
fn receipt_closure_33_absence_preserves_both_legacy_encodings_and_hash_streams() {
    #[derive(Hash)]
    struct LegacyClosure {
        scoped: bool,
        code: Option<ClosureCode>,
        environment: LayoutId,
        captures: PoolRange<CaptureValue>,
        destination: DestinationClass,
        placement: Option<Placement>,
        anchor: SourceAnchorId,
    }
    snapshot_struct!(LegacyClosure {
        scoped,
        code,
        environment,
        captures,
        destination,
        placement,
        anchor
    });
    #[derive(Hash)]
    struct LegacyEntry {
        contract: ContentHash,
        parameters: Vec<ReceiptEntrySlot>,
        captures: Vec<ReceiptEntrySlot>,
        result: Option<SlotId>,
    }
    snapshot_struct!(LegacyEntry {
        contract,
        parameters,
        captures,
        result
    });
    let (_, _, _, mut site, mut entry) = fixture();
    site.receipts = None;
    entry.environment = None;
    let old = LegacyClosure {
        scoped: site.scoped,
        code: site.code,
        environment: site.environment,
        captures: site.captures,
        destination: site.destination.clone(),
        placement: site.placement,
        anchor: site.anchor,
    };
    assert_eq!(hashed(&old), hashed(&site));
    let text = hirsel_codec::to_text(&old).replacen("(LegacyClosure", "(ClosureSite", 1);
    assert_eq!(hirsel_codec::to_text(&site), text);
    assert_eq!(hirsel_codec::from_text::<ClosureSite>(&text).unwrap(), site);
    let old = LegacyEntry {
        contract: entry.contract,
        parameters: entry.parameters.clone(),
        captures: entry.captures.clone(),
        result: entry.result,
    };
    assert_eq!(hashed(&old), hashed(&entry));
    let text = hirsel_codec::to_text(&old).replacen("(LegacyEntry", "(ReceiptEntry", 1);
    assert_eq!(hirsel_codec::to_text(&entry), text);
    assert_eq!(
        hirsel_codec::from_text::<ReceiptEntry>(&text).unwrap(),
        entry
    );
}

#[test]
fn receipt_closure_33_entry_rejects_mismatched_original_capture_storage() {
    let (unit, table, abi, _, entry) = fixture();
    for mutation in 0..8 {
        let mut unit = unit.clone();
        let mut table = table.clone();
        let wrapper = entry.environment.unwrap();
        let base = table.layouts[wrapper.0 as usize].fields[0].layout;
        match mutation {
            0 => unit
                .slots
                .retain(|s| !matches!(s.kind, SlotKind::Capture(_))),
            1 => unit.slots.push(unit.slots[0].clone()),
            2 => unit.slots[0].kind = SlotKind::Capture(1),
            3 => unit.slots[0].ty = LirTypeId(1),
            4 => unit.values[0].ty = LirTypeId(1),
            5 => table.layouts[base.0 as usize].fields[0].offset = 8,
            6 => table.layouts[base.0 as usize].fields[0].unaligned = true,
            7 => table.layouts[base.0 as usize].size = SizeSpec::Fixed(16),
            _ => unreachable!(),
        }
        assert!(
            entry.check(&abi, &unit, &table).is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn receipt_closure_33_entry_preserves_complete_once_capture_storage() {
    let (unit, table, abi, _, entry) = fixture_with_once(true);
    entry.check(&abi, &unit, &table).unwrap();
    let wrapper = entry.environment.unwrap();
    let (base, companions) = hirsel_lir::receipt_closure::borrowed_environment(
        &table,
        wrapper,
        abi.receipts.as_ref().unwrap(),
    )
    .unwrap();
    let base = &table.layouts[base.0 as usize];
    assert_eq!(unit.captures.len(), 3);
    assert_eq!(base.fields[2].offset, 16); // Existing once live word.
    assert_eq!(companions[0].offset, 24); // Strictly after the complete base.
    let mut bad = unit.clone();
    bad.slots.retain(|s| s.kind != SlotKind::Capture(2));
    assert!(entry.check(&abi, &bad, &table).is_err());
}

#[test]
fn receipt_closure_33_environment_rejects_a_selector_for_the_once_live_word() {
    let (_, mut table, abi, _, entry) = fixture_with_once(true);
    let wrapper = entry.environment.unwrap();
    let base = table.layouts[wrapper.0 as usize].fields[0].layout;
    let original = abi.receipts.as_ref().unwrap();
    let mut captures = original.captures.clone();
    captures[0].ordinal = 2; // The live word is not an original source capture.
    let contract = ReceiptContract::new(vec![], None, captures, vec![]).unwrap();
    table.layouts[wrapper.0 as usize].key = hirsel_lir::receipt_closure::environment_key(
        &table.layouts[base.0 as usize].key,
        contract.content().unwrap(),
    );
    assert_eq!(
        hirsel_lir::receipt_closure::borrowed_environment(&table, wrapper, &contract),
        Err("receipt environment names a foreign original capture")
    );
}
