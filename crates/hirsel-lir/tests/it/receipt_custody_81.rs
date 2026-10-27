//! Public C controls for source-bound local metadata and affine cleanup.
use crate::support::{self, ENTRY, ROOT};
use hirsel_core_contract::{Manifest, OperationId};
use hirsel_layout::{
    layout::{LayoutBuilder, Owners},
    operation::OperationRegistry,
};
use hirsel_lir::validate::{self, Inputs, ViolationKind as K};
use hirsel_lir::{
    LayoutTable, TargetSpec, ir::*, layout_types::*, operation::*, sealed::SealDescriptor,
    sealed_payload::*, stage::Rc,
};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{Primitive as P, Type, TypeKind};

struct Declarations {
    registry: OperationRegistry,
    payloads: Vec<SourcePayload>,
}
impl OperationDeclarations for Declarations {
    fn derive(
        &self,
        op: &OperationId,
        args: &[OperationArgument],
        target: &TargetSpec,
    ) -> Result<(hirsel_lir::FnAbi, LayoutTable), hirsel_lir::layout::LayoutError> {
        self.registry.derive(op, args, target)
    }
    fn seal_events(&self, _: &DefPath, _: ContentHash) -> Option<Vec<SourceEvent>> {
        Some(vec![])
    }
    fn seal_payloads(&self, _: &DefPath, _: ContentHash) -> Option<Vec<SourcePayload>> {
        Some(self.payloads.clone())
    }
    fn seal_descriptor_for_source(
        &self,
        _: &DefPath,
        _: ContentHash,
        root: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(root, table, target)
    }
}

fn fixture() -> (support::Lir<Rc>, Declarations, ValueId, ValueId, LayoutId) {
    let types = support::session();
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(P::I64)));
    let boxed = types.intern(Type::with_kind(TypeKind::Box(scalar)));
    let native = NativeType::Box(Box::new(NativeType::Primitive(Primitive::I64)));
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut layouts = LayoutBuilder::new(&target, &owners);
    let layout = layouts.layout(&types, boxed).unwrap();
    let receipt_layout = layouts.payload_receipt(&native);
    let mut t = support::Lir::<Rc>::new(&[], None);
    let offset = t.table.layouts.len() as u32;
    for mut row in layouts.finish().layouts {
        row.for_each_reference_mut(&mut |id| id.0 += offset);
        t.table.layouts.push(row);
    }
    let layout = LayoutId(layout.0 + offset);
    let meta = LayoutId(receipt_layout.0 + offset);
    let descriptor = t
        .operations
        .seal_descriptor(&native, &t.table, &target)
        .unwrap();
    t.builder.unit.tables.seal_descriptors.push(descriptor);
    let payload = match t.table.layouts[layout.0 as usize].kind {
        LayoutKind::Box { payload } => payload,
        _ => unreachable!(),
    };
    let leaf = t.value_as(ValueRepr::Scalar(ScalarKind::I64), payload, Holding::Plain);
    let ty = t.builder.unit.values[leaf.index()].ty;
    let constant = ConstantId(t.builder.unit.tables.constants.len() as u32);
    t.builder.unit.tables.constants.push(ConstantData {
        ty,
        value: ConstantValue::Integer(7),
    });
    t.builder.push(
        ENTRY,
        Inst::Const {
            result: leaf,
            constant,
        },
    );
    let root = t.value_as(ValueRepr::Block, layout, Holding::Owned);
    let construct = t.construct_site(
        ConstructorKind::Box,
        t.table.layouts[layout.0 as usize].references()[0],
        &[(0, leaf)],
        t.heap(),
    );
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: root,
            construct,
            failure,
        },
    );
    let source = t.builder.unit.tables.source_anchors[0].clone();
    let recipe = PayloadRecipe {
        source,
        operation: PayloadOperation::Allocate {
            destination: PayloadDestination::Heap,
        },
        inputs: vec![PayloadOperand {
            place: 0,
            ty: NativeType::Primitive(Primitive::I64),
            producer: PayloadProducer::Literal(PayloadLiteral::Integer(7)),
            capture_slot: None,
        }],
    };
    let declarations = Declarations {
        registry: t.operations.clone(),
        payloads: vec![SourcePayload {
            root_type: native,
            recipe: recipe.clone(),
        }],
    };
    t.builder.unit.tables.seal_payloads.push(PayloadTable {
        descriptor: SealDescriptorId(0),
        bindings: vec![PayloadBinding {
            recipe,
            values: vec![leaf],
        }],
    });
    let receipt = t.value_as(ValueRepr::PayloadReceipt, meta, Holding::Owned);
    let inputs = PoolRange::push(
        &mut t.builder.unit.pools.receipt_inputs,
        [ReceiptInput {
            original: leaf,
            receipt: None,
        }],
    );
    t.builder.push(
        ENTRY,
        Inst::PayloadReceipt {
            result: receipt,
            value: root,
            shape: SealDescriptorId(0),
            binding: PayloadBindingId(0),
            inputs,
        },
    );
    (t, declarations, root, receipt, meta)
}

fn release_at(
    t: &mut support::Lir<Rc>,
    block: BlockId,
    value: ValueId,
    layout: LayoutId,
    remaining: &[ValueId],
) {
    let cleanup = t.releasing(remaining);
    let failure = t.failure(cleanup);
    let anchor = t.instruction_anchor();
    t.builder.push(
        block,
        Inst::Release {
            value,
            layout,
            live: MaskRef::All,
            anchor,
            failure,
        },
    );
}
fn finish(
    mut t: support::Lir<Rc>,
    root: ValueId,
    receipt: ValueId,
    meta: LayoutId,
) -> (LirUnit<Rc>, LayoutTable, Manifest) {
    release_at(&mut t, ENTRY, receipt, meta, &[root]);
    let layout = t.payload(root);
    release_at(&mut t, ENTRY, root, layout, &[]);
    t.builder.ret(ENTRY, None);
    t.finish()
}
fn check(
    unit: &LirUnit<Rc>,
    table: &LayoutTable,
    manifest: &Manifest,
    declarations: &Declarations,
) -> Vec<validate::Violation> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = Inputs {
        target: &target,
        layouts: table,
        manifest,
        operations: declarations,
        discharge: None,
        memo: Default::default(),
    };
    let ordinary = validate::check(unit, &inputs);
    assert_eq!(ordinary, validate::check_scoped_transport(unit, &inputs));
    let ready = unit.clone().into_stage::<hirsel_lir::stage::Ready>();
    let ready_errors = validate::check(&ready, &inputs);
    assert_eq!(
        ordinary.is_empty(),
        ready_errors.is_empty(),
        "Ready custody: {ready_errors:?}"
    );
    ordinary
}

#[test]
fn local_source_snapshot_has_independent_source_proof_and_one_metadata_holder() {
    let (t, declarations, root, receipt, meta) = fixture();
    let (unit, table, manifest) = finish(t, root, receipt, meta);
    let errors = check(&unit, &table, &manifest, &declarations);
    assert!(errors.is_empty(), "{errors:#?}");
    for mutation in 0..4 {
        let mut broken = unit.clone();
        match mutation {
            0 => {
                broken.tables.seal_payloads[0].bindings[0].recipe.inputs[0].producer =
                    PayloadProducer::Literal(PayloadLiteral::Integer(8))
            }
            1 => broken.tables.seal_payloads.clear(),
            2 => {
                if let Inst::PayloadReceipt { value, .. } = &mut broken.instructions[2] {
                    *value = receipt;
                }
            }
            3 => {
                broken.values[receipt.index()].holding = Holding::Plain;
            }
            _ => unreachable!(),
        }
        assert!(
            !check(&broken, &table, &manifest, &declarations).is_empty(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn metadata_leaks_double_release_and_forget_are_rejected() {
    let (t, declarations, root, receipt, meta) = fixture();
    let (unit, table, manifest) = finish(t, root, receipt, meta);
    assert!(check(&unit, &table, &manifest, &declarations).is_empty());
    for mutation in 0..3 {
        let mut broken = unit.clone();
        match mutation {
            0 => broken.terminators[ENTRY.index()] = Terminator::Return(Some(receipt)),
            1 => {
                broken
                    .instructions
                    .insert(3, Inst::Forget { value: receipt });
                for (index, block) in broken.blocks.iter_mut().enumerate() {
                    if index == ENTRY.index() {
                        block.instructions.len += 1;
                    } else {
                        block.instructions.start += 1;
                    }
                }
            }
            2 => {
                if let Inst::Release { value, layout, .. } = &mut broken.instructions[4] {
                    *value = receipt;
                    *layout = meta;
                }
            }
            _ => unreachable!(),
        }
        let errors = check(&broken, &table, &manifest, &declarations);
        assert!(!errors.is_empty(), "mutation {mutation}");
        if mutation == 1 {
            assert!(errors.iter().any(|v| matches!(&v.kind, K::Sealed(text) if text.contains("outside metadata custody"))), "{errors:#?}");
        }
    }
}

#[test]
fn a_whole_local_slot_moves_metadata_without_minting_a_second_holder() {
    let (mut t, declarations, root, receipt, meta) = fixture();
    let ty = t.builder.unit.values[receipt.index()].ty;
    let slot = SlotId(t.builder.unit.slots.len() as u32);
    t.builder.unit.slots.push(SlotInfo {
        ty,
        kind: SlotKind::Local,
        scope: ROOT,
    });
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let address = AddressId(t.builder.unit.addresses.len() as u32);
    t.builder.unit.addresses.push(AddressInfo {
        source_place: None,
        unchecked: vec![],
        base: AddressBase::Slot(slot),
        projection: PoolRange::EMPTY,
        ty,
    });
    t.prepare(ENTRY, address, &[root, receipt]);
    let version = t.store(ENTRY, address, receipt, slot);
    let moved = t.value_as(ValueRepr::PayloadReceipt, meta, Holding::Owned);
    t.builder.push(
        ENTRY,
        Inst::Read {
            result: moved,
            address,
            reaching: version,
            mode: ReadMode::Take,
        },
    );
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    let (unit, table, manifest) = finish(t, root, moved, meta);
    let errors = check(&unit, &table, &manifest, &declarations);
    assert!(errors.is_empty(), "{errors:#?}");
    let mut broken = unit.clone();
    let read = broken
        .instructions
        .iter_mut()
        .find(|i| matches!(i, Inst::Read { result, .. } if *result == moved))
        .unwrap();
    if let Inst::Read { reaching, .. } = read {
        *reaching = receipt;
    }
    assert!(!check(&broken, &table, &manifest, &declarations).is_empty());
}

#[test]
fn block_arguments_transfer_one_metadata_credit_on_every_edge() {
    let (mut t, declarations, root, receipt, meta) = fixture();
    let joined = t.value_as(ValueRepr::PayloadReceipt, meta, Holding::Owned);
    let join = t.builder.block(BlockKind::Body, &[joined]);
    t.builder.jump(ENTRY, join, &[receipt]);
    release_at(&mut t, join, joined, meta, &[root]);
    let payload = t.payload(root);
    release_at(&mut t, join, root, payload, &[]);
    t.builder.ret(join, None);
    let (unit, table, manifest) = t.finish();
    let errors = check(&unit, &table, &manifest, &declarations);
    assert!(errors.is_empty(), "{errors:#?}");
    let mut broken = unit.clone();
    let release = broken
        .instructions
        .iter_mut()
        .find(|inst| matches!(inst, Inst::Release { value, .. } if *value == joined))
        .unwrap();
    if let Inst::Release { value, .. } = release {
        *value = receipt;
    }
    assert!(!check(&broken, &table, &manifest, &declarations).is_empty());
}

#[test]
fn metadata_layout_in_an_unused_table_does_not_refuse_an_ordinary_unit() {
    let (t, _, _, _, _) = fixture();
    let mut ordinary = support::Lir::<Rc>::new(&[], None);
    ordinary.builder.ret(ENTRY, None);
    let (unit, _, manifest) = ordinary.finish();
    let declarations = Declarations {
        registry: t.operations.clone(),
        payloads: vec![],
    };
    let errors = check(&unit, &t.table, &manifest, &declarations);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn an_agreeing_source_recipe_cannot_invent_a_different_heap_origin() {
    let (t, mut declarations, root, receipt, meta) = fixture();
    let (mut unit, table, manifest) = finish(t, root, receipt, meta);
    assert!(check(&unit, &table, &manifest, &declarations).is_empty());
    let operation = PayloadOperation::Allocate {
        destination: PayloadDestination::Static,
    };
    declarations.payloads[0].recipe.operation = operation.clone();
    unit.tables.seal_payloads[0].bindings[0].recipe.operation = operation;
    let errors = check(&unit, &table, &manifest, &declarations);
    assert!(errors.iter().any(|v| matches!(&v.kind, K::Sealed(text) if text.contains("ordered ordinary heap construction"))), "{errors:#?}");
}
