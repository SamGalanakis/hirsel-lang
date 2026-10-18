//! Mutexes and atomics (CONC-4, CONC-5, FAIL-4) over the runtime's cells
//! (`hirsel_rt_v2_mutex_*`, `hirsel_rt_v2_atomic_*`, `hirsel-rt/src/sync.rs`).
//!
//! A payload crosses as the `Some` image of an `Option` of known size, which
//! the runtime copies: the layout of `Mutex[T]` requires a pointer-free payload
//! with no drop, and an atomic's is a scalar. `mutex.with` takes the payload
//! into a local, calls the body with it `mut` and puts it back; a body that
//! fails kills the mutex and propagates. Acquiring a dead mutex fails FAIL-4,
//! continued by the chain of the body that killed it.
use super::LibraryBody;
use super::builder::{Body, Cx};
use super::iter::{drop_local, local, option};
use super::tasks::{
    Intrinsic, argument, exclusive, intrinsic, invoke, operation, option_of, parameter,
};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;
use hirsel_types::GenericArgument;

type R<T> = Result<T, PassError>;

pub static MUTEX_CREATE: LibraryBody = LibraryBody {
    path: || DefPath::core("mutex.create"),
    build: mutex_create,
};
pub static MUTEX_WITH: LibraryBody = LibraryBody {
    path: || DefPath::core("mutex.with"),
    build: mutex_with,
};
pub static ATOMIC_CREATE: LibraryBody = LibraryBody {
    path: || DefPath::core("atomic.create"),
    build: atomic_create,
};
pub static ATOMIC_LOAD: LibraryBody = LibraryBody {
    path: || DefPath::core("atomic.load"),
    build: atomic_load,
};
pub static ATOMIC_STORE: LibraryBody = LibraryBody {
    path: || DefPath::core("atomic.store"),
    build: atomic_store,
};

/// The payload type, its `Option`'s layout and size, and the named entries.
fn entries(cx: &mut Cx<'_>, names: &[&str]) -> R<(LayoutId, u64, Vec<Intrinsic>)> {
    let payload = cx.type_actual(0)?;
    let (option_layout, size) = option_of(cx, payload)?;
    let entries = names
        .iter()
        .map(|name| intrinsic(cx, name, vec![GenericArgument::Type(payload)]))
        .collect::<R<Vec<_>>>()?;
    Ok((option_layout, size, entries))
}

/// Call `entry` and continue in a new body block; its result.
fn call(
    b: &mut Body<'_, '_>,
    entry: &Intrinsic,
    arguments: Vec<CallArgument>,
    loans: &[LoanId],
) -> Option<ValueId> {
    let failure = b.failure(None);
    let callee = operation(b, entry);
    invoke(
        b,
        callee,
        entry.abi,
        arguments,
        loans,
        failure,
        BlockKind::Body,
    )
}

/// Pass `Some(value)` to `entry` by reference, with `handle` first when given, and
/// the image's `size` last; the local holding it is dropped after the call.
fn put(
    b: &mut Body<'_, '_>,
    entry: &Intrinsic,
    handle: Option<ValueId>,
    option_layout: LayoutId,
    size: u64,
    value: ValueId,
) -> R<Option<ValueId>> {
    let some = option(b, option_layout, 1, &[value]);
    let (slot, address) = local(b, some)?;
    let (view, loan) = b.borrow(address, false)?;
    let size = b.u64(size)?;
    let first = u32::from(handle.is_some());
    let mut arguments: Vec<_> = handle
        .map(|h| argument(0, Passing::Borrow, h))
        .into_iter()
        .collect();
    arguments.push(argument(first, Passing::Borrow, view));
    arguments.push(argument(first + 1, Passing::Borrow, size));
    let result = call(b, entry, arguments, &[loan]);
    drop_local(b, slot, address);
    Ok(result)
}

/// Move the payload out of the `Some` an entry wrote into the local `slot`, whose
/// storage then ends.
fn take_some(
    b: &mut Body<'_, '_>,
    slot: SlotId,
    address: AddressId,
    option_layout: LayoutId,
) -> R<ValueId> {
    let (tag, tag_loan) = b.borrow(address, false)?;
    let some = b.block(BlockKind::Body, &[]);
    let broken = b.block(BlockKind::Body, &[]);
    let some_edge = b.edge(some, &[]);
    let broken_edge = b.edge(broken, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm {
            value: 1,
            edge: some_edge,
        }],
    );
    b.terminate(Terminator::Switch {
        value: tag,
        arms,
        otherwise: broken_edge,
    });
    b.at(broken);
    b.end_loan(tag_loan);
    b.terminate(Terminator::InvariantFault {
        reason: "a mutex or atomic cell delivered no payload".into(),
    });
    b.at(some);
    b.end_loan(tag_loan);
    let payload = b.record(option_layout).tag.as_ref().unwrap().variants[..]
        .iter()
        .find(|v| v.ordinal == 1)
        .unwrap()
        .fields[0]
        .layout;
    let field = b.place(
        AddressBase::Address(address),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        payload,
        None,
    )?;
    let value = b.take(field)?;
    b.emptied(slot);
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
    Ok(value)
}

/// A `None` local for an entry to write a payload into.
fn none_local(b: &mut Body<'_, '_>, option_layout: LayoutId) -> R<(SlotId, AddressId)> {
    let none = option(b, option_layout, 0, &[]);
    local(b, none)
}

/// End a cleanup block by continuing into the cleanup `failure` leads to.
fn continue_cleanup(b: &mut Body<'_, '_>, failure: FailureId) {
    let edge = b.ed.unit.tables.failures[failure.index()].cleanup;
    let target = b.ed.unit.tables.edges[edge.index()].target;
    b.jump(target, &[]);
}

fn ret_unit(b: &mut Body<'_, '_>) {
    let unit =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(unit);
}

/// `mutex.create(tasks, take v) -> Mutex[T]`.
fn mutex_create(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let (option_layout, size, entries) = entries(cx, &["MutexCreate"])?;
    let mut b = Body::new(cx)?;
    let &[_, v] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("mutex.create parameters".into()));
    };
    let value = b.take(v.address)?;
    let mutex = put(&mut b, &entries[0], None, option_layout, size, value)?;
    b.ret(mutex);
    Ok(b.finish())
}

/// `mutex.with(tasks, m, body) -> R`.
fn mutex_with(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let callback = parameter(&cx.store, cx.callable, 2)?;
    let body_abi = cx
        .layouts
        .fn_abi(&cx.store, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let (option_layout, size, entries) =
        entries(cx, &["MutexTake", "MutexPut", "MutexKill", "MutexCause"])?;
    let [take, put_back, kill, cause] = &entries[..] else {
        unreachable!()
    };
    let mut b = Body::new(cx)?;
    let &[_, m, body] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("mutex.with parameters".into()));
    };
    let m = b.read_bits(m.address)?;
    let (slot, address) = none_local(&mut b, option_layout)?;
    let size_value = b.u64(size)?;
    let arguments = vec![
        argument(0, Passing::Borrow, m),
        exclusive(&mut b, 1, slot, address),
        argument(2, Passing::Borrow, size_value),
    ];
    let status = call(&mut b, take, arguments, &[])
        .ok_or_else(|| PassError::Malformed("a mutex acquisition without a status".into()))?;
    let held = b.held_state();
    let status_layout = b.layout_of(status);
    let zero = b.constant(status_layout, ConstantValue::Integer(0));
    let acquired = b.compare(BinaryOp::Eq, status, zero)?;
    let (taken, dead) = (b.block(BlockKind::Body, &[]), b.block(BlockKind::Body, &[]));
    b.branch(acquired, taken, dead);

    // FAIL-4: a dead mutex fails here, continued by the chain that killed it.
    let continued = b.block(BlockKind::FailureCleanup, &[]);
    b.at(continued);
    let rest = b.failure(None);
    let callee = operation(&mut b, cause);
    invoke(
        &mut b,
        callee,
        cause.abi,
        vec![argument(0, Passing::Borrow, m)],
        &[],
        rest,
        BlockKind::FailureCleanup,
    );
    continue_cleanup(&mut b, rest);
    b.at(dead);
    let raised = b.failure_to(continued, &[], Some("FAIL-4"));
    b.terminate(Terminator::Raise(raised));

    b.at(taken);
    b.restore_held(&held);
    let value = take_some(&mut b, slot, address, option_layout)?;
    let (payload, payload_address) = local(&mut b, value)?;
    // A failed body kills the mutex, and its payload is dropped.
    let normal = b.current();
    let unwinding = b.block(BlockKind::FailureCleanup, &[]);
    b.at(unwinding);
    let rest = b.failure(None);
    let callee = operation(&mut b, kill);
    invoke(
        &mut b,
        callee,
        kill.abi,
        vec![argument(0, Passing::Borrow, m)],
        &[],
        rest,
        BlockKind::FailureCleanup,
    );
    continue_cleanup(&mut b, rest);
    b.at(normal);
    let failed = b.failure_to(unwinding, &[], None);
    let (callee, loan) = b.borrow(body.address, false)?;
    let arguments = vec![exclusive(&mut b, 0, payload, payload_address)];
    let result = invoke(
        &mut b,
        (Callee::Indirect(callee), Vec::new()),
        body_abi,
        arguments,
        &[loan],
        failed,
        BlockKind::Body,
    );
    let kept = result.map(|r| local(&mut b, r)).transpose()?;
    let value = b.take(payload_address)?;
    b.emptied(payload);
    b.push(Inst::StorageDead {
        slot: payload,
        if_live: false,
    });
    put(&mut b, put_back, Some(m), option_layout, size, value)?;
    let result = match kept {
        Some((slot, address)) => {
            let value = b.take(address)?;
            b.emptied(slot);
            b.push(Inst::StorageDead {
                slot,
                if_live: false,
            });
            Some(value)
        }
        None => None,
    };
    b.ret(result);
    Ok(b.finish())
}

/// `atomic.create(tasks, value) -> Atomic[A]`.
fn atomic_create(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let (option_layout, size, entries) = entries(cx, &["AtomicCreate"])?;
    let mut b = Body::new(cx)?;
    let &[_, v] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("atomic.create parameters".into()));
    };
    let value = b.read_bits(v.address)?;
    let atomic = put(&mut b, &entries[0], None, option_layout, size, value)?;
    b.ret(atomic);
    Ok(b.finish())
}

/// `atomic.load(tasks, a) -> A`.
fn atomic_load(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let (option_layout, size, entries) = entries(cx, &["AtomicLoad"])?;
    let mut b = Body::new(cx)?;
    let &[_, a] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("atomic.load parameters".into()));
    };
    let a = b.read_bits(a.address)?;
    let (slot, address) = none_local(&mut b, option_layout)?;
    let size = b.u64(size)?;
    let arguments = vec![
        argument(0, Passing::Borrow, a),
        exclusive(&mut b, 1, slot, address),
        argument(2, Passing::Borrow, size),
    ];
    call(&mut b, &entries[0], arguments, &[]);
    let value = take_some(&mut b, slot, address, option_layout)?;
    b.ret(Some(value));
    Ok(b.finish())
}

/// `atomic.store(tasks, a, value)`.
fn atomic_store(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let (option_layout, size, entries) = entries(cx, &["AtomicStore"])?;
    let mut b = Body::new(cx)?;
    let &[_, a, v] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("atomic.store parameters".into()));
    };
    let a = b.read_bits(a.address)?;
    let value = b.read_bits(v.address)?;
    put(&mut b, &entries[0], Some(a), option_layout, size, value)?;
    ret_unit(&mut b);
    Ok(b.finish())
}
