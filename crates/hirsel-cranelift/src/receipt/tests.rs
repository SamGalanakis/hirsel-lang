use super::*;
use crate::module::Emitter;
use cranelift_codegen::{
    Context,
    ir::{self, InstBuilder, condcodes::IntCC, types},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use hirsel_core_contract::Manifest;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::ir::*;
use hirsel_lir::stage::{InputIdentity, Ready};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::BTreeSet;

use hirsel_lir::abi_types::*;
use hirsel_lir::ir::{ReceiptEntry, ReceiptEntrySlot};
use hirsel_lir::layout_types::*;
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_lir::sealed_payload_source::*;

pub(crate) fn fixture(target: &TargetSpec) -> (LayoutTable, LirUnit<Ready>, FnAbi) {
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
    let mut scalar = table.layouts[0].clone();
    scalar.kind = LayoutKind::Scalar(ScalarKind::U64);
    scalar.drop.clear();
    scalar.copy = CopyRecipe::Bits;
    scalar.abi.scalars = vec![(0, ScalarKind::U64)];
    table.layouts.push(scalar);
    unit.types.push(LirType {
        repr: ValueRepr::Scalar(ScalarKind::U64),
        layout: LayoutId(1),
    });
    unit.values.clear();
    for _ in 0..3 {
        let value = ValueId(unit.values.len() as u32);
        unit.values.push(ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Plain,
        });
        unit.parameters.push(value);
    }
    let parameters = (0..3)
        .map(|source| {
            let slot = SlotId(unit.slots.len() as u32);
            unit.slots.push(SlotInfo {
                ty: LirTypeId(0),
                kind: SlotKind::ReceiptParameter(source),
                scope: ScopeId(0),
            });
            let version = ValueId(unit.values.len() as u32);
            unit.values.push(ValueInfo {
                ty: LirTypeId(0),
                holding: Holding::Plain,
            });
            ReceiptEntrySlot {
                source,
                slot,
                version,
            }
        })
        .collect();
    unit.slots.push(SlotInfo {
        ty: LirTypeId(0),
        kind: SlotKind::ReceiptResult,
        scope: ScopeId(0),
    });
    let ty = ReceiptType {
        source_type: digest,
        profile: digest,
    };
    let contract = ReceiptContract::new(
        [ReceiptMode::Borrow, ReceiptMode::Take, ReceiptMode::MutSlot]
            .into_iter()
            .enumerate()
            .map(|(source, mode)| ReceiptParameter {
                source: source as u32,
                ty: ty.clone(),
                mode,
            })
            .collect(),
        Some(ReceiptResult { ty }),
        vec![],
        vec![],
    )
    .unwrap();
    let entry = ReceiptEntry {
        environment: None,
        contract: contract.content().unwrap(),
        parameters,
        captures: vec![],
        result: Some(SlotId(3)),
    };
    let mut abi = abi;
    abi.parameters = [
        hirsel_core_contract::Passing::Borrow,
        hirsel_core_contract::Passing::Take,
        hirsel_core_contract::Passing::Mut,
    ]
    .into_iter()
    .enumerate()
    .map(|(source, convention)| AbiParameter {
        source: source as u32,
        convention,
        passing: if convention == hirsel_core_contract::Passing::Mut {
            ParameterPassing::ExclusiveAddress
        } else {
            ParameterPassing::ByValue
        },
        layout: LayoutId(1),
        slots: vec![PhysicalSlot {
            ty: if convention == hirsel_core_contract::Passing::Mut {
                PhysicalType::Pointer
            } else {
                PhysicalType::I64
            },
            size: 8,
            align: 8,
        }],
        extension: ScalarExtension::None,
    })
    .collect();
    abi.key.receipt_contract = Some(entry.contract);
    abi.receipts = Some(contract);
    unit.tables.receipt_entry = Some(entry);
    (table, unit, abi)
}

// Private backend probes bypass C admission: opaque pointer transport is not
// evidence for source profiles, holder custody, capture fields or public seal.
const HARNESS: &str = r#"
#include <stdint.h>
#include <assert.h>
extern uint8_t untouched(void *, uint64_t, uint64_t, uint64_t *, void *, void *, void **, void **);
extern uint8_t published(void *, uint64_t, uint64_t, uint64_t *, void *, void *, void **, void **);
int main(void) {
    unsigned char a, b, c, d;
    uint64_t ordinary = 91;
    void *mutated = &c, *output = &d;
    assert(untouched(0, 17, 23, &ordinary, &a, &b, &mutated, &output) == 0);
    assert(mutated == &b && output == &d && ordinary == 91);
    mutated = &c;
    assert(published(0, 17, 23, &ordinary, &a, &b, &mutated, &output) == 0);
    assert(mutated == &b && output == &a && ordinary == 91);
}
"#;

pub(crate) fn execute(target: &TargetSpec, bytes: Vec<u8>, label: &str, source: &str) {
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
    std::fs::write(&c, source).unwrap();
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

fn probe(target: &TargetSpec, malformed: u8) -> Vec<u8> {
    let (table, mut unit, abi) = fixture(target);
    if malformed == 1 {
        unit.tables.receipt_entry = None;
    }
    if malformed == 2 {
        unit.values[3].holding = Holding::Owned;
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
    let mut em = Emitter::new(crate::module::Inputs::units(&inputs), false).unwrap();
    for publish in [false, true] {
        let name = if publish { "published" } else { "untouched" };
        let sig = em.runtime_signature(&abi, &unit.header.definition).unwrap();
        let id = em.declare_function(name, Linkage::Export, &sig.ir).unwrap();
        let mut ctx = Context::new();
        ctx.func = ir::Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
        let mut bc = FunctionBuilderContext::new();
        let b = FunctionBuilder::new(&mut ctx.func, &mut bc);
        let mut f = FnEmitter::new(&mut em, &unit, b, sig);
        let bound = f.bind_receipt_entry();
        if malformed != 0 {
            assert!(bound.is_err());
            assert!(em.manifested.imports.is_empty());
            return vec![];
        }
        bound.unwrap();
        let borrowed = f.load_ptr(f.slots[0].unwrap().ptr);
        let taken = f.load_ptr(f.slots[1].unwrap().ptr);
        let left = f.b.ins().icmp(IntCC::Equal, borrowed, f.entry_params[4]);
        let right = f.b.ins().icmp(IntCC::Equal, taken, f.entry_params[5]);
        let correct = f.b.ins().band(left, right);
        let ptr = Shape::Scalar(ScalarKind::Pointer);
        f.store(ptr, V::One(taken), f.slots[2].unwrap()).unwrap();
        if publish {
            f.store(ptr, V::One(borrowed), f.slots[3].unwrap()).unwrap();
        }
        let zero = f.b.ins().iconst(types::I8, 0);
        let one = f.b.ins().iconst(types::I8, 1);
        let status = f.b.ins().select(correct, zero, one);
        f.b.ins().return_(&[status]);
        f.b.seal_all_blocks();
        f.b.finalize(f.em.module.target_config());
        em.define(name, id, &mut ctx).unwrap();
    }
    assert!(em.manifested.imports.is_empty());
    em.module.finish().emit().unwrap()
}

#[test]
fn native_receipt_entry_preserves_input_handles_mut_aliasing_and_unwritten_result_on_both_targets()
{
    for target in TargetSpec::executed() {
        execute(
            &target,
            probe(&target, 0),
            "cranelift-receipt-entry",
            HARNESS,
        );
    }
}

#[test]
fn native_receipt_entry_rejects_missing_bindings_and_owning_version_claims() {
    for mutation in [1, 2] {
        probe(&TargetSpec::x86_64_unknown_linux_gnu(), mutation);
    }
}
