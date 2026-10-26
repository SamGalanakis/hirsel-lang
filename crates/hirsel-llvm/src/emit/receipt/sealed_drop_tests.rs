use super::super::*;
use hirsel_lir::{abi_types::*, layout_types::*};
// Private backend probes: C sentinels check the physical release boundary.
// Actual owner and Shared custody is tested by the independent runtime walker.
fn fixture(target: &TargetSpec) -> (LayoutTable, LirUnit<Ready>, FnAbi) {
    let (mut table, mut unit, mut abi) = super::tests::fixture(target);
    table.layouts.truncate(1);
    let mut root = table.layouts[0].clone();
    root.kind = LayoutKind::Scalar(ScalarKind::U64);
    root.scan.clear();
    root.drop.clear();
    root.copy = CopyRecipe::Bits;
    table.layouts.push(root);
    table.layouts[0].kind = LayoutKind::SealedRegion { root: LayoutId(1) };
    table.layouts[0].copy = CopyRecipe::NotCopyable;
    table.layouts[0].drop = vec![DropStep::ReleaseSealed { offset: 0 }];
    let mut parent = table.layouts[0].clone();
    parent.kind = LayoutKind::Aggregate;
    parent.size = SizeSpec::Fixed(16);
    parent.fields = vec![FieldLayout {
        ordinal: 0,
        offset: 8,
        layout: LayoutId(0),
        unaligned: false,
    }];
    parent.drop = vec![DropStep::Child {
        offset: 8,
        layout: LayoutId(0),
    }];
    table.layouts.push(parent);
    unit.tables.receipt_entry = None;
    unit.slots.clear();
    unit.parameters.clear();
    abi.receipts = None;
    abi.key.receipt_contract = None;
    abi.caller_site = false;
    abi.parameters.truncate(1);
    abi.parameters[0].layout = LayoutId(0);
    abi.parameters[0].passing = ParameterPassing::ByValue;
    abi.parameters[0].slots[0].ty = PhysicalType::Pointer;
    unit.types = vec![LirType {
        repr: ValueRepr::Handle,
        layout: LayoutId(0),
    }];
    unit.values = vec![ValueInfo {
        ty: LirTypeId(0),
        holding: Holding::Owned,
    }];
    unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 0,
        site: SiteRange {
            source: 0,
            start: 0,
            end: 1,
        },
    });
    (table, unit, abi)
}
const HARNESS: &str = r#"
#include <stdint.h>
#include <assert.h>
static unsigned released;
void hirsel_rt_v2_sealed_release(void *ctx, void *owner) {
    assert(ctx == (void *)41 && owner == (void *)43); ++released;
}
EXTRA
extern uint8_t direct(void *, void *);
extern uint8_t nested(void *, void *);
int main(void) {
    assert(direct((void *)41, (void *)43) == 0); assert(released == 1);
    assert(nested((void *)41, (void *)43) == 0); assert(released == 2);
}
"#;

fn probe(target: &TargetSpec) -> Vec<u8> {
    let context = Context::new();
    let module = Module::new(&context, "sealed-drop-glue-probe");
    let machine = machine(target, false).unwrap();
    module.set_target(&machine);
    let (table, unit, abi) = fixture(target);
    let keys = hirsel_lir::LayoutKeys::new(&table);
    let manifest = hirsel_core_contract::Manifest::skeleton([]);
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
    for nested in [false, true] {
        let signature = Signature::runtime(&module, &abi, &unit.header.definition).unwrap();
        let function = module
            .function(if nested { "nested" } else { "direct" }, signature.ty)
            .unwrap();
        let mut f = FnEmitter::new(
            &module,
            &inputs,
            &unit,
            function,
            signature,
            &mut manifested,
        );
        f.builder.position(f.prologue_block);
        let id = LayoutId(if nested { 2 } else { 0 });
        let place = f.stack(id).unwrap();
        let field = if nested {
            f.field_place(place, 8, LayoutId(0), false).unwrap()
        } else {
            place
        };
        f.store(
            Shape::Scalar(ScalarKind::Pointer),
            module.parameter(function, 1),
            field,
        );
        let status = f
            .drop_glue(f.const_u64(0), |walk| {
                walk.steps(
                    &glue::Scope::of(id),
                    place.ptr,
                    8,
                    &table.layouts[id.0 as usize].drop,
                )
            })
            .unwrap();
        f.builder.ret(status);
    }
    assert!(
        manifested
            .imports
            .contains_key("hirsel_rt_v2_sealed_release")
    );
    assert!(
        !manifested
            .imports
            .contains_key("hirsel_rt_v2_receipt_release")
    );
    module.verify().unwrap();
    optimize_and_emit(&module, &machine, OptLevel::None, true, false).unwrap()
}
#[test]
fn sealed_runtime_drop_33_compiled_glue_releases_direct_and_nested_owner_on_both_targets() {
    for target in TargetSpec::executed() {
        super::tests::execute(
            &target,
            probe(&target),
            "llvm-sealed-drop",
            &HARNESS.replace("EXTRA", ""),
        );
    }
}
