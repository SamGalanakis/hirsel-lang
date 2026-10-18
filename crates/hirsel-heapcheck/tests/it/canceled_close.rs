//! A canceled operation that takes a handle (the driver model's `tls.connect` and `tls.close`,
//! as `hirsel-rt/src/host_waits.rs` completes them): its scope deadline cancels a stalled
//! wait, and the close that follows completes `Canceled` through the model's `canceled`,
//! taking and closing its stream. Hand-built programs whose only bug lies on that canceled
//! path, each with a fixed twin that passes every run.
use crate::fixtures::*;
use hirsel_core_contract::{OperationId, Passing};
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{
    Config, OpOutcome, OpValue, Operations, Perturbation, Program, ViolationKind, fault_campaign,
};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{CopyRecipe, DropStep, LayoutKind, ScalarKind, TargetSpec};
use hirsel_lir::stage::Rc;
use hirsel_syntax::{ContentHash, DefPath};

/// The model's one monotonic clock handle and its one stream.
const CLOCK: u128 = 7;
const STREAM: u64 = 41;

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

fn flag(value: bool) -> OpValue {
    OpValue::Bits {
        scalar: ScalarKind::Bool,
        bits: u128::from(value),
    }
}

/// `open() -> Stream`; `snapshot() -> Stream`, the stream's identity without ownership;
/// `peek(borrow Stream)`; `stall(clock, until) -> bool`, a stalled scripted source, false
/// when canceled; `close(take Stream) -> bool`, true when it closed the stream, false when
/// canceled, taking the stream either way.
#[derive(Clone)]
struct Streams {
    now: u64,
}

impl Operations for Streams {
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
    fn canceled(&mut self, id: &OperationId, args: &[OpValue], _: u32) -> Option<OpOutcome> {
        if id == &op("stall") {
            return Some(OpOutcome::Value(flag(false)));
        }
        (id == &op("close")).then(|| {
            assert_eq!(args, &[OpValue::Handle(STREAM)]);
            OpOutcome::Complete {
                result: Ok(flag(false)),
                consumed: vec![0],
            }
        })
    }
    fn call(&mut self, id: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome {
        match args {
            [] if id == &op("open") => OpOutcome::Value(OpValue::OpenHandle(STREAM)),
            [] if id == &op("snapshot") => OpOutcome::Value(OpValue::Handle(STREAM)),
            [OpValue::Handle(STREAM)] if id == &op("peek") => OpOutcome::Value(OpValue::Unit),
            [_, _] if id == &op("stall") => OpOutcome::Pending,
            [OpValue::Handle(STREAM)] if id == &op("close") => OpOutcome::Complete {
                result: Ok(flag(true)),
                consumed: vec![0],
            },
            [OpValue::Bits { bits, .. }] if id == &print_op() => {
                output.extend(format!("{bits}\n").as_bytes());
                OpOutcome::Value(OpValue::Unit)
            }
            _ => OpOutcome::Unsupported,
        }
    }
    fn close_handle(&mut self, handle: u64, _: &mut Vec<u8>) -> Result<Option<String>, String> {
        assert_eq!(handle, STREAM);
        Ok(None)
    }
}

/// Where a control peeks at its stream through a snapshot of its identity.
#[derive(Clone, Copy, PartialEq)]
enum Peek {
    Never,
    BeforeClose,
    AfterClose,
}

/// `task.scope_until(deadline 1000) { let s = open(); let x = Box(1); stall(); if close(take
/// s) { print(x); release x } else { <release x only when released> } }`, with a peek at `s`.
fn canceled_close(released: bool, peek: Peek) -> Program<Rc> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let deadline = l.record(&[c.u64, c.u64]);
    let stream = l.scalar(ScalarKind::U64);
    let record = l.get_mut(stream);
    record.kind = LayoutKind::Handle;
    record.drop = vec![DropStep::CloseHandle { offset: 0 }];
    record.copy = CopyRecipe::NotCopyable;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let clock = f.int(CLOCK);
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
    let s = f.b.owned(ValueRepr::Handle, stream);
    let open = f.b.operation(op("open"));
    f.call(open, &[], Some(s));
    let identity = f.b.plain(ValueRepr::Handle, stream);
    let snapshot = f.b.operation(op("snapshot"));
    f.call(snapshot, &[], Some(identity));
    let peek_op = f.b.operation(op("peek"));
    let peek_args = [(Passing::Borrow, ArgumentValue::Value(identity))];
    if peek == Peek::BeforeClose {
        f.call(peek_op, &peek_args, None);
    }
    let one = f.int(1);
    let x = f.boxed(one);
    let until = f.int(100);
    let stalled = f.b.plain(BOOL, c.bool);
    let stall = f.b.operation(op("stall"));
    f.call(
        stall,
        &[
            (Passing::Borrow, ArgumentValue::Value(clock)),
            (Passing::Borrow, ArgumentValue::Value(until)),
        ],
        Some(stalled),
    );
    let closed = f.b.plain(BOOL, c.bool);
    let close = f.b.operation(op("close"));
    f.call(
        close,
        &[(Passing::Take, ArgumentValue::Value(s))],
        Some(closed),
    );
    if peek == Peek::AfterClose {
        f.call(peek_op, &peek_args, None);
    }
    let finished = f.b.block(BlockKind::Body, &[]);
    let canceled = f.b.block(BlockKind::Body, &[]);
    let join = f.b.block(BlockKind::Body, &[]);
    let yes = f.b.edge(finished, &[]);
    let no = f.b.edge(canceled, &[]);
    f.b.term(
        f.cur,
        Terminator::Branch {
            condition: closed,
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
    f.cur = canceled;
    if released {
        f.release(x, c.boxed);
    }
    let to_join = f.b.edge(join, &[]);
    f.b.term(f.cur, Terminator::Jump(to_join));
    f.cur = join;
    let error = f.b.constant(BOOL, c.bool, ConstantValue::Bool(false));
    let no_error = f.b.plain(BOOL, c.bool);
    f.inst(Inst::Const {
        result: no_error,
        constant: error,
    });
    let exit = f.b.operation(intrinsic("TaskScopeExitUntil"));
    let expired = f.b.plain(BOOL, c.bool);
    f.call(
        exit,
        &[
            (Passing::Borrow, ArgumentValue::Value(scope)),
            (Passing::Borrow, ArgumentValue::Value(no_error)),
        ],
        Some(expired),
    );
    f.ret(None);
    program(target, table.clone(), vec![f.finish(&table)])
}

fn campaign(released: bool, peek: Peek) -> hirsel_heapcheck::Campaign {
    fault_campaign(
        &canceled_close(released, peek),
        &path("main"),
        &[],
        &mut || Box::new(Streams { now: 0 }),
        &Config::default(),
    )
}

#[test]
fn a_leak_on_the_path_where_a_canceled_close_takes_its_stream_is_found() {
    // Only the deadline ends the stall, so the clean run takes the canceled close: the broken
    // twin's leak is found there, and the fixed twin passes every run with its stream closed.
    let broken = campaign(false, Peek::Never);
    let (at, violation) = broken.first_violation().expect("the canceled path leaks");
    assert_eq!(at, Perturbation::Clean);
    assert_eq!(violation.kind, ViolationKind::LeakedCredit);
    let fixed = campaign(true, Peek::Never);
    assert!(fixed.first_violation().is_none(), "{fixed:?}");
    assert_eq!(fixed.clean.output, b"");
    assert!(!fixed.clean.result.unwrap().observation.blocked);
}

#[test]
fn a_use_of_the_stream_a_canceled_close_took_is_found() {
    // The canceled close closed the stream: a later use of its identity is a use after free,
    // and the same use before the close passes every run.
    let late = campaign(true, Peek::AfterClose);
    let (at, violation) = late.first_violation().expect("the stream is closed");
    assert_eq!(at, Perturbation::Clean);
    assert_eq!(violation.kind, ViolationKind::UseAfterFree);
    let early = campaign(true, Peek::BeforeClose);
    assert!(early.first_violation().is_none(), "{early:?}");
}
