//! Root hand-back after growth followed by a language failure. The public ABI
//! adapter and a direct internal call must both publish the grown pointer.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::{descriptor_registry::DescriptorRegistry, ir::*, layout_types::ScalarKind};
use hirsel_llvm::emit_object;
use support::{native::*, programs::Program, ready::ready_program, *};

fn program(target: &hirsel_lir::layout_types::TargetSpec) -> Program {
    let mut program = ready_program(target);
    let table = &program.table;
    let list = table.layout("list64");
    let integer = table.scalar(ScalarKind::I64);
    {
        let mut leaf = Unit::new(table, "reserve_then_fail", "push list64");
        leaf.builder.unit.header.origin = UnitOrigin::Library;
        leaf.param(list);
        let x = leaf.param(integer);
        let slot = leaf.slot(list, SlotKind::Parameter(0));
        let address = leaf.address(AddressBase::Slot(slot), &[], list);
        leaf.prepare(ENTRY, address, None, SourceAnchorId(0));
        let loan = LoanId(leaf.builder.unit.tables.loans.len() as u32);
        leaf.builder.unit.tables.loans.push(LoanInfo {
            address,
            mutable: false,
        });
        leaf.builder.push(
            ENTRY,
            Inst::BeginLoan {
                loan,
                address,
                mutable: false,
                if_inactive: false,
            },
        );
        let view = leaf.value(list, Holding::Borrowed(loan));
        leaf.builder.push(
            ENTRY,
            Inst::Read {
                result: view,
                address,
                reaching: leaf.builder.unit.parameters[0],
                mode: ReadMode::Borrow,
            },
        );
        let before = leaf.plain(table.scalar(ScalarKind::U64));
        leaf.builder.push(
            ENTRY,
            Inst::SeqCapacity {
                result: before,
                value: view,
            },
        );
        leaf.builder.push(
            ENTRY,
            Inst::EndLoan {
                loan,
                if_active: false,
            },
        );
        let capacity = leaf.int(ENTRY, ScalarKind::U64, 1024);
        let version = leaf.version(slot);
        leaf.builder.push(
            ENTRY,
            Inst::SeqReserve {
                address,
                capacity,
                version,
            },
        );
        let loan = LoanId(leaf.builder.unit.tables.loans.len() as u32);
        leaf.builder.unit.tables.loans.push(LoanInfo {
            address,
            mutable: false,
        });
        leaf.builder.push(
            ENTRY,
            Inst::BeginLoan {
                loan,
                address,
                mutable: false,
                if_inactive: false,
            },
        );
        let view = leaf.value(list, Holding::Borrowed(loan));
        leaf.builder.push(
            ENTRY,
            Inst::Read {
                result: view,
                address,
                reaching: version,
                mode: ReadMode::Borrow,
            },
        );
        let after = leaf.plain(table.scalar(ScalarKind::U64));
        leaf.builder.push(
            ENTRY,
            Inst::SeqCapacity {
                result: after,
                value: view,
            },
        );
        leaf.builder.push(
            ENTRY,
            Inst::EndLoan {
                loan,
                if_active: false,
            },
        );
        let enough = leaf.binary(ENTRY, BinaryOp::Ge, after, capacity);
        let preserved = leaf.binary(ENTRY, BinaryOp::Ge, after, before);
        let enough = leaf.binary(ENTRY, BinaryOp::BitAnd, enough, preserved);
        let failure = leaf.fails(Some("MEM-14"));
        leaf.builder.push(
            ENTRY,
            Inst::Check {
                condition: enough,
                failure,
            },
        );
        let one = leaf.int(ENTRY, ScalarKind::I64, 1);
        leaf.binary(ENTRY, BinaryOp::Add, x, one);
        leaf.builder.ret(ENTRY, None);
        program
            .units
            .push(seal(table, &program.manifest, leaf.finish()));
    }

    let mut caller = Unit::new(table, "call_reserve_then_fail", "push list64");
    caller.param(list);
    let x = caller.param(integer);
    let slot = caller.slot(list, SlotKind::Parameter(0));
    let address = caller.address(AddressBase::Slot(slot), &[], list);
    caller.prepare(ENTRY, address, None, SourceAnchorId(0));
    let version = caller.version(slot);
    let symbol = caller.symbol(SymbolKind::Function, "reserve_then_fail");
    let normal = caller.builder.block(BlockKind::Body, &[]);
    let failure = caller.fails(None);
    caller.call(
        ENTRY,
        Callee::Direct(symbol),
        "push list64",
        &[
            (Passing::Mut, ArgumentValue::Address { address, version }),
            (Passing::Borrow, ArgumentValue::Value(x)),
        ],
        normal,
        &[],
        failure,
    );
    caller.builder.ret(normal, None);
    program
        .units
        .push(seal(table, &program.manifest, caller.finish()));
    program
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
        callbacks: &hirsel_lir::emission::NO_CALLBACKS,
        registry: DescriptorRegistry::new(&program.table.table, &program.finalizers),
        opt,
        measurement: false,
    };
    emit_object(&program.units(), &inputs).unwrap().bytes
}

#[test]
#[ignore = "native: run the focused root_ssa matrix"]
fn growth_roots_are_handed_back_before_both_status_outcomes() {
    for target in hirsel_lir::layout_types::TargetSpec::executed() {
        let program = program(&target);
        let registry = DescriptorRegistry::new(&program.table.table, &program.finalizers);
        let list = program.table.layout("list64");
        let hirsel_lir::layout_types::LayoutKind::Block { payload } =
            program.table.table.layout(list).unwrap().kind
        else {
            panic!("sequence block")
        };
        let index = registry.index(payload).unwrap();
        let hirsel_lir::layout_types::LayoutKind::Sequence { element } =
            program.table.table.layout(payload).unwrap().kind
        else {
            panic!("a counted sequence")
        };
        // The one element's word, past the count and capacity (runtime ABI 5).
        let first = hirsel_lir::layout::sequence_elements_offset(
            program.table.table.layout(element).unwrap(),
        )
        .unwrap();
        let (extent, word) = (first + 8, first / 8);
        let caller = program.c_call("call_reserve_then_fail", "call_caller");
        let c = format!(
            r#"
#include <stdint.h>
#include <stdio.h>
{caller}
int main(void) {{
  hirsel_rt_v2_config config = {{.abi_version = HIRSEL_RT_V2_ABI_VERSION, .flags = HIRSEL_RT_V2_CONFIG_DIAGNOSTIC}};
  hirsel_rt_v2_ctx *ctx = NULL;
  assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
  uint8_t (*entries[])(hirsel_rt_v2_ctx *, void *, int64_t) = {{call_caller}};
  for (unsigned i = 0; i < 1; i++) for (unsigned fail = 0; fail < 2; fail++) {{
    hirsel_rt_v2_header *root = hirsel_rt_v2_alloc(ctx, {index}, {extent}, 8, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
    uint64_t *data = hirsel_rt_v2_payload(root);
    data[0] = 1; data[1] = 1; data[{word}] = 42;
    hirsel_rt_v2_header *old = root;
    assert(entries[i](ctx, &root, fail ? INT64_MAX : 0) == fail);
    assert(root != old && hirsel_rt_v2_capacity(root) >= 8200);
    data = hirsel_rt_v2_payload(root);
    assert(root->count == 1 && data[0] == 1 && data[{word}] == 42);
    hirsel_rt_v2_failure_clear(ctx);
    assert(hirsel_rt_v2_release(ctx, root, {index}, NULL, 0, 0) == 0);
  }}
  hirsel_rt_v2_stats stats;
  hirsel_rt_v2_ctx_stats(ctx, &stats);
  assert(stats.live_blocks == 0);
  hirsel_rt_v2_ctx_destroy(ctx);
  puts("root hand-back ok");
}}
"#
        );
        for opt in [OptLevel::None, OptLevel::Speed] {
            let dir = scratch(&format!("root-ssa-{}-{opt:?}", target.triple));
            let source = dir.join("root.c");
            std::fs::write(&source, &c).unwrap();
            let bytes = object(&program, opt);
            let executable = link(&target, &dir, &[("root.o", &bytes)], &[source]);
            assert_eq!(run(&target, &executable), "root hand-back ok\n");
        }
    }
}
