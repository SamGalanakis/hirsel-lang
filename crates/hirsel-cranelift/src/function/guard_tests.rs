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

const HARNESS: &str = r#"
#include <stdint.h>
#include <assert.h>
#include <string.h>
static unsigned entered, exited;
static uint8_t failure;
void ENTER(void *ctx, void *guard) {
    assert(ctx==(void*)41); memset(guard,0,SIZE); ((uint64_t*)guard)[LAST]=73; ++entered;
}
uint8_t EXIT(void *ctx, void *guard, uint64_t anchor, uint64_t site) {
    assert(ctx==(void*)41 && anchor==19 && site==23);
    assert(((uint64_t*)guard)[LAST]==73); ++exited; return failure ? 2 : 0;
}
extern uint8_t probe(void *, uint64_t, uint64_t, void *, const void *, void **);
int main(void) {
    assert(probe((void*)41,19,23,0,0,0)==0); assert(entered==1 && exited==1);
    failure=1;
    assert(probe((void*)41,19,23,0,0,0)==2); assert(entered==2 && exited==2);
}
"#;
#[test]
fn demanded_guard_native_entry_exit_selects_exact_record_and_cleanup_on_both_targets() {
    for target in TargetSpec::executed() {
        for demanded in [false, true] {
            let source = HARNESS
                .replace(
                    "ENTER",
                    if demanded {
                        "hirsel_rt_v2_receipt_region_enter"
                    } else {
                        "hirsel_rt_v2_arena_enter"
                    },
                )
                .replace(
                    "EXIT",
                    if demanded {
                        "hirsel_rt_v2_receipt_region_exit"
                    } else {
                        "hirsel_rt_v2_region_exit"
                    },
                )
                .replace("SIZE", if demanded { "80" } else { "64" })
                .replace("LAST", if demanded { "9" } else { "7" });
            crate::receipt::tests::execute(
                &target,
                probe(&target, demanded),
                "cranelift-demanded-guard",
                &source,
            );
        }
    }
}
fn probe(target: &TargetSpec, demanded: bool) -> Vec<u8> {
    let (table, mut unit, abi) = crate::receipt::seal_bits_tests::fixture(target, 1, 0);
    unit.tables.regions.push(RegionInstance {
        scope: ScopeId(0),
        anchor: SourceAnchorId(0),
        seal_demand: demanded,
    });
    unit.types.push(LirType {
        repr: ValueRepr::RegionMark,
        layout: LayoutId(1),
    });
    unit.values.push(ValueInfo {
        ty: LirTypeId(3),
        holding: Holding::Plain,
    });
    unit.instructions.push(Inst::ArenaToken {
        result: ValueId(2),
        mark: ValueId(3),
        budget: None,
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
    f.allocate_slots().unwrap();
    let failed = f.b.create_block();
    f.failures.insert(FailureId(0), failed);
    f.instruction(&Inst::RegionEnter {
        result: ValueId(3),
        region: RegionInstanceId(0),
    })
    .unwrap();
    f.instruction(&Inst::RegionExit {
        mark: ValueId(3),
        region: RegionInstanceId(0),
        failure: FailureId(0),
    })
    .unwrap();
    let zero = f.b.ins().iconst(types::I8, 0);
    f.b.ins().return_(&[zero]);
    f.b.switch_to_block(failed);
    let two = f.b.ins().iconst(types::I8, 2);
    f.b.ins().return_(&[two]);
    f.b.seal_all_blocks();
    f.b.finalize(f.em.module.target_config());
    assert!(
        ctx.func
            .display()
            .to_string()
            .contains(&format!("explicit_slot {}", if demanded { 80 } else { 64 }))
    );
    em.define("probe", id, &mut ctx).unwrap();
    em.module.finish().emit().unwrap()
}
