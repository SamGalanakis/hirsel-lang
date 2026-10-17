//! Source-free controls for persistent environment live bits, checked by C and H
//! and emitted on both targets. The caller retains and releases its callable.
use crate::support;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::{CallMultiplicity, CallableShape};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::Ready;
use hirsel_types::Multiplicity;
use support::programs::Program;
use support::*;

fn schema() -> Schema {
    let mut s = Schema::new();
    let unit = s.types.unit();
    let number = s.types.p(hirsel_types::Primitive::U64);
    s.name("number", number);
    let block = s.types.boxed(number);
    s.name("block", block);
    let many = s
        .types
        .function(&[], unit, hirsel_types::Multiplicity::Many, 0);
    s.name("many", many);
    let mut once = s.types.store.get(many).unwrap().clone();
    once.modes.multiplicity = Multiplicity::Once;
    let once = s.types.store.push(once);
    s.name("once", once);
    s.environment("leaf_env", "leaf", &[(block, Passing::Take)]);
    s.shaped("many_leaf", CallableShape::Closure, &[], unit);
    s.movable_environment(
        "outer_env",
        "outer",
        &[(once, Passing::Take), (once, Passing::Take)],
        &[0],
    );
    s.signature("main", &[], unit);
    let _leaf = s.shaped("leaf", CallableShape::Closure, &[], unit);
    let _outer = s.shaped("outer", CallableShape::Closure, &[], unit);
    s
}

fn closure(
    u: &mut Unit<'_>,
    env: &str,
    code: &str,
    layout: &str,
    captures: &[ValueId],
    pending: &[(ValueId, hirsel_lir::layout_types::LayoutId)],
) -> ValueId {
    let symbol = u.symbol(SymbolKind::Function, code);
    let captures = PoolRange::push(
        &mut u.builder.unit.pools.captures,
        captures.iter().map(|&value| CaptureValue {
            value,
            usage: CaptureUse::Take,
        }),
    );
    let id = ClosureId(u.builder.unit.tables.closures.len() as u32);
    u.builder.unit.tables.closures.push(ClosureSite {
        code: Some(ClosureCode {
            symbol,
            abi: u.table.abi(code),
        }),
        environment: u.table.layout(env),
        captures,
        destination: DestinationClass::Heap,
        placement: Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
        anchor: SourceAnchorId(0),
    });
    let result = u.value(u.table.layout(layout), Holding::Owned);
    let failure = u.releasing(pending, None, SourceAnchorId(0));
    u.builder.push(
        ENTRY,
        Inst::Closure {
            result,
            closure: id,
            failure,
        },
    );
    result
}

fn program(target: &TargetSpec, called: bool, weakened: bool) -> Program {
    let mut table = schema().build(target);
    for name in ["leaf", "outer"] {
        let abi = table.abi(name);
        table.table.fn_abis[abi.0 as usize].multiplicity = CallMultiplicity::Once;
    }
    let many = table.abi("many_leaf");
    let mut abi = table.table.fn_abis[many.0 as usize].clone();
    abi.multiplicity = CallMultiplicity::Many;
    let id = hirsel_lir::abi_types::FnAbiId(table.table.fn_abis.len() as u32);
    table.table.fn_abis.push(abi);
    table.abis.insert("many_leaf", id);
    let m = manifest();
    let mut leaf = Unit::new(&table, "leaf", "leaf");
    leaf.builder.ret(ENTRY, None);
    let leaf = seal(&table, &m, leaf.finish());
    let mut many_leaf = Unit::new(&table, "many_leaf", "many_leaf");
    many_leaf.builder.ret(ENTRY, None);
    let many_leaf = seal(&table, &m, many_leaf.finish());
    let mut body = Unit::new(&table, "outer", "outer");
    for i in 0..2 {
        let slot = body.slot(table.layout("once"), SlotKind::Capture(i));
        let version = body.version(slot);
        body.builder.unit.captures.push(version);
    }
    let word_layout = table.table.layouts[table.layout("outer_env").0 as usize]
        .fields
        .last()
        .unwrap()
        .layout;
    let flags = body.slot(word_layout, SlotKind::Capture(2));
    let flag_version = body.version(flags);
    body.builder.unit.captures.push(flag_version);
    body.builder.unit.tables.init_flags.push(InitFlag {
        path: PoolRange::EMPTY,
        value: flag_version,
        slot: flags,
        environment: Some(table.layout("outer_env")),
    });
    let at = body.address(AddressBase::Slot(SlotId(0)), &[], table.layout("once"));
    let word = body.address(AddressBase::Slot(flags), &[], word_layout);
    body.prepare(ENTRY, at, None, SourceAnchorId(0));
    body.prepare(ENTRY, word, None, SourceAnchorId(0));
    let moved = body.value(table.layout("once"), Holding::Owned);
    body.builder.push(
        ENTRY,
        Inst::Read {
            result: moved,
            address: at,
            reaching: body.builder.unit.captures[0],
            mode: ReadMode::Take,
        },
    );
    let old = body.read(ENTRY, word, flag_version);
    let next = body.plain(word_layout);
    body.builder.push(
        ENTRY,
        Inst::UpdateMask {
            result: next,
            mask: old,
            field: 0,
            initialized: false,
        },
    );
    body.store(ENTRY, word, next, flags);
    body.release(ENTRY, moved, table.layout("once"));
    body.builder.ret(ENTRY, None);
    let outer = seal(&table, &m, body.finish());

    let mut main = Unit::new(&table, "main", "main");
    let value = if weakened {
        let number = main.int(ENTRY, ScalarKind::U64, 33);
        let block = main.construct_block(
            ENTRY,
            ConstructorKind::Box,
            table.layout("number"),
            table.layout("block"),
            &[(0, number)],
        );
        let many = closure(&mut main, "leaf_env", "many_leaf", "many", &[block], &[]);
        let once = main.value(table.layout("once"), Holding::Owned);
        main.builder.push(
            ENTRY,
            Inst::Weaken {
                result: once,
                value: many,
            },
        );
        once
    } else {
        let mut captures = Vec::new();
        for n in [11, 22] {
            let value = main.int(ENTRY, ScalarKind::U64, n);
            let pending: Vec<_> = captures
                .iter()
                .map(|&v| (v, table.layout("once")))
                .collect();
            let failure = main.releasing(&pending, None, SourceAnchorId(0));
            let block = main.construct_block_to(
                ENTRY,
                ConstructorKind::Box,
                table.layout("number"),
                table.layout("block"),
                &[(0, value)],
                failure,
                None,
            );
            captures.push(closure(
                &mut main,
                "leaf_env",
                "leaf",
                "once",
                &[block],
                &pending,
            ));
        }
        closure(&mut main, "outer_env", "outer", "once", &captures, &[])
    };
    let after = if called {
        let after = main.builder.block(BlockKind::Body, &[]);
        let failure = main.releasing(&[(value, table.layout("once"))], None, SourceAnchorId(0));
        main.call(
            ENTRY,
            Callee::Indirect(value),
            if weakened { "leaf" } else { "outer" },
            &[],
            after,
            &[],
            failure,
        );
        after
    } else {
        ENTRY
    };
    main.release(after, value, table.layout("once"));
    main.builder.ret(after, None);
    let main = seal(&table, &m, main.finish());
    Program {
        table,
        manifest: m,
        units: vec![main, leaf, many_leaf, outer],
        statics: vec![],
        finalizers: Default::default(),
    }
}

fn heap_program(p: &Program) -> hirsel_heapcheck::Program<Ready> {
    hirsel_heapcheck::Program::<Ready> {
        target: p.table.target.clone(),
        layouts: p.table.table.clone(),
        units: p.units.iter().map(|u| u.unit().clone()).collect(),
        statics: vec![],
    }
}

fn heap(p: &Program) -> hirsel_heapcheck::Run {
    hirsel_heapcheck::run(
        &heap_program(p),
        &path("main"),
        &[],
        &mut hirsel_heapcheck::NoOperations,
        &Default::default(),
    )
}

#[test]
fn a_called_once_environment_drops_only_its_remaining_capture() {
    let p = program(&TargetSpec::x86_64_unknown_linux_gnu(), true, false);
    let run = heap(&p);
    assert_eq!(run.result.as_ref().unwrap().observation.failures.len(), 0);
    assert_eq!((run.counters.allocations, run.counters.frees), (5, 5));
    let allocations: Vec<_> = run
        .trace
        .iter()
        .filter_map(|event| {
            if let hirsel_heapcheck::Event::Alloc {
                alloc,
                owner: "heap",
                ..
            } = event
            {
                Some(*alloc)
            } else {
                None
            }
        })
        .collect();
    let released = |id| {
        run.trace.iter().position(|event| matches!(event, hirsel_heapcheck::Event::Release { alloc, .. } if *alloc == id)).unwrap()
    };
    assert!(released(allocations[1]) < released(allocations[4]));
    assert!(released(allocations[4]) < released(allocations[3]));
}

#[test]
fn an_uncalled_once_environment_drops_every_capture() {
    heap(&program(
        &TargetSpec::x86_64_unknown_linux_gnu(),
        false,
        false,
    ))
    .result
    .unwrap();
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn capture_masks_agree_on_both_native_targets() {
    use support::native::*;
    for target in TargetSpec::executed() {
        for (called, weakened) in [(false, false), (true, false), (true, true)] {
            let p = program(&target, called, weakened);
            heap(&p).result.unwrap();
            for opt in [
                hirsel_lir::emission::OptLevel::None,
                hirsel_lir::emission::OptLevel::Speed,
            ] {
                let codes = failure_codes();
                let id = input();
                let inputs = hirsel_lir::emission::EmitInputs {
                    input: &id,
                    target: &target,
                    layouts: &p.table.table,
                    manifest: &p.manifest,
                    failures: &codes,
                    statics: &[],
                    finalizers: &p.finalizers,
                    inline_instances: &std::collections::BTreeSet::new(),
                    available_externally: &std::collections::BTreeSet::new(),
                    registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                        &p.table.table,
                        &p.finalizers,
                    ),
                    opt,
                    measurement: false,
                };
                let object = hirsel_llvm::emit_object(&p.units(), &inputs).unwrap();
                let dir = scratch(&format!(
                    "oncecap-{}-{called}-{weakened}-{opt:?}",
                    target.triple
                ));
                let main = p.c_call("main", "call_main");
                let source = dir.join("main.c");
                std::fs::write(&source, format!(r#"#include "hirsel_rt_v2.h"
#include <stdio.h>
#include <string.h>
{main}
int main(void) {{ hirsel_rt_v2_config config; memset(&config, 0, sizeof(config)); config.abi_version = HIRSEL_RT_V2_ABI_VERSION; config.flags = HIRSEL_RT_V2_CONFIG_DIAGNOSTIC; hirsel_rt_v2_ctx *ctx; if(hirsel_rt_v2_ctx_create(&config, &ctx)) return 1; for(unsigned i=0;i<1000;i++) {{ if(call_main(ctx)) return 2; hirsel_rt_v2_stats stats; hirsel_rt_v2_ctx_stats(ctx, &stats); if(stats.live_blocks) return 3; }} hirsel_rt_v2_ctx_destroy(ctx); puts("ok"); return 0; }}"#)).unwrap();
                let executable = link(&target, &dir, &[("oncecap.o", &object.bytes)], &[source]);
                assert_eq!(run(&target, &executable), "ok\n");
            }
        }
    }
}

#[test]
fn a_many_closure_weakened_to_once_retains_caller_cleanup() {
    heap(&program(
        &TargetSpec::x86_64_unknown_linux_gnu(),
        true,
        true,
    ))
    .result
    .unwrap();
}

#[test]
fn live_bits_and_capture_cells_must_agree_in_both_directions() {
    use hirsel_heapcheck::ViolationKind;
    for missing_clear in [false, true] {
        let p = program(&TargetSpec::x86_64_unknown_linux_gnu(), true, false);
        heap(&p).result.unwrap();
        let mut raw = heap_program(&p);
        let body = raw
            .units
            .iter_mut()
            .find(|u| u.header.definition == path("outer"))
            .unwrap();
        let mut instructions = Vec::new();
        for block in &mut body.blocks {
            let old = block.instructions.get(&body.instructions).unwrap();
            block.instructions = PoolRange::push(
                &mut instructions,
                old.iter().copied().filter(|inst| {
                    if missing_clear {
                        !matches!(inst, Inst::Store { .. })
                    } else {
                        !matches!(
                            inst,
                            Inst::Read {
                                mode: ReadMode::Take,
                                ..
                            } | Inst::Release { .. }
                        )
                    }
                }),
            );
        }
        body.instructions = instructions;
        let run = hirsel_heapcheck::run(
            &raw,
            &path("main"),
            &[],
            &mut hirsel_heapcheck::NoOperations,
            &Default::default(),
        );
        assert_eq!(
            run.violation(),
            Some(ViolationKind::UninitializedScan),
            "{run:?}"
        );
    }
}

#[test]
fn environment_layouts_require_their_exact_live_word_and_guards() {
    let p = program(&TargetSpec::x86_64_unknown_linux_gnu(), true, false);
    let id = p.table.layout("outer_env");
    for mutation in 0..4 {
        assert!(
            hirsel_lir::layout_validate::validate_table(&p.table.table, &p.table.target).is_ok()
        );
        let mut table = p.table.table.clone();
        let record = &mut table.layouts[id.0 as usize];
        match mutation {
            0 => {
                let guard = record
                    .drop
                    .iter_mut()
                    .find(|s| {
                        matches!(
                            s,
                            hirsel_lir::layout_types::DropStep::ActiveVariant {
                                form: hirsel_lir::layout_types::TagForm::LiveBit { .. },
                                ..
                            }
                        )
                    })
                    .unwrap();
                if let hirsel_lir::layout_types::DropStep::ActiveVariant { form, .. } = guard {
                    *form = hirsel_lir::layout_types::TagForm::LiveBit { bit: 1 };
                }
            }
            1 => record.drop.retain(|s| {
                !matches!(
                    s,
                    hirsel_lir::layout_types::DropStep::ActiveVariant {
                        form: hirsel_lir::layout_types::TagForm::LiveBit { .. },
                        ..
                    }
                )
            }),
            2 => record.scan.retain(|s| {
                !matches!(
                    s,
                    hirsel_lir::layout_types::ScanStep::ActiveVariant {
                        form: hirsel_lir::layout_types::TagForm::LiveBit { .. },
                        ..
                    }
                )
            }),
            3 => record.fields.last_mut().unwrap().layout = p.table.layout("once"),
            _ => unreachable!(),
        }
        assert!(hirsel_lir::layout_validate::validate_table(&table, &p.table.target).is_err());
    }
}

#[test]
fn closure_construction_initializes_every_declared_capture() {
    let p = program(&TargetSpec::x86_64_unknown_linux_gnu(), true, false);
    let mut raw = p.units[0].unit().clone();
    let inputs = hirsel_lir::validate::Inputs {
        discharge: None,
        operations: &(),
        target: &p.table.target,
        layouts: &p.table.table,
        manifest: &p.manifest,
    };
    assert!(hirsel_lir::validate::check(&raw, &inputs).is_empty());
    let environment = p.table.layout("outer_env");
    let closure = raw
        .tables
        .closures
        .iter_mut()
        .find(|c| c.environment == environment)
        .unwrap();
    closure.captures.len -= 1;
    let violations = hirsel_lir::validate::check(&raw, &inputs);
    assert!(
        violations
            .iter()
            .any(|v| v.kind == hirsel_lir::validate::ViolationKind::Arity("closure captures"))
    );
}

#[test]
fn initialization_masks_share_the_named_live_word_limit() {
    let mut schema = schema();
    let number = schema.types.p(hirsel_types::Primitive::U64);
    let unit = schema.types.unit();
    schema.signature(
        "mask_limit",
        &[(hirsel_syntax::PassingMode::Borrow, number)],
        unit,
    );
    let table = schema.build(&TargetSpec::x86_64_unknown_linux_gnu());
    let m = manifest();
    for fields in [64, 65] {
        let mut u = Unit::new(&table, "mask_limit", "mask_limit");
        let ty = u.ty(ValueRepr::InitMask { fields }, table.layout("number"));
        let mask = u.param(table.layout("number"));
        u.builder.unit.values[mask.index()].ty = ty;
        let next = u.plain(table.layout("number"));
        u.builder.unit.values[next.index()].ty = ty;
        u.builder.push(
            ENTRY,
            Inst::UpdateMask {
                result: next,
                mask,
                field: fields - 1,
                initialized: false,
            },
        );
        u.builder.ret(ENTRY, None);
        let sealed = seal(&table, &m, u.finish());
        let codes = failure_codes();
        let id = input();
        let finalizers = Default::default();
        let inputs = hirsel_lir::emission::EmitInputs {
            input: &id,
            target: &table.target,
            layouts: &table.table,
            manifest: &m,
            failures: &codes,
            statics: &[],
            finalizers: &finalizers,
            inline_instances: &std::collections::BTreeSet::new(),
            available_externally: &std::collections::BTreeSet::new(),
            registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                &table.table,
                &finalizers,
            ),
            opt: hirsel_lir::emission::OptLevel::None,
            measurement: false,
        };
        let result = hirsel_llvm::emit_object(&[&sealed], &inputs);
        if fields == 64 {
            result.unwrap();
        } else {
            let hirsel_llvm::CodegenError::Unsupported { definition, what } = result.unwrap_err()
            else {
                panic!("expected implementation limit");
            };
            assert_eq!(definition, path("mask_limit"));
            assert_eq!(
                what,
                "initialization mask requires 65 bits; native live-mask word implementation limit is 64 bits"
            );
        }
    }
}
