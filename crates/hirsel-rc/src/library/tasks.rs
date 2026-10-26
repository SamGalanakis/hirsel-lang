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
//! `task.scope_until` is the same body over `TaskScopeEnterUntil`, which arms the scope's
//! deadline, and `TaskScopeExitUntil`, which says whether it fired: the result is then
//! `Expired(r)`, otherwise `Finished(r)` (CONC-3).
use super::builder::{Body, Cx};
use super::iter::{local, option};
use super::{LibraryBody, Request, native_path};
use crate::PassError;
use crate::instance::{Actuals, InstanceError, instantiate_function};
use hirsel_core_contract::{OperationId, Passing};
use hirsel_layout::layout::EnvironmentShape;
use hirsel_lir::abi_types::{CallableShape, FnAbiId};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::operation::OperationArgument;
use hirsel_lir::stage::Value;
use hirsel_syntax::{DefId, DefPath};
use hirsel_types::{GenericArgument, SignatureKind, Type, TypeId, TypeKind};

type R<T> = Result<T, PassError>;

pub static SCOPE: LibraryBody = LibraryBody {
    path: || DefPath::core("task.scope"),
    build: scope,
};
pub static SCOPE_UNTIL: LibraryBody = LibraryBody {
    path: || DefPath::core("task.scope_until"),
    build: scope_until,
};
pub static ASYNC: LibraryBody = LibraryBody {
    path: || DefPath::core("task.async"),
    build: spawn,
};
pub static AWAIT: LibraryBody = LibraryBody {
    path: || DefPath::core("task.await"),
    build: join,
};
pub static AWAIT_AT: LibraryBody = LibraryBody {
    path: || DefPath::core("task.await_at"),
    build: join_at,
};
pub static CHECK_CANCELED: LibraryBody = LibraryBody {
    path: || DefPath::core("task.check_canceled"),
    build: check_canceled,
};
pub static MAIN: LibraryBody = LibraryBody {
    path: || native_path("task_main"),
    build: main,
};

/// A runtime entry under this body's actuals: its operation, its `FnAbi` and
/// its callable type.
pub(super) struct Intrinsic {
    operation: OperationId,
    pub(super) abi: FnAbiId,
    /// The actuals the operation's declared ABI is derived under.
    arguments: Vec<OperationArgument>,
    pub(super) callable: TypeId,
}

pub(super) fn intrinsic(
    cx: &mut Cx<'_>,
    name: &str,
    arguments: Vec<GenericArgument>,
) -> R<Intrinsic> {
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
    let actuals = Actuals {
        binder: (!arguments.is_empty()).then(|| super::binder(&path)),
        arguments,
        outer: Vec::new(),
    };
    let callable = instantiate_function(
        &cx.program.types,
        &cx.program.memo,
        function,
        hirsel_types::Modes::global(),
        &actuals,
    )
    .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
    let abi = cx
        .layouts
        .fn_abi(cx.types, callable, CallableShape::Definition)
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    let abi = if signature.metadata.track_caller {
        cx.layouts
            .with_caller_site(abi)
            .map_err(|e| PassError::Layout(path.clone(), e))?
    } else {
        abi
    };
    let arguments = hirsel_layout::operation::collect_arguments(cx.types, &actuals.arguments)
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    Ok(Intrinsic {
        operation,
        abi,
        arguments,
        callable,
    })
}

/// The type of parameter `ordinal` of a `Function` row.
pub(super) fn parameter(
    types: &hirsel_types::TypeInterner,
    callable: TypeId,
    ordinal: usize,
) -> R<TypeId> {
    match types.get(callable).map(|t| &t.kind) {
        Some(TypeKind::Function(f)) if f.parameters.len() > ordinal => Ok(f.parameters[ordinal].ty),
        _ => Err(PassError::Malformed(format!(
            "a task callable has no parameter {ordinal}"
        ))),
    }
}

/// `Option[R]`, its layout and its storage bytes.
pub(super) fn option_of(cx: &mut Cx<'_>, result: TypeId) -> R<(LayoutId, u64)> {
    let ty = cx.types.intern(Type::with_kind(TypeKind::Named {
        definition: DefId::intern(&DefPath::core("Option")),
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

pub(super) fn argument(parameter: u32, passing: Passing, value: ValueId) -> CallArgument {
    CallArgument {
        parameter,
        passing,
        value: ArgumentValue::Value(value),
    }
}

/// A `mut` argument: the slot's new version is defined at the call.
pub(super) fn exclusive(
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
pub(super) fn invoke(
    b: &mut Body<'_, '_>,
    callee: (Callee, Vec<OperationArgument>),
    abi: FnAbiId,
    arguments: Vec<CallArgument>,
    loans: &[LoanId],
    failure: FailureId,
    after: BlockKind,
) -> Option<ValueId> {
    let invocation = matches!(callee.0, Callee::Indirect(_) | Callee::Bound { .. });
    if invocation {
        let admission = b.admission_failure(&callee.0, &arguments, loans, failure);
        b.push(Inst::FrameEnter { failure: admission });
    }
    let shape = b.cx.layouts.table().fn_abis[abi.0 as usize].clone();
    let result = shape.result.map(|r| b.fresh(r.layout));
    let failure = if loans.is_empty() && !invocation {
        failure
    } else {
        let at = b.current();
        let ending = b.block(BlockKind::FailureCleanup, &[]);
        b.at(ending);
        if invocation {
            b.push(Inst::FrameLeave);
        }
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
        receipts: None,
        pretransfer_failure: None,
        forward_caller: !matches!(callee.0, Callee::Indirect(_) | Callee::Bound { .. }),
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
    if invocation {
        b.push(Inst::FrameLeave);
    }
    for &loan in loans.iter().rev() {
        b.end_loan(loan);
    }
    result
}

pub(super) fn operation(
    b: &mut Body<'_, '_>,
    entry: &Intrinsic,
) -> (Callee, Vec<OperationArgument>) {
    let reference = OperationRef(b.ed.unit.tables.operations.len() as u32);
    b.ed.unit.tables.operations.push(entry.operation.clone());
    (Callee::Operation(reference), entry.arguments.clone())
}

fn scope(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    scoped(cx, false)
}

fn scope_until(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    scoped(cx, true)
}

/// `task.scope`, or with `until` `task.scope_until`.
fn scoped(cx: &mut Cx<'_>, until: bool) -> R<LirUnit<Value>> {
    use super::arena::{payload, variant};
    let result = cx.type_actual(0)?;
    let is_result = matches!(
        cx.types.get(result).map(|t| &t.kind),
        Some(TypeKind::Named { definition, .. }) if *definition.path() == DefPath::core("Result")
    );
    let callback = parameter(cx.types, cx.callable, if until { 2 } else { 1 })?;
    let body_abi = cx
        .layouts
        .fn_abi(cx.types, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let (enter, exit) = if until {
        ("TaskScopeEnterUntil", "TaskScopeExitUntil")
    } else {
        ("TaskScopeEnter", "TaskScopeExit")
    };
    let enter = intrinsic(cx, enter, Vec::new())?;
    let exit = intrinsic(cx, exit, Vec::new())?;
    let mut b = Body::new(cx)?;
    let (tasks, deadline, body) = match b.parameters.as_slice() {
        &[tasks, body] if !until => (tasks, None, body),
        &[tasks, deadline, body] if until => (tasks, Some(deadline), body),
        _ => return Err(PassError::Malformed("task.scope parameters".into())),
    };
    let tasks = b.read_bits(tasks.address)?;
    let mut arguments = vec![argument(0, Passing::Borrow, tasks)];
    let mut loans = Vec::new();
    if let Some(deadline) = deadline {
        let (deadline, loan) = b.borrow(deadline.address, false)?;
        arguments.push(argument(1, Passing::Borrow, deadline));
        loans.push(loan);
    }
    let propagate = b.failure(None);
    let callee = operation(&mut b, &enter);
    let s = invoke(
        &mut b,
        callee,
        enter.abi,
        arguments,
        &loans,
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
    let expired = invoke(
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
    if !until {
        b.ret(value);
        return Ok(b.finish());
    }
    // `Expired(r)` when the deadline fired, otherwise `Finished(r)`.
    let expired =
        expired.ok_or_else(|| PassError::Malformed("TaskScopeExitUntil returns a flag".into()))?;
    let bounded =
        b.cx.fn_abi()
            .result
            .map(|r| r.layout)
            .ok_or_else(|| PassError::Malformed("task.scope_until returns Bounded".into()))?;
    let at = b.current();
    let mut edges = Vec::new();
    for ordinal in [0u32, 1] {
        let arm = b.block(BlockKind::Body, &[]);
        b.at(arm);
        let body = match value {
            Some(value) => Some(value),
            None => payload(&b, bounded, ordinal).map(|unit| b.constant(unit, ConstantValue::Unit)),
        };
        let wrapped = variant(&mut b, bounded, "Bounded", ordinal, body.as_slice());
        b.ret(Some(wrapped));
        edges.push(b.edge(arm, &[]));
    }
    b.at(at);
    b.terminate(Terminator::Branch {
        condition: expired,
        yes: edges[1],
        no: edges[0],
    });
    Ok(b.finish())
}

fn spawn(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let result = cx.type_actual(0)?;
    let f = parameter(cx.types, cx.callable, 1)?;
    let entry = intrinsic(cx, "TaskSpawn", vec![GenericArgument::Type(result)])?;
    // The child's code: `$native.task_main` keyed by the spawned callable's type,
    // with the intrinsic's `once fn() -> ()` as its own callable type.
    let path = native_path("task_main");
    let main_ty = parameter(cx.types, entry.callable, 1)?;
    let environment = cx
        .layouts
        .environment(
            cx.types,
            &EnvironmentShape {
                definition: DefId::intern(&path),
                body: 0,
                captures: &[(f, Passing::Take)],
                sizes: &[],
                // The child moves the callable out to drop it with its frame (CONC-2).
                movable: &[0],
            },
        )
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    let mut request = Request::native(
        super::body_index(&path).expect("registered task main"),
        &path,
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
        receipts: None,
        scoped: false,
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
    let flags = b.u64(0)?;
    let task = invoke(
        &mut b,
        callee,
        entry.abi,
        vec![
            argument(0, Passing::Borrow, s),
            argument(1, Passing::Take, main),
            argument(2, Passing::Borrow, flags),
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
    join_handle(b, &entry, option_layout, size, s, h)
}

/// `task.await_at(s, mut set, index)`: a member that is absent or already awaited fails
/// CONC-2; otherwise the set gives up its handle, which is joined as `task.await` joins.
fn join_at(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let result = cx.type_actual(0)?;
    let (option_layout, size) = option_of(cx, result)?;
    let generics = vec![GenericArgument::Type(result)];
    let has = intrinsic(cx, "TaskSetHas", generics.clone())?;
    let take = intrinsic(cx, "TaskSetTake", generics.clone())?;
    let entry = intrinsic(cx, "TaskJoin", generics)?;
    let mut b = Body::new(cx)?;
    let &[s, set, index] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("task.await_at parameters".into()));
    };
    let s = b.read_bits(s.address)?;
    let index = b.read_bits(index.address)?;
    let (word, loan) = b.borrow(set.address, false)?;
    let failure = b.failure(None);
    let callee = operation(&mut b, &has);
    let member = invoke(
        &mut b,
        callee,
        has.abi,
        vec![
            argument(0, Passing::Borrow, s),
            argument(1, Passing::Borrow, word),
            argument(2, Passing::Borrow, index),
        ],
        &[loan],
        failure,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("TaskSetHas returns a bool".into()))?;
    let failure = b.failure(Some("CONC-2"));
    b.push(Inst::Check {
        condition: member,
        failure,
    });
    let failure = b.failure(None);
    let callee = operation(&mut b, &take);
    let arguments = vec![
        argument(0, Passing::Borrow, s),
        exclusive(&mut b, 1, set.slot, set.address),
        argument(2, Passing::Borrow, index),
    ];
    let h = invoke(
        &mut b,
        callee,
        take.abi,
        arguments,
        &[],
        failure,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("TaskSetTake returns a handle".into()))?;
    join_handle(b, &entry, option_layout, size, s, h)
}

/// Join `h` of scope `s` into a fresh `None` slot and return the result it moved in.
fn join_handle(
    mut b: Body<'_, '_>,
    entry: &Intrinsic,
    option_layout: LayoutId,
    size: u64,
    s: ValueId,
    h: ValueId,
) -> R<LirUnit<Value>> {
    let none = option(&mut b, option_layout, 0, &[]);
    let (slot, address) = local(&mut b, none)?;
    let size = b.u64(size)?;
    let failure = b.failure(None);
    // The retained Option image is lent exclusively to TaskJoin. End this
    // exact loan on both outcomes before inspecting or releasing the image.
    let (_, result_loan) = b.borrow(address, true)?;
    let callee = operation(&mut b, entry);
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
        &[result_loan],
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
    // A unit result has no return value in the ABI.
    let returned = b.cx.fn_abi().result.map(|_| value);
    b.ret(returned);
    Ok(b.finish())
}

fn main(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let f = cx.type_actual(0)?;
    let result = match cx.types.get(f).map(|t| &t.kind) {
        Some(TypeKind::Function(function)) => function.result,
        _ => {
            return Err(PassError::Malformed(
                "a task main captures no callable".into(),
            ));
        }
    };
    let f_abi = cx
        .layouts
        .fn_abi(cx.types, f, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let (option_layout, size) = option_of(cx, result)?;
    let entry = intrinsic(cx, "TaskFinish", vec![GenericArgument::Type(result)])?;
    let descriptor_layout = cx.primitive(hirsel_types::Primitive::U32)?;
    let environment = cx
        .environment()
        .ok_or_else(|| PassError::Malformed("a task main without environment".into()))?;
    let record = cx.record(environment);
    let field = record.fields[0].layout;
    let live = record
        .fields
        .last()
        .map(|w| (w.ordinal, w.layout))
        .ok_or_else(|| {
            PassError::Malformed("a task main environment without its live word".into())
        })?;
    let mut b = Body::new(cx)?;
    let f = b.capture(0, field)?;
    let word = b.capture(live.0, live.1)?;
    b.ed.unit.tables.init_flags.push(InitFlag {
        path: PoolRange::EMPTY,
        value: word.value,
        slot: word.slot,
        environment: Some(environment),
    });
    // The callable's captures die with the child's frame, before its result is held
    // (CONC-2): move it out of the environment, whose release then skips it.
    let moved = b.take(f.address)?;
    let old = b.read_bits(word.address)?;
    let cleared = b.value(live.1, Holding::Plain);
    b.push(Inst::UpdateMask {
        result: cleared,
        mask: old,
        field: 0,
        initialized: false,
    });
    b.store(word.address, cleared)?;
    let (f_slot, f_address) = local(&mut b, moved)?;
    let (callee, loan) = b.borrow(f_address, false)?;
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
            let ty = b.cx.types.intern(Type::unit());
            let unit = b.cx.layout(ty)?;
            b.constant(unit, ConstantValue::Unit)
        }
    };
    // The result is held before the callable's drop, whose failure then releases it.
    let some = option(&mut b, option_layout, 1, &[value]);
    let (slot, address) = local(&mut b, some)?;
    super::iter::drop_local(&mut b, f_slot, f_address);
    let none = option(&mut b, option_layout, 0, &[]);
    let (none_slot, none_address) = local(&mut b, none)?;
    let size = b.u64(size)?;
    let descriptor = b.constant(descriptor_layout, ConstantValue::Descriptor(option_layout));
    let (none, none_loan) = b.borrow(none_address, false)?;
    let failure = b.failure(None);
    let callee = operation(&mut b, &entry);
    let arguments = vec![
        exclusive(&mut b, 0, slot, address),
        argument(1, Passing::Borrow, none),
        argument(2, Passing::Borrow, size),
        argument(3, Passing::Borrow, descriptor),
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

/// `task.check_canceled(s) -> Result[(), CancelError]` (CONC-3): `Ok(())`, or
/// `Error(Canceled(cause))` for the running task's cancellation.
fn check_canceled(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    use super::arena::{RESULT_ERROR, RESULT_OK, payload, variant};
    let entry = intrinsic(cx, "TaskCanceled", Vec::new())?;
    let returned = cx
        .fn_abi()
        .result
        .map(|r| r.layout)
        .ok_or_else(|| PassError::Malformed("task.check_canceled returns a Result".into()))?;
    let mut b = Body::new(cx)?;
    let &[s] = b.parameters.as_slice() else {
        return Err(PassError::Malformed(
            "task.check_canceled parameters".into(),
        ));
    };
    let error = payload(&b, returned, RESULT_ERROR)
        .ok_or_else(|| PassError::Malformed("a CancelError result".into()))?;
    let cause = payload(&b, error, 0)
        .ok_or_else(|| PassError::Malformed("a CancelError without its cause".into()))?;
    let s = b.read_bits(s.address)?;
    let failure = b.failure(None);
    let callee = operation(&mut b, &entry);
    let status = invoke(
        &mut b,
        callee,
        entry.abi,
        vec![argument(0, Passing::Borrow, s)],
        &[],
        failure,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("TaskCanceled returns a status".into()))?;
    let at = b.current();
    let mut arms = Vec::new();
    for code in 0..=4u32 {
        let arm = b.block(BlockKind::Body, &[]);
        b.at(arm);
        let value = if code == 0 {
            let unit =
                payload(&b, returned, RESULT_OK).map(|unit| b.constant(unit, ConstantValue::Unit));
            variant(&mut b, returned, "Result", RESULT_OK, unit.as_slice())
        } else {
            let cause = variant(&mut b, cause, "CancelCause", code - 1, &[]);
            let canceled = variant(&mut b, error, "CancelError", 0, &[cause]);
            variant(&mut b, returned, "Result", RESULT_ERROR, &[canceled])
        };
        b.ret(Some(value));
        let edge = b.edge(arm, &[]);
        arms.push(SwitchArm { value: code, edge });
    }
    let broken = b.block(BlockKind::Body, &[]);
    b.at(broken);
    b.terminate(Terminator::InvariantFault {
        reason: "a cancellation cause past CancelCause".into(),
    });
    b.at(at);
    let otherwise = b.edge(broken, &[]);
    let arms = PoolRange::push(&mut b.ed.unit.pools.switch_arms, arms);
    b.terminate(Terminator::Switch {
        value: status,
        arms,
        otherwise,
    });
    Ok(b.finish())
}
