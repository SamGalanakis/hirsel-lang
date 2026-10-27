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

fn probe(target: &TargetSpec, malformed: u8) -> Vec<u8> {
    let (table, mut unit, abi) = super::seal_bits_tests::fixture(target, 4, 0);
    use hirsel_lir::sealed_payload::*;
    unit.tables.seal_payloads.push(PayloadTable {
        descriptor: SealDescriptorId(0),
        bindings: vec![PayloadBinding {
            recipe: PayloadRecipe {
                source: SourceAnchor {
                    body: 0,
                    block: 0,
                    statement: 0,
                    site: SiteRange {
                        source: 0,
                        start: 0,
                        end: 1,
                    },
                },
                operation: if malformed == 1 {
                    PayloadOperation::Boundary
                } else {
                    PayloadOperation::Allocate {
                        destination: PayloadDestination::Heap,
                    }
                },
                inputs: vec![],
            },
            values: vec![],
        }],
    });
    unit.values.push(ValueInfo {
        ty: LirTypeId(3),
        holding: Holding::Owned,
    });
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
    let read = f.payload_receipt(
        ValueId(4),
        ValueId(1),
        SealDescriptorId(0),
        PayloadBindingId(0),
        PoolRange::EMPTY,
    );
    if malformed != 0 {
        assert!(read.is_err());
        assert!(em.manifested.imports.is_empty());
        return vec![];
    }
    read.unwrap();
    let receipt = f.get1(ValueId(4)).unwrap();
    f.store_at(receipt, f.entry_params[5], 0);
    let status = f.b.ins().iconst(types::I8, 0);
    f.b.ins().return_(&[status]);
    f.b.seal_all_blocks();
    f.b.finalize(f.em.module.target_config());
    em.define("probe", id, &mut ctx).unwrap();
    em.module.finish().emit().unwrap()
}

const HARNESS: &str = r#"
#include <stdint.h>
#include <assert.h>
const uint32_t descriptor_index __asm__("INDEX_SYMBOL") = 7;
static unsigned snapshots;
void *hirsel_rt_v2_receipt_construct(void *ctx, uint32_t index, const void *value,
    uint8_t kind, void **inputs, uint64_t count) {
    assert(ctx == (void*)41 && index == 7 && kind == 1 && count == 0);
    assert(*(void *const*)value == (void*)101);
    ++snapshots; return (void*)45;
}
extern uint8_t probe(void *, uint64_t, uint64_t, void *, const void *, void **, void *);
int main(void) {
    void *input = (void*)101, *receipt = 0;
    assert(probe((void*)41, 19, 23, (void*)43, &input, &receipt, (void*)0) == 0);
    assert(receipt == (void*)45 && snapshots == 1 && input == (void*)101);
}
"#;

#[test]
fn receipt_construct_96_emits_original_heap_recipe_on_both_targets() {
    for target in TargetSpec::executed() {
        let (table, _, _) = super::seal_bits_tests::fixture(&target, 4, 0);
        let keys = hirsel_lir::LayoutKeys::new(&table);
        let symbol = hirsel_lir::glue::descriptor_index_symbol(&keys.key(LayoutId(1)));
        super::tests::execute(
            &target,
            probe(&target, 0),
            "cranelift-receipt-construct-96",
            &HARNESS.replace("INDEX_SYMBOL", &symbol),
        );
    }
}

#[test]
fn receipt_construct_96_refuses_an_unimplemented_source_recipe_before_imports() {
    probe(&TargetSpec::x86_64_unknown_linux_gnu(), 1);
}
