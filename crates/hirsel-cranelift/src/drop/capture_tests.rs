use crate::function::{FnEmitter, Shape, V};
use crate::module::Emitter;
use cranelift_codegen::{
    Context,
    ir::{self, InstBuilder, condcodes::IntCC, types},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use hirsel_core_contract::Manifest;
use hirsel_lir::emission::{EmitInputs, OptLevel, symbol_name};
use hirsel_lir::stage::{InputIdentity, Ready};
use hirsel_lir::{abi_types::*, ir::*, layout_types::*, sealed_payload_source::*};
use std::collections::{BTreeMap, BTreeSet};
// Private emitter probes exercise transport only, without claiming C/H admission.
fn fixture(
    target: &TargetSpec,
    once: bool,
    usage: CaptureUse,
) -> (LayoutTable, LirUnit<Ready>, FnAbi, FnAbi) {
    let (mut table, mut unit, mut entry_abi) = crate::receipt::tests::fixture(target);
    table.layouts[0].copy = CopyRecipe::NotCopyable;
    let mut pointer = table.layouts[1].clone();
    pointer.kind = LayoutKind::Scalar(ScalarKind::Pointer);
    pointer.abi.scalars = vec![(0, ScalarKind::Pointer)];
    table.layouts.push(pointer); // 2
    let mut base = table.layouts[1].clone();
    base.kind = if once {
        LayoutKind::Environment {
            movable: vec![0, 2],
        }
    } else {
        LayoutKind::Aggregate
    };
    base.size = SizeSpec::Fixed(if once { 32 } else { 24 });
    base.fields = (0..if once { 4 } else { 3 })
        .map(|ordinal| FieldLayout {
            ordinal,
            offset: u64::from(ordinal) * 8,
            layout: LayoutId(1),
            unaligned: false,
        })
        .collect();
    base.abi.scalars.clear();
    table.layouts.push(base); // 3
    let hash = unit.header.instance;
    let contract = ReceiptContract::new(
        vec![],
        None,
        [ReceiptMode::Take, ReceiptMode::Borrow, ReceiptMode::Take]
            .into_iter()
            .enumerate()
            .map(|(ordinal, mode)| ReceiptCapture {
                ordinal: ordinal as u32,
                mode,
                ty: ReceiptType {
                    source_type: hash,
                    profile: hash,
                },
            })
            .collect(),
        vec![],
    )
    .unwrap();
    let extent = if once { 32 } else { 24 };
    let mut wrapper = table.layouts[3].clone();
    wrapper.key = hirsel_lir::receipt_closure::owned_environment_key(
        &wrapper.key,
        contract.content().unwrap(),
    );
    wrapper.kind = LayoutKind::Aggregate;
    wrapper.copy = CopyRecipe::NotCopyable;
    wrapper.size = SizeSpec::Fixed(extent + 24);
    wrapper.fields = vec![FieldLayout {
        ordinal: 0,
        offset: 0,
        layout: LayoutId(3),
        unaligned: false,
    }];
    for (index, layout) in [0, 2, 0].into_iter().enumerate() {
        wrapper.fields.push(FieldLayout {
            ordinal: index as u32 + 1,
            offset: extent + index as u64 * 8,
            layout: LayoutId(layout),
            unaligned: false,
        });
    }
    wrapper.drop = vec![
        DropStep::ReleaseReceipt {
            offset: extent + 16,
        },
        DropStep::ReleaseReceipt { offset: extent },
    ];
    table.layouts.push(wrapper); // 4
    let mut callable = table.layouts[3].clone();
    callable.kind = LayoutKind::Callable {
        environment: Some(LayoutId(4)),
    };
    callable.size = SizeSpec::Fixed(16);
    callable.fields.clear();
    callable.drop = vec![DropStep::Environment { offset: 8 }];
    callable.scan = vec![ScanStep::Environment { offset: 8 }];
    table.layouts.push(callable); // 5
    let mut constructor_abi = entry_abi.clone();
    constructor_abi.receipts = None;
    constructor_abi.key.receipt_contract = None;
    constructor_abi.parameters.truncate(1);
    constructor_abi.parameters[0].layout = LayoutId(2);
    constructor_abi.parameters[0].passing = ParameterPassing::ExclusiveAddress;
    constructor_abi.parameters[0].slots[0].ty = PhysicalType::Pointer;
    entry_abi.shape = CallableShape::Closure;
    entry_abi.environment = true;
    entry_abi.multiplicity = if once {
        CallMultiplicity::Once
    } else {
        CallMultiplicity::Many
    };
    entry_abi.parameters.clear();
    entry_abi.receipts = Some(contract.clone());
    entry_abi.key.receipt_contract = Some(contract.content().unwrap());
    table.fn_abis.push(entry_abi.clone());
    unit.types = vec![
        LirType {
            repr: ValueRepr::Scalar(ScalarKind::U64),
            layout: LayoutId(1),
        },
        LirType {
            repr: ValueRepr::PayloadReceipt,
            layout: LayoutId(0),
        },
        LirType {
            repr: ValueRepr::Callable,
            layout: LayoutId(5),
        },
    ];
    unit.values.clear();
    unit.slots.clear();
    unit.parameters.clear();
    unit.captures.clear();
    for ordinal in 0..if once { 4 } else { 3 } {
        let id = ValueId(unit.values.len() as u32);
        unit.values.push(ValueInfo {
            ty: LirTypeId(0),
            holding: Holding::Plain,
        });
        unit.captures.push(id);
        unit.slots.push(SlotInfo {
            ty: LirTypeId(0),
            kind: SlotKind::Capture(ordinal),
            scope: ScopeId(0),
        });
    }
    let mut bindings = Vec::new();
    let mut captures = Vec::new();
    for ordinal in 0..3 {
        let value = ValueId(unit.values.len() as u32);
        unit.values.push(ValueInfo {
            ty: LirTypeId(1),
            holding: if ordinal == 1 {
                Holding::Borrowed(LoanId(0))
            } else {
                Holding::Owned
            },
        });
        captures.push(ReceiptClosureCapture {
            ordinal,
            receipt: value,
        });
        let version = ValueId(unit.values.len() as u32);
        unit.values.push(ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Plain,
        });
        let slot = SlotId(unit.slots.len() as u32);
        unit.slots.push(SlotInfo {
            ty: LirTypeId(1),
            kind: SlotKind::ReceiptCapture(ordinal),
            scope: ScopeId(0),
        });
        bindings.push(ReceiptEntrySlot {
            source: ordinal,
            slot,
            version,
        });
    }
    let result = ValueId(unit.values.len() as u32);
    unit.values.push(ValueInfo {
        ty: LirTypeId(2),
        holding: Holding::Owned,
    });
    unit.tables.receipt_entry = Some(ReceiptEntry {
        environment: Some(LayoutId(4)),
        contract: contract.content().unwrap(),
        parameters: vec![],
        captures: bindings,
        result: None,
    });
    unit.tables.symbols.push(Symbol {
        kind: SymbolKind::Function,
        definition: hirsel_syntax::DefPath::core("receipt_capture_entered"),
        instance: Some(hash),
    });
    let capture_values = (0..3)
        .map(|ordinal| CaptureValue {
            value: ValueId(ordinal),
            usage: if ordinal == 1 {
                CaptureUse::Borrow
            } else {
                usage
            },
        })
        .collect::<Vec<_>>();
    let ordinary = PoolRange::push(&mut unit.pools.captures, capture_values);
    unit.tables.closures.push(ClosureSite {
        code: Some(ClosureCode {
            symbol: SymbolId(0),
            abi: FnAbiId(0),
        }),
        captures: ordinary,
        environment: LayoutId(4),
        scoped: true,
        placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
        destination: DestinationClass::Heap,
        anchor: SourceAnchorId(0),
        receipts: Some(ReceiptClosure {
            contract: contract.content().unwrap(),
            captures,
        }),
    });
    unit.tables.source_anchors.push(SourceAnchor {
        body: 0,
        block: 0,
        statement: 0,
        site: SiteRange {
            source: 0,
            start: 0,
            end: 1,
        },
    });
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
    assert_eq!(result.index() + 1, unit.values.len());
    (table, unit, entry_abi, constructor_abi)
}

const HARNESS: &str = r#"
#include <stdint.h>
#include <stdlib.h>
#include <assert.h>
static unsigned allocations, released;
static void *block;
static uint64_t context[1024];
void *hirsel_rt_v2_alloc(void *ctx, uint32_t descriptor, uint64_t size, uint64_t align, uint8_t domain, uint64_t site) {
    (void)descriptor; (void)site; (void)domain;
    assert(ctx == (void*)context && size == EXTENT && align == 8);
    ++allocations; block = calloc(1, size + 8); return block;
}
void hirsel_rt_v2_receipt_release(void *receipt) {
    assert(receipt == (void*)(uintptr_t)(released % 2 == 0 ? 103 : 101)); ++released;
}
EXTRA
extern uint8_t construct(void *, void **);
extern uint8_t entered(void *, void *) __asm__("ENTRY_SYMBOL");
extern uint8_t entered_fail(void *, void *);
extern uint8_t cleanup(void *, void *);
int main(void) {
    for (int fail = 0; fail < 2; ++fail) {
        void *environment = 0;
        assert(construct((void*)context, &environment) == 0 && environment == block);
        uint64_t *payload = (uint64_t *)((char*)environment + 8);
        assert(payload[0] == 11 && payload[1] == 22 && payload[2] == 33);
        LIVE_CHECK
        assert((fail ? entered_fail((void*)context, environment) : entered((void*)context, environment)) == fail);
        assert(released == fail * 2);
        assert(cleanup((void*)context, environment) == 0 && released == (fail + 1) * 2);
        free(block);
    }
    assert(allocations == 2 && released == 4);
}
"#;

fn probe(target: &TargetSpec, once: bool, usage: CaptureUse, malformed: u8) -> (Vec<u8>, String) {
    let (mut table, mut unit, entry_abi, constructor_abi) = fixture(target, once, usage);
    if malformed == 1 {
        table.layouts[4].fields[2].offset += 8;
    }
    if malformed == 2 {
        table.layouts[4].drop.reverse();
    }
    if malformed == 3 {
        table.layouts[4].fields[1].layout = LayoutId(2);
    }
    if malformed == 4 {
        unit.tables.closures[0].receipts.as_mut().unwrap().captures[0].receipt = ValueId(0);
    }
    if malformed != 0 {
        let site = &unit.tables.closures[0];
        assert!(
            site.receipts
                .as_ref()
                .unwrap()
                .check_transport(site, &entry_abi, &unit, &table)
                .is_err()
        );
        if malformed < 4 {
            assert!(
                unit.tables
                    .receipt_entry
                    .as_ref()
                    .unwrap()
                    .check_transport(&entry_abi, &unit, &table)
                    .is_err()
            );
        }
        return (vec![], String::new());
    }
    let entry_name = symbol_name(
        &unit.tables.symbols[0].definition,
        unit.tables.symbols[0].instance.as_ref(),
    );
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
    for kind in 0..4 {
        let abi = if kind == 0 {
            &constructor_abi
        } else {
            &entry_abi
        };
        let sig = em.runtime_signature(abi, &unit.header.definition).unwrap();
        let name = match kind {
            0 => "construct",
            1 => &entry_name,
            2 => "entered_fail",
            _ => "cleanup",
        };
        let id = em.declare_function(name, Linkage::Export, &sig.ir).unwrap();
        let mut ctx = Context::new();
        ctx.func = ir::Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
        let mut bc = FunctionBuilderContext::new();
        let b = FunctionBuilder::new(&mut ctx.func, &mut bc);
        let mut f = FnEmitter::new(&mut em, &unit, b, sig);
        if kind == 0 {
            for (ordinal, value) in [11, 22, 33].into_iter().enumerate() {
                let value = f.u64c(value);
                f.define1(ValueId(ordinal as u32), value).unwrap();
            }
            for (ordinal, capture) in unit.tables.closures[0]
                .receipts
                .as_ref()
                .unwrap()
                .captures
                .iter()
                .enumerate()
            {
                let value = f.u64c(101 + ordinal as u64);
                f.define1(capture.receipt, value).unwrap();
            }
            let result = ValueId(unit.values.len() as u32 - 1);
            let callable = f.closure(result, ClosureId(0)).unwrap();
            f.define(result, callable).unwrap();
            let (_, environment) = f.callable_parts(result).unwrap();
            f.store_at(environment, f.entry_params[1], 0);
            let status = f.b.ins().iconst(types::I8, 0);
            f.b.ins().return_(&[status]);
        } else if kind < 3 {
            f.bind_captures().unwrap();
            f.bind_receipt_entry().unwrap();
            let mut correct = f.b.ins().iconst(types::I8, 1);
            for (slot, expected) in [(0, 11), (1, 22), (2, 33)].into_iter().chain(if once {
                vec![(3, 3)]
            } else {
                vec![]
            }) {
                let value = f.load_ptr(f.slots[slot].unwrap().ptr);
                let eq = f.b.ins().icmp_imm(IntCC::Equal, value, expected);
                correct = f.b.ins().band(correct, eq);
            }
            for binding in &unit.tables.receipt_entry.as_ref().unwrap().captures {
                let value = f.load_ptr(f.slots[binding.slot.index()].unwrap().ptr);
                let eq =
                    f.b.ins()
                        .icmp_imm(IntCC::Equal, value, 101 + i64::from(binding.source));
                correct = f.b.ins().band(correct, eq);
            }
            let success = f.b.ins().iconst(types::I8, i64::from(kind == 2));
            let wrong = f.b.ins().iconst(types::I8, 2);
            let status = f.b.ins().select(correct, success, wrong);
            f.b.ins().return_(&[status]);
        } else {
            let payload = f.payload_of(f.environment.unwrap(), LayoutId(4)).unwrap();
            let status = f
                .finalize_inline(payload, MaskRef::All, CleanupAnchorId(0))
                .unwrap();
            f.b.ins().return_(&[status]);
        }
        f.b.seal_all_blocks();
        f.b.finalize(f.em.module.target_config());
        em.define(name, id, &mut ctx).unwrap();
    }
    em.registry_table(&unit.header.definition).unwrap();
    em.cells().unwrap();
    em.drain_helpers(&unit.header.definition).unwrap();
    let object = em.module.finish().emit().unwrap();
    let extra = FINALIZE.replace("REGISTRY", &registry.symbol());
    let index = hirsel_lir::glue::descriptor_index_symbol(&keys.key(LayoutId(4)));
    let harness = HARNESS
        .replace("INDEX_SYMBOL", &index)
        .replace("ENTRY_SYMBOL", &entry_name)
        .replace("EXTENT", if once { "56" } else { "48" })
        .replace(
            "LIVE_CHECK",
            if once { "assert(payload[3] == 3);" } else { "" },
        )
        .replace("EXTRA", &extra);
    (object, harness)
}

#[test]
fn owning_capture_136_transports_copy_take_once_and_failure_cleanup_on_both_targets() {
    for target in TargetSpec::executed() {
        for once in [false, true] {
            for usage in [CaptureUse::Copy, CaptureUse::Take] {
                let (object, harness) = probe(&target, once, usage, 0);
                crate::receipt::tests::execute(
                    &target,
                    object,
                    "cranelift-owning-capture-136",
                    &harness,
                );
            }
        }
    }
}

#[test]
fn owning_capture_136_refuses_forged_geometry_recipes_types_and_metadata_operands() {
    for malformed in 1..=4 {
        probe(
            &TargetSpec::x86_64_unknown_linux_gnu(),
            true,
            CaptureUse::Take,
            malformed,
        );
    }
}

const FINALIZE: &str = r#"
typedef struct { uint32_t kind, field, variant, span; uint64_t offset, stride, count, count_offset; void *layout; } Step;
typedef struct { uint32_t magic, version; uint64_t size, align; void *finalizer; const Step *steps; uint64_t step_count; uint32_t field_count, flags; uint64_t logical; void *logical_count, *scan; uint64_t scan_words; } Layout;
extern const Layout *registry[] __asm__("REGISTRY");
uint8_t hirsel_rt_v2_finalize(void *ctx, uint32_t descriptor, void *value, const uint64_t *mask, void *continuation, uint64_t anchor, uint64_t site) {
    (void)ctx; (void)continuation; (void)anchor; (void)site;
    const Layout *layout = registry[descriptor - 2];
    assert(layout->step_count == 2 && mask == 0);
    for (unsigned i = 0; i < 2; ++i) {
        const Step *step = &layout->steps[i];
        assert(step->kind == 13 && step->span == 0 && step->stride == 0);
        hirsel_rt_v2_receipt_release(*(void **)((char *)value + step->offset));
    }
    return 0;
}
"#;
