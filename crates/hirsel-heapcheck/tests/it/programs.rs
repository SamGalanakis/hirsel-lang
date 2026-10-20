//! Executor behaviour beyond the acceptance fixtures: fault injection at every
//! failure-capable site, the terminal audit on language failure, exemption
//! records, closures, tokens, copy-on-write and deep release.
use crate::fixtures;

use fixtures::*;
use hirsel_core_contract::Passing;
use hirsel_heapcheck::build::{CLEANUP, Layouts, path};
use hirsel_heapcheck::{Config, Exemption, INJECTED_RULE, Operations, Program, Run, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::{Placed, Rc, Reuse, Stage};
use hirsel_lir::statics::StaticData;

fn execute<S: Stage>(p: &Program<S>) -> Run {
    run(p, &path("main"), &[], &mut Printer, &Config::default())
}

fn layouts() -> (Layouts, Common) {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    (l, c)
}

fn ok(run: &Run) -> &hirsel_heapcheck::Audit {
    run.result
        .as_ref()
        .unwrap_or_else(|v| panic!("expected a clean audit: {v}"))
}

#[test]
fn a_tooling_budget_still_stops_a_runaway_and_accepts_its_finite_twin() {
    let make = |runaway| {
        let (l, c) = layouts();
        let (target, table) = l.finish();
        let mut f = F::<Rc>::new("main", c);
        let failure = f.propagates();
        f.inst(Inst::FrameEnter { failure });
        if runaway {
            let body = f.b.block(BlockKind::Body, &[]);
            let edge = f.b.edge(body, &[]);
            f.b.term(f.cur, Terminator::Jump(edge));
            f.b.term(body, Terminator::Jump(edge));
        } else {
            f.inst(Inst::FrameLeave);
            f.ret(None);
        }
        program(target, table.clone(), vec![f.finish(&table)])
    };
    let config = Config {
        step_limit: 16,
        ..Config::default()
    };
    let finite = run(&make(false), &path("main"), &[], &mut Printer, &config);
    ok(&finite);
    let runaway = run(&make(true), &path("main"), &[], &mut Printer, &config);
    assert_eq!(runaway.violation(), Some(ViolationKind::Unsupported));
    assert_eq!(runaway.steps, config.step_limit + 1);
    assert!(
        runaway
            .result
            .unwrap_err()
            .message
            .contains("step limit reached")
    );
}

/// Checked arithmetic, a check, a resource whose finalizer has its own sites,
/// and explicit cleanup on every failure edge. With `leaky`, the cleanup after
/// the addition forgets to release `x`.
fn campaign_program(leaky: bool) -> Program<Rc> {
    campaign_program_with_return(leaky, false)
}

fn campaign_program_with_return(leaky: bool, return_resource: bool) -> Program<Rc> {
    let (mut l, c) = layouts();
    let boxed = c.boxed;
    let resource = l.resource(
        &[if return_resource { c.u64 } else { boxed }],
        Some(path("R.drop")),
    );
    let resource_ref = l.uncounted(resource);
    let (target, mut table) = l.finish();

    let mut fin = F::<Rc>::new("R.drop", c);
    source_finalizer(&mut fin.b, &mut table, resource_ref);
    let this = fin
        .b
        .value(ValueRepr::Uncounted, resource_ref, Holding::Plain);
    fin.b.parameter(this);
    let leave = fin.b.block(BlockKind::FailureCleanup, &[]);
    fin.b.push(leave, Inst::FrameLeave);
    fin.b.term(leave, Terminator::Propagate);
    let entered = fin.propagates();
    fin.inst(Inst::FrameEnter { failure: entered });
    let t = {
        let k = fin.b.constant(BOOL, c.bool, ConstantValue::Bool(true));
        let v = fin.b.plain(BOOL, c.bool);
        fin.inst(Inst::Const {
            result: v,
            constant: k,
        });
        v
    };
    let failure = fin.raises("FIN-CHECK", leave);
    fin.inst(Inst::Check {
        condition: t,
        failure,
    });
    fin.inst(Inst::FrameLeave);
    fin.ret(None);
    let fin = fin.finish(&table);

    let mut f = F::<Rc>::new("main", c);
    // Failure cleanups, innermost last.
    let fail_x = f.b.block(BlockKind::FailureCleanup, &[]);
    let fail_s = f.b.block(BlockKind::FailureCleanup, &[]);
    let entered = f.propagates();
    f.inst(Inst::FrameEnter { failure: entered });
    let ten = f.int(10);
    let x = f.boxed(ten);
    let v = f.unbox(x);
    let one = f.int(1);
    let y = f.b.plain(U64, c.u64);
    let failure = f.raises("ARITH-1", fail_x);
    f.inst(Inst::Binary {
        result: y,
        op: BinaryOp::Add,
        left: v,
        right: one,
        failure,
    });
    let hundred = f.int(100);
    let small = f.b.plain(BOOL, c.bool);
    let failure = f.propagates();
    f.inst(Inst::Binary {
        result: small,
        op: BinaryOp::Lt,
        left: y,
        right: hundred,
        failure,
    });
    let failure = f.raises("CHECK-1", fail_x);
    f.inst(Inst::Check {
        condition: small,
        failure,
    });
    let z = if return_resource { y } else { f.boxed(y) };
    let k =
        f.b.construct(ConstructorKind::Tuple, resource, &[(0, z)], HEAP);
    let r = f.b.owned(ValueRepr::Aggregate, resource);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: r,
        construct: k,
        failure,
    });
    let s = f.b.slot(ValueRepr::Aggregate, resource, SlotKind::Local);
    let at =
        f.b.address(AddressBase::Slot(s), &[], ValueRepr::Aggregate, resource);
    f.inst(Inst::StorageLive { slot: s });
    f.inst(Inst::Store {
        address: at,
        value: r,
        version: r,
    });
    let op = f.b.operation(print_op());
    let call =
        f.b.call(op, &[(Passing::Borrow, ArgumentValue::Value(y))], None);
    let next = f.b.block(BlockKind::Body, &[]);
    let normal = f.b.edge(next, &[]);
    let e = f.b.edge(fail_s, &[]);
    let failure = f.b.failure(e, None, &[]);
    f.b.term(
        f.cur,
        Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        },
    );
    f.cur = next;
    let drop_failed = f.b.block(BlockKind::FailureCleanup, &[]);
    let e = f.b.edge(drop_failed, &[]);
    let failure = f.b.failure(e, None, &[]);
    let returned = if return_resource {
        let value = f.b.owned(ValueRepr::Aggregate, resource);
        f.inst(Inst::Read {
            result: value,
            address: at,
            reaching: r,
            mode: ReadMode::Take,
        });
        Some(value)
    } else {
        f.inst(Inst::Drop {
            address: at,
            if_live: false,
            anchor: CLEANUP,
            failure,
        });
        None
    };
    f.inst(Inst::StorageDead {
        slot: s,
        if_live: false,
    });
    f.release(x, boxed);
    f.inst(Inst::FrameLeave);
    f.ret(returned);
    // The finalizer failed while dropping s: its fields are already released.
    f.cur = drop_failed;
    f.inst(Inst::StorageDead {
        slot: s,
        if_live: false,
    });
    f.release(x, boxed);
    f.inst(Inst::FrameLeave);
    f.b.term(drop_failed, Terminator::Propagate);
    // Before s holds the resource.
    f.cur = fail_s;
    let failure = f.propagates();
    f.inst(Inst::Drop {
        address: at,
        if_live: true,
        anchor: CLEANUP,
        failure,
    });
    f.inst(Inst::StorageDead {
        slot: s,
        if_live: false,
    });
    f.release(x, boxed);
    f.inst(Inst::FrameLeave);
    f.b.term(fail_s, Terminator::Propagate);
    f.cur = fail_x;
    if !leaky {
        f.release(x, boxed);
    }
    f.inst(Inst::FrameLeave);
    f.b.term(fail_x, Terminator::Propagate);
    let main = f.finish(&table);
    program(target, table, vec![main, fin])
}

fn printer() -> Box<dyn Operations> {
    Box::new(Printer)
}

#[test]
fn every_failure_capable_site_fails_cleanly() {
    let p = campaign_program(false);
    let campaign = fault_campaign(&p, &path("main"), &[], &mut printer, &Config::default());
    let clean = ok(&campaign.clean);
    assert_eq!(clean.observation.output, b"11\n");
    // main: frame admission, the addition, the check; the finalizer's entry,
    // its own admission and its check.
    assert_eq!(campaign.clean.sites, 6);
    assert_eq!(campaign.injected.len(), 6);
    for (site, run) in &campaign.injected {
        let audit = run
            .result
            .as_ref()
            .unwrap_or_else(|v| panic!("site {site}: {v}"));
        assert_eq!(
            audit.observation.failures.first().map(|f| f.rule.as_str()),
            Some(INJECTED_RULE),
            "site {site}"
        );
        assert_eq!(audit.observation.returned, None, "site {site}");
    }
}

#[test]
fn a_failure_cleanup_that_skips_a_release_is_found_at_its_site() {
    let p = campaign_program(true);
    let campaign = fault_campaign(&p, &path("main"), &[], &mut printer, &Config::default());
    ok(&campaign.clean);
    let (site, violation) = campaign.first_violation().expect("the leak is found");
    assert_eq!(site, hirsel_heapcheck::Perturbation::Injected(1));
    assert_eq!(violation.kind, ViolationKind::LeakedCredit);
}

/// Large native consumers need every audit, but not every successful report's
/// retained trace. Broken cleanup must keep the same first witness and still
/// visit later sites; terminal finalizers belong to the same domain.
#[test]
fn streaming_campaign_keeps_clean_and_first_failure_without_dropping_sites() {
    for leaky in [false, true] {
        for returned in [false, true] {
            let p = campaign_program_with_return(leaky, returned);
            for full_recount_every_step in [false, true] {
                for reexecute_campaign in [false, true] {
                    let config = Config {
                        full_recount_every_step,
                        reexecute_campaign,
                        compare_campaigns: true,
                        ..Config::default()
                    };
                    let collected = fault_campaign(&p, &path("main"), &[], &mut printer, &config);
                    let streamed = hirsel_heapcheck::audit_campaign(
                        &p,
                        &path("main"),
                        &[],
                        &mut printer,
                        &config,
                    );
                    assert_eq!(streamed.clean, collected.clean);
                    assert_eq!(streamed.injected, collected.injected.len() as u64);
                    assert_eq!(streamed.injected, collected.clean.sites);
                    assert_eq!(streamed.fired, collected.fired.len() as u64);
                    assert_eq!(streamed.first_violation(), collected.first_violation());
                    let expected = collected
                        .injected
                        .iter()
                        .find(|(_, run)| run.result.is_err())
                        .map(|(site, run)| {
                            (hirsel_heapcheck::Perturbation::Injected(*site), run.clone())
                        });
                    assert_eq!(streamed.first_failure, expected);
                }
            }
        }
    }
}

/// `let x = Box(1); check(false)` with x released on the failure path or not.
fn failing_check(release: bool) -> Program<Rc> {
    let (l, c) = layouts();
    let boxed = c.boxed;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let cleanup = f.b.block(BlockKind::FailureCleanup, &[]);
    let one = f.int(1);
    let x = f.boxed(one);
    let v = f.unbox(x);
    f.print(v);
    let k = f.b.constant(BOOL, c.bool, ConstantValue::Bool(false));
    let no = f.b.plain(BOOL, c.bool);
    f.inst(Inst::Const {
        result: no,
        constant: k,
    });
    let failure = f.raises("TEST-1", cleanup);
    f.inst(Inst::Check {
        condition: no,
        failure,
    });
    f.release(x, boxed);
    f.ret(None);
    f.cur = cleanup;
    if release {
        f.release(x, boxed);
    }
    f.b.term(cleanup, Terminator::Propagate);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn a_language_failure_is_audited_like_a_success() {
    let run = execute(&failing_check(true));
    let audit = ok(&run);
    assert_eq!(audit.observation.output, b"1\n");
    assert_eq!(audit.observation.returned, None);
    let rules: Vec<_> = audit
        .observation
        .failures
        .iter()
        .map(|f| f.rule.as_str())
        .collect();
    assert_eq!(rules, ["TEST-1"]);
    let leak = execute(&failing_check(false));
    assert_eq!(leak.output, b"1\n");
    assert_eq!(leak.violation(), Some(ViolationKind::LeakedCredit));
}

/// `x = Box(1)`, two dups and three releases: with a tiny count maximum the
/// block saturates and stays alive under a named overflow record.
fn saturating() -> Program<Rc> {
    let (l, c) = layouts();
    let boxed = c.boxed;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let one = f.int(1);
    let x = f.boxed(one);
    f.inst(Inst::Dup { value: x });
    f.inst(Inst::Dup { value: x });
    for _ in 0..3 {
        f.release(x, boxed);
    }
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn counts_saturate_to_sticky_immortality_with_an_exemption_record() {
    let p = saturating();
    let free = execute(&p);
    let audit = ok(&free);
    assert!(audit.exemptions.is_empty());
    let config = Config {
        count_max: 2,
        ..Config::default()
    };
    let saturated = run(&p, &path("main"), &[], &mut Printer, &config);
    let audit = ok(&saturated);
    assert!(matches!(
        audit.exemptions.as_slice(),
        [Exemption::Overflow { .. }]
    ));
    assert!(
        saturated
            .trace
            .iter()
            .any(|e| matches!(e, hirsel_heapcheck::Event::Saturate { .. }))
    );
}

fn forgetting(forget: bool) -> Program<Rc> {
    let (l, c) = layouts();
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let one = f.int(1);
    let x = f.boxed(one);
    if forget {
        f.inst(Inst::Forget { value: x });
    }
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn forget_is_a_named_exemption_and_nothing_else_is() {
    let kept = execute(&forgetting(true));
    let audit = ok(&kept);
    assert!(matches!(
        audit.exemptions.as_slice(),
        [Exemption::Forget { allocations, .. }] if allocations.len() == 1
    ));
    assert_eq!(
        execute(&forgetting(false)).violation(),
        Some(ViolationKind::LeakedCredit)
    );
}

/// A closure taking `x` into its heap environment; the call reads the capture.
#[test]
fn closure_environments_own_their_captures() {
    let (mut l, c) = layouts();
    let boxed = c.boxed;
    let env = l.record(&[boxed]);
    let callable = l.callable(Some(env));
    let (target, table) = l.finish();

    let mut get = F::<Rc>::new("get", c);
    let cap = get.b.slot(BLOCK, boxed, SlotKind::Capture(0));
    let environment = get.b.unit.captures[0];
    let at = get.b.address(
        AddressBase::Slot(cap),
        &[ProjectionStep::Field(0)],
        U64,
        c.u64,
    );
    let v = get.b.plain(U64, c.u64);
    let failure = get.propagates();
    get.inst(Inst::Address {
        address: at,
        failure,
    });
    get.inst(Inst::Read {
        result: v,
        address: at,
        reaching: environment,
        mode: ReadMode::Bits,
    });
    get.print(v);
    get.ret(None);
    let get = get.finish(&table);

    let mut f = F::<Rc>::new("main", c);
    let n = f.int(42);
    let x = f.boxed(n);
    let k =
        f.b.closure(path("get"), env, &[(x, CaptureUse::Take)], HEAP);
    let fun = f.b.owned(ValueRepr::Callable, callable);
    let failure = f.propagates();
    f.inst(Inst::Closure {
        result: fun,
        closure: k,
        failure,
    });
    f.call(Callee::Indirect(fun), &[], None);
    f.call(Callee::Indirect(fun), &[], None);
    f.release(fun, callable);
    f.ret(None);
    let main = f.finish(&table);
    let p = program(target, table, vec![main, get]);
    let run = execute(&p);
    assert_eq!(ok(&run).observation.output, b"42\n42\n");
    assert_eq!(run.counters.allocations, 2);
    assert_eq!(run.counters.frees, 2);
}

/// Token misuse: consumed twice, or installed into a different size class.
fn token_program(reuse_twice: bool, wide: bool) -> Program<Reuse> {
    let (mut l, c) = layouts();
    let boxed = c.boxed;
    let quad = l.record(&[c.u64, c.u64, c.u64, c.u64]);
    let quad_box = l.block(quad);
    let (target, table) = l.finish();
    let mut f = F::<Reuse>::new("main", c);
    let one = f.int(1);
    let x = f.boxed(one);
    let token = f.b.owned(ValueRepr::Token, boxed);
    let failure = f.propagates();
    f.inst(Inst::Reset {
        result: token,
        value: x,
        layout: boxed,
        live: MaskRef::All,
        anchor: CLEANUP,
        failure,
    });
    let two = f.int(2);
    let (k, layout) = if wide {
        let fields = [(0, two), (1, two), (2, two), (3, two)];
        (
            f.b.construct(ConstructorKind::Tuple, quad_box, &fields, HEAP),
            quad_box,
        )
    } else {
        (
            f.b.construct(ConstructorKind::Tuple, boxed, &[(0, two)], HEAP),
            boxed,
        )
    };
    let y = f.b.owned(BLOCK, layout);
    let failure = f.propagates();
    f.inst(Inst::Reuse {
        result: y,
        token,
        construct: k,
        kept: None,
        failure,
    });
    if reuse_twice {
        f.inst(Inst::Dispose { token });
    }
    f.release(y, layout);
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn tokens_are_linear_and_exactly_compatible() {
    let reused = execute(&token_program(false, false));
    ok(&reused);
    assert_eq!(reused.counters.reuses, 1);
    assert_eq!(reused.counters.allocations, 1);
    assert_eq!(
        execute(&token_program(true, false)).violation(),
        Some(ViolationKind::TokenReused)
    );
    assert_eq!(
        execute(&token_program(false, true)).violation(),
        Some(ViolationKind::IncompatibleReuse)
    );
}

/// A write into a block another holder shares, or into a unique one.
fn writing(shared: bool) -> Program<Rc> {
    let (l, c) = layouts();
    let boxed = c.boxed;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let one = f.int(1);
    let x = f.boxed(one);
    if shared {
        f.inst(Inst::Dup { value: x });
    }
    let shelf = f.b.slot(U64, c.u64, SlotKind::Displaced);
    f.inst(Inst::StorageLive { slot: shelf });
    let at = f.b.address(
        AddressBase::Value(x),
        &[ProjectionStep::Field(0)],
        U64,
        c.u64,
    );
    let two = f.int(2);
    let failure = f.propagates();
    let version = f.b.plain(U64, c.u64);
    let displaced_version = f.b.plain(U64, c.u64);
    f.inst(Inst::Replace {
        address: at,
        value: two,
        displaced: shelf,
        version,
        displaced_version,
        if_live: false,
        failure,
    });
    let v = f.unbox(x);
    f.print(v);
    f.inst(Inst::StorageDead {
        slot: shelf,
        if_live: false,
    });
    if shared {
        f.release(x, boxed);
    }
    f.release(x, boxed);
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn writes_require_a_unique_block() {
    assert_eq!(ok(&execute(&writing(false))).observation.output, b"2\n");
    assert_eq!(
        execute(&writing(true)).violation(),
        Some(ViolationKind::WriteShared)
    );
}

fn releasing_twice(twice: bool) -> Program<Rc> {
    let (l, c) = layouts();
    let boxed = c.boxed;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let one = f.int(1);
    let x = f.boxed(one);
    f.release(x, boxed);
    if twice {
        f.release(x, boxed);
    }
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn a_second_release_of_one_credit_underflows() {
    ok(&execute(&releasing_twice(false)));
    assert_eq!(
        execute(&releasing_twice(true)).violation(),
        Some(ViolationKind::CreditUnderflow)
    );
}

#[test]
fn static_data_is_an_explicit_immortal_record() {
    let (l, c) = layouts();
    let boxed = c.boxed;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let sym = f.b.symbol(SymbolKind::Constant, path("ANSWER"));
    let s = f.b.owned(BLOCK, boxed);
    f.inst(Inst::ReadStatic {
        result: s,
        symbol: sym,
    });
    let v = f.unbox(s);
    f.print(v);
    f.inst(Inst::Dup { value: s });
    f.release(s, boxed);
    f.release(s, boxed);
    f.ret(None);
    let main = f.finish(&table);
    let mut p = program(target, table, vec![main]);
    p.statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("ANSWER"),
            instance: None,
        },
        c.cell,
        42u64.to_le_bytes().to_vec(),
    ));
    let run = execute(&p);
    let audit = ok(&run);
    assert_eq!(audit.observation.output, b"42\n");
    assert!(matches!(
        audit.exemptions.as_slice(),
        [Exemption::Static { .. }]
    ));
}

fn chain_program(n: u128) -> Program<Rc> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let list = l.unit();
    let next = l.block(list);
    let shape = l.variant(&[&[], &[next]]);
    let record = l.get(shape).clone();
    *l.get_mut(list) = record;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let i = f.b.plain(U64, c.u64);
    let acc = f.b.owned(ValueRepr::Aggregate, list);
    let body = f.b.block(BlockKind::Body, &[i, acc]);
    let done = f.b.block(BlockKind::Body, &[]);
    f.b.symbol(SymbolKind::Type, path("List"));
    let nil_k = f.b.construct(
        ConstructorKind::Variant {
            definition: SymbolId(0),
            variant: 0,
        },
        list,
        &[],
        HEAP,
    );
    let nil = f.b.owned(ValueRepr::Aggregate, list);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: nil,
        construct: nil_k,
        failure,
    });
    let zero = f.int(0);
    let enter = f.b.edge(body, &[zero, nil]);
    f.b.term(f.cur, Terminator::Jump(enter));
    f.cur = body;
    // acc = Link(next: Box(acc)); i += 1
    let boxed_k = f.b.construct(ConstructorKind::Box, next, &[(0, acc)], HEAP);
    let tail = f.b.owned(BLOCK, next);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: tail,
        construct: boxed_k,
        failure,
    });
    let cons_k = f.b.construct(
        ConstructorKind::Variant {
            definition: SymbolId(0),
            variant: 1,
        },
        list,
        &[(0, tail)],
        HEAP,
    );
    let acc2 = f.b.owned(ValueRepr::Aggregate, list);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: acc2,
        construct: cons_k,
        failure,
    });
    let one = f.int(1);
    let i2 = f.b.plain(U64, c.u64);
    let failure = f.propagates();
    f.inst(Inst::Binary {
        result: i2,
        op: BinaryOp::Add,
        left: i,
        right: one,
        failure,
    });
    let limit = f.int(n);
    let more = f.b.plain(BOOL, c.bool);
    let failure = f.propagates();
    f.inst(Inst::Binary {
        result: more,
        op: BinaryOp::Lt,
        left: i2,
        right: limit,
        failure,
    });
    let again = f.b.edge(body, &[i2, acc2]);
    let last = f.b.owned(ValueRepr::Aggregate, list);
    let out = f.b.block(BlockKind::Body, &[last]);
    let leave = f.b.edge(out, &[acc2]);
    f.b.term(
        body,
        Terminator::Branch {
            condition: more,
            yes: again,
            no: leave,
        },
    );
    f.cur = out;
    f.release(last, list);
    let e = f.b.edge(done, &[]);
    f.b.term(out, Terminator::Jump(e));
    f.cur = done;
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

/// A cons list of `n` nodes built in a loop, then released at once: the
/// release drains iteratively, never on the Rust stack.
#[test]
fn a_deep_list_releases_without_recursion() {
    let n = 30_000;
    let p = chain_program(n);
    let config = Config {
        check_every_step: false,
        step_limit: 10_000_000,
        ..Config::default()
    };
    // A thread with a small stack: recursion proportional to depth would overflow.
    let run = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || run(&p, &path("main"), &[], &mut Printer, &config))
        .unwrap()
        .join()
        .unwrap();
    ok(&run);
    assert_eq!(run.counters.allocations, n as u64);
    assert_eq!(run.counters.frees, n as u64);
}

#[test]
fn only_post_rc_stages_execute() {
    let (l, c) = layouts();
    let (target, table) = l.finish();
    let mut f = F::<Placed>::new("main", c);
    f.ret(None);
    let main = f.finish(&table);
    let p = program(target, table, vec![main]);
    assert_eq!(execute(&p).violation(), Some(ViolationKind::Malformed));
}

/// A slot address prepared before its `StorageLive`: preparing a slot or a
/// field-only path is frame arithmetic, so liveness is checked where the
/// address is used. With `early`, the address is read before the slot is
/// live, which is still `UninitializedRead` at that use.
fn prepared_before_live(early: bool) -> Program<Rc> {
    let (l, c) = layouts();
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let s = f.b.slot(U64, c.u64, SlotKind::Local);
    let at = f.b.address(AddressBase::Slot(s), &[], U64, c.u64);
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: at,
        failure,
    });
    let seven = f.int(7);
    if early {
        let v = f.b.plain(U64, c.u64);
        f.inst(Inst::Read {
            result: v,
            address: at,
            reaching: seven,
            mode: ReadMode::Bits,
        });
    }
    f.inst(Inst::StorageLive { slot: s });
    let version = f.b.plain(U64, c.u64);
    f.inst(Inst::Store {
        address: at,
        value: seven,
        version,
    });
    let v = f.b.plain(U64, c.u64);
    f.inst(Inst::Read {
        result: v,
        address: at,
        reaching: version,
        mode: ReadMode::Bits,
    });
    f.inst(Inst::StorageDead {
        slot: s,
        if_live: false,
    });
    f.ret(Some(v));
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn a_slot_address_prepared_before_its_storage_is_live_executes() {
    let run = execute(&prepared_before_live(false));
    let audit = ok(&run);
    assert_eq!(
        audit.observation.returned,
        Some(hirsel_heapcheck::Observed::Integer {
            scalar: hirsel_lir::layout_types::ScalarKind::U64,
            bits: 7
        })
    );
}

#[test]
fn a_slot_address_used_before_its_storage_is_live_is_an_uninitialized_read() {
    let run = execute(&prepared_before_live(true));
    assert_eq!(run.violation(), Some(ViolationKind::UninitializedRead));
}

/// A callable without captures still has an initialized null environment word.
/// The nested tuple is the shape of a returned closure tuple used as a field.
fn null_environment_tuple(missing_scalar: bool) -> Program<Rc> {
    let (mut l, c) = layouts();
    let env = l.record(&[]);
    let callable = l.callable(None);
    let pair = l.record(&[callable, c.u64]);
    let outer = l.record(&[pair]);
    let (target, table) = l.finish();
    let mut get = F::<Rc>::new("get", c);
    get.ret(None);
    let get = get.finish(&table);
    let mut f = F::<Rc>::new("main", c);
    let closure = f.b.closure(path("get"), env, &[], HEAP);
    let fun = f.b.owned(ValueRepr::Callable, callable);
    let failure = f.propagates();
    f.inst(Inst::Closure {
        result: fun,
        closure,
        failure,
    });
    let n = f.int(42);
    let fields = if missing_scalar {
        vec![(0, fun)]
    } else {
        vec![(0, fun), (1, n)]
    };
    let construct = f.b.construct(ConstructorKind::Tuple, pair, &fields, HEAP);
    let p = f.b.owned(ValueRepr::Aggregate, pair);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: p,
        construct,
        failure,
    });
    let construct =
        f.b.construct(ConstructorKind::Tuple, outer, &[(0, p)], HEAP);
    let o = f.b.owned(ValueRepr::Aggregate, outer);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: o,
        construct,
        failure,
    });
    f.print(n);
    f.release(o, outer);
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main, get])
}

#[test]
fn nested_callable_tuples_initialize_null_environments_without_accepting_unwritten_fields() {
    let clean = execute(&null_environment_tuple(false));
    assert_eq!(ok(&clean).observation.output, b"42\n");
    assert_eq!(
        execute(&null_environment_tuple(true)).violation(),
        Some(ViolationKind::UninitializedRead)
    );
}

#[test]
fn a_32_link_chain_audits_every_step_in_both_modes() {
    ok(&execute(&chain_program(32)));
}

#[test]
fn terminal_result_finalizers_keep_every_campaign_site() {
    let p = campaign_program_with_return(false, true);
    let campaign = fault_campaign(&p, &path("main"), &[], &mut printer, &Config::default());
    assert_eq!(campaign.clean.sites, 6);
    assert_eq!(campaign.injected.len(), 6);
    for (_, run) in &campaign.injected {
        ok(run);
    }
    ok(&campaign.clean);
}

#[test]
fn host_operation_models_are_never_cloned() {
    struct Host;
    impl Operations for Host {
        fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
            panic!("host cloned")
        }
        fn call(
            &mut self,
            op: &hirsel_core_contract::OperationId,
            args: &[hirsel_heapcheck::OpValue],
            output: &mut Vec<u8>,
        ) -> hirsel_heapcheck::OpOutcome {
            Printer.call(op, args, output)
        }
    }
    let p = campaign_program(false);
    let campaign = hirsel_heapcheck::fault_campaign(
        &p,
        &path("main"),
        &[],
        &mut || Box::new(Host),
        &Config::default(),
    );
    assert_eq!(campaign.clean.sites, 6);
    for (_, run) in &campaign.injected {
        ok(run);
    }
}

#[test]
fn a_configured_finite_bound_must_fit_the_production_u32_counter() {
    let program = campaign_program(false);
    for count_max in [0, u32::MAX as u64, u64::MAX] {
        let config = Config {
            count_max,
            ..Config::default()
        };
        let actual = run(&program, &path("main"), &[], &mut Printer, &config);
        assert_eq!(actual.violation(), Some(ViolationKind::Malformed));
    }
    ok(&execute(&program));
}
