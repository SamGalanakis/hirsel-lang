//! `task.isolate[R](tasks, take body)` (FAIL-2) over the runtime's `isolate.rs`.
//!
//! The body reads the running task's failure record count, then calls `body`. A
//! successful call returns `Ok(r)`. A failed one has unwound the callback's
//! frames (FAIL-3) when its failure cleanup reaches the `Isolate` terminator,
//! which catches every record since that count and resumes here. The caught
//! chain becomes the `Failure` value innermost first: each record's fields come
//! from the `$intrinsic.Isolate*` entries, and each record but the primary is
//! boxed as the cause of the one before it.
use super::LibraryBody;
use super::arena::{RESULT_ERROR, RESULT_OK, payload, variant};
use super::builder::{Body, Cx};
use super::iter::{drop_local, local, option};
use super::slots::{construct, nominal};
use super::tasks::{Intrinsic, argument, intrinsic, invoke, operation, parameter};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;

type R<T> = Result<T, PassError>;

pub static ISOLATE: LibraryBody = LibraryBody {
    path: || DefPath::core("task.isolate"),
    build: isolate,
};

/// `FailureKind`'s variants, by the ordinal `$intrinsic.IsolateKind` returns.
const KINDS: u32 = 5;

pub(super) struct Entries {
    begin: Intrinsic,
    count: Intrinsic,
    kind: Intrinsic,
    rule: Intrinsic,
    definition: Intrinsic,
    line: Intrinsic,
    column: Intrinsic,
    spawn: Intrinsic,
}

/// The layouts of a `Failure` record and its parts.
pub(super) struct Shape {
    failure: LayoutId,
    kind: LayoutId,
    site: LayoutId,
    /// `Option[box Failure]` and its `Some` payload, the box.
    cause: LayoutId,
    boxed: LayoutId,
}

fn field(b: &Body<'_, '_>, layout: LayoutId, ordinal: u32) -> R<LayoutId> {
    b.record(layout)
        .fields
        .iter()
        .find(|f| f.ordinal == ordinal)
        .map(|f| f.layout)
        .ok_or_else(|| PassError::Malformed(format!("a Failure part has no field {ordinal}")))
}

fn isolate(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let callback = parameter(cx.types, cx.callable, 1)?;
    let body_abi = cx
        .layouts
        .fn_abi(cx.types, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let returned = cx
        .fn_abi()
        .result
        .map(|r| r.layout)
        .ok_or_else(|| PassError::Malformed("task.isolate returns a Result".into()))?;
    let entries = entries(cx)?;
    let mut b = Body::new(cx)?;
    let &[tasks, body] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("task.isolate parameters".into()));
    };
    let failure = payload(&b, returned, RESULT_ERROR)
        .ok_or_else(|| PassError::Malformed("task.isolate's Result has no error".into()))?;
    let shape = shape(&b, failure)?;
    let tasks = b.read_bits(tasks.address)?;
    let propagate = b.failure(None);
    let callee = operation(&mut b, &entries.begin);
    let start = invoke(
        &mut b,
        callee,
        entries.begin.abi,
        vec![argument(0, Passing::Borrow, tasks)],
        &[],
        propagate,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("IsolateBegin returns a count".into()))?;
    // The callable dies with this frame on both outcomes.
    let f = b.take(body.address)?;
    let (f_slot, f_address) = local(&mut b, f)?;

    // A failed callback has unwound; the catch resumes ordinary code.
    let normal = b.current();
    let catching = b.block(BlockKind::FailureCleanup, &[]);
    let caught = b.block(BlockKind::Body, &[]);
    b.at(catching);
    let resume = b.edge(caught, &[]);
    b.terminate(Terminator::Isolate {
        start,
        caught: resume,
    });
    b.at(normal);
    let unwound = b.failure_to(catching, &[], None);
    let (callee, loan) = b.borrow(f_address, false)?;
    let value = invoke(
        &mut b,
        (Callee::Indirect(callee), Vec::new()),
        body_abi,
        Vec::new(),
        &[loan],
        unwound,
        BlockKind::Body,
    );
    // Dropping the callback can fail. Keep its owned result in the frame's
    // cleanup until that drop succeeds, then transfer it into Ok.
    let held = value.map(|value| local(&mut b, value)).transpose()?;
    drop_local(&mut b, f_slot, f_address);
    let ok = match held {
        Some((slot, address)) => {
            let value = b.take(address)?;
            b.push(Inst::StorageDead {
                slot,
                if_live: false,
            });
            vec![value]
        }
        None => match payload(&b, returned, RESULT_OK) {
            Some(unit) => vec![b.constant(unit, ConstantValue::Unit)],
            None => Vec::new(),
        },
    };
    let value = variant(&mut b, returned, "Result", RESULT_OK, &ok);
    b.ret(Some(value));

    b.at(caught);
    drop_local(&mut b, f_slot, f_address);
    let failure = chain(&mut b, &entries, &shape, tasks)?;
    let value = variant(&mut b, returned, "Result", RESULT_ERROR, &[failure]);
    b.ret(Some(value));
    Ok(b.finish())
}

/// The caught chain's entries, under no actuals.
pub(super) fn entries(cx: &mut Cx<'_>) -> R<Entries> {
    let mut entry = |name: &str| intrinsic(cx, name, Vec::new());
    Ok(Entries {
        begin: entry("IsolateBegin")?,
        count: entry("IsolateCount")?,
        kind: entry("IsolateKind")?,
        rule: entry("IsolateRule")?,
        definition: entry("IsolateDefinition")?,
        line: entry("IsolateLine")?,
        column: entry("IsolateColumn")?,
        spawn: entry("IsolateSpawn")?,
    })
}

/// The parts of the `Failure` record at `failure`.
pub(super) fn shape(b: &Body<'_, '_>, failure: LayoutId) -> R<Shape> {
    let cause = field(b, failure, 4)?;
    Ok(Shape {
        failure,
        kind: field(b, failure, 0)?,
        site: field(b, failure, 2)?,
        cause,
        boxed: payload(b, cause, 1)
            .ok_or_else(|| PassError::Malformed("a Failure cause holds no box".into()))?,
    })
}

/// `IsolateBegin(tasks)`: the running task's failure record count, the start of a catch.
pub(super) fn begin(b: &mut Body<'_, '_>, entries: &Entries, tasks: ValueId) -> R<ValueId> {
    read(b, &entries.begin, tasks, None)
}

/// Call a runtime entry of the caught chain; its outcome propagates.
fn read(
    b: &mut Body<'_, '_>,
    entry: &Intrinsic,
    tasks: ValueId,
    index: Option<ValueId>,
) -> R<ValueId> {
    let mut arguments = vec![argument(0, Passing::Borrow, tasks)];
    arguments.extend(index.map(|i| argument(1, Passing::Borrow, i)));
    let failure = b.failure(None);
    let callee = operation(b, entry);
    invoke(
        b,
        callee,
        entry.abi,
        arguments,
        &[],
        failure,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("an isolation entry without its result".into()))
}

/// The caught chain as one `Failure`, built from its last record to its primary.
pub(super) fn chain(
    b: &mut Body<'_, '_>,
    entries: &Entries,
    shape: &Shape,
    tasks: ValueId,
) -> R<ValueId> {
    let count = read(b, &entries.count, tasks, None)?;
    let none = option(b, shape.cause, 0, &[]);
    let (cause_slot, cause_address) = local(b, none)?;
    let all_ones = b.u64(u64::MAX)?;
    let last = b.add(ArithMode::Wrapping, count, all_ones);
    let u64_layout = b.layout_of(count);
    let index = b.value(u64_layout, Holding::Plain);
    let version = b.version(cause_slot);
    let initial = b.reaching(cause_slot)?;
    let record = b.block(BlockKind::Body, &[index, version]);
    b.jump(record, &[last, initial]);

    b.at(record);
    b.set_reaching(cause_slot, version);
    let zero = b.u64(0)?;
    let primary = b.compare(BinaryOp::Eq, index, zero)?;
    let failure = failure_at(b, entries, shape, tasks, index, cause_address)?;
    let done = b.block(BlockKind::Body, &[]);
    let more = b.block(BlockKind::Body, &[]);
    b.branch(primary, done, more);

    b.at(more);
    let boxed = construct(b, shape.boxed, ConstructorKind::Box, &[failure])?;
    let some = option(b, shape.cause, 1, &[boxed]);
    b.store(cause_address, some)?;
    b.hold(cause_slot, cause_address);
    let previous = b.add(ArithMode::Wrapping, index, all_ones);
    let after = b.reaching(cause_slot)?;
    b.jump(record, &[previous, after]);

    b.at(done);
    b.emptied(cause_slot);
    b.push(Inst::StorageDead {
        slot: cause_slot,
        if_live: false,
    });
    Ok(failure)
}

/// Caught record `index` as a `Failure` whose cause is moved out of `cause`.
/// Each construct consumes its fields on both outcomes, so the site is built
/// before the record's other owned parts leave their temporaries.
fn failure_at(
    b: &mut Body<'_, '_>,
    entries: &Entries,
    shape: &Shape,
    tasks: ValueId,
    index: ValueId,
    cause: AddressId,
) -> R<ValueId> {
    let kind = kind(b, entries, shape, tasks, index)?;
    let line = read(b, &entries.line, tasks, Some(index))?;
    let column = read(b, &entries.column, tasks, Some(index))?;
    let mut held = Vec::new();
    for entry_point in [&entries.rule, &entries.definition, &entries.spawn] {
        let value = read(b, entry_point, tasks, Some(index))?;
        held.push(local(b, value)?);
    }
    let take = |b: &mut Body<'_, '_>, (slot, address): (SlotId, AddressId)| -> R<ValueId> {
        let value = b.take(address)?;
        b.push(Inst::StorageDead {
            slot,
            if_live: false,
        });
        Ok(value)
    };
    let definition = take(b, held[1])?;
    let symbol = nominal(b, "SourceSite");
    let site = construct(
        b,
        shape.site,
        ConstructorKind::Record(symbol),
        &[definition, line, column],
    )?;
    let rule = take(b, held[0])?;
    let spawn = take(b, held[2])?;
    let cause = b.take(cause)?;
    let symbol = nominal(b, "Failure");
    construct(
        b,
        shape.failure,
        ConstructorKind::Record(symbol),
        &[kind, rule, site, spawn, cause],
    )
}

/// Caught record `index`'s `FailureKind`, from its ordinal.
fn kind(
    b: &mut Body<'_, '_>,
    entries: &Entries,
    shape: &Shape,
    tasks: ValueId,
    index: ValueId,
) -> R<ValueId> {
    let ordinal = read(b, &entries.kind, tasks, Some(index))?;
    let value = b.value(shape.kind, Holding::Plain);
    let joined = b.block(BlockKind::Body, &[value]);
    let arms: Vec<_> = (0..KINDS).map(|_| b.block(BlockKind::Body, &[])).collect();
    let invalid = b.block(BlockKind::Body, &[]);
    let otherwise = b.edge(invalid, &[]);
    let edges: Vec<_> = arms.iter().map(|&arm| b.edge(arm, &[])).collect();
    let switch = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        edges
            .into_iter()
            .enumerate()
            .map(|(value, edge)| SwitchArm {
                value: value as u32,
                edge,
            }),
    );
    b.terminate(Terminator::Switch {
        value: ordinal,
        arms: switch,
        otherwise,
    });
    for (ordinal, arm) in arms.into_iter().enumerate() {
        b.at(arm);
        let kind = variant(b, shape.kind, "FailureKind", ordinal as u32, &[]);
        b.jump(joined, &[kind]);
    }
    b.at(invalid);
    b.terminate(Terminator::InvariantFault {
        reason: "a failure kind ordinal past FailureKind".into(),
    });
    b.at(joined);
    Ok(value)
}
