use super::*;
use hirsel_core_contract::Manifest;

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
    let context = Context::new();
    let module = Module::new(&context, "receipt-holder-probes");
    let machine = machine(target, false).unwrap();
    module.set_target(&machine);
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
    for stored in [false, true] {
        let signature = Signature::runtime(&module, &abi, &unit.header.definition).unwrap();
        let function = module
            .function(if stored { "stored" } else { "direct" }, signature.ty)
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
        f.define(ValueId(0), module.parameter(function, 1)).unwrap();
        assert!(matches!(
            f.value_shape(ValueId(0)).unwrap(),
            Shape::Scalar(ScalarKind::Pointer)
        ));
        if malformed {
            assert!(
                f.instruction(&release(MaskRef::Static(MaskId(0)), LayoutId(0)))
                    .is_err()
            );
            assert!(
                f.instruction(&Inst::Reset {
                    result: ValueId(0),
                    value: ValueId(0),
                    layout: LayoutId(0),
                    live: MaskRef::All,
                    anchor: CleanupAnchorId(0),
                    failure: FailureId(0),
                })
                .is_err()
            );
            assert!(manifested.imports.is_empty());
            break;
        }
        if stored || nested {
            let place = f.stack(LayoutId(u32::from(nested))).unwrap();
            let field = if nested {
                f.field_place(place, 8, LayoutId(0), false).unwrap()
            } else {
                place
            };
            f.store(
                Shape::Scalar(ScalarKind::Pointer),
                f.get(ValueId(0)).unwrap(),
                field,
            );
            f.dup_holders(place).unwrap();
            if nested {
                let anchor = f.const_u64(0);
                f.drop_glue(anchor, |walk| {
                    walk.steps(
                        &glue::Scope::of(LayoutId(1)),
                        place.ptr,
                        8,
                        &table.layouts[1].drop,
                    )
                })
                .unwrap();
            }
        } else {
            f.instruction(&Inst::Dup { value: ValueId(0) }).unwrap();
        }
        if !nested {
            f.instruction(&release(MaskRef::All, LayoutId(0))).unwrap();
        }
        f.instruction(&release(MaskRef::All, LayoutId(0))).unwrap();
        f.builder.ret(module.const_int(module.int_type(8), 0));
    }
    if malformed {
        return vec![];
    }
    module.verify().unwrap();
    if !nested {
        assert_eq!(
            manifested
                .imports
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "hirsel_rt_v2_receipt_release",
                "hirsel_rt_v2_receipt_retain"
            ]
        );
    }
    let text = module.text();
    assert!(
        !text.contains("load i32"),
        "metadata is not a language block: {text}"
    );
    optimize_and_emit(&module, &machine, OptLevel::None, true, false).unwrap()
}

#[test]
fn receipt_holders_execute_through_only_the_metadata_abi_on_both_targets() {
    for target in TargetSpec::executed() {
        execute(
            &target,
            probe(&target, false, false),
            "llvm-receipt-holders",
        );
    }
}

#[test]
fn receipt_partial_release_and_reuse_refuse_before_importing_runtime_helpers() {
    probe(&TargetSpec::x86_64_unknown_linux_gnu(), true, false);
}

#[test]
fn nested_receipt_copy_and_compiled_child_drop_execute_on_both_targets() {
    for target in TargetSpec::executed() {
        execute(
            &target,
            probe(&target, false, true),
            "llvm-receipt-child-drop",
        );
    }
}
