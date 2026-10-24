//! Independent source inventory survives candidate marker removal.
use crate::fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, ViolationKind};
use hirsel_lir::{invocations::ReferenceInvocation, ir::*, stage::Rc};

fn inventory_program() -> hirsel_heapcheck::Program<Rc> {
    let mut layouts = Layouts::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
    let common = common(&mut layouts);
    let (target, table) = layouts.finish();
    let mut fixture = F::<Rc>::new("main", common);
    let failure = fixture.raises("LANG-10", fixture.propagate);
    fixture.inst(Inst::FrameEnter { failure });
    fixture.inst(Inst::FrameLeave);
    fixture.ret(None);
    let unit = fixture.finish(&table);
    let mut good = program(target, table, vec![unit]);
    // This is an independently declared call to an empty, inlined body: its
    // machine call has disappeared, but the source invocation must remain.
    good.invocations.insert(
        path("main"),
        vec![ReferenceInvocation {
            body: 0,
            block: 0,
            statement: 0,
            restart: false,
            primitive: None,
            callee: None,
        }],
    );
    good.invocation_bodies
        .push(hirsel_lir::invocations::ReferenceBody {
            definition: path("main"),
            instance: good.units[0].header.instance,
            body: 0,
        });
    good
}

#[test]
fn independent_invocation_inventory_rejects_deleting_both_events() {
    let good = inventory_program();
    let accepted = run(&good, &path("main"), &[], &mut Printer, &Config::default());
    assert!(accepted.result.is_ok(), "{:?}", accepted.result);
    let mut forged = good;
    let unit = &mut forged.units[0];
    let old = std::mem::take(&mut unit.instructions);
    for block in &mut unit.blocks {
        let retained = block
            .instructions
            .get(&old)
            .unwrap()
            .iter()
            .filter(|i| !matches!(i, Inst::FrameEnter { .. } | Inst::FrameLeave))
            .cloned();
        block.instructions = PoolRange::push(&mut unit.instructions, retained);
    }
    let rejected = run(
        &forged,
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert_eq!(
        rejected.result.unwrap_err().kind,
        ViolationKind::FrameImbalance
    );
}

#[test]
fn candidate_body_selector_cannot_hide_the_independent_invocation_inventory() {
    let mut forged = inventory_program();
    let unit = &mut forged.units[0];
    for anchor in &mut unit.tables.source_anchors {
        anchor.body = 1;
    }
    let old = std::mem::take(&mut unit.instructions);
    for block in &mut unit.blocks {
        block.instructions = PoolRange::push(
            &mut unit.instructions,
            block
                .instructions
                .get(&old)
                .unwrap()
                .iter()
                .filter(|i| !matches!(i, Inst::FrameEnter { .. } | Inst::FrameLeave))
                .cloned(),
        );
    }
    let rejected = run(
        &forged,
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert_eq!(
        rejected.result.unwrap_err().kind,
        ViolationKind::FrameImbalance
    );
}

#[test]
fn generated_callback_rejects_deleting_both_events_independently() {
    let mut layouts = Layouts::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
    let common = common(&mut layouts);
    let callable = layouts.callable(None);
    let (target, table) = layouts.finish();
    let mut callback = F::<Rc>::new("callback", common);
    callback.ret(None);
    let callback = callback.finish(&table);
    let mut main = F::<Rc>::new("main", common);
    main.b.unit.header.origin = UnitOrigin::Library;
    let symbol = main.b.symbol(SymbolKind::Function, path("callback"));
    let fun = main.b.owned(ValueRepr::Callable, callable);
    main.inst(Inst::FunctionRef {
        result: fun,
        symbol,
    });
    let admission = main.raises("LANG-10", main.propagate);
    main.inst(Inst::FrameEnter { failure: admission });
    let call = main.b.call(Callee::Indirect(fun), &[], None);
    let normal = main.b.block(BlockKind::Body, &[]);
    let retired = main.b.block(BlockKind::FailureCleanup, &[]);
    main.b.push(retired, Inst::FrameLeave);
    let outward = main.b.edge(main.propagate, &[]);
    main.b.term(retired, Terminator::Jump(outward));
    let reject = main.b.edge(retired, &[]);
    let failure = main.b.failure(reject, None, &[]);
    let normal_edge = main.b.edge(normal, &[]);
    main.b.term(
        main.cur,
        Terminator::Call {
            call,
            normal: Some(normal_edge),
            failure,
        },
    );
    main.cur = normal;
    main.inst(Inst::FrameLeave);
    main.release(fun, callable);
    main.ret(None);
    let mut good = program(target, table.clone(), vec![main.finish(&table), callback]);
    let accepted = run(&good, &path("main"), &[], &mut Printer, &Config::default());
    assert!(accepted.result.is_ok(), "{:?}", accepted.result);
    let unit = &mut good.units[0];
    let old = std::mem::take(&mut unit.instructions);
    for block in &mut unit.blocks {
        block.instructions = PoolRange::push(
            &mut unit.instructions,
            block
                .instructions
                .get(&old)
                .unwrap()
                .iter()
                .filter(|i| !matches!(i, Inst::FrameEnter { .. } | Inst::FrameLeave))
                .cloned(),
        );
    }
    let rejected = run(&good, &path("main"), &[], &mut Printer, &Config::default());
    assert_eq!(
        rejected.result.unwrap_err().kind,
        ViolationKind::FrameImbalance
    );
}
