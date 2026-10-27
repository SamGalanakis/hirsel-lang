//! Original construction controls use independent source expectations. Ready
//! remains refused until the custody lane admits the native receipt vocabulary.
use crate::support::*;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::sealed_payload::*;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::Primitive;

struct Source<'a> {
    registry: &'a hirsel_layout::operation::OperationRegistry,
    payloads: Vec<SourcePayload>,
}
impl OperationDeclarations for Source<'_> {
    fn derive(
        &self,
        op: &hirsel_core_contract::OperationId,
        args: &[OperationArgument],
        target: &TargetSpec,
    ) -> Result<(hirsel_lir::FnAbi, LayoutTable), hirsel_lir::layout::LayoutError> {
        self.registry.derive(op, args, target)
    }
    fn seal_descriptor_for_source(
        &self,
        _: &DefPath,
        _: ContentHash,
        root: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(root, table, target)
    }
    fn seal_events(&self, _: &DefPath, _: ContentHash) -> Option<Vec<SourceEvent>> {
        Some(vec![])
    }
    fn seal_payloads(&self, _: &DefPath, _: ContentHash) -> Option<Vec<SourcePayload>> {
        Some(self.payloads.clone())
    }
    fn seal_source_witness(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<SealSourceWitness> {
        Some(SealSourceWitness::from_expectations(
            hash("independent constructor source"),
            definition.clone(),
            revision("receipt_construct_96"),
            instance,
            &[],
            &self.payloads,
        ))
    }
}

fn fixture(target: &TargetSpec) -> (Table, LirUnit<hirsel_lir::stage::Ready>, SourcePayload) {
    let mut schema = Schema::new();
    let number = schema.types.p(Primitive::U64);
    let list = schema.types.list(number);
    schema.name("list", list);
    schema.signature("receipt_construct_96", &[], number);
    let mut table = schema.build(target);
    let args = hirsel_layout::operation::collect_arguments(
        &schema.types.store,
        &[hirsel_types::GenericArgument::Type(list)],
    )
    .unwrap();
    let OperationArgument::Type(root_type) = &args[0] else {
        panic!("list")
    };
    let root_type = root_type.clone();
    let descriptor = table
        .declarations
        .seal_descriptor(&root_type, &table.table, target)
        .unwrap();
    let mut metadata = table.table.layouts[table.header.0 as usize].clone();
    metadata.kind = LayoutKind::PayloadReceipt {
        root: root_type.clone(),
    };
    metadata.size = SizeSpec::Fixed(8);
    metadata.align = 8;
    metadata.fields.clear();
    metadata.tag = None;
    metadata.scan.clear();
    metadata.drop = vec![DropStep::ReleaseReceipt { offset: 0 }];
    metadata.copy = CopyRecipe::NotCopyable;
    metadata.finalizer = None;
    metadata.abi = AbiInputs {
        scalars: vec![(0, ScalarKind::Pointer)],
        unaligned: false,
    };
    let meta_id = LayoutId(table.table.layouts.len() as u32);
    table.table.layouts.push(metadata);
    table.keys = hirsel_lir::LayoutKeys::new(&table.table);
    let payload = SourcePayload {
        root_type,
        recipe: PayloadRecipe {
            source: SourceAnchor {
                body: 0,
                block: 0,
                statement: 0,
                site: SiteRange {
                    source: 0,
                    start: 10,
                    end: 20,
                },
            },
            operation: PayloadOperation::Allocate {
                destination: PayloadDestination::Heap,
            },
            inputs: [11u128, 12]
                .into_iter()
                .enumerate()
                .map(|(i, value)| PayloadOperand {
                    place: i as u32,
                    ty: NativeType::Primitive(hirsel_lir::operation::Primitive::U64),
                    producer: PayloadProducer::Literal(PayloadLiteral::Integer(value)),
                    capture_slot: None,
                })
                .collect(),
        },
    };
    let mut unit = Unit::new(&table, "receipt_construct_96", "receipt_construct_96");
    let a = unit.int(ENTRY, ScalarKind::U64, 11);
    let b = unit.int(ENTRY, ScalarKind::U64, 12);
    let LayoutKind::Block { payload: sequence } =
        table.table.layouts[table.layout("list").0 as usize].kind
    else {
        panic!("list")
    };
    let list = unit.construct_block(
        ENTRY,
        ConstructorKind::Sequence,
        sequence,
        table.layout("list"),
        &[(0, a), (1, b)],
    );
    unit.builder.unit.tables.seal_descriptors.push(descriptor);
    unit.builder.unit.tables.seal_payloads.push(PayloadTable {
        descriptor: SealDescriptorId(0),
        bindings: vec![PayloadBinding {
            recipe: payload.recipe.clone(),
            values: vec![a, b],
        }],
    });
    let ty = unit.ty(ValueRepr::PayloadReceipt, meta_id);
    let receipt = ValueId(unit.builder.unit.values.len() as u32);
    unit.builder.unit.values.push(ValueInfo {
        ty,
        holding: Holding::Owned,
    });
    let inputs = PoolRange::push(
        &mut unit.builder.unit.pools.receipt_inputs,
        [
            ReceiptInput {
                original: a,
                receipt: None,
            },
            ReceiptInput {
                original: b,
                receipt: None,
            },
        ],
    );
    unit.builder.push(
        ENTRY,
        Inst::PayloadReceipt {
            arena: hirsel_lir::ir::OptionalValueId::NONE,
            result: receipt,
            value: list,
            shape: SealDescriptorId(0),
            binding: PayloadBindingId(0),
            inputs,
        },
    );
    unit.release_all(ENTRY, &[(receipt, meta_id), (list, sequence)]);
    let sum = unit.binary(ENTRY, BinaryOp::Add, a, b);
    unit.builder.term(ENTRY, Terminator::Return(Some(sum)));
    let unit = unit.finish();
    (table, unit, payload)
}

#[test]
fn receipt_construct_96_c_admits_original_recipe_and_rejects_swapped_inputs() {
    for target in TargetSpec::executed() {
        let (table, unit, payload) = fixture(&target);
        let source = Source {
            registry: &table.declarations,
            payloads: vec![payload],
        };
        let manifest = manifest();
        let inputs = hirsel_lir::validate::Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &source,
            target: &target,
            layouts: &table.table,
            manifest: &manifest,
        };
        let result = hirsel_lir::validate::validate(
            unit.clone().into_stage::<hirsel_lir::stage::Reuse>(),
            &inputs,
        );
        assert!(
            result.is_ok(),
            "C rejected original construction: {:#?}",
            result.err().map(|e| e.violations)
        );
        let mut wrong = unit.clone().into_stage::<hirsel_lir::stage::Reuse>();
        wrong.pools.receipt_inputs[0].original = ValueId(1);
        assert!(hirsel_lir::validate::validate(wrong, &inputs).is_err());
    }
}
