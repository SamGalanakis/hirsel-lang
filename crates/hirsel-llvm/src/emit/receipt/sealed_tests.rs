use super::super::*;
use hirsel_core_contract::Manifest;
use hirsel_lir::abi_types::*;
use hirsel_lir::layout_types::*;

// Private physical-emission probes bypass C admission. They test byte loads
// and the approved helper boundary; they do not qualify public sealed APIs.
pub(super) fn fixture(
    target: &TargetSpec,
    kind: u8,
    malformed: bool,
) -> (LayoutTable, LirUnit<Ready>, FnAbi) {
    let (mut table, mut unit, mut abi) = super::tests::fixture(target);
    unit.tables.receipt_entry = None;
    unit.slots.clear();
    unit.parameters.clear();
    abi.receipts = None;
    abi.caller_site = true;
    abi.key.receipt_contract = None;
    let mut root = table.layouts[0].clone();
    root.drop.clear();
    root.scan.clear();
    root.copy = CopyRecipe::Bits;
    root.kind = match kind {
        0 => LayoutKind::Unit,
        1 => LayoutKind::Scalar(ScalarKind::U64),
        2 => LayoutKind::Scalar(ScalarKind::Bool),
        _ => LayoutKind::Aggregate,
    };
    root.size = SizeSpec::Fixed(match kind {
        0 => 0,
        2 => 1,
        3 => 16,
        _ => 8,
    });
    root.align = if kind == 0 || kind == 2 { 1 } else { 8 };
    table.layouts.truncate(1);
    table.layouts.push(root);
    table.layouts[0].kind = if malformed {
        LayoutKind::Handle
    } else {
        LayoutKind::SealedRegion { root: LayoutId(1) }
    };
    let mut parameter = abi.parameters[0].clone();
    parameter.source = 0;
    parameter.layout = LayoutId(0);
    parameter.passing = ParameterPassing::ByValue;
    parameter.slots[0].ty = PhysicalType::Pointer;
    parameter.convention = hirsel_core_contract::Passing::Borrow;
    let mut output = parameter.clone();
    output.source = 1;
    output.layout = LayoutId(1);
    output.passing = ParameterPassing::ExclusiveAddress;
    output.convention = hirsel_core_contract::Passing::Mut;
    abi.parameters = vec![parameter, output];
    unit.types = vec![
        LirType {
            repr: ValueRepr::Handle,
            layout: LayoutId(0),
        },
        LirType {
            repr: match kind {
                0 => ValueRepr::Unit,
                1 => ValueRepr::Scalar(ScalarKind::U64),
                2 => ValueRepr::Scalar(ScalarKind::Bool),
                _ => ValueRepr::Aggregate,
            },
            layout: LayoutId(1),
        },
    ];
    unit.values = vec![
        ValueInfo {
            ty: LirTypeId(0),
            holding: Holding::Plain,
        },
        ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Plain,
        },
    ];
    (table, unit, abi)
}

const HARNESS: &str = r#"
#include <stdint.h>
#include <assert.h>
static uint64_t root[2] = {17, 29};
static unsigned seen;
const void *hirsel_rt_v2_sealed_root(void *ctx, const void *owner) {
    assert(ctx == (void *)41 && owner == (void *)43);
    seen++;
    return root;
}
extern uint8_t probe(void *, uint64_t, uint64_t, const void *, void *);
int main(void) {
    uint64_t output[2] = {101, 103};
    assert(probe((void *)41, 0, 0, (void *)43, output) == 0);
    assert(seen == 1);
    EXPECT
}
"#;

#[test]
fn sealed_root_native_loads_unit_scalar_bool_and_aggregate_on_both_targets() {
    for target in TargetSpec::executed() {
        for (kind, expected) in [
            (0, "assert(output[0] == 101 && output[1] == 103);"),
            (1, "assert(output[0] == 17 && output[1] == 103);"),
            (
                2,
                "assert(((uint8_t *)output)[0] == 1 && output[1] == 103);",
            ),
            (3, "assert(output[0] == 17 && output[1] == 29);"),
        ] {
            super::tests::execute(
                &target,
                probe(&target, kind, false),
                "llvm-sealed-root",
                &HARNESS.replace("EXPECT", expected),
            );
        }
    }
}

#[test]
fn sealed_root_native_refuses_foreign_owner_layout_before_runtime_import() {
    probe(&TargetSpec::x86_64_unknown_linux_gnu(), 1, true);
}

fn probe(target: &TargetSpec, kind: u8, malformed: bool) -> Vec<u8> {
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
    f.define(ValueId(0), module.parameter(function, 3)).unwrap();
    let read = f.borrow_sealed_root(ValueId(1), ValueId(0));
    if malformed {
        assert!(matches!(read, Err(CodegenError::Invalid { .. })));
        assert!(manifested.imports.is_empty());
        return vec![];
    }
    read.unwrap();
    let shape = f.layout_shape(LayoutId(1)).unwrap();
    let output = Place {
        columns: None,
        ptr: module.parameter(function, 4),
        layout: LayoutId(1),
        align: table.layouts[1].align,
        block: None,
    };
    f.store(shape, f.get(ValueId(1)).unwrap(), output);
    f.builder.ret(module.const_int(module.int_type(8), 0));
    module.verify().unwrap();
    optimize_and_emit(&module, &machine, OptLevel::None, true, false).unwrap()
}
