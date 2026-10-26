use super::super::*;
use hirsel_core_contract::Manifest;
use hirsel_lir::abi_types::*;
use hirsel_lir::layout_types::*;

use hirsel_lir::logical_storage::{LogicalBytes, LogicalStorageGraph, StorageClass, StorageFact};
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_lir::sealed::{SealDescriptor, StorageAccess};
use hirsel_lir::structural::{Shape as LogicalShape, ShapeGraph};

fn fixture(target: &TargetSpec, kind: u8, malformed: u8) -> (LayoutTable, LirUnit<Ready>, FnAbi) {
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
            super::tests::execute(&target, probe(&target, kind, 0), "llvm-seal-bits", &source);
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
    let context = Context::new();
    let module = Module::new(&context, "sealed-root-probe");
    let machine = machine(target, false).unwrap();
    module.set_target(&machine);
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
    let mut manifested = Manifested::default();
    let signature = Signature::runtime(&module, &abi, &unit.header.definition).unwrap();
    let function = module.function("probe", signature.ty).unwrap();
    let mut f = FnEmitter::new(
        &module,
        &inputs,
        &unit,
        function,
        signature,
        &mut manifested,
    );
    f.builder.position(f.prologue_block);
    f.define(ValueId(2), module.parameter(function, 3)).unwrap();
    let input = Place {
        columns: None,
        ptr: module.parameter(function, 4),
        layout: LayoutId(1),
        align: table.layouts[1].align,
        block: None,
    };
    let value = f.load(f.layout_shape(LayoutId(1)).unwrap(), input);
    f.define(ValueId(1), value).unwrap();
    let read = f.prepare_seal_bits(
        ValueId(0),
        ValueId(2),
        ValueId(1),
        SealDescriptorId(0),
        SourceAnchorId(0),
    );
    if malformed != 0 {
        assert!(read.is_err());
        assert!(manifested.imports.is_empty());
        return vec![];
    }
    let (status, output) = read.unwrap();
    let owner = f.builder.load(module.ptr_type(), output, 8);
    f.builder.store(owner, module.parameter(function, 5), 8);
    f.builder.ret(status);
    module.verify().unwrap();
    optimize_and_emit(&module, &machine, OptLevel::None, true, false).unwrap()
}
