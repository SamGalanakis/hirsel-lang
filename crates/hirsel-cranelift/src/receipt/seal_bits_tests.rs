use crate::function::{FnEmitter, Place};
use crate::module::Emitter;
use cranelift_codegen::{
    Context,
    ir::{self, InstBuilder, types},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use hirsel_core_contract::Manifest;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::stage::{InputIdentity, Ready};
use hirsel_lir::{abi_types::*, ir::*, layout_types::*};
use std::collections::BTreeSet;

use hirsel_lir::logical_storage::{LogicalBytes, LogicalStorageGraph, StorageClass, StorageFact};
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_lir::sealed::{SealDescriptor, StorageAccess};
use hirsel_lir::structural::{Shape as LogicalShape, ShapeGraph};

pub(crate) fn fixture(
    target: &TargetSpec,
    kind: u8,
    malformed: u8,
) -> (LayoutTable, LirUnit<Ready>, FnAbi) {
    let (mut table, mut unit, mut abi) = super::sealed_tests::fixture(target, kind.min(2), false);
    let (native, shape) = match kind {
        0 => (NativeType::Tuple(vec![]), LogicalShape::Fields(vec![])),
        1 => (NativeType::Primitive(Primitive::U64), LogicalShape::Leaf),
        2 => (NativeType::Primitive(Primitive::Bool), LogicalShape::Leaf),
        _ => {
            table.layouts[1].kind = LayoutKind::Scalar(ScalarKind::U128);
            table.layouts[1].size = SizeSpec::Fixed(16);
            table.layouts[1].align = 16;
            unit.types[1].repr = ValueRepr::Scalar(ScalarKind::U128);
            (NativeType::Primitive(Primitive::U128), LogicalShape::Leaf)
        }
    };
    let graph = ShapeGraph {
        root: 0,
        nodes: vec![shape],
    };
    let logical = LogicalStorageGraph {
        shape: graph.clone(),
        facts: vec![StorageFact {
            bytes: LogicalBytes::Bytes(0),
            class: StorageClass::Inline,
        }],
    };
    let projection = hirsel_lir::structural_access::AccessPlan::derive(
        native.clone(),
        graph,
        &table,
        LayoutId(1),
    )
    .unwrap();
    unit.tables.seal_descriptors.push(SealDescriptor {
        root_type: native,
        root_layout: LayoutId(1),
        logical,
        projection,
        access: vec![StorageAccess {
            layout: LayoutId(1),
            scan: vec![],
        }],
    });
    let mut arena = table.layouts[0].clone();
    arena.kind = LayoutKind::Arena;
    table.layouts.push(arena);
    unit.types.push(LirType {
        repr: ValueRepr::Arena,
        layout: LayoutId(2),
    });
    unit.values.push(ValueInfo {
        ty: LirTypeId(2),
        holding: Holding::Plain,
    });
    let mut output = abi.parameters[0].clone();
    output.source = 2;
    output.passing = ParameterPassing::ExclusiveAddress;
    abi.parameters.push(output);
    unit.header.origin = UnitOrigin::Library;
    if kind == 4 {
        use hirsel_lir::operation::{BuiltinType, OperationArgument};
        use hirsel_lir::sealed_payload_source::*;
        let mut scalar = table.layouts[1].clone();
        scalar.kind = LayoutKind::Scalar(ScalarKind::U64);
        scalar.size = SizeSpec::Fixed(8);
        scalar.align = 8;
        scalar.abi.scalars = vec![(0, ScalarKind::U64)];
        table.layouts.push(scalar.clone()); // 3, scalar
        let mut sequence = scalar;
        sequence.kind = LayoutKind::Sequence {
            element: LayoutId(3),
        };
        sequence.size = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(16),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Constant(8),
            SizeTerm::Multiply,
            SizeTerm::Add,
        ]);
        sequence.fields = vec![
            FieldLayout {
                ordinal: 0,
                offset: 0,
                layout: LayoutId(3),
                unaligned: false,
            },
            FieldLayout {
                ordinal: 1,
                offset: 8,
                layout: LayoutId(3),
                unaligned: false,
            },
        ];
        sequence.copy = CopyRecipe::NotCopyable;
        sequence.abi.scalars.clear();
        table.layouts.push(sequence); // 4, payload
        let row = &mut table.layouts[1];
        row.kind = LayoutKind::Block {
            payload: LayoutId(4),
        };
        row.size = SizeSpec::Fixed(8);
        row.align = 8;
        row.copy = CopyRecipe::Dup;
        row.scan = vec![ScanStep::Counted { offset: 0 }];
        row.drop = vec![DropStep::Release { offset: 0 }];
        row.abi.scalars = vec![(0, ScalarKind::Pointer)];
        unit.types[1].repr = ValueRepr::Block;
        let native = NativeType::Builtin(
            BuiltinType::List,
            vec![OperationArgument::Type(NativeType::Primitive(
                Primitive::U64,
            ))],
        );
        let graph = ShapeGraph {
            root: 0,
            nodes: vec![LogicalShape::Sequence(1), LogicalShape::Leaf],
        };
        let projection = hirsel_lir::structural_access::AccessPlan::derive(
            native.clone(),
            graph.clone(),
            &table,
            LayoutId(1),
        )
        .unwrap();
        unit.tables.seal_descriptors[0] = SealDescriptor {
            root_type: native.clone(),
            root_layout: LayoutId(1),
            projection,
            logical: LogicalStorageGraph {
                shape: graph,
                facts: vec![
                    StorageFact {
                        bytes: LogicalBytes::Bytes(8),
                        class: StorageClass::Block,
                    },
                    StorageFact {
                        bytes: LogicalBytes::Bytes(8),
                        class: StorageClass::Inline,
                    },
                ],
            },
            access: [1, 3, 4]
                .into_iter()
                .map(|id| StorageAccess {
                    layout: LayoutId(id),
                    scan: table.layouts[id as usize].scan.clone(),
                })
                .collect(),
        };
        let mut metadata = table.layouts[3].clone();
        metadata.kind = LayoutKind::PayloadReceipt { root: native };
        metadata.drop = vec![DropStep::ReleaseReceipt { offset: 0 }];
        metadata.copy = CopyRecipe::Dup;
        metadata.abi.scalars = vec![(0, ScalarKind::Pointer)];
        table.layouts.push(metadata); // 5
        unit.types.push(LirType {
            repr: ValueRepr::PayloadReceipt,
            layout: LayoutId(5),
        });
        unit.values.push(ValueInfo {
            ty: LirTypeId(3),
            holding: Holding::Plain,
        });
        unit.parameters = vec![ValueId(2), ValueId(1), ValueId(0)];
        unit.slots.push(SlotInfo {
            ty: LirTypeId(3),
            kind: SlotKind::ReceiptParameter(1),
            scope: ScopeId(0),
        });
        let contract = ReceiptContract::new(
            vec![ReceiptParameter {
                source: 1,
                mode: ReceiptMode::Take,
                ty: ReceiptType {
                    source_type: unit.header.instance,
                    profile: unit.header.instance,
                },
            }],
            None,
            vec![],
            vec![],
        )
        .unwrap();
        let content = contract.content().unwrap();
        unit.tables.receipt_entry = Some(ReceiptEntry {
            contract: content,
            parameters: vec![ReceiptEntrySlot {
                source: 1,
                slot: SlotId(0),
                version: ValueId(3),
            }],
            captures: vec![],
            result: None,
            environment: None,
        });
        abi.parameters[1].convention = hirsel_core_contract::Passing::Take;
        abi.receipts = Some(contract);
        abi.key.receipt_contract = Some(content);
    }
    match malformed {
        1 => table.layouts[1].scan.push(ScanStep::Counted { offset: 0 }),
        2 => table.layouts[1]
            .drop
            .push(DropStep::ReleaseSealed { offset: 0 }),
        3 => table.layouts[1].kind = LayoutKind::Scalar(ScalarKind::Pointer),
        4 => unit.tables.seal_descriptors[0].root_type = NativeType::Primitive(Primitive::Bytes),
        5 => unit.types[2].repr = ValueRepr::Handle,
        6 => table.layouts[0].kind = LayoutKind::Handle,
        7 => unit.tables.seal_descriptors.clear(),
        8 => unit.tables.seal_descriptors[0].access.clear(),
        9 => unit.tables.receipt_entry = None,
        10 => unit.tables.receipt_entry.as_mut().unwrap().parameters[0].source = 0,
        _ => (),
    }
    (table, unit, abi)
}

const HARNESS: &str = r#"
#include <stdint.h>
#include <assert.h>
#include <string.h>
const uint32_t descriptor_index __asm__("INDEX_SYMBOL") = 7;
static uint64_t expected[2] = {17,29};
static unsigned released, prepared, snapped;
static uint8_t failure;
void *hirsel_rt_v2_receipt_bits(void *ctx, uint32_t index, const void *image) {
    assert(ctx == (void*)41 && index == 7);
    assert(((uintptr_t)image % ALIGN) == 0);
    assert(memcmp(image, expected, EXTENT) == 0);
    ++snapped; return (void*)45;
}
uint8_t hirsel_rt_v2_seal_prepare_commit(void *ctx, void *grant, const void *image,
    uint32_t index, const void *receipt, uint64_t site, void **output) {
    assert(ctx == (void*)41 && grant == (void*)43 && receipt == (void*)45);
    assert(index == 7 && site == 23 && snapped == prepared+1);
    assert(memcmp(image, expected, EXTENT) == 0);
    *output = failure ? 0 : (void*)47;
    ++prepared; return failure ? 2 : 0;
}
void hirsel_rt_v2_receipt_release(void *receipt) {
    assert(receipt == (void*)45 && prepared == released+1); ++released;
}
extern uint8_t probe(void *, uint64_t, uint64_t, void *, const void *, void **);
int main(void) {
    _Alignas(16) uint64_t input[2] = {17,29};
    BOOL
    void *owner = (void*)99;
    assert(probe((void*)41, 19, 23, (void*)43, input, &owner) == 0);
    assert(owner == (void*)47 && released == 1 && input[1] == 29);
    failure = 1; owner = (void*)99;
    assert(probe((void*)41, 19, 23, (void*)43, input, &owner) == 2);
    assert(owner == 0 && released == 2 && input[1] == 29);
}
"#;

// Private physical probes: public C admission and demanded-guard production
// remain required. The four-helper runtime boundary is independently tested.
#[test]
fn seal_bits_native_emission_preserves_typed_images_and_refusal_custody_on_both_targets() {
    for target in TargetSpec::executed() {
        for (kind, extent, align) in [(0, 0, 1), (1, 8, 8), (2, 1, 1), (3, 16, 16)] {
            let (table, _, _) = fixture(&target, kind, 0);
            let keys = hirsel_lir::LayoutKeys::new(&table);
            let symbol = hirsel_lir::glue::descriptor_index_symbol(&keys.key(LayoutId(1)));
            let source = HARNESS
                .replace("INDEX_SYMBOL", &symbol)
                .replace("EXTENT", &extent.to_string())
                .replace("ALIGN", &align.to_string())
                .replace(
                    "BOOL",
                    if kind == 2 {
                        "input[0]=expected[0]=1;"
                    } else {
                        ""
                    },
                );
            super::tests::execute(
                &target,
                probe(&target, kind, 0),
                "cranelift-seal-bits",
                &source,
            );
        }
    }
}
#[test]
fn seal_bits_native_emission_refuses_unproved_payloads_before_runtime_imports() {
    for mutation in 1..=8 {
        probe(&TargetSpec::x86_64_unknown_linux_gnu(), 1, mutation);
    }
}
fn probe(target: &TargetSpec, kind: u8, malformed: u8) -> Vec<u8> {
    let (table, unit, abi) = fixture(target, kind, malformed);
    let keys = hirsel_lir::LayoutKeys::new(&table);
    let manifest = Manifest::skeleton([]);
    let input = InputIdentity {
        interface_version: hirsel_lir::INTERFACE_VERSION,
        digest: unit.header.instance,
    };
    let empty = BTreeSet::new();
    let inputs = EmitInputs {
        ahead: None,
        continuation: None,
        alternate: false,
        index: None,
        verify_backend: true,
        input: &input,
        target,
        layouts: &table,
        keys: &keys,
        manifest: &manifest,
        statics: &[],
        inline_instances: &empty,
        available_externally: &empty,
        callbacks: false,
        foreign: &hirsel_lir::emission::NO_FOREIGN,
        tracking: false,
        opt: OptLevel::None,
        measurement: false,
        fuel: false,
    };
    let mut em = Emitter::new(crate::module::Inputs::units(&inputs), false).unwrap();
    let sig = em.runtime_signature(&abi, &unit.header.definition).unwrap();
    let id = em
        .declare_function("probe", Linkage::Export, &sig.ir)
        .unwrap();
    let mut ctx = Context::new();
    ctx.func = ir::Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
    let mut bc = FunctionBuilderContext::new();
    let b = FunctionBuilder::new(&mut ctx.func, &mut bc);
    let mut f = FnEmitter::new(&mut em, &unit, b, sig);
    if kind == 4 && malformed < 9 {
        f.bind_receipt_entry().unwrap();
    }

    f.define1(ValueId(2), f.entry_params[3]).unwrap();
    let input = Place {
        columns: None,
        ptr: f.entry_params[4],
        layout: LayoutId(1),
        align: table.layouts[1].align,
        block: None,
    };
    let value = f.load(f.layout_shape(LayoutId(1)).unwrap(), input);
    f.define(ValueId(1), value).unwrap();
    let read = f.prepare_seal(
        ValueId(0),
        ValueId(2),
        ValueId(1),
        SealDescriptorId(0),
        SourceAnchorId(0),
    );
    if malformed != 0 {
        assert!(read.is_err());
        assert!(em.manifested.imports.is_empty());
        return vec![];
    }
    let (status, output) = read.unwrap();
    let owner = f.load_ptr(output);
    f.store_at(owner, f.entry_params[5], 0);
    f.b.ins().return_(&[status]);
    f.b.seal_all_blocks();
    f.b.finalize(f.em.module.target_config());
    em.define("probe", id, &mut ctx).unwrap();
    em.module.finish().emit().unwrap()
}

const HEAP_HARNESS: &str = r#"
#include <stdint.h>
#include <assert.h>
const uint32_t descriptor_index __asm__("INDEX_SYMBOL") = 7;
static unsigned prepared;
static uint8_t failure;
uint8_t hirsel_rt_v2_seal_prepare_commit(void *ctx, void *grant, const void *image,
    uint32_t index, const void *receipt, uint64_t site, void **output) {
    assert(ctx == (void*)41 && grant == (void*)43 && receipt == (void*)45);
    assert(index == 7 && site == 23);
    assert(*(const void *const*)image == (void*)101);
    *output = failure ? 0 : (void*)47;
    ++prepared; return failure ? 2 : 0;
}
extern uint8_t probe(void *, uint64_t, uint64_t, void *, const void *, void **, void *);
int main(void) {
    void *input = (void*)101, *owner = (void*)99;
    assert(probe((void*)41, 19, 23, (void*)43, &input, &owner, (void*)45) == 0);
    assert(owner == (void*)47 && input == (void*)101 && prepared == 1);
    failure = 1; owner = (void*)99;
    assert(probe((void*)41, 19, 23, (void*)43, &input, &owner, (void*)45) == 2);
    assert(owner == 0 && input == (void*)101 && prepared == 2);
}
"#;
#[test]
fn seal_heap_native_emission_passes_original_entry_receipt_without_releasing_it() {
    for target in TargetSpec::executed() {
        let (table, _, _) = fixture(&target, 4, 0);
        let keys = hirsel_lir::LayoutKeys::new(&table);
        let symbol = hirsel_lir::glue::descriptor_index_symbol(&keys.key(LayoutId(1)));
        let source = HEAP_HARNESS.replace("INDEX_SYMBOL", &symbol);
        super::tests::execute(
            &target,
            probe(&target, 4, 0),
            "cranelift-seal-heap",
            &source,
        );
    }
}
#[test]
fn seal_heap_native_emission_refuses_missing_or_unpaired_receipt_before_imports() {
    for mutation in [9, 10] {
        probe(&TargetSpec::x86_64_unknown_linux_gnu(), 4, mutation);
    }
}
