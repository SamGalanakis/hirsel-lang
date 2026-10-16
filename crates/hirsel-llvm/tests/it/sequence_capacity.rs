//! Capacity loads agree with the runtime across allocation classes and kinds.
use crate::support;

use hirsel_lir::layout::sequence_elements_offset;
use hirsel_lir::layout_types::{LayoutKind, SizeSpec, TargetSpec};
use hirsel_lir::{descriptor_registry::DescriptorRegistry, ir::*};
use hirsel_llvm::{EmitInputs, OptLevel, emit_object};
use hirsel_syntax::PassingMode;
use hirsel_types::Primitive;
use support::{native::*, programs::Program, *};

fn program(target: &TargetSpec) -> Program {
    let mut schema = Schema::new();
    let word = schema.types.p(Primitive::U64);
    let wide = schema.types.p(Primitive::U128);
    let unit = schema.types.unit();
    for (name, element) in [("word", word), ("wide", wide), ("zero", unit)] {
        let list = schema.types.list(element);
        schema.name(name, list);
        schema.signature(name, &[(PassingMode::Mut, list)], word);
    }
    let table = schema.build(target);
    let mut units = Vec::new();
    let manifest = manifest();
    for name in ["word", "wide", "zero"] {
        let list = table.layout(name);
        let mut u = Unit::new(&table, name, name);
        let this = u.param(list);
        let slot = u.slot(list, SlotKind::Parameter(0));
        let address = u.address(AddressBase::Slot(slot), &[], list);
        u.prepare(ENTRY, address, None, SourceAnchorId(0));
        u.lir.tables.loans.push(LoanInfo {
            address,
            mutable: false,
        });
        let loan = LoanId(0);
        u.push(
            ENTRY,
            Inst::BeginLoan {
                loan,
                address,
                mutable: false,
                if_inactive: false,
            },
        );
        let view = u.value(list, Holding::Borrowed(loan));
        u.push(
            ENTRY,
            Inst::Read {
                result: view,
                address,
                reaching: this,
                mode: ReadMode::Borrow,
            },
        );
        let result = u.plain(table.scalar(hirsel_lir::layout_types::ScalarKind::U64));
        u.push(
            ENTRY,
            Inst::SeqCapacity {
                result,
                value: view,
            },
        );
        u.push(
            ENTRY,
            Inst::EndLoan {
                loan,
                if_active: false,
            },
        );
        u.ret(ENTRY, Some(result));
        units.push(seal(&table, &manifest, u.finish()));
    }
    Program {
        table,
        manifest,
        units,
        statics: Vec::new(),
        finalizers: Default::default(),
    }
}

fn object(program: &Program, opt: OptLevel) -> Vec<u8> {
    let input = input();
    let failures = failure_codes();
    let inputs = EmitInputs {
        input: &input,
        target: &program.table.target,
        layouts: &program.table.table,
        manifest: &program.manifest,
        failures: &failures,
        statics: &program.statics,
        finalizers: &program.finalizers,
        inline_instances: &std::collections::BTreeSet::new(),
        available_externally: &std::collections::BTreeSet::new(),
        registry: DescriptorRegistry::new(&program.table.table, &program.finalizers),
        opt,
        measurement: false,
    };
    let artifact = emit_object(&program.units(), &inputs).unwrap();
    assert!(
        !artifact
            .imports
            .iter()
            .any(|i| i.name == "hirsel_rt_v2_capacity")
    );
    artifact.bytes
}

#[test]
fn capacity_has_no_runtime_call_on_either_target() {
    for target in TargetSpec::executed() {
        let program = program(&target);
        for opt in [OptLevel::None, OptLevel::Speed] {
            object(&program, opt);
        }
    }
}

#[test]
#[ignore = "native: focused capacity controls on both targets"]
fn metadata_capacity_matches_runtime_and_full_length_range() {
    for target in TargetSpec::executed() {
        let program = program(&target);
        let registry = DescriptorRegistry::new(&program.table.table, &program.finalizers);
        let names = ["word", "wide", "zero"];
        let table = &program.table.table;
        // Each list's descriptor index, first-element offset and stride, from its layout.
        let sequences = names.map(|name| {
            let LayoutKind::Block { payload } =
                table.layout(program.table.layout(name)).unwrap().kind
            else {
                panic!("a list block")
            };
            let LayoutKind::Sequence { element } = table.layout(payload).unwrap().kind else {
                panic!("a counted sequence")
            };
            let element = table.layout(element).unwrap();
            let SizeSpec::Fixed(stride) = element.size else {
                panic!("a fixed element stride")
            };
            (
                registry.index(payload).unwrap(),
                sequence_elements_offset(element).unwrap(),
                stride,
            )
        });
        let column = |f: fn(&(u32, u64, u64)) -> String| {
            sequences.iter().map(f).collect::<Vec<_>>().join(", ")
        };
        let (descriptors, starts, strides) = (
            column(|s| s.0.to_string()),
            column(|s| s.1.to_string()),
            column(|s| s.2.to_string()),
        );
        let calls: String = names
            .iter()
            .map(|name| program.c_call(name, &format!("call_{name}")))
            .collect();
        let c = format!(
            r#"
#include <stdint.h>
#include <stdio.h>
{calls}
int main(void) {{
  hirsel_rt_v2_config config = {{.abi_version = HIRSEL_RT_V2_ABI_VERSION}};
  hirsel_rt_v2_ctx *ctx = NULL;
  assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
  uint32_t descriptors[] = {{{descriptors}}};
  uint8_t (*queries[])(hirsel_rt_v2_ctx *, void *, void *) = {{call_word, call_wide, call_zero}};
  uint64_t strides[] = {{{strides}}}, starts[] = {{{starts}}};
  uint64_t extents[] = {{16, 32, 48, 64, 256, 320, 1024, 4096, 4104, 65536}};
  for (unsigned q = 0; q < 3; q++) for (unsigned a = 0; a < 3; a++) for (unsigned n = 0; n < sizeof extents / sizeof *extents; n++) {{
    hirsel_rt_v2_header *root = hirsel_rt_v2_alloc(ctx, descriptors[q], extents[n], 8u << a, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
    uint64_t bytes = hirsel_rt_v2_capacity(root), length = 1;
    *(uint64_t *)hirsel_rt_v2_payload(root) = length;
    uint64_t result = 0;
    assert(queries[q](ctx, &result, &root) == 0);
    uint64_t room = bytes > starts[q] ? bytes - starts[q] : 0;
    uint64_t expected = strides[q] ? room / strides[q] : UINT64_MAX;
    if (expected < length) expected = length;
    assert(result == expected);
    /* No elements are owned: capacity only observes metadata and the count. */
    *(uint64_t *)hirsel_rt_v2_payload(root) = 0;
    assert(hirsel_rt_v2_release(ctx, root, descriptors[q], NULL, 0) == 0);
  }}
  for (unsigned kind = 2; kind <= 3; kind++) for (unsigned q = 0; q < 3; q++) {{
    struct {{ uint64_t capacity, align; hirsel_rt_v2_header header; uint64_t length; }} storage = {{
      64, 16, {{UINT32_MAX, (kind << 29) | descriptors[q]}}, UINT64_MAX
    }};
    hirsel_rt_v2_header *root = &storage.header;
    uint64_t result = 0;
    assert(queries[q](ctx, &result, &root) == 0 && result == UINT64_MAX);
    storage.length = 2;
    assert(queries[q](ctx, &result, &root) == 0);
    assert(result == (strides[q] ? (64 - starts[q]) / strides[q] : UINT64_MAX));
  }}
  hirsel_rt_v2_stats stats;
  hirsel_rt_v2_ctx_stats(ctx, &stats);
  assert(stats.live_blocks == 0);
  hirsel_rt_v2_ctx_destroy(ctx);
  puts("inline capacity ok");
}}
"#
        );
        for opt in [OptLevel::None, OptLevel::Speed] {
            let dir = scratch(&format!("capacity-{}-{opt:?}", target.triple));
            let source = dir.join("capacity.c");
            std::fs::write(&source, &c).unwrap();
            let bytes = object(&program, opt);
            let executable = link(&target, &dir, &[("capacity.o", &bytes)], &[source]);
            assert_eq!(run(&target, &executable), "inline capacity ok\n");
        }
    }
}
