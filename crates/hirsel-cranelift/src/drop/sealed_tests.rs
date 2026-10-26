use crate::function::{FnEmitter, Place};
use crate::module::Emitter;
use cranelift_codegen::{
    Context,
    ir::{self, InstBuilder},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::stage::{InputIdentity, Ready};
use hirsel_lir::{abi_types::*, ir::*, layout_types::*};
use std::collections::{BTreeMap, BTreeSet};
// Private backend probes: C sentinels check the physical release boundary.
// Actual owner and Shared custody is tested by the independent runtime walker.
fn fixture(target: &TargetSpec) -> (LayoutTable, LirUnit<Ready>, FnAbi) {
    let (mut table, mut unit, mut abi) = crate::receipt::tests::fixture(target);
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
    parent.drop = vec![DropStep::ReleaseSealed { offset: 8 }];
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

const FINALIZE: &str = r#"
typedef struct { uint32_t kind, field, variant, span; uint64_t offset, stride, count, count_offset; void *layout; } Step;
typedef struct { uint32_t magic, version; uint64_t size, align; void *finalizer; const Step *steps; uint64_t step_count; uint32_t field_count, flags; uint64_t logical; void *logical_count, *scan; uint64_t scan_words; } Layout;
extern const Layout *registry[] __asm__("REGISTRY");
uint8_t hirsel_rt_v2_finalize(void *ctx, uint32_t descriptor, void *value, const uint64_t *mask, void *continuation, uint64_t anchor, uint64_t site) {
    (void)continuation; (void)anchor; (void)site;
    const Layout *layout = registry[descriptor - 2];
    assert(layout->version == 18 && layout->step_count == 1 && mask == 0);
    const Step *step = &layout->steps[0];
    assert(step->kind == 14 && step->span == 0 && step->stride == 0);
    hirsel_rt_v2_sealed_release(ctx, *(void **)((char *)value + step->offset));
    return 0;
}
"#;
fn probe(target: &TargetSpec) -> (Vec<u8>, String) {
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

    let finalizers = BTreeMap::new();
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(&table, &finalizers);
    let failures = hirsel_lir::emission::FailureCodes::default();
    let glue = hirsel_lir::glue::UnitGlue::default();
    let glue_inputs = hirsel_lir::emission::GlueInputs {
        target,
        layouts: &table,
        keys: &keys,
        registry: registry.clone(),
        finalizers: &finalizers,
        glue: &glue,
        failures: &failures,
        statics: &[],
        callbacks: &[],
        foreign: &hirsel_lir::emission::NO_FOREIGN,
        aliasing_unstated: false,
    };
    let mut em = Emitter::new(
        crate::module::Inputs {
            glue: Some(&glue_inputs),
            ..crate::module::Inputs::units(&inputs)
        },
        false,
    )
    .unwrap();
    for nested in [false, true] {
        let name = if nested { "nested" } else { "direct" };
        let sig = em.runtime_signature(&abi, &unit.header.definition).unwrap();
        let id = em.declare_function(name, Linkage::Export, &sig.ir).unwrap();
        let mut ctx = Context::new();
        ctx.func = ir::Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
        let mut bc = FunctionBuilderContext::new();
        let b = FunctionBuilder::new(&mut ctx.func, &mut bc);
        let mut f = FnEmitter::new(&mut em, &unit, b, sig);
        let layout = LayoutId(if nested { 2 } else { 0 });
        let place = f.stack(layout).unwrap();
        let field = Place {
            ptr: if nested {
                f.offset(place.ptr, 8)
            } else {
                place.ptr
            },
            layout: LayoutId(0),
            ..place
        };
        let owner = f.entry_params[1];
        f.store(
            crate::function::Shape::Scalar(ScalarKind::Pointer),
            crate::function::V::One(owner),
            field,
        )
        .unwrap();
        let status = f
            .finalize_inline(place, MaskRef::All, CleanupAnchorId(0))
            .unwrap();
        f.b.ins().return_(&[status]);
        f.b.seal_all_blocks();
        f.b.finalize(f.em.module.target_config());
        em.define(name, id, &mut ctx).unwrap();
    }
    em.registry_table(&unit.header.definition).unwrap();
    em.cells().unwrap();
    em.drain_helpers(&unit.header.definition).unwrap();
    (
        em.module.finish().emit().unwrap(),
        HARNESS.replace("EXTRA", &FINALIZE.replace("REGISTRY", &registry.symbol())),
    )
}
#[test]
fn sealed_runtime_drop_33_emitted_descriptors_release_direct_and_nested_owner_on_both_targets() {
    for target in TargetSpec::executed() {
        let (object, harness) = probe(&target);
        crate::receipt::tests::execute(&target, object, "cranelift-sealed-drop", &harness);
    }
}
