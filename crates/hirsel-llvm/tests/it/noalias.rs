//! `noalias` on `mut` place addresses (`docs/spec/implementation.md`, Aliasing facts):
//! the Ready probes' `bump(mut x)` states it on its address parameter, its by-value
//! parameters and runtime declarations state none, and the same program with a C
//! callback named states none anywhere.
use crate::support;

use std::collections::BTreeSet;

use hirsel_lir::emission::{EmitInputs, OptLevel};

const NO_CALLBACKS: BTreeSet<DefPath> = BTreeSet::new();
use hirsel_lir::ir::{SlotKind, UnitOrigin};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_llvm::emit_optimized_ir;
use hirsel_syntax::{DefPath, PassingMode};
use hirsel_types::{BuiltinType, GenericArgument, Primitive};
use support::programs::Program;
use support::ready::*;
use support::*;

fn ir(program: &Program, opt: OptLevel, callbacks: &BTreeSet<DefPath>) -> String {
    let input = input();
    let none = BTreeSet::new();
    let inputs = EmitInputs {
        input: &input,
        target: &program.table.target,
        layouts: &program.table.table,
        keys: &program.table.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &none,
        available_externally: &none,
        callbacks: !callbacks.is_empty(),
        tracking: false,
        opt,
        measurement: false,
        fuel: false,
    };
    emit_optimized_ir(&program.units(), &inputs)
        .unwrap_or_else(|e| panic!("{}: {e}", program.table.target.triple))
}

/// The parameter list of the definition whose symbol contains `name`, not its thunk.
fn parameters<'a>(ir: &'a str, name: &str) -> &'a str {
    let line = ir
        .lines()
        .find(|l| l.starts_with("define") && l.contains(name) && !l.contains("@_H1T"))
        .unwrap_or_else(|| panic!("no definition of {name}: {ir}"));
    let start = line.find('(').expect("a parameter list");
    &line[start..]
}

#[test]
fn a_mut_place_address_is_noalias_unless_the_program_names_a_callback() {
    let callback: BTreeSet<DefPath> = [DefPath::core("callback")].into();
    for target in TargetSpec::executed() {
        let program = ready_program(&target);
        for opt in [OptLevel::None, OptLevel::Speed] {
            let stated = ir(&program, opt, &NO_CALLBACKS);
            // bump(ctx, mut x): the context states nothing, the address is noalias.
            let bump = parameters(&stated, "4bump");
            assert_eq!(bump.matches("noalias").count(), 1, "{opt:?}: {bump}");
            for attribute in ["ptr noalias", "align 8", "dereferenceable(8)"] {
                assert!(bump.contains(attribute), "{opt:?}: {bump}");
            }
            // A by-value caller: no aliasing attribute.
            let caller = parameters(&stated, "11mut__caller");
            assert!(!caller.contains("noalias"), "{opt:?}: {caller}");
            // Runtime declarations state no parameter aliasing (an allocator's
            // result is the only `noalias` they carry).
            for declared in stated
                .lines()
                .filter(|l| l.starts_with("declare") && l.contains("@hirsel_rt_v2_"))
            {
                let (_, parameters) = declared.split_once('(').unwrap();
                assert!(!parameters.contains("noalias"), "{opt:?}: {declared}");
            }
            let unstated = ir(&program, opt, &callback);
            let bump = parameters(&unstated, "4bump");
            assert!(!bump.contains("noalias"), "{opt:?}: {bump}");
            if opt == OptLevel::None {
                assert!(!bump.contains("dereferenceable"), "{opt:?}: {bump}");
                assert!(!bump.contains("align "), "{opt:?}: {bump}");
            }
        }
    }
}

/// Each signature uses a real, independently validated layout and Ready unit.
/// The expected IR fragments pin extent and alignment separately from pointer width.
#[test]
fn mut_address_layouts_and_private_root_exclusions() {
    for target in TargetSpec::executed() {
        let mut schema = Schema::new();
        let unit = schema.types.unit();
        let byte = schema.types.p(Primitive::U8);
        let wide = schema.types.p(Primitive::U128);
        let word = schema.types.p(Primitive::U64);
        let aggregate = schema.types.tuple(&[word, word, word]);
        let choices = schema.types.variant(&[&[word, word], &[]]);
        let packed = schema
            .types
            .builtin(BuiltinType::Packed, vec![GenericArgument::Type(choices)]);
        let list = schema.types.list(word);
        let boxed = schema.types.boxed(word);
        let cases = [
            ("empty", unit, false),
            ("byte", byte, false),
            ("wide", wide, false),
            ("aggregate", aggregate, false),
            // The last variant is smaller than the first: do not pick tag zero or max.
            ("packed", packed, false),
            ("root_list", list, true),
            ("root_box", boxed, true),
        ];
        for &(name, ty, _) in &cases {
            schema.name(name, ty);
            schema.signature(name, &[(PassingMode::Mut, ty)], unit);
        }
        let table = schema.build(&target);
        let manifest = manifest();
        let mut units = Vec::new();
        for &(name, _, library) in &cases {
            let mut u = Unit::new(&table, name, name);
            let layout = table.layout(name);
            u.param(layout);
            u.slot(layout, SlotKind::Parameter(0));
            if library {
                u.builder.unit.header.origin = UnitOrigin::Library;
            }
            u.builder.ret(ENTRY, None);
            units.push(seal(&table, &manifest, u.finish()));
        }
        let program = Program {
            table,
            manifest,
            units,
            statics: Vec::new(),
            finalizers: Default::default(),
        };
        let text = ir(&program, OptLevel::None, &NO_CALLBACKS);
        for &(name, _, library) in &cases {
            let symbol = hirsel_lir::emission::symbol_name(&path(name), Some(&instance(name)));
            let signature = parameters(&text, &format!("@{symbol}("));
            let prefix = format!("declare i8 @{name}(ptr, ");
            let expected = include_str!("../ir/mut_addresses.ll")
                .lines()
                .find_map(|line| line.strip_prefix(&prefix))
                .unwrap()
                .strip_suffix(')')
                .unwrap();
            assert!(signature.contains(expected), "{name}: {signature}");
            assert_eq!(signature.matches("noalias").count(), 1, "{signature}");
            if library {
                let private = parameters(&text, &format!("@{symbol}.roots9("));
                for forbidden in ["noalias", "dereferenceable", "align "] {
                    assert!(!private.contains(forbidden), "{private}");
                }
            }
        }
    }
}

/// A caller forwarding a packed field to an ordinary mut-word entry. The leaf
/// writes first, then optionally fails: both continuations must see the write.
fn packed_program(target: &TargetSpec, scalar: hirsel_lir::layout_types::ScalarKind) -> Program {
    use hirsel_core_contract::Passing;
    use hirsel_lir::ir::*;
    let mut schema = Schema::new();
    let primitive = support::programs::primitive(scalar);
    let definition = path("Wire");
    let mut t = crate::support::session_types();
    let byte = t.p(Primitive::U8);
    let field = t.p(primitive);
    let shape = t.record(&[byte, field, byte]);
    schema.owners.nominals.insert(
        hirsel_syntax::DefId::intern(&definition.clone()),
        hirsel_layout::layout::OwnerRepresentation::Defined(hirsel_tir::RepresentationRecord {
            definition: definition.clone(),
            representation_revision: hash("packed mut wire"),
            generics: Vec::new(),
            private_shape: shape,
            layout_promises: hirsel_types::Layout {
                packed: true,
                ..Default::default()
            },
            resource: false,
            finalizer: None,
            unsafe_ffi_package: false,
        }),
    );
    let wire = schema.types.named(&definition, Vec::new());
    let word = schema.types.p(primitive);
    let unit = schema.types.unit();
    schema.name("wire", wire);
    schema.name("word", word);
    for (name, ty) in [("packed_caller", wire), ("write_then_fail", word)] {
        schema.signature(
            name,
            &[(PassingMode::Mut, ty), (PassingMode::Borrow, word)],
            unit,
        );
    }
    let table = schema.build(target);
    let manifest = manifest();
    let word = table.layout("word");
    let mut leaf = Unit::new(&table, "write_then_fail", "write_then_fail");
    leaf.param(word);
    let denominator = leaf.param(word);
    let slot = leaf.slot(word, SlotKind::Parameter(0));
    let at = leaf.address(AddressBase::Slot(slot), &[], word);
    leaf.prepare(ENTRY, at, None, SourceAnchorId(0));
    let value = leaf.int(ENTRY, scalar, 42);
    let displaced = leaf.slot(word, SlotKind::Displaced);
    leaf.builder
        .push(ENTRY, Inst::StorageLive { slot: displaced });
    let version = leaf.version(slot);
    let displaced_version = leaf.version(displaced);
    let failure = leaf.fails(None);
    leaf.builder.push(
        ENTRY,
        Inst::Replace {
            address: at,
            value,
            displaced,
            version,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    leaf.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: displaced,
            if_live: false,
        },
    );
    let one = leaf.int(ENTRY, scalar, 1);
    leaf.binary(ENTRY, BinaryOp::Div, one, denominator);
    leaf.builder.ret(ENTRY, None);
    let mut units = vec![seal(&table, &manifest, leaf.finish())];
    let mut caller = Unit::new(&table, "packed_caller", "packed_caller");
    let wire = table.layout("wire");
    caller.param(wire);
    let denominator = caller.param(word);
    let slot = caller.slot(wire, SlotKind::Parameter(0));
    let at = caller.address(AddressBase::Slot(slot), &[ProjectionStep::Field(1)], word);
    caller.prepare(ENTRY, at, None, SourceAnchorId(0));
    let version = caller.version(slot);
    let symbol = caller.symbol(SymbolKind::Function, "write_then_fail");
    let normal = caller.builder.block(BlockKind::Body, &[]);
    let failure = caller.fails(None);
    caller.call(
        ENTRY,
        Callee::Direct(symbol),
        "write_then_fail",
        &[
            (
                Passing::Mut,
                ArgumentValue::Address {
                    address: at,
                    version,
                },
            ),
            (Passing::Borrow, ArgumentValue::Value(denominator)),
        ],
        normal,
        &[],
        failure,
    );
    caller.builder.ret(normal, None);
    units.push(seal(&table, &manifest, caller.finish()));
    Program {
        table,
        manifest,
        units,
        statics: Vec::new(),
        finalizers: Default::default(),
    }
}

#[test]
#[ignore = "native: all packed-word offsets and both status outcomes on both targets at O0/O3"]
fn packed_mut_call_storage_is_aligned_and_writes_back_on_failure() {
    use hirsel_lir::layout_types::ScalarKind;
    use support::native::*;
    for (scalar, c_type, bytes) in [
        (ScalarKind::U16, "uint16_t", 2),
        (ScalarKind::U32, "uint32_t", 4),
        (ScalarKind::U64, "uint64_t", 8),
        (ScalarKind::U128, "unsigned __int128", 16),
    ] {
        for target in TargetSpec::executed() {
            let mut program = packed_program(&target, scalar);
            let binding = program.c_call("packed_caller", "call_packed");
            // Keep the leaf opaque to LLVM and check the address in independently
            // compiled C. Native unaligned loads alone can pass even under UB.
            let symbol = hirsel_lir::emission::symbol_name(
                &path("write_then_fail"),
                Some(&instance("write_then_fail")),
            );
            let leaf = hirsel_llvm::c_definition(
                &program.table.table,
                program.abi_of("write_then_fail"),
                &symbol,
                "packed_leaf",
            );
            program.units.remove(0);
            let source = format!(
                r#"
#include <stdint.h>
#include <string.h>
#include <stdio.h>
static uint8_t packed_leaf(hirsel_rt_v2_ctx *ctx, void *address, {c_type} denominator) {{
  (void)ctx;
  assert((uintptr_t)address % {bytes} == 0);
  {c_type} value = 42;
  memcpy(address, &value, {bytes});
  return denominator == 0;
}}
{leaf}
{binding}
int main(void) {{
  hirsel_rt_v2_config config = {{.abi_version = HIRSEL_RT_V2_ABI_VERSION}};
  hirsel_rt_v2_ctx *ctx = NULL;
  assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
  for (unsigned offset = 0; offset < {bytes}; offset++) for (unsigned fail = 0; fail < 2; fail++) {{
    _Alignas(16) uint8_t bytes[64];
    memset(bytes, 0xa5, sizeof bytes);
    {c_type} value = 41;
    memcpy(bytes + offset + 1, &value, {bytes});
    assert(call_packed(ctx, bytes + offset, fail ? 0 : 1) == fail);
    memcpy(&value, bytes + offset + 1, {bytes});
    assert(value == 42);
    for (unsigned i = 0; i < sizeof bytes; i++)
      if (i < offset + 1 || i >= offset + 1 + {bytes}) assert(bytes[i] == 0xa5);
    hirsel_rt_v2_failure_clear(ctx);
  }}
  hirsel_rt_v2_ctx_destroy(ctx);
  puts("packed mut write-back ok");
}}
"#
            );
            for opt in [OptLevel::None, OptLevel::Speed] {
                let dir = scratch(&format!("packed-mut-{}-{scalar:?}-{opt:?}", target.triple));
                let c = dir.join("packed.c");
                std::fs::write(&c, &source).unwrap();
                let text = ir(&program, opt, &NO_CALLBACKS);
                let object = hirsel_llvm::compile_ir(&text, &target, OptLevel::None).unwrap();
                let executable = link(
                    &target,
                    &dir,
                    &[
                        ("packed.o", &object.bytes),
                        ("program-glue.o", &program.glue()),
                    ],
                    &[c],
                );
                assert_eq!(run(&target, &executable), "packed mut write-back ok\n");
            }
        }
    }
}
