use super::*;
use crate::module::Emitter;
use crate::receipt::tests::{execute, fixture};
use cranelift_codegen::Context;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_module::Linkage;
use hirsel_core_contract::Manifest;
use hirsel_lir::abi_types::FnAbiId;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::layout_types::{LayoutTable, TargetSpec};
use hirsel_lir::stage::InputIdentity;
use hirsel_lir::stage::Ready;
use hirsel_syntax::ContentHash;

fn call_fixture(
    target: &TargetSpec,
    malformed: u8,
) -> (LayoutTable, LirUnit<Ready>, FnAbi, String) {
    let (mut table, mut unit, abi) = fixture(target);
    table.fn_abis.push(abi.clone());
    unit.header.origin = UnitOrigin::Library;
    unit.tables.receipt_entry = None;
    unit.slots.clear();
    unit.slots.push(SlotInfo {
        ty: LirTypeId(1),
        kind: SlotKind::Parameter(2),
        scope: ScopeId(0),
    });
    unit.slots.push(SlotInfo {
        ty: LirTypeId(0),
        kind: SlotKind::Local,
        scope: ScopeId(0),
    });
    unit.terminators.push(Terminator::Unreachable);
    unit.values[4].holding = Holding::Owned;
    unit.values[5].holding = Holding::Owned;
    unit.values.push(ValueInfo {
        ty: LirTypeId(0),
        holding: Holding::Plain,
    });
    for (ty, slot) in [(1, 0), (0, 1)] {
        unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: vec![],
            base: AddressBase::Slot(SlotId(slot)),
            projection: PoolRange::EMPTY,
            ty: LirTypeId(ty),
        });
    }
    let callee = DefPath::core("receipt_call_callee");
    let name = hirsel_lir::emission::symbol_name(&callee, None);
    unit.tables.symbols.push(Symbol {
        kind: SymbolKind::Function,
        definition: callee,
        instance: None,
    });
    let arguments = PoolRange::push(
        &mut unit.pools.call_arguments,
        [
            CallArgument {
                parameter: 0,
                passing: hirsel_core_contract::Passing::Borrow,
                value: ArgumentValue::Value(ValueId(0)),
            },
            CallArgument {
                parameter: 1,
                passing: hirsel_core_contract::Passing::Take,
                value: ArgumentValue::Value(ValueId(1)),
            },
            CallArgument {
                parameter: 2,
                passing: hirsel_core_contract::Passing::Mut,
                value: ArgumentValue::Address {
                    address: AddressId(0),
                    version: ValueId(2),
                },
            },
        ],
    );
    let mut receipts = ReceiptCall {
        contract: abi.key.receipt_contract.unwrap(),
        arguments: vec![
            ReceiptArgument::Borrow {
                parameter: 0,
                receipt: ValueId(3),
            },
            ReceiptArgument::Take {
                parameter: 1,
                receipt: ValueId(4),
            },
            ReceiptArgument::Mut {
                parameter: 2,
                address: AddressId(1),
                version: ValueId(6),
            },
        ],
        result: Some(ValueId(5)),
    };
    if malformed == 1 {
        receipts.contract = ContentHash::of_bytes(b"wrong full contract");
    }
    if malformed == 2 {
        receipts.arguments[1] = ReceiptArgument::Borrow {
            parameter: 1,
            receipt: ValueId(4),
        };
    }
    unit.tables.calls.push(CallSite {
        pretransfer_failure: Some(FailureId(1)),
        forward_caller: true,
        callee: Callee::Direct(SymbolId(0)),
        operation_arguments: vec![],
        abi: FnAbiId(0),
        arguments,
        result: None,
        destination: None,
        declared: hirsel_lir::effect::Effects::ALL,
        anchor: SourceAnchorId(0),
        receipts: Some(receipts),
    });
    unit.blocks.push(hirsel_lir::ir::Block {
        kind: BlockKind::Body,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::EMPTY,
        anchor: SourceAnchorId(0),
    });
    unit.tables.edges.push(Edge {
        target: BlockId(0),
        arguments: PoolRange::EMPTY,
    });
    (table, unit, abi, name)
}

// This uses the real backend call path, including its status-3 split. The
// callee is an ABI sentinel, not an admitted source implementation/provider.
const CALL_HARNESS: &str = r#"
#include <stdint.h>
#include <assert.h>
static unsigned char borrowed, taken, initial, sentinel;
uint8_t callee(void *, uint64_t, uint64_t, uint64_t *, void *, void *, void **, void **) __asm__("CALLEE_NAME");
uint8_t callee(void *ctx, uint64_t status, uint64_t other, uint64_t *ordinary, void *b, void *t, void **mutated, void **output) {
    assert(ctx == 0 && other == 23 && b == &borrowed && t == &taken);
    if (status == 3) return 3;
    *ordinary = 42 + status; *mutated = t;
    if (status == 0) *output = b;
    return status;
}
extern uint8_t invoke(void *, uint64_t, uint64_t, uint64_t *, void *, void *, void **, void **);
int main(void) {
    for (uint64_t status = 0; status < 4; ++status) {
        if (status == 2) continue;
        uint64_t ordinary = 91;
        void *mutated = &initial, *output = &sentinel;
        assert(invoke(0, status, 23, &ordinary, &borrowed, &taken, &mutated, &output) == status);
        assert(ordinary == (status == 3 ? 91 : 42 + status));
        assert(mutated == (status == 3 ? &initial : &taken));
        assert(output == (status == 0 ? &borrowed : &sentinel));
    }
}
"#;

fn call_probe(target: &TargetSpec, malformed: u8) -> (Vec<u8>, String) {
    let (table, unit, abi, callee) = call_fixture(target, malformed);
    let keys = hirsel_lir::LayoutKeys::new(&table);
    let manifest = Manifest::skeleton([]);
    let input = InputIdentity {
        interface_version: hirsel_lir::INTERFACE_VERSION,
        digest: unit.header.instance,
    };
    let empty = BTreeSet::new();
    let inputs = EmitInputs {
        ahead: None,
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
        .declare_function("invoke", Linkage::Export, &sig.ir)
        .unwrap();
    let mut ctx = Context::new();
    ctx.func = ir::Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
    let mut bc = FunctionBuilderContext::new();
    let b = FunctionBuilder::new(&mut ctx.func, &mut bc);
    let mut f = FnEmitter::new(&mut em, &unit, b, sig);
    for (value, parameter) in [(0, 1), (1, 2), (3, 4), (4, 5)] {
        f.define1(ValueId(value), f.entry_params[parameter])
            .unwrap();
    }
    for (index, parameter, layout) in [(0, 3, 1), (1, 6, 0)] {
        f.addresses[index] = Some(Place {
            columns: None,
            ptr: f.entry_params[parameter],
            layout: LayoutId(layout),
            align: 8,
            block: None,
        });
    }
    let normal = f.b.create_block();
    let failed = f.b.create_block();
    let rejected = f.b.create_block();
    f.blocks.push(normal);
    f.params.push(vec![]);
    f.failures.insert(FailureId(0), failed);
    f.failures.insert(FailureId(1), rejected);
    let called = f.call_site_with_abi(CallId(0), Some(EdgeId(0)), FailureId(0), &abi);
    if malformed != 0 {
        let error = called.unwrap_err().to_string();
        assert!(
            error.contains(if malformed == 1 {
                "different complete callee contract"
            } else {
                "selectors or modes"
            }),
            "{error}"
        );
        assert!(em.manifested.imports.is_empty());
        return (vec![], callee);
    }
    called.unwrap();
    f.b.switch_to_block(normal);
    let result = f.get1(ValueId(5)).unwrap();
    f.b.ins().store(flags(), result, f.entry_params[7], 0);
    let zero = f.b.ins().iconst(types::I8, 0);
    f.b.ins().return_(&[zero]);
    for (block, status) in [(failed, 1), (rejected, 3)] {
        f.b.switch_to_block(block);
        let status = f.b.ins().iconst(types::I8, status);
        f.b.ins().return_(&[status]);
    }
    f.b.seal_all_blocks();
    f.b.finalize(f.em.module.target_config());
    em.define("invoke", id, &mut ctx).unwrap();
    assert_eq!(em.manifested.imports.len(), 1);
    assert!(em.manifested.imports.contains_key(&callee));
    (em.module.finish().emit().unwrap(), callee)
}

#[test]
fn native_receipt_call_keeps_rejection_entered_failure_and_normal_outputs_distinct_on_both_targets()
{
    for target in TargetSpec::executed() {
        let (bytes, callee) = call_probe(&target, 0);
        execute(
            &target,
            bytes,
            "cranelift-receipt-call",
            &CALL_HARNESS.replace("CALLEE_NAME", &callee),
        );
    }
}

#[test]
fn native_receipt_call_refuses_wrong_contract_or_modes_before_importing_the_callee() {
    for mutation in [1, 2] {
        call_probe(&TargetSpec::x86_64_unknown_linux_gnu(), mutation);
    }
}
