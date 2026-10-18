//! Time (`src/exec/clock.rs`): hand-built programs whose leak lies only on the path where a
//! deadline fires before an operation completes. Virtual time alone never takes that path, so
//! the clean run and every injected failure pass; the run that fires the deadline early at a
//! timer choice point finds the leak. Each has a fixed twin that passes every run.
use crate::fixtures::*;
use hirsel_core_contract::{OperationId, Passing};
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{
    Config, OpOutcome, OpValue, Operations, Perturbation, Program, ViolationKind, fault_campaign,
};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::Rc;
use hirsel_syntax::{ContentHash, DefPath};

/// The model's one monotonic clock handle.
const CLOCK: u128 = 7;

fn op(name: &str) -> OperationId {
    OperationId {
        definition: path(name),
        declaration_digest: ContentHash::of_bytes(name.as_bytes()),
    }
}

fn intrinsic(name: &str) -> OperationId {
    OperationId {
        definition: DefPath::core(&format!("$intrinsic.{name}")),
        declaration_digest: ContentHash::of_bytes(name.as_bytes()),
    }
}

fn done(completed: bool) -> OpValue {
    OpValue::Bits {
        scalar: ScalarKind::Bool,
        bits: u128::from(completed),
    }
}

/// `sleep(clock, until) -> bool`: true once the reading reached `until`, false when canceled
/// (a cancellation point). `run(clock, at) -> bool`: an instant run racing its deadline `at`,
/// false when the deadline fires first. `print(u64)` as the fixtures print.
#[derive(Clone)]
struct Clocked {
    now: u64,
}

impl Operations for Clocked {
    fn domain(&self) -> hirsel_heapcheck::OperationDomain {
        hirsel_heapcheck::OperationDomain::Simulation
    }
    fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
        Some(Box::new(self.clone()))
    }
    fn monotonic(&self, clock: u64) -> Option<u64> {
        (u128::from(clock) == CLOCK).then_some(self.now)
    }
    fn advance_monotonic(&mut self, ticks: u64) {
        self.now = self.now.max(ticks);
    }
    fn canceled(&mut self, op: &OperationId, _: &[OpValue], _: u32) -> Option<OpOutcome> {
        (op == &self::op("sleep")).then(|| OpOutcome::Value(done(false)))
    }
    fn call(&mut self, id: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome {
        let word = |v: &OpValue| match v {
            OpValue::Bits { bits, .. } => *bits as u64,
            _ => panic!("{v:?}"),
        };
        match args {
            [clock, until] if id == &op("sleep") => {
                let (clock, until) = (word(clock), word(until));
                if self.now >= until {
                    OpOutcome::Value(done(true))
                } else {
                    OpOutcome::Wait { clock, until }
                }
            }
            [clock, at] if id == &op("run") => OpOutcome::Deadline {
                clock: word(clock),
                at: word(at),
                completed: Box::new(OpOutcome::Value(done(true))),
                timed_out: Box::new(OpOutcome::Value(done(false))),
            },
            [OpValue::Bits { bits, .. }] if id == &print_op() => {
                output.extend(format!("{bits}\n").as_bytes());
                OpOutcome::Value(OpValue::Unit)
            }
            _ => OpOutcome::Unsupported,
        }
    }
}

/// The operation a control races against its deadline.
#[derive(Clone, Copy)]
enum Race {
    /// `task.scope_until(deadline 1000) { sleep(until 100) }`: the scope deadline cancels the
    /// sleep when it fires first.
    ScopeSleep,
    /// `run(deadline 100)`: the run times out when its deadline fires first.
    Run,
}

/// `let x = Box(1)`, then the race: when it completes, print x's value and release x;
/// otherwise release x only in the fixed twin.
fn race(kind: Race, fixed: bool) -> Program<Rc> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let deadline = l.record(&[c.u64, c.u64]);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let clock = f.int(CLOCK);
    let scope = match kind {
        Race::ScopeSleep => {
            let tasks = f.int(0);
            let at = f.int(1000);
            let k = f.b.construct(
                ConstructorKind::Tuple,
                deadline,
                &[(0, clock), (1, at)],
                HEAP,
            );
            let d = f.b.plain(ValueRepr::Aggregate, deadline);
            let failure = f.propagates();
            f.inst(Inst::Construct {
                result: d,
                construct: k,
                failure,
            });
            let enter = f.b.operation(intrinsic("TaskScopeEnterUntil"));
            let scope = f.b.plain(U64, c.u64);
            f.call(
                enter,
                &[
                    (Passing::Borrow, ArgumentValue::Value(tasks)),
                    (Passing::Borrow, ArgumentValue::Value(d)),
                ],
                Some(scope),
            );
            Some(scope)
        }
        Race::Run => None,
    };
    let one = f.int(1);
    let x = f.boxed(one);
    let until = f.int(100);
    let completed = f.b.plain(BOOL, c.bool);
    let callee = f.b.operation(op(match kind {
        Race::ScopeSleep => "sleep",
        Race::Run => "run",
    }));
    f.call(
        callee,
        &[
            (Passing::Borrow, ArgumentValue::Value(clock)),
            (Passing::Borrow, ArgumentValue::Value(until)),
        ],
        Some(completed),
    );
    let finished = f.b.block(BlockKind::Body, &[]);
    let interrupted = f.b.block(BlockKind::Body, &[]);
    let join = f.b.block(BlockKind::Body, &[]);
    let yes = f.b.edge(finished, &[]);
    let no = f.b.edge(interrupted, &[]);
    f.b.term(
        f.cur,
        Terminator::Branch {
            condition: completed,
            yes,
            no,
        },
    );
    f.cur = finished;
    let value = f.unbox(x);
    f.print(value);
    f.release(x, c.boxed);
    let to_join = f.b.edge(join, &[]);
    f.b.term(f.cur, Terminator::Jump(to_join));
    f.cur = interrupted;
    if fixed {
        f.release(x, c.boxed);
    }
    let to_join = f.b.edge(join, &[]);
    f.b.term(f.cur, Terminator::Jump(to_join));
    f.cur = join;
    if let Some(scope) = scope {
        let error = f.b.constant(BOOL, c.bool, ConstantValue::Bool(false));
        let flag = f.b.plain(BOOL, c.bool);
        f.inst(Inst::Const {
            result: flag,
            constant: error,
        });
        let exit = f.b.operation(intrinsic("TaskScopeExitUntil"));
        let expired = f.b.plain(BOOL, c.bool);
        f.call(
            exit,
            &[
                (Passing::Borrow, ArgumentValue::Value(scope)),
                (Passing::Borrow, ArgumentValue::Value(flag)),
            ],
            Some(expired),
        );
    }
    f.ret(None);
    program(target, table.clone(), vec![f.finish(&table)])
}

fn campaign(kind: Race, fixed: bool) -> hirsel_heapcheck::Campaign {
    let program = race(kind, fixed);
    fault_campaign(
        &program,
        &path("main"),
        &[],
        &mut || Box::new(Clocked { now: 0 }),
        &Config::default(),
    )
}

fn check(kind: Race) {
    // Virtual time completes the operation first: the leak is invisible to the clean run and
    // to every injected failure.
    let broken = campaign(kind, false);
    let clean = broken.clean.result.as_ref().expect("the clean run passes");
    assert_eq!(broken.clean.output, b"1\n");
    assert!(clean.observation.failures.is_empty());
    for (site, run) in &broken.injected {
        assert!(run.result.is_ok(), "site {site}: {:?}", run.result);
    }
    assert!(broken.clean.choices > 0);
    // The deadline fired before the operation completed: the release is missing.
    let (at, violation) = broken.first_violation().expect("the fired run leaks");
    assert_eq!(at, Perturbation::Fired(0));
    assert_eq!(violation.kind, ViolationKind::LeakedCredit);
    assert_eq!(broken.fired[0].1.output, b"");
    let fixed = campaign(kind, true);
    assert!(fixed.first_violation().is_none(), "{fixed:?}");
    assert_eq!(fixed.fired.len() as u64, fixed.clean.choices);
    assert_eq!(fixed.fired[0].1.output, b"");
}

#[test]
fn a_leak_where_a_scope_deadline_cancels_a_sleep_is_found_only_by_firing_it_early() {
    check(Race::ScopeSleep);
}

#[test]
fn a_leak_where_a_run_times_out_is_found_only_by_firing_its_deadline_early() {
    check(Race::Run);
}
