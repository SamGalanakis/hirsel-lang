//! Backend controls for layouts source/Ready validation cannot currently produce inline.
//! Exercise emitted machine code against the real runtime, not a model of its calls.
use super::*;
use crate::module::{Emitter, Hidden, Sig};
use cranelift_codegen::{
    Context,
    ir::{self, AbiParam},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use hirsel_core_contract::Manifest;
use hirsel_lir::abi_types::*;
use hirsel_lir::descriptor_registry::DescriptorRegistry;
use hirsel_lir::emission::{EmitInputs, FailureCodes, OptLevel, symbol_name};
use hirsel_lir::layout_types::*;
use hirsel_lir::stage::{InputIdentity, Ready, StageKind};
use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "inline-probe".into(),
            version: "0".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}

fn record(target: &TargetSpec, kind: LayoutKind, size: SizeSpec) -> LayoutRecord {
    LayoutRecord {
        key: LayoutIdentity {
            owner_revision: None,
            instance: None,
            target: target.digest(),
            layout_version: hirsel_lir::LAYOUT_VERSION,
        },
        kind,
        size,
        align: 8,
        fields: vec![],
        tag: None,
        initialized: InitializedBytes::Fields,
        scan: vec![],
        drop: vec![],
        copy: CopyRecipe::Fields,
        finalizer: None,
        storage: vec![StorageKind::Heap],
        abi: AbiInputs {
            scalars: vec![],
            unaligned: false,
        },
    }
}

fn layouts(target: &TargetSpec) -> LayoutTable {
    let mut table = LayoutTable::new(target);
    let mut scalar = record(
        target,
        LayoutKind::Scalar(ScalarKind::U64),
        SizeSpec::Fixed(8),
    );
    scalar.copy = CopyRecipe::Bits;
    scalar.abi.scalars.push((0, ScalarKind::U64));
    table.layouts.push(scalar);
    let mut block = record(
        target,
        LayoutKind::Box {
            payload: LayoutId(0),
        },
        SizeSpec::Fixed(8),
    );
    block.scan.push(ScanStep::Counted { offset: 0 });
    block.drop.push(DropStep::Release { offset: 0 });
    block.copy = CopyRecipe::Dup;
    block.abi.scalars.push((0, ScalarKind::Pointer));
    table.layouts.push(block);
    let field = |ordinal, offset, layout| FieldLayout {
        ordinal,
        offset,
        layout: LayoutId(layout),
        unaligned: false,
    };
    let mut resource = record(target, LayoutKind::Resource, SizeSpec::Fixed(8));
    resource.fields = vec![field(0, 0, 0)];
    resource.drop = vec![DropStep::Finalize];
    resource.finalizer = Some(FinalizerRef {
        definition: path("drop"),
        body: 0,
        unsafe_ffi_package: false,
    });
    resource.copy = CopyRecipe::NotCopyable;
    table.layouts.push(resource);
    let mut pair = record(target, LayoutKind::Aggregate, SizeSpec::Fixed(16));
    pair.fields = vec![field(0, 0, 1), field(1, 8, 1)];
    pair.scan = vec![
        ScanStep::Counted { offset: 0 },
        ScanStep::Counted { offset: 8 },
    ];
    pair.drop = vec![
        DropStep::Release { offset: 8 },
        DropStep::Release { offset: 0 },
    ];
    table.layouts.push(pair.clone());
    let mut variant = record(target, LayoutKind::Variant, SizeSpec::Fixed(24));
    variant.tag = Some(TagEncoding {
        offset: 0,
        form: TagForm::Ordinal,
        variants: vec![VariantLayout {
            ordinal: 0,
            fields: vec![field(0, 8, 1), field(1, 16, 1)],
        }],
    });
    variant.drop = vec![DropStep::ActiveVariant {
        tag_offset: 0,
        form: TagForm::Ordinal,
        variants: vec![(
            0,
            vec![
                DropStep::Release { offset: 16 },
                DropStep::Release { offset: 8 },
            ],
        )],
    }];
    table.layouts.push(variant);
    let mut child = record(target, LayoutKind::Aggregate, SizeSpec::Fixed(16));
    child.fields = vec![field(0, 0, 1), field(1, 8, 2)];
    child.scan = vec![ScanStep::Counted { offset: 0 }];
    child.drop = vec![
        DropStep::Child {
            offset: 8,
            layout: LayoutId(2),
        },
        DropStep::Release { offset: 0 },
    ];
    table.layouts.push(child);
    let mut sequence = record(
        target,
        LayoutKind::Sequence {
            element: LayoutId(1),
        },
        SizeSpec::Dynamic(vec![SizeTerm::Constant(16)]),
    );
    sequence.fields = vec![field(0, 0, 0), field(1, 8, 0)];
    sequence.scan = vec![ScanStep::Sequence {
        count_offset: 0,
        offset: 16,
        stride: 8,
        element: LayoutId(1),
    }];
    sequence.drop = vec![DropStep::ElementsReverse {
        count_offset: 0,
        offset: 16,
        stride: 8,
        element: LayoutId(1),
    }];
    table.layouts.push(sequence);
    let columns = SoaColumns {
        count_offset: 0,
        capacity_offset: 8,
        data_offset: 16,
        capacity_quantum: 8,
        columns: vec![
            SoaColumn {
                field: 0,
                prefix: 0,
                stride: 8,
                layout: LayoutId(1),
                arm: None,
            },
            SoaColumn {
                field: 1,
                prefix: 8,
                stride: 8,
                layout: LayoutId(1),
                arm: None,
            },
        ],
        tag: None,
    };
    let mut soa = record(
        target,
        LayoutKind::SoaColumns {
            element: LayoutId(3),
        },
        SizeSpec::Dynamic(vec![SizeTerm::Constant(16)]),
    );
    soa.fields = vec![field(0, 0, 0), field(1, 8, 0)];
    soa.scan = vec![ScanStep::Columns(columns.clone())];
    soa.drop = vec![DropStep::ColumnsReverse(columns)];
    table.layouts.push(soa);
    table
}

fn object(target: &TargetSpec) -> (Vec<u8>, String) {
    let table = layouts(target);
    let input = InputIdentity {
        interface_version: hirsel_lir::INTERFACE_VERSION,
        digest: ContentHash::of_bytes(b"probe"),
    };
    let manifest = Manifest::skeleton([]);
    let failures = FailureCodes::default();
    let instance = ContentHash::of_bytes(b"drop instance");
    let finalizers = BTreeMap::from([(LayoutId(2), instance)]);
    let empty = BTreeSet::new();
    let registry = DescriptorRegistry::new(&table, &finalizers);
    let keys = hirsel_lir::LayoutKeys::new(&table);
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
    // The probes and the descriptors they name share one object: unit and glue inputs.
    let mut em = Emitter::new(
        crate::module::Inputs {
            glue: Some(&glue_inputs),
            ..crate::module::Inputs::units(&inputs)
        },
        false,
    )
    .unwrap();
    let abi = FnAbi {
        receipts: None,
        nonretaining: Vec::new(),
        nonretaining_captures: Vec::new(),
        scoped_contract: hirsel_lir::abi::scoped_contract_digest(&[], &[]),
        scoped_entries: Vec::new(),
        key: FnAbiKey {
            receipt_contract: None,
            callable: input.digest,
            abi_version: hirsel_lir::ABI_VERSION,
        },
        shape: CallableShape::Definition,
        multiplicity: CallMultiplicity::Many,
        context: true,
        environment: false,
        caller_site: false,
        destination: false,
        result: None,
        diverges: false,
        result_arena: None,
        parameters: vec![],
        version: hirsel_lir::ABI_VERSION,
    };
    let mut unit = LirUnit::<Ready>::new(UnitHeader {
        source_count: 1,
        key: hirsel_lir::objects::UnitKey(hirsel_syntax::ContentHash([0; 32])),
        track_caller: false,
        stage: StageKind::Ready,
        lir_version: hirsel_lir::LIR_VERSION,
        origin: UnitOrigin::Source,
        definition: path("probe"),
        implementation_revision: instance,
        instance,
        target: table.target,
        abi: FnAbiId(0),
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
    unit.types.push(LirType {
        repr: ValueRepr::InitMask { fields: 2 },
        layout: LayoutId(0),
    });
    unit.values.push(ValueInfo {
        ty: LirTypeId(0),
        holding: Holding::Plain,
    });
    unit.pools.mask_words.push(2);
    unit.tables.masks.push(FieldMask {
        fields: 2,
        words: PoolRange::new(0, 1),
    });
    let probes = [
        ("record_mask", 3, false, MaskRef::Dynamic(ValueId(0))),
        ("variant_mask", 4, false, MaskRef::Dynamic(ValueId(0))),
        ("variant_static", 4, false, MaskRef::Static(MaskId(0))),
        ("child_finalizer", 5, false, MaskRef::All),
        ("sequence_copy", 6, true, MaskRef::All),
        ("columns_copy", 7, true, MaskRef::All),
        ("columns_drop", 7, false, MaskRef::All),
    ];
    for (name, layout, copy, mask) in probes {
        let mut sig = ir::Signature::new(em.call_conv);
        sig.params.extend([
            AbiParam::new(PTR),
            AbiParam::new(PTR),
            AbiParam::new(types::I64),
        ]);
        sig.returns.push(AbiParam::new(types::I8).uext());
        let id = em.declare_function(name, Linkage::Export, &sig).unwrap();
        let mut ctx = Context::new();
        ctx.func = ir::Function::with_name_signature(ir::UserFuncName::default(), sig.clone());
        let mut bc = FunctionBuilderContext::new();
        let b = FunctionBuilder::new(&mut ctx.func, &mut bc);
        let mut f = FnEmitter::new(
            &mut em,
            &unit,
            b,
            Sig {
                abi: &abi,
                ir: sig,
                hidden: vec![Hidden::Context],
                first: vec![],
                value: None,
                ahead: None,
            },
        );
        let base = f.entry_params[1];
        f.define1(ValueId(0), f.entry_params[2]).unwrap();
        let status = if copy {
            f.dup_scan(base, &table.layouts[layout].scan).unwrap();
            f.b.ins().iconst(types::I8, 0)
        } else {
            f.finalize_inline(
                Place {
                    columns: None,
                    ptr: base,
                    layout: LayoutId(layout as u32),
                    align: 8,
                    block: None,
                },
                mask,
                CleanupAnchorId(0),
            )
            .unwrap()
        };
        f.b.ins().return_(&[status]);
        f.b.seal_all_blocks();
        f.b.finalize(f.em.module.target_config());
        em.define(name, id, &mut ctx).unwrap();
    }
    em.registry_table(&path("probe")).unwrap();
    em.cells().unwrap();
    em.drain_helpers(&path("probe")).unwrap();
    let source = HARNESS
        .replace("REGISTRY_NAME", &registry.symbol())
        .replace("REGISTRY_COUNT", &registry.external_count().to_string())
        .replace(
            "SCALAR_INDEX",
            &registry.index(LayoutId(0)).unwrap().to_string(),
        )
        .replace("DROP_NAME", &symbol_name(&path("drop"), Some(&instance)));
    (em.module.finish().emit().unwrap(), source)
}

fn checked(command: &mut Command) -> std::process::Output {
    let out = command.output().unwrap();
    assert!(out.status.success(), "{command:?}: {out:?}");
    out
}

#[test]
fn inline_sequences_columns_masks_and_finalizers_execute_on_both_targets() {
    for target in TargetSpec::executed() {
        let (bytes, source) = object(&target);
        let dir = std::env::temp_dir().join(format!(
            "cranelift-inline-{}-{}",
            std::process::id(),
            target.triple
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let obj = dir.join("probe.o");
        let c = dir.join("probe.c");
        let exe = dir.join("probe");
        std::fs::write(&obj, bytes).unwrap();
        std::fs::write(&c, source).unwrap();
        let arm = target.triple.starts_with("aarch64");
        let mut cc = if arm {
            Command::new(std::env::var_os("HIRSEL_AARCH64_CC").unwrap())
        } else {
            Command::new(
                PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap()).join("bin/clang"),
            )
        };
        let archive = PathBuf::from(std::env::var_os("HIRSEL_RUNTIME_ARCHIVES").unwrap())
            .join(&target.triple)
            .join("diagnostic/libhirsel_rt.a");
        checked(
            cc.args(["-std=gnu11", "-O0", "-DHIRSEL_RT_ENABLE_STATS", "-I"])
                .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../hirsel-rt/include"))
                .arg(c)
                .arg(obj)
                .arg(archive)
                .args(["-lm", "-lpthread", "-ldl", "-lrt", "-lutil", "-o"])
                .arg(&exe),
        );
        let out = if arm {
            checked(
                Command::new(std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap())
                    .arg("-L")
                    .arg(std::env::var_os("HIRSEL_AARCH64_SYSROOT").unwrap())
                    .arg(&exe),
            )
        } else {
            checked(&mut Command::new(&exe))
        };
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "7 inline controls passed\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

const HARNESS: &str = r#"
#include "hirsel_rt_v2.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
extern const hirsel_rt_v2_layout *registry[] __asm__("REGISTRY_NAME");
extern uint8_t record_mask(void *, void *, uint64_t);
extern uint8_t variant_mask(void *, void *, uint64_t);
extern uint8_t variant_static(void *, void *, uint64_t);
extern uint8_t child_finalizer(void *, void *, uint64_t);
extern uint8_t sequence_copy(void *, void *, uint64_t);
extern uint8_t columns_copy(void *, void *, uint64_t);
extern uint8_t columns_drop(void *, void *, uint64_t);
static uint64_t finalized;
uint8_t body(hirsel_rt_v2_ctx *ctx, uint64_t *value) __asm__("DROP_NAME");
uint8_t body(hirsel_rt_v2_ctx *ctx, uint64_t *value) {
    finalized = *value;
    if (*value == 13) { hirsel_rt_v2_failure_begin(ctx, 1, 101, 17, 17); return 1; }
    return 0;
}
static hirsel_rt_v2_header *fresh(hirsel_rt_v2_ctx *ctx) {
    return hirsel_rt_v2_alloc(ctx, SCALAR_INDEX, 8, 8, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
}
static void release(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *p) {
    assert(hirsel_rt_v2_release(ctx, p, SCALAR_INDEX, NULL, 0, 0) == 0);
}
static uint64_t live(hirsel_rt_v2_ctx *ctx) {
    hirsel_rt_v2_stats s; hirsel_rt_v2_ctx_stats(ctx, &s); return s.live_blocks;
}
int main(void) {
    hirsel_rt_v2_config cfg = {0};
    cfg.abi_version = HIRSEL_RT_V2_ABI_VERSION;
    cfg.layouts = registry; cfg.layout_count = REGISTRY_COUNT;
    hirsel_rt_v2_ctx *ctx = NULL;
    assert(hirsel_rt_v2_ctx_create(&cfg, &ctx) == 0);
    hirsel_rt_v2_header *a = fresh(ctx), *b = fresh(ctx);
    void *pair[] = {a, b};
    assert(record_mask(ctx, pair, 1) == 0); assert(live(ctx) == 1); release(ctx, b);
    a = fresh(ctx); b = fresh(ctx);
    uint64_t variant[] = {0, (uint64_t)a, (uint64_t)b};
    assert(variant_mask(ctx, variant, 1) == 0); assert(live(ctx) == 1); release(ctx, b);
    a = fresh(ctx); b = fresh(ctx); variant[1] = (uint64_t)a; variant[2] = (uint64_t)b;
    assert(variant_static(ctx, variant, 0) == 0); assert(live(ctx) == 1); release(ctx, a);
    uint64_t child[] = {(uint64_t)fresh(ctx), 9};
    assert(child_finalizer(ctx, child, 0) == 0); assert(finalized == 9); assert(live(ctx) == 0);
    child[0] = (uint64_t)fresh(ctx); child[1] = 13;
    assert(child_finalizer(ctx, child, 0) == 1); assert(finalized == 13); assert(live(ctx) == 0);
    hirsel_rt_v2_failure_clear(ctx);
    a = fresh(ctx); b = fresh(ctx);
    uint64_t seq[] = {2, 2, (uint64_t)a, (uint64_t)b};
    assert(sequence_copy(ctx, seq, 0) == 0); release(ctx, a); release(ctx, b);
    assert(live(ctx) == 2); release(ctx, a); release(ctx, b);
    a = fresh(ctx); b = fresh(ctx); hirsel_rt_v2_header *c = fresh(ctx), *d = fresh(ctx);
    uint64_t columns[] = {2, 8, (uint64_t)a, (uint64_t)b, 0,0,0,0,0,0, (uint64_t)c, (uint64_t)d, 0,0,0,0,0,0};
    assert(columns_copy(ctx, columns, 0) == 0);
    assert(columns_drop(ctx, columns, 0) == 0); assert(live(ctx) == 4);
    release(ctx, a); release(ctx, b); release(ctx, c); release(ctx, d); assert(live(ctx) == 0);
    hirsel_rt_v2_ctx_destroy(ctx);
    puts("7 inline controls passed");
}
"#;
