//! A sequence capacity is its header word (runtime ABI 5), which the runtime leaves exact.
use crate::support;

use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::layout::sequence_elements_offset;
use hirsel_lir::layout_types::{LayoutKind, SizeSpec, TargetSpec};
use hirsel_lir::{descriptor_registry::DescriptorRegistry, ir::*};
use hirsel_llvm::emit_object;
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
        u.builder.unit.tables.loans.push(LoanInfo {
            address,
            mutable: false,
        });
        let loan = LoanId(0);
        u.builder.push(
            ENTRY,
            Inst::BeginLoan {
                loan,
                address,
                mutable: false,
                if_inactive: false,
            },
        );
        let view = u.value(list, Holding::Borrowed(loan));
        u.builder.push(
            ENTRY,
            Inst::Read {
                result: view,
                address,
                reaching: this,
                mode: ReadMode::Borrow,
            },
        );
        let result = u.plain(table.scalar(hirsel_lir::layout_types::ScalarKind::U64));
        u.builder.push(
            ENTRY,
            Inst::SeqCapacity {
                result,
                value: view,
            },
        );
        u.builder.push(
            ENTRY,
            Inst::EndLoan {
                loan,
                if_active: false,
            },
        );
        u.builder.ret(ENTRY, Some(result));
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
    let inputs = EmitInputs {
        input: &input,
        target: &program.table.target,
        layouts: &program.table.table,
        keys: &program.table.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &std::collections::BTreeSet::new(),
        available_externally: &std::collections::BTreeSet::new(),
        callbacks: false,
        tracking: false,
        opt,
        measurement: false,
        fuel: false,
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
fn capacity_is_the_header_word_that_seq_grow_leaves_exact() {
    for target in TargetSpec::executed() {
        let program = program(&target);
        let registry = DescriptorRegistry::new(&program.table.table, &program.finalizers);
        let names = ["word", "wide", "zero"];
        let table = &program.table.table;
        // Each list's descriptor index, first-element offset, stride and element alignment,
        // from its layout.
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
                u64::from(element.align),
            )
        });
        let column = |f: fn(&(u32, u64, u64, u64)) -> String| {
            sequences.iter().map(f).collect::<Vec<_>>().join(", ")
        };
        let (descriptors, starts, strides, aligns) = (
            column(|s| s.0.to_string()),
            column(|s| s.1.to_string()),
            column(|s| s.2.to_string()),
            column(|s| s.3.to_string()),
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
  uint64_t strides[] = {{{strides}}}, starts[] = {{{starts}}}, aligns[] = {{{aligns}}};
  uint64_t extents[] = {{16, 32, 48, 64, 256, 320, 1024, 4096, 4104, 65536}};
  for (unsigned q = 0; q < 3; q++) for (unsigned a = 0; a < 3; a++) for (unsigned n = 0; n < sizeof extents / sizeof *extents; n++) {{
    hirsel_rt_v2_header *root = hirsel_rt_v2_alloc(ctx, descriptors[q], extents[n], 8u << a, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
    uint64_t bytes = hirsel_rt_v2_capacity(root);
    uint64_t *words = hirsel_rt_v2_payload(root);
    words[0] = 0;
    /* A reserve the block already holds leaves it alone with its capacity word exact. */
    assert(hirsel_rt_v2_seq_grow(ctx, root, strides[q], aligns[q], 0) == root);
    uint64_t room = bytes > starts[q] ? bytes - starts[q] : 0;
    uint64_t exact = strides[q] ? room / strides[q] : UINT64_MAX;
    uint64_t result = 0;
    assert(queries[q](ctx, &result, &root) == 0 && result == exact);
    /* The query is the word itself, whatever the metadata holds. */
    if (strides[q]) {{
      words[1] = exact / 2;
      assert(queries[q](ctx, &result, &root) == 0 && result == exact / 2);
    }}
    assert(hirsel_rt_v2_release(ctx, root, descriptors[q], NULL, 0, 0) == 0);
  }}
  hirsel_rt_v2_stats stats;
  hirsel_rt_v2_ctx_stats(ctx, &stats);
  assert(stats.live_blocks == 0);
  hirsel_rt_v2_ctx_destroy(ctx);
  puts("header capacity ok");
}}
"#
        );
        for opt in [OptLevel::None, OptLevel::Speed] {
            let dir = scratch(&format!("capacity-{}-{opt:?}", target.triple));
            let source = dir.join("capacity.c");
            std::fs::write(&source, &c).unwrap();
            let bytes = object(&program, opt);
            let executable = link(
                &target,
                &dir,
                &[("capacity.o", &bytes), ("program-glue.o", &program.glue())],
                &[source],
            );
            assert_eq!(run(&target, &executable), "header capacity ok\n");
        }
    }
}
