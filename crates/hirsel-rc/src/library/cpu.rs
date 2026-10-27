//! Closed CPU batches use ordinary task entries and typed library closures.
use super::builder::{Body, Cx, Direct};
use super::iter::{drop_local, local, option};
use super::tasks::{argument, exclusive, intrinsic, operation, option_of, parameter};
use super::{LibraryBody, Request, native_path};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;
use hirsel_types::{BuiltinType, GenericArgument, Type, TypeId, TypeKind};

type R<T> = Result<T, PassError>;

pub static WORKER: LibraryBody = LibraryBody {
    path: || native_path("cpu_worker"),
    build: worker,
};

fn builtin(cx: &Cx<'_>, kind: BuiltinType, element: TypeId) -> TypeId {
    cx.types.intern(Type::with_kind(TypeKind::Builtin {
        kind,
        arguments: vec![GenericArgument::Type(element)].into(),
    }))
}

fn require(cx: &mut Cx<'_>, name: &str, element: TypeId) -> R<(Direct, TypeId)> {
    let path = DefPath::core(name);
    let request = Request::operation(
        cx.program,
        super::body_index(&path)
            .ok_or_else(|| PassError::Malformed(format!("CPU helper {name}")))?,
        &path,
        vec![GenericArgument::Type(element)],
    )?;
    let callable = request.callable;
    if name == "$intrinsic.ListConsume_open" {
        let TypeKind::Function(function) = &cx.types.row(callable).kind else {
            unreachable!()
        };
        let TypeKind::Named {
            definition,
            arguments,
        } = cx.types.row(function.result).kind.clone()
        else {
            return Err(PassError::Malformed("CPU cursor nominal".into()));
        };
        let path = native_path("owned_cursor_drop");
        let actuals = crate::instance::Actuals {
            binder: Some(super::binder(&path)),
            arguments: arguments.to_vec(),
            outer: vec![],
        };
        let drop = Request::finalizer(
            cx.types,
            super::body_index(&path).unwrap(),
            &path,
            definition.path(),
            &actuals,
        );
        cx.require(drop)?;
    }
    Ok((cx.require(request)?, callable))
}

fn direct(
    b: &mut Body<'_, '_>,
    target: &Direct,
    arguments: Vec<CallArgument>,
    loans: &[LoanId],
) -> Option<ValueId> {
    let symbol = b.symbol(target.symbol.clone());
    b.call(Callee::Direct(symbol), vec![], target.abi, arguments, loans)
}

/// Each task owns one input job. Its mapper is borrowed from one immutable
/// Shared root, exactly as shared.with lends its frozen value.
fn worker(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let _job = cx.type_actual(0)?;
    let result = cx.type_actual(1)?;
    let mapper = cx.type_actual(2)?;
    let mapper_layout = cx.layout(mapper)?;
    let mapper_abi = cx
        .layouts
        .fn_abi(cx.types, mapper, CallableShape::Closure)
        .map_err(|error| PassError::Layout(cx.path.clone(), error))?;
    let (held_layout, held_size) = option_of(cx, result)?;
    let finish = intrinsic(cx, "TaskFinish", vec![GenericArgument::Type(result)])?;
    let descriptor_layout = cx.primitive(hirsel_types::Primitive::U32)?;
    let environment = cx
        .environment()
        .ok_or_else(|| PassError::Malformed("CPU worker environment".into()))?;
    let fields = cx.record(environment).fields.clone();
    let mut b = Body::new(cx)?;
    let root = b.capture(0, fields[0].layout)?;
    let jobs = b.capture(1, fields[1].layout)?;
    b.capture(2, fields[2].layout)?;
    b.capture(3, fields[3].layout)?;
    b.capture(4, fields[4].layout)?;
    let live = b.capture(5, fields[5].layout)?;
    b.ed.unit.tables.init_flags.push(InitFlag {
        path: PoolRange::EMPTY,
        value: live.value,
        slot: live.slot,
        environment: Some(environment),
    });
    let mapper_address = b.place(
        AddressBase::Address(root.address),
        &[ProjectionStep::Field(0)],
        mapper_layout,
        None,
    )?;
    let (mapper, loan) = b.borrow(mapper_address, false)?;
    let input = b.take(jobs.address)?;
    let old = b.read_bits(live.address)?;
    let cleared = b.value(fields[5].layout, Holding::Plain);
    b.push(Inst::UpdateMask {
        result: cleared,
        mask: old,
        field: 0,
        initialized: false,
    });
    b.store(live.address, cleared)?;
    let value = b
        .call(
            Callee::Indirect(mapper),
            vec![],
            mapper_abi,
            vec![argument(0, Passing::Take, input)],
            &[loan],
        )
        .ok_or_else(|| PassError::Malformed("CPU mapper returns a value".into()))?;
    let some = option(&mut b, held_layout, 1, &[value]);
    let (slot, address) = local(&mut b, some)?;
    let none = option(&mut b, held_layout, 0, &[]);
    let (none_slot, none_address) = local(&mut b, none)?;
    let (none, none_loan) = b.borrow(none_address, false)?;
    let size = b.u64(held_size)?;
    let descriptor = b.constant(descriptor_layout, ConstantValue::Descriptor(held_layout));
    let callee = operation(&mut b, &finish);
    let args = vec![
        exclusive(&mut b, 0, slot, address),
        argument(1, Passing::Borrow, none),
        argument(2, Passing::Borrow, size),
        argument(3, Passing::Borrow, descriptor),
    ];
    b.call(callee.0, callee.1, finish.abi, args, &[none_loan]);
    drop_local(&mut b, none_slot, none_address);
    drop_local(&mut b, slot, address);
    let unit =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(unit);
    Ok(b.finish())
}

pub static MAP: LibraryBody = LibraryBody {
    path: || DefPath::core("cpu.map_scoped"),
    build: map,
};

fn field(
    b: &mut Body<'_, '_>,
    address: AddressId,
    variant: u32,
    layout: hirsel_lir::layout_types::LayoutId,
) -> R<ValueId> {
    let payload = b.place(
        AddressBase::Address(address),
        &[ProjectionStep::VariantField { variant, field: 0 }],
        layout,
        None,
    )?;
    b.take(payload)
}

fn make_worker(
    b: &mut Body<'_, '_>,
    code: &Direct,
    environment: hirsel_lir::layout_types::LayoutId,
    layout: hirsel_lir::layout_types::LayoutId,
    root: ValueId,
    jobs: ValueId,
    tasks: ValueId,
    width: ValueId,
    ordinal: ValueId,
) -> ValueId {
    let symbol = b.symbol(code.symbol.clone());
    let captures = PoolRange::push(
        &mut b.ed.unit.pools.captures,
        [
            CaptureValue {
                value: root,
                usage: CaptureUse::Take,
            },
            CaptureValue {
                value: jobs,
                usage: CaptureUse::Take,
            },
            CaptureValue {
                value: tasks,
                usage: CaptureUse::Copy,
            },
            CaptureValue {
                value: width,
                usage: CaptureUse::Copy,
            },
            CaptureValue {
                value: ordinal,
                usage: CaptureUse::Copy,
            },
        ],
    );
    let closure = ClosureId(b.ed.unit.tables.closures.len() as u32);
    b.ed.unit.tables.closures.push(ClosureSite {
        receipts: None,
        scoped: false,
        code: Some(ClosureCode {
            symbol,
            abi: code.abi,
        }),
        environment,
        captures,
        destination: DestinationClass::Inferred,
        placement: None,
        anchor: b.anchor,
    });
    let result = b.fresh(layout);
    let failure = b.failure(None);
    b.push(Inst::Closure {
        result,
        closure,
        failure,
    });
    result
}

fn map(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    use hirsel_layout::layout::EnvironmentShape;
    use hirsel_types::{GenericArgument::Type as Actual, Primitive};
    let job = cx.type_actual(0)?;
    let result = cx.type_actual(1)?;
    let prepare = parameter(cx.types, cx.callable, 2)?;
    let mapper = parameter(cx.types, cx.callable, 3)?;
    let tasks_type = parameter(cx.types, cx.callable, 0)?;
    let width_type = parameter(cx.types, cx.callable, 1)?;
    let list_job = builtin(cx, BuiltinType::List, job);
    let list_result = builtin(cx, BuiltinType::List, result);
    let shared_mapper = builtin(cx, BuiltinType::Shared, mapper);
    let root_layout = cx.layout(shared_mapper)?;

    let results_layout = cx.layout(list_result)?;
    let job_layout = cx.layout(job)?;
    let result_layout = cx.layout(result)?;
    let prepare_abi = cx
        .layouts
        .fn_abi(cx.types, prepare, CallableShape::Closure)
        .map_err(|error| PassError::Layout(cx.path.clone(), error))?;
    let pool = intrinsic(cx, "CpuMapScoped", vec![])?;
    let scope_enter = intrinsic(cx, "TaskScopeEnter", vec![])?;
    let scope_exit = intrinsic(cx, "TaskScopeExit", vec![])?;
    let spawn = intrinsic(cx, "TaskSpawn", vec![Actual(result)])?;
    let join = intrinsic(cx, "TaskJoin", vec![Actual(result)])?;
    let handle_type = match &cx.types.row(spawn.callable).kind {
        TypeKind::Function(f) => f.result,
        _ => unreachable!(),
    };
    let handles_type = builtin(cx, BuiltinType::List, handle_type);
    let handles_layout = cx.layout(handles_type)?;
    let (open_jobs, _) = require(cx, "$intrinsic.ListConsume_open", job)?;
    let (next_job, _) = require(cx, "$intrinsic.ListConsume_next", job)?;
    let (push_handle, _) = require(cx, "list.push", handle_type)?;
    let (open_handles, _) = require(cx, "$intrinsic.ListConsume_open", handle_type)?;
    let (next_handle, _) = require(cx, "$intrinsic.ListConsume_next", handle_type)?;
    let (push_result, _) = require(cx, "list.push", result)?;
    let worker_type = parameter(cx.types, spawn.callable, 1)?;
    let path = native_path("cpu_worker");
    let ordinal_type = cx
        .types
        .intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let environment = cx
        .layouts
        .environment(
            cx.types,
            &EnvironmentShape {
                definition: hirsel_syntax::DefId::intern(&path),
                body: 0,
                captures: &[
                    (shared_mapper, Passing::Take),
                    (job, Passing::Take),
                    (tasks_type, Passing::Borrow),
                    (width_type, Passing::Borrow),
                    (ordinal_type, Passing::Borrow),
                ],
                sizes: &[],
                movable: &[1],
            },
        )
        .map_err(|error| PassError::Layout(path.clone(), error))?;
    let mut request = Request::native(
        super::body_index(&path).unwrap(),
        &path,
        vec![Actual(job), Actual(result), Actual(mapper)],
        worker_type,
    );
    request.shape = CallableShape::Closure;
    request.environment = Some(environment);
    let worker = cx.require(request)?;
    let worker_layout = cx.layout(worker_type)?;
    let (result_option, result_size) = option_of(cx, result)?;
    let (job_option, _) = option_of(cx, job)?;
    let (handle_option, _) = option_of(cx, handle_type)?;
    let u32_layout = cx.primitive(Primitive::U32)?;
    let mut b = Body::new(cx)?;
    let [tasks, width, prepare, mapper] = b.parameters[..] else {
        return Err(PassError::Malformed("CPU map parameters".into()));
    };
    let tasks_value = b.read_bits(tasks.address)?;
    let width_value = b.read_bits(width.address)?;
    let zero32 = b.constant(u32_layout, ConstantValue::Integer(0));
    let one32 = b.constant(u32_layout, ConstantValue::Integer(1));
    let two32 = b.constant(u32_layout, ConstantValue::Integer(2));
    let zero = b.u64(0)?;
    let one = b.u64(1)?;
    let callee = operation(&mut b, &pool);
    let status = b
        .call(
            callee.0,
            callee.1,
            pool.abi,
            vec![
                argument(0, Passing::Borrow, tasks_value),
                argument(1, Passing::Borrow, width_value),
                argument(2, Passing::Borrow, zero32),
                argument(3, Passing::Borrow, zero),
            ],
            &[],
        )
        .unwrap();
    let admitted = b.compare(BinaryOp::Eq, status, zero32)?;
    let start = b.block(BlockKind::Body, &[]);
    let refused = b.block(BlockKind::Body, &[]);
    b.branch(admitted, start, refused);
    b.at(refused);
    return_error(&mut b, status)?;
    b.at(start);
    let callee = operation(&mut b, &scope_enter);
    let scope = b
        .call(
            callee.0,
            callee.1,
            scope_enter.abi,
            vec![argument(0, Passing::Borrow, tasks_value)],
            &[],
        )
        .unwrap();
    // Failure cleanup consumes all owned locals, then joins the scope before
    // returning to the original topology. A failed exit must still leave.
    let normal = b.current();
    let held = b.held_state();
    let unwind = b.block(BlockKind::FailureCleanup, &[]);
    let leave_failed = b.block(BlockKind::FailureCleanup, &[]);
    b.restore_held(&[]);
    b.at(leave_failed);
    let callee = operation(&mut b, &pool);
    let failure = b.failure(None);
    super::tasks::invoke(
        &mut b,
        callee,
        pool.abi,
        vec![
            argument(0, Passing::Borrow, tasks_value),
            argument(1, Passing::Borrow, width_value),
            argument(2, Passing::Borrow, one32),
            argument(3, Passing::Borrow, zero),
        ],
        &[],
        failure,
        BlockKind::FailureCleanup,
    );
    b.terminate(Terminator::Propagate);
    b.at(unwind);
    b.propagation(Some(leave_failed));
    let bool_layout = b.bool_layout()?;
    let error = b.constant(bool_layout, ConstantValue::Bool(true));
    let callee = operation(&mut b, &scope_exit);
    let failure = b.failure(None);
    super::tasks::invoke(
        &mut b,
        callee,
        scope_exit.abi,
        vec![
            argument(0, Passing::Borrow, scope),
            argument(1, Passing::Borrow, error),
        ],
        &[],
        failure,
        BlockKind::FailureCleanup,
    );
    b.jump(leave_failed, &[]);
    b.at(normal);
    b.restore_held(&held);
    b.propagation(Some(unwind));
    let (prepare_value, prepare_loan) = b.borrow(prepare.address, false)?;
    let jobs = b
        .call(
            Callee::Indirect(prepare_value),
            vec![],
            prepare_abi,
            vec![],
            &[prepare_loan],
        )
        .ok_or_else(|| PassError::Malformed("CPU preparation result".into()))?;
    let (jobs_slot, jobs_address) = local(&mut b, jobs)?;
    drop_local(&mut b, prepare.slot, prepare.address);
    let (jobs_view, jobs_loan) = b.borrow(jobs_address, false)?;
    let count = b.seq_len(jobs_view)?;
    b.end_loan(jobs_loan);
    let callee = operation(&mut b, &pool);
    let status = b
        .call(
            callee.0,
            callee.1,
            pool.abi,
            vec![
                argument(0, Passing::Borrow, tasks_value),
                argument(1, Passing::Borrow, width_value),
                argument(2, Passing::Borrow, two32),
                argument(3, Passing::Borrow, count),
            ],
            &[],
        )
        .unwrap();
    let valid = b.compare(BinaryOp::Eq, status, zero32)?;
    let work = b.block(BlockKind::Body, &[]);
    let oversized = b.block(BlockKind::Body, &[]);
    let work_held = b.held_state();
    b.branch(valid, work, oversized);
    b.at(oversized);
    b.propagation(Some(leave_failed));
    drop_local(&mut b, jobs_slot, jobs_address);
    let bool_layout = b.bool_layout()?;
    let clean = b.constant(bool_layout, ConstantValue::Bool(false));
    b.propagation(Some(leave_failed));
    let callee = operation(&mut b, &scope_exit);
    b.call(
        callee.0,
        callee.1,
        scope_exit.abi,
        vec![
            argument(0, Passing::Borrow, scope),
            argument(1, Passing::Borrow, clean),
        ],
        &[],
    );
    b.propagation(None);
    let callee = operation(&mut b, &pool);
    b.call(
        callee.0,
        callee.1,
        pool.abi,
        vec![
            argument(0, Passing::Borrow, tasks_value),
            argument(1, Passing::Borrow, width_value),
            argument(2, Passing::Borrow, one32),
            argument(3, Passing::Borrow, zero),
        ],
        &[],
    );
    return_error(&mut b, status)?;
    b.at(work);
    b.restore_held(&work_held);
    b.propagation(Some(unwind));
    let mapper_value = b.take(mapper.address)?;
    let root = b.fresh(root_layout);
    b.push(Inst::Freeze {
        result: root,
        value: mapper_value,
    });
    let (root_slot, root_address) = local(&mut b, root)?;
    let jobs = b.take(jobs_address)?;
    b.emptied(jobs_slot);
    let cursor = direct(
        &mut b,
        &open_jobs,
        vec![argument(0, Passing::Take, jobs)],
        &[],
    )
    .unwrap();
    let (cursor_slot, cursor_address) = local(&mut b, cursor)?;
    let empty = super::slots::construct(&mut b, handles_layout, ConstructorKind::Sequence, &[])?;
    let (handles_slot, handles_address) = local(&mut b, empty)?;
    // Logical task order is input order, independently of which of the W
    // physical workers executes each task. Job settlement uses this order.
    b.seq_reserve(handles_address, count)?;
    let index = local(&mut b, zero)?;
    let spawn_slots = [cursor_slot, handles_slot, index.0];
    let (head, spawn_versions) = merge_block(&mut b, &spawn_slots);
    let chunk = b.block(BlockKind::Body, &[]);
    let joined = b.block(BlockKind::Body, &[]);
    let loop_held = b.held_state();
    jump_merge(&mut b, head, &spawn_slots)?;
    at_merge(&mut b, head, &spawn_slots, &spawn_versions);
    let q = b.take(index.1)?;
    let more = b.compare(BinaryOp::Lt, q, count)?;
    b.branch(more, chunk, joined);
    b.at(chunk);
    // The parent issues the canonical ordinal before publishing the child.
    // The next spawn binds this receipt to its generation-qualified owner.
    let action = b.constant(u32_layout, ConstantValue::Integer(5));
    let callee = operation(&mut b, &pool);
    let status = b
        .call(
            callee.0,
            callee.1,
            pool.abi,
            vec![
                argument(0, Passing::Borrow, tasks_value),
                argument(1, Passing::Borrow, width_value),
                argument(2, Passing::Borrow, action),
                argument(3, Passing::Borrow, q),
            ],
            &[],
        )
        .unwrap();
    let admitted = b.compare(BinaryOp::Eq, status, zero32)?;
    let execute = b.block(BlockKind::Body, &[]);
    b.branch(admitted, execute, joined);
    b.at(execute);
    let item = cursor_next(&mut b, &next_job, cursor_slot, cursor_address)?;
    let job = take_some(&mut b, item, job_option, job_layout)?;
    let job = local(&mut b, job)?;
    let failure = b.failure(None);
    let root = b.copy(root_address, failure)?;
    let jobs = b.take(job.1)?;
    b.emptied(job.0);

    let closure = make_worker(
        &mut b,
        &worker,
        environment,
        worker_layout,
        root,
        jobs,
        tasks_value,
        width_value,
        q,
    );
    let callee = operation(&mut b, &spawn);
    let handle = b
        .call(
            callee.0,
            callee.1,
            spawn.abi,
            vec![
                argument(0, Passing::Borrow, scope),
                argument(1, Passing::Take, closure),
                argument(2, Passing::Borrow, zero),
            ],
            &[],
        )
        .unwrap();
    push_value(
        &mut b,
        &push_handle,
        (handles_slot, handles_address),
        handle,
    )?;
    let next = b.add(ArithMode::Wrapping, q, one);
    b.store(index.1, next)?;
    jump_merge(&mut b, head, &spawn_slots)?;
    b.at(joined);
    for (&slot, &version) in spawn_slots.iter().zip(&spawn_versions) {
        b.set_reaching(slot, version);
    }
    b.restore_held(&loop_held);
    drop_local(&mut b, cursor_slot, cursor_address);
    drop_local(&mut b, root_slot, root_address);
    let handles = b.take(handles_address)?;
    b.emptied(handles_slot);
    let handles = direct(
        &mut b,
        &open_handles,
        vec![argument(0, Passing::Take, handles)],
        &[],
    )
    .unwrap();
    let members = local(&mut b, handles)?;
    let empty = super::slots::construct(&mut b, results_layout, ConstructorKind::Sequence, &[])?;
    let out = local(&mut b, empty)?;
    b.seq_reserve(out.1, count)?;
    let collect_slots = [members.0, out.0];
    let (collect, collect_versions) = merge_block(&mut b, &collect_slots);
    let collect_one = b.block(BlockKind::Body, &[]);
    let complete = b.block(BlockKind::Body, &[]);
    let collect_held = b.held_state();
    jump_merge(&mut b, collect, &collect_slots)?;
    at_merge(&mut b, collect, &collect_slots, &collect_versions);
    let member = cursor_next(&mut b, &next_handle, members.0, members.1)?;
    let member = local(&mut b, member)?;
    let member_loan = switch_some(&mut b, member.1, collect_one, complete)?;
    b.at(collect_one);
    b.end_loan(member_loan);
    let handle_layout = b.cx.layout(handle_type)?;
    let handle = field(&mut b, member.1, 1, handle_layout)?;
    b.emptied(member.0);
    let none = option(&mut b, result_option, 0, &[]);
    let result = local(&mut b, none)?;
    let (_, loan) = b.borrow(result.1, true)?;
    let size = b.u64(result_size)?;
    let callee = operation(&mut b, &join);
    let args = vec![
        argument(0, Passing::Borrow, scope),
        argument(1, Passing::Take, handle),
        exclusive(&mut b, 2, result.0, result.1),
        argument(3, Passing::Borrow, size),
    ];
    b.call(callee.0, callee.1, join.abi, args, &[loan]);
    let value = field(&mut b, result.1, 1, result_layout)?;
    b.emptied(result.0);
    push_value(&mut b, &push_result, out, value)?;
    jump_merge(&mut b, collect, &collect_slots)?;
    b.at(complete);
    b.end_loan(member_loan);
    b.set_reaching(out.0, collect_versions[1]);
    b.restore_held(&collect_held);
    drop_local(&mut b, member.0, member.1);
    drop_local(&mut b, members.0, members.1);
    let clean = b.constant(bool_layout, ConstantValue::Bool(false));
    b.propagation(Some(leave_failed));
    let callee = operation(&mut b, &scope_exit);
    b.call(
        callee.0,
        callee.1,
        scope_exit.abi,
        vec![
            argument(0, Passing::Borrow, scope),
            argument(1, Passing::Borrow, clean),
        ],
        &[],
    );
    b.propagation(None);
    let callee = operation(&mut b, &pool);
    let leave_status = b
        .call(
            callee.0,
            callee.1,
            pool.abi,
            vec![
                argument(0, Passing::Borrow, tasks_value),
                argument(1, Passing::Borrow, width_value),
                argument(2, Passing::Borrow, one32),
                argument(3, Passing::Borrow, zero),
            ],
            &[],
        )
        .unwrap();
    let admitted = b.compare(BinaryOp::Eq, leave_status, zero32)?;
    let success = b.block(BlockKind::Body, &[]);
    let refused = b.block(BlockKind::Body, &[]);
    b.branch(admitted, success, refused);
    b.at(refused);
    return_error(&mut b, leave_status)?;
    b.at(success);
    let result = b.take(out.1)?;
    b.emptied(out.0);
    let result_layout = b.cx.fn_abi().result.unwrap().layout;
    let result = super::arena::variant(&mut b, result_layout, "Result", 0, &[result]);
    b.ret(Some(result));
    let _ = (result_option, result_size);
    Ok(b.finish())
}

fn cursor_next(
    b: &mut Body<'_, '_>,
    next: &Direct,
    slot: SlotId,
    address: AddressId,
) -> R<ValueId> {
    let (_, loan) = b.borrow(address, true)?;
    let args = vec![exclusive(b, 0, slot, address)];
    direct(b, next, args, &[loan]).ok_or_else(|| PassError::Malformed("CPU cursor result".into()))
}

fn push_value(
    b: &mut Body<'_, '_>,
    push: &Direct,
    target: (SlotId, AddressId),
    value: ValueId,
) -> R<()> {
    let (_, loan) = b.borrow(target.1, true)?;
    let args = vec![
        exclusive(b, 0, target.0, target.1),
        argument(1, Passing::Take, value),
    ];
    direct(b, push, args, &[loan]);
    Ok(())
}

fn switch_some(b: &mut Body<'_, '_>, address: AddressId, yes: BlockId, no: BlockId) -> R<LoanId> {
    let (tag, loan) = b.borrow(address, false)?;
    let yes = b.edge(yes, &[]);
    let no = b.edge(no, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm {
            value: 1,
            edge: yes,
        }],
    );
    b.terminate(Terminator::Switch {
        value: tag,
        arms,
        otherwise: no,
    });
    Ok(loan)
}

fn take_some(
    b: &mut Body<'_, '_>,
    value: ValueId,
    _option: hirsel_lir::layout_types::LayoutId,
    payload: hirsel_lir::layout_types::LayoutId,
) -> R<ValueId> {
    let slot = local(b, value)?;
    let result = field(b, slot.1, 1, payload)?;
    b.emptied(slot.0);
    b.push(Inst::StorageDead {
        slot: slot.0,
        if_live: false,
    });
    Ok(result)
}

fn return_error(b: &mut Body<'_, '_>, status: ValueId) -> R<()> {
    let result = b.cx.fn_abi().result.unwrap().layout;
    let error = super::arena::payload(b, result, 1)
        .ok_or_else(|| PassError::Malformed("CPU error result".into()))?;
    let at = b.current();
    let broken = b.block(BlockKind::Body, &[]);
    b.at(broken);
    b.terminate(Terminator::InvariantFault {
        reason: "invalid CPU pool outcome".into(),
    });
    let mut arms = vec![];
    for code in (1..=11u32).chain(16..=19) {
        let arm = b.block(BlockKind::Body, &[]);
        b.at(arm);
        let (variant, values) = if code >= 16 {
            let cause = super::arena::payload(b, error, 8).unwrap();
            let value = super::arena::variant(b, cause, "CancelCause", code - 16, &[]);
            (8, vec![value])
        } else {
            (if code <= 8 { code - 1 } else { code }, vec![])
        };
        let value = super::arena::variant(b, error, "CpuPoolError", variant, &values);
        let value = super::arena::variant(b, result, "Result", 1, &[value]);
        // Returning an expected refusal still consumes both callable inputs.
        let held = b.held_state();
        let returned = local(b, value)?;
        for &(slot, address) in held.iter().rev() {
            drop_local(b, slot, address);
        }
        let value = b.take(returned.1)?;
        b.push(Inst::StorageDead {
            slot: returned.0,
            if_live: false,
        });
        b.restore_held(&held);
        b.ret(Some(value));
        let edge = b.edge(arm, &[]);
        arms.push(SwitchArm { value: code, edge });
    }
    b.at(at);
    let arms = PoolRange::push(&mut b.ed.unit.pools.switch_arms, arms);
    let otherwise = b.edge(broken, &[]);
    b.terminate(Terminator::Switch {
        value: status,
        arms,
        otherwise,
    });
    Ok(())
}

fn merge_block(b: &mut Body<'_, '_>, slots: &[SlotId]) -> (BlockId, Vec<ValueId>) {
    let versions: Vec<_> = slots.iter().map(|&s| b.version(s)).collect();
    (b.block(BlockKind::Body, &versions), versions)
}
fn jump_merge(b: &mut Body<'_, '_>, block: BlockId, slots: &[SlotId]) -> R<()> {
    let arguments = slots
        .iter()
        .map(|&s| b.reaching(s))
        .collect::<R<Vec<_>>>()?;
    b.jump(block, &arguments);
    Ok(())
}
fn at_merge(b: &mut Body<'_, '_>, block: BlockId, slots: &[SlotId], versions: &[ValueId]) {
    b.at(block);
    for (&slot, &version) in slots.iter().zip(versions) {
        b.set_reaching(slot, version);
    }
}
