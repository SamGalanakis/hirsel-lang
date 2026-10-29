use super::*;
use crate::module::{Emitter, Hidden, Sig};
use cranelift_codegen::{
    Context,
    ir::{self, AbiParam},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use hirsel_core_contract::Manifest;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::stage::{InputIdentity, Ready};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::BTreeSet;

use hirsel_lir::abi_types::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::operation::{NativeType, Primitive};

fn fixture(target: &TargetSpec) -> (LayoutTable, LirUnit<Ready>, FnAbi) {
    let definition = DefPath::core("receipt_holder_probe");
    let digest = ContentHash::of_bytes(b"private receipt holder probe");
    let mut table = LayoutTable::new(target);
    table.layouts.push(LayoutRecord {
        key: LayoutIdentity {
            owner_revision: None,
            instance: None,
            target: target.digest(),
            layout_version: hirsel_lir::LAYOUT_VERSION,
        },
        kind: LayoutKind::PayloadReceipt {
            root: NativeType::Primitive(Primitive::U64),
        },
        size: SizeSpec::Fixed(8),
        align: 8,
        fields: vec![],
        tag: None,
        initialized: InitializedBytes::Fields,
        scan: vec![],
        drop: vec![DropStep::ReleaseReceipt { offset: 0 }],
        copy: CopyRecipe::Dup,
        finalizer: None,
        storage: vec![StorageKind::Heap],
        abi: AbiInputs {
            scalars: vec![(0, ScalarKind::Pointer)],
            unaligned: false,
        },
    });
    let mut unit = LirUnit::<Ready>::new(UnitHeader {
        source_count: 1,
        key: hirsel_lir::objects::UnitKey(digest),
        track_caller: false,
        stage: hirsel_lir::stage::StageKind::Ready,
        lir_version: hirsel_lir::LIR_VERSION,
        origin: UnitOrigin::Source,
        definition,
        implementation_revision: digest,
        instance: digest,
        target: table.target,
        abi: FnAbiId(0),
    });
    unit.types.push(LirType {
        repr: ValueRepr::PayloadReceipt,
        layout: LayoutId(0),
    });
    unit.values.push(ValueInfo {
        ty: LirTypeId(0),
        holding: Holding::Owned,
    });
    let abi = FnAbi {
        receipts: None,
        nonretaining: vec![],
        nonretaining_captures: vec![],
        scoped_contract: hirsel_lir::abi::scoped_contract_digest(&[], &[]),
        scoped_entries: vec![],
        key: FnAbiKey {
            receipt_contract: None,
            callable: digest,
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
        parameters: vec![AbiParameter {
            source: 0,
            convention: hirsel_core_contract::Passing::Borrow,
            passing: ParameterPassing::ByValue,
            layout: LayoutId(0),
            slots: vec![PhysicalSlot {
                ty: PhysicalType::Pointer,
                size: 8,
                align: 8,
            }],
            extension: ScalarExtension::None,
        }],
        version: hirsel_lir::ABI_VERSION,
    };
    (table, unit, abi)
}

fn release(live: MaskRef, layout: LayoutId) -> Inst {
    Inst::Release {
        value: ValueId(0),
        layout,
        live,
        anchor: CleanupAnchorId(0),
        failure: FailureId(0),
    }
}

// These private lowering probes deliberately bypass Ready admission. They test the
// metadata ABI's dispatch and pointer representation, not source/custody authority.
// The C callbacks are ABI sentinels: a language-block/root helper or wrong pointer
// would either leave an unresolved symbol or fail the exact exchange assertions.
const HARNESS: &str = r#"
#include <assert.h>
#include <stdint.h>
static unsigned holders, retains, releases;
static unsigned char identity;
void hirsel_rt_v2_receipt_retain(void *p) {
    assert(p == &identity && holders > 0); ++holders; ++retains;
}
void hirsel_rt_v2_receipt_release(void *p) {
    assert(p == &identity && holders > 0); --holders; ++releases;
}
extern uint8_t direct(void *, void *);
extern uint8_t stored(void *, void *);
int main(void) {
    holders = 1; assert(direct(0, &identity) == 0);
    assert(holders == 0 && retains == 1 && releases == 2);
    holders = 1; assert(stored(0, &identity) == 0);
    assert(holders == 0 && retains == 2 && releases == 4);
}
"#;

fn execute(target: &TargetSpec, bytes: Vec<u8>, label: &str) {
    use std::path::PathBuf;
    use std::process::Command;
    fn checked(command: &mut Command) {
        let out = command.output().unwrap();
        assert!(out.status.success(), "{command:?}: {out:?}");
    }
    let dir =
        std::env::temp_dir().join(format!("{label}-{}-{}", std::process::id(), target.triple));
    std::fs::create_dir_all(&dir).unwrap();
    let c = dir.join("probe.c");
    let object = dir.join("probe.o");
    let executable = dir.join("probe");
    std::fs::write(&c, HARNESS).unwrap();
    std::fs::write(&object, bytes).unwrap();
    let arm = target.triple.starts_with("aarch64");
    let mut cc = if arm {
        Command::new(std::env::var_os("HIRSEL_AARCH64_CC").unwrap())
    } else {
        Command::new(
            PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap()).join("bin/clang"),
        )
    };
    checked(
        cc.args(["-std=c11", "-O0"])
            .arg(c)
            .arg(object)
            .arg("-o")
            .arg(&executable),
    );
    if arm {
        checked(
            Command::new(std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap())
                .arg("-L")
                .arg(std::env::var_os("HIRSEL_AARCH64_SYSROOT").unwrap())
                .arg(executable),
        );
    } else {
        checked(&mut Command::new(executable));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn probe(target: &TargetSpec, malformed: bool, nested: bool) -> Vec<u8> {
    let (mut table, unit, abi) = fixture(target);
    if nested {
        let mut parent = table.layouts[0].clone();
        parent.kind = LayoutKind::Aggregate;
        parent.size = SizeSpec::Fixed(16);
        parent.fields = vec![FieldLayout {
            ordinal: 0,
            offset: 8,
            layout: LayoutId(0),
            unaligned: false,
        }];
        parent.scan = vec![ScanStep::Inline {
            offset: 8,
            layout: LayoutId(0),
        }];
        parent.drop = vec![DropStep::Child {
            offset: 8,
            layout: LayoutId(0),
        }];
        parent.copy = CopyRecipe::Fields;
        parent.abi.scalars.clear();
        table.layouts.push(parent);
    }
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
    for stored in [false, true] {
        let name = if stored { "stored" } else { "direct" };
        let mut sig = ir::Signature::new(em.call_conv);
        sig.params.extend([AbiParam::new(PTR), AbiParam::new(PTR)]);
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
                first: vec![1],
                value: None,
                ahead: None,
                unique: None,
            },
        );
        f.define1(ValueId(0), f.entry_params[1]).unwrap();
        assert!(matches!(
            f.value_shape(ValueId(0)).unwrap(),
            crate::function::Shape::Scalar(ScalarKind::Pointer)
        ));
        if malformed {
            assert!(
                f.instruction_body(&release(MaskRef::Static(MaskId(0)), LayoutId(0)))
                    .is_err()
            );
            assert!(
                f.instruction_body(&Inst::Reset {
                    result: ValueId(0),
                    value: ValueId(0),
                    layout: LayoutId(0),
                    live: MaskRef::All,
                    anchor: CleanupAnchorId(0),
                    failure: FailureId(0),
                })
                .is_err()
            );
            assert!(em.manifested.imports.is_empty());
            return vec![];
        }
        if stored || nested {
            let place = f.stack(LayoutId(u32::from(nested))).unwrap();
            let field = if nested {
                Place {
                    ptr: f.offset(place.ptr, 8),
                    layout: LayoutId(0),
                    ..place
                }
            } else {
                place
            };
            let value = f.get(ValueId(0)).unwrap();
            f.store(
                crate::function::Shape::Scalar(ScalarKind::Pointer),
                value,
                field,
            )
            .unwrap();
            f.dup_holders(place).unwrap();
        } else {
            f.instruction_body(&Inst::Dup { value: ValueId(0) })
                .unwrap();
        }
        f.instruction_body(&release(MaskRef::All, LayoutId(0)))
            .unwrap();
        f.instruction_body(&release(MaskRef::All, LayoutId(0)))
            .unwrap();
        let status = f.b.ins().iconst(types::I8, 0);
        f.b.ins().return_(&[status]);
        f.b.seal_all_blocks();
        f.b.finalize(f.em.module.target_config());
        em.define(name, id, &mut ctx).unwrap();
    }
    assert_eq!(
        em.manifested
            .imports
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "hirsel_rt_v2_receipt_release",
            "hirsel_rt_v2_receipt_retain"
        ]
    );
    em.module.finish().emit().unwrap()
}

#[test]
fn receipt_holders_execute_through_only_the_metadata_abi_on_both_targets() {
    for target in TargetSpec::executed() {
        execute(
            &target,
            probe(&target, false, false),
            "cranelift-receipt-holders",
        );
    }
}

#[test]
fn receipt_partial_release_and_reuse_refuse_before_importing_runtime_helpers() {
    probe(&TargetSpec::x86_64_unknown_linux_gnu(), true, false);
}

#[test]
fn nested_receipt_copy_uses_metadata_custody_on_both_targets() {
    for target in TargetSpec::executed() {
        execute(
            &target,
            probe(&target, false, true),
            "cranelift-receipt-child-copy",
        );
    }
}
