//! Structured concurrency (CONC-1, CONC-2) over the native scheduler's entries
//! (`hirsel_rt_v2_task_*`, `hirsel-rt/src/tasks.rs`).
//!
//! A result crosses tasks as a whole `Option[R]` image, so no body depends on
//! `R`'s layout beyond its size. A child runs `$native.task_main[F]`, a closure
//! whose environment holds the spawned `f`: it calls `f`, puts `Some(r)` in a
//! local slot and hands the slot to `$intrinsic.TaskFinish`, which parks the
//! child until its result is joined (the slot then holds `None`) or discarded
//! (it still holds `Some(r)`); either way the child's own drop of the slot is
//! the result's one release. `task.await` passes a `None` slot to
//! `$intrinsic.TaskJoin`, which moves the child's image in.
//!
//! `task.scope` enters a scope, calls its body with it, and exits the scope
//! on both outcomes: a failed body, or one returning `Err` when `R` is a
//! `Result`, exits as an error, which cancels the children before joining
//! them; a failed child fails the exit, and the body's result is dropped.
use super::builder::{Body, Cx};
use super::iter::{local, option};
use super::{BINDER, LibraryBody, Request, native_path};
use crate::PassError;
use crate::instance::{Actuals, InstanceError, instantiate_function};
use hirsel_core_contract::{OperationId, Passing};
use hirsel_layout::layout::EnvironmentShape;
use hirsel_lir::abi_types::{CallableShape, FnAbiId};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::operation::OperationArgument;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;
use hirsel_types::{GenericArgument, SignatureKind, Type, TypeId, TypeKind};

type R<T> = Result<T, PassError>;

pub static SCOPE: LibraryBody = LibraryBody {
    path: || DefPath::core("task.scope"),
    build: scope,
};
pub static ASYNC: LibraryBody = LibraryBody {
    path: || DefPath::core("task.async"),
    build: spawn,
};
pub static AWAIT: LibraryBody = LibraryBody {
    path: || DefPath::core("task.await"),
    build: join,
};
pub static MAIN: LibraryBody = LibraryBody {
    path: || native_path("task_main"),
    build: main,
};

/// A runtime entry under this body's actuals: its operation, its `FnAbi` and
/// its callable type, a row of `store` (a copy of the body's store).
struct Intrinsic {
    operation: OperationId,
    abi: FnAbiId,
    /// The actuals the operation's declared ABI is derived under.
    arguments: Vec<OperationArgument>,
    store: hirsel_types::TypeStore,
    callable: TypeId,
}

fn intrinsic(cx: &mut Cx<'_>, name: &str, arguments: Vec<GenericArgument>) -> R<Intrinsic> {
    let path = DefPath::core(&format!("$intrinsic.{name}"));
    let operation = cx
        .program
        .operations
        .get(&path)
        .cloned()
        .ok_or_else(|| PassError::Malformed(format!("{path:?} is not an operation")))?;
    let signature = cx
        .program
        .signatures
        .get(&path)
        .ok_or_else(|| InstanceError::MissingSignature(path.clone()))?
        .clone();
    let SignatureKind::Function(function) = &signature.kind else {
        return Err(PassError::Malformed(format!("{path:?} is not a function")));
    };
    let mut actuals = Actuals {
        binder: (!arguments.is_empty()).then_some(BINDER),
        store: cx.store.clone(),
        arguments,
        outer: Vec::new(),
    };
    let callable = instantiate_function(
        &signature,
        function,
        hirsel_types::Modes::global(),
        &mut actuals,
    )
    .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
    let abi = cx
        .layouts
        .fn_abi(&actuals.store, callable, CallableShape::Definition)
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    let arguments = hirsel_layout::operation::collect_arguments(&actuals.store, &actuals.arguments)
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    Ok(Intrinsic {
        operation,
        abi,
        arguments,
        store: actuals.store,
        callable,
    })
}

/// The type of parameter `ordinal` of a `Function` row.
fn parameter(store: &hirsel_types::TypeStore, callable: TypeId, ordinal: usize) -> R<TypeId> {
    match store.get(callable).map(|t| &t.kind) {
        Some(TypeKind::Function(f)) if f.parameters.len() > ordinal => Ok(f.parameters[ordinal].ty),
        _ => Err(PassError::Malformed(format!(
            "a task callable has no parameter {ordinal}"
        ))),
    }
}

/// `Option[R]` in the body's store, its layout and its storage bytes.
fn option_of(cx: &mut Cx<'_>, result: TypeId) -> R<(LayoutId, u64)> {
    let ty = cx.store.push(Type::with_kind(TypeKind::Named {
        definition: DefPath::core("Option"),
        arguments: vec![GenericArgument::Type(result)].into(),
    }));
    let layout = cx.layout(ty)?;
    let size = cx
        .record(layout)
        .size
        .storage()
        .ok_or_else(|| PassError::Malformed("a task result has no fixed size".into()))?;
    Ok((layout, size))
}

fn argument(parameter: u32, passing: Passing, value: ValueId) -> CallArgument {
    CallArgument {
        parameter,
        passing,
        value: ArgumentValue::Value(value),
    }
}

/// A `mut` argument: the slot's new version is defined at the call.
fn exclusive(
    b: &mut Body<'_, '_>,
    parameter: u32,
    slot: SlotId,
    address: AddressId,
) -> CallArgument {
    let version = b.version(slot);
    b.set_reaching(slot, version);
    CallArgument {
        parameter,
        passing: Passing::Mut,
        value: ArgumentValue::Address { address, version },
    }
}

/// Call `callee` and continue in a new block of kind `after`. Both outcomes
/// end `loans`, in reverse; a failure then continues along `failure`'s edge.
fn invoke(
    b: &mut Body<'_, '_>,
    callee: (Callee, Vec<OperationArgument>),
    abi: FnAbiId,
    arguments: Vec<CallArgument>,
    loans: &[LoanId],
    failure: FailureId,
    after: BlockKind,
) -> Option<ValueId> {
    let shape = b.cx.layouts.table().fn_abis[abi.0 as usize].clone();
    let result = shape.result.map(|r| b.fresh(r.layout));
    let failure = if loans.is_empty() {
        failure
    } else {
        let at = b.current();
        let ending = b.block(BlockKind::FailureCleanup, &[]);
        b.at(ending);
        for &loan in loans.iter().rev() {
            b.end_loan(loan);
        }
        let tail = b.ed.unit.tables.failures[failure.index()].cleanup;
        b.terminate(Terminator::Jump(tail));
        b.at(at);
        b.failure_to(ending, &[], None)
    };
    let next = b.block(after, &[]);
    let normal = b.edge(next, &[]);
    let arguments = PoolRange::push(&mut b.ed.unit.pools.call_arguments, arguments);
    let call = CallId(b.ed.unit.tables.calls.len() as u32);
    b.ed.unit.tables.calls.push(CallSite {
        operation_arguments: callee.1,
        callee: callee.0,
        abi,
        arguments,
        result,
        destination: shape.destination.then_some(DestinationClass::Inferred),
        declared: crate::import::CALL_EFFECTS,
        anchor: b.anchor,
    });
    b.terminate(Terminator::Call {
        call,
        normal: Some(normal),
        failure,
    });
    b.at(next);
    for &loan in loans.iter().rev() {
        b.end_loan(loan);
    }
    result
}

fn operation(b: &mut Body<'_, '_>, entry: &Intrinsic) -> (Callee, Vec<OperationArgument>) {
    let reference = OperationRef(b.ed.unit.tables.operations.len() as u32);
    b.ed.unit.tables.operations.push(entry.operation.clone());
    (Callee::Operation(reference), entry.arguments.clone())
}

fn scope(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let result = cx.type_actual(0)?;
    let is_result = matches!(
        cx.store.get(result).map(|t| &t.kind),
        Some(TypeKind::Named { definition, .. }) if *definition == DefPath::core("Result")
    );
    let callback = parameter(&cx.store, cx.callable, 1)?;
    let body_abi = cx
        .layouts
        .fn_abi(&cx.store, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let enter = intrinsic(cx, "TaskScopeEnter", Vec::new())?;
    let exit = intrinsic(cx, "TaskScopeExit", Vec::new())?;
    let mut b = Body::new(cx)?;
    let &[tasks, body] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("task.scope parameters".into()));
    };
    let tasks = b.read_bits(tasks.address)?;
    let propagate = b.failure(None);
    let callee = operation(&mut b, &enter);
    let s = invoke(
        &mut b,
        callee,
        enter.abi,
        vec![argument(0, Passing::Borrow, tasks)],
        &[],
        propagate,
        BlockKind::Body,
    )
    .expect("a scope");
    let bool_layout = b.bool_layout()?;

    // A failed body exits the scope as an error, then propagates.
    let normal = b.current();
    let unwinding = b.block(BlockKind::FailureCleanup, &[]);
    b.at(unwinding);
    let error = b.constant(bool_layout, ConstantValue::Bool(true));
    let propagate = b.failure(None);
    let callee = operation(&mut b, &exit);
    invoke(
        &mut b,
        callee,
        exit.abi,
        vec![
            argument(0, Passing::Borrow, s),
            argument(1, Passing::Borrow, error),
        ],
        &[],
        propagate,
        BlockKind::FailureCleanup,
    );
    b.terminate(Terminator::Propagate);
    b.at(normal);
    let unwound = b.failure_to(unwinding, &[], None);

    let (callee, loan) = b.borrow(body.address, false)?;
    let value = invoke(
        &mut b,
        (Callee::Indirect(callee), Vec::new()),
        body_abi,
        vec![argument(0, Passing::Borrow, s)],
        &[loan],
        unwound,
        BlockKind::Body,
    );
    let held = value.map(|v| local(&mut b, v)).transpose()?;
    let flag = b.value(bool_layout, Holding::Plain);
    let joined = b.block(BlockKind::Body, &[flag]);
    match held {
        Some((_, address)) if is_result => {
            let (tag, tag_loan) = b.borrow(address, false)?;
            let failed = b.block(BlockKind::Body, &[]);
            let succeeded = b.block(BlockKind::Body, &[]);
            let failed_edge = b.edge(failed, &[]);
            let succeeded_edge = b.edge(succeeded, &[]);
            let arms = PoolRange::push(
                &mut b.ed.unit.pools.switch_arms,
                [SwitchArm {
                    value: 1,
                    edge: failed_edge,
                }],
            );
            b.terminate(Terminator::Switch {
                value: tag,
                arms,
                otherwise: succeeded_edge,
            });
            for (block, error) in [(failed, true), (succeeded, false)] {
                b.at(block);
                b.end_loan(tag_loan);
                let error = b.constant(bool_layout, ConstantValue::Bool(error));
                b.jump(joined, &[error]);
            }
        }
        _ => {
            let error = b.constant(bool_layout, ConstantValue::Bool(false));
            b.jump(joined, &[error]);
        }
    }
    b.at(joined);
    // A failed exit drops the body's result with the rest of the frame.
    let failure = b.failure(None);
    let callee = operation(&mut b, &exit);
    invoke(
        &mut b,
        callee,
        exit.abi,
        vec![
            argument(0, Passing::Borrow, s),
            argument(1, Passing::Borrow, flag),
        ],
        &[],
        failure,
        BlockKind::Body,
    );
    let value = match held {
        Some((slot, address)) => {
            let value = b.take(address)?;
            b.push(Inst::StorageDead {
                slot,
                if_live: false,
            });
            Some(value)
        }
        None => None,
    };
    b.ret(value);
    Ok(b.finish())
}

fn spawn(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let result = cx.type_actual(0)?;
    let f = parameter(&cx.store, cx.callable, 1)?;
    let entry = intrinsic(cx, "TaskSpawn", vec![GenericArgument::Type(result)])?;
    // The child's code: `$native.task_main` keyed by the spawned callable's type,
    // with the intrinsic's `once fn() -> ()` as its own callable type.
    let path = native_path("task_main");
    let main_ty = parameter(&entry.store, entry.callable, 1)?;
    let environment = cx
        .layouts
        .environment(&EnvironmentShape {
            definition: &path,
            body: 0,
            store: &entry.store,
            captures: &[(f, Passing::Take)],
            sizes: &[],
            movable: &[],
        })
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    let mut request = Request::native(
        super::body_index(&path).expect("registered task main"),
        &path,
        entry.store.clone(),
        vec![GenericArgument::Type(f)],
        main_ty,
    );
    request.shape = CallableShape::Closure;
    request.environment = Some(environment);
    let direct = cx.require(request)?;
    let spawn_abi = cx.layouts.table().fn_abis[entry.abi.0 as usize].clone();
    let main_layout = spawn_abi.parameters[1].layout;

    let mut b = Body::new(cx)?;
    let &[s, f] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("task.async parameters".into()));
    };
    let s = b.read_bits(s.address)?;
    let f = b.take(f.address)?;
    let code = b.symbol(direct.symbol);
    let captures = PoolRange::push(
        &mut b.ed.unit.pools.captures,
        [CaptureValue {
            value: f,
            usage: CaptureUse::Take,
        }],
    );
    let closure = ClosureId(b.ed.unit.tables.closures.len() as u32);
    b.ed.unit.tables.closures.push(ClosureSite {
        code: Some(ClosureCode {
            symbol: code,
            abi: direct.abi,
        }),
        environment,
        captures,
        destination: DestinationClass::Inferred,
        placement: None,
        anchor: b.anchor,
    });
    let main = b.fresh(main_layout);
    let failure = b.failure(None);
    b.push(Inst::Closure {
        result: main,
        closure,
        failure,
    });
    // A take argument transfers at entry: a failed spawn releases `main` itself.
    let failure = b.failure(None);
    let callee = operation(&mut b, &entry);
    let task = invoke(
        &mut b,
        callee,
        entry.abi,
        vec![
            argument(0, Passing::Borrow, s),
            argument(1, Passing::Take, main),
        ],
        &[],
        failure,
        BlockKind::Body,
    );
    b.ret(task);
    Ok(b.finish())
}

fn join(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let result = cx.type_actual(0)?;
    let (option_layout, size) = option_of(cx, result)?;
    let entry = intrinsic(cx, "TaskJoin", vec![GenericArgument::Type(result)])?;
    let mut b = Body::new(cx)?;
    let &[s, h] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("task.await parameters".into()));
    };
    let s = b.read_bits(s.address)?;
    let h = b.take(h.address)?;
    let none = option(&mut b, option_layout, 0, &[]);
    let (slot, address) = local(&mut b, none)?;
    let size = b.u64(size)?;
    let failure = b.failure(None);
    let callee = operation(&mut b, &entry);
    let arguments = vec![
        argument(0, Passing::Borrow, s),
        argument(1, Passing::Take, h),
        exclusive(&mut b, 2, slot, address),
        argument(3, Passing::Borrow, size),
    ];
    invoke(
        &mut b,
        callee,
        entry.abi,
        arguments,
        &[],
        failure,
        BlockKind::Body,
    );
    // A completed join always moved `Some(r)` in.
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
        reason: "a joined task delivered no result".into(),
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
    b.ret(Some(value));
    Ok(b.finish())
}

fn main(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let f = cx.type_actual(0)?;
    let result = match cx.store.get(f).map(|t| &t.kind) {
        Some(TypeKind::Function(function)) => function.result,
        _ => {
            return Err(PassError::Malformed(
                "a task main captures no callable".into(),
            ));
        }
    };
    let f_abi = cx
        .layouts
        .fn_abi(&cx.store, f, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let (option_layout, size) = option_of(cx, result)?;
    let entry = intrinsic(cx, "TaskFinish", vec![GenericArgument::Type(result)])?;
    let environment = cx
        .environment()
        .ok_or_else(|| PassError::Malformed("a task main without environment".into()))?;
    let field = cx.record(environment).fields[0].layout;
    let mut b = Body::new(cx)?;
    let f = b.capture(0, field)?;
    let (callee, loan) = b.borrow(f.address, false)?;
    let propagate = b.failure(None);
    let value = invoke(
        &mut b,
        (Callee::Indirect(callee), Vec::new()),
        f_abi,
        Vec::new(),
        &[loan],
        propagate,
        BlockKind::Body,
    );
    let value = match value {
        Some(value) => value,
        None => {
            let ty = b.cx.store.push(Type::unit());
            let unit = b.cx.layout(ty)?;
            b.constant(unit, ConstantValue::Unit)
        }
    };
    let some = option(&mut b, option_layout, 1, &[value]);
    let (slot, address) = local(&mut b, some)?;
    let none = option(&mut b, option_layout, 0, &[]);
    let (none_slot, none_address) = local(&mut b, none)?;
    let size = b.u64(size)?;
    let (none, none_loan) = b.borrow(none_address, false)?;
    let failure = b.failure(None);
    let callee = operation(&mut b, &entry);
    let arguments = vec![
        exclusive(&mut b, 0, slot, address),
        argument(1, Passing::Borrow, none),
        argument(2, Passing::Borrow, size),
    ];
    invoke(
        &mut b,
        callee,
        entry.abi,
        arguments,
        &[none_loan],
        failure,
        BlockKind::Body,
    );
    // The slot holds `None` after a join and `Some(r)` after a discard.
    super::iter::drop_local(&mut b, none_slot, none_address);
    super::iter::drop_local(&mut b, slot, address);
    let unit =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(unit);
    Ok(b.finish())
}
