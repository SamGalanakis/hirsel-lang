//! Runner calls (RUN-4) over the runtime's runner entries (`hirsel-rt/src/runners.rs`).
//!
//! `runner.compute` and `runner.call` encode their argument with the canonical codec and hand
//! `$intrinsic.RunnerSend` the entry, the bytes and the callee's `main`:
//! `$native.runner_main[F]`, a static `fn() -> ()` the runtime runs as the callee run's root
//! task. The caller then waits for the outcome code (`RunnerWait`), takes its bytes
//! (`RunnerTake`) and decodes the `Ok` value, the `Failed` error or the `Faulted` failure into
//! fresh storage of its own run; the other codes name a nullary reason or cause. Nothing but
//! bytes crosses between runs, which share no storage (OWN-6).
//!
//! The callee's `main` reads its entry, its argument's bytes and, for the call form, its root
//! Io from the runtime, decodes the argument and calls the entry inside a catch, as
//! `task.isolate` does. It encodes the entry's `Ok` or `Error` payload, or the caught chain
//! as one `Failure`, and replies with the bytes (`RunnerReply`, tags 0, 1 and 2).
//!
//! `runner.spawn` sends the call from the spawning task, so its ordinal and checks come before
//! the spawner yields, then spawns `task.async`'s child over `$native.runner_await[R, E]`, a
//! closure of the call's ticket that waits for and decodes the outcome.
use super::arena::{RESULT_ERROR, RESULT_OK, payload, variant};
use super::builder::{Body, Cx, Direct};
use super::iter::{drop_local, local};
use super::slots::{construct, nominal};
use super::tasks::{Intrinsic, argument, intrinsic, invoke, operation, parameter};
use super::{LibraryBody, Request, native_path};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_layout::layout::EnvironmentShape;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;
use hirsel_types::{GenericArgument, Primitive, Type, TypeId, TypeInterner, TypeKind};

type R<T> = Result<T, PassError>;

pub static COMPUTE: LibraryBody = LibraryBody {
    path: || DefPath::core("runner.compute"),
    build: compute,
};
pub static CALL: LibraryBody = LibraryBody {
    path: || DefPath::core("runner.call"),
    build: call_form,
};
pub static SPAWN: LibraryBody = LibraryBody {
    path: || DefPath::core("runner.spawn"),
    build: spawn,
};
pub static GRANTS: LibraryBody = LibraryBody {
    path: || DefPath::core("runner.grants"),
    build: grants,
};
pub static MAIN: LibraryBody = LibraryBody {
    path: || native_path("runner_main"),
    build: main,
};
pub static AWAIT: LibraryBody = LibraryBody {
    path: || native_path("runner_await"),
    build: awaited,
};

/// `RunnerSend`'s form: the callee entry takes only its argument, or its root Io first.
const FORM_COMPUTE: u64 = 0;
const FORM_CALL: u64 = 1;
/// `runner.spawn`'s call form: its child, not the sender, waits for the outcome.
const FORM_SPAWN: u64 = 2;

/// The outcome codes `RunnerWait` returns: a kind in the high bits, a reason or cause below.
const OUTCOME_OK: u32 = 0;
const OUTCOME_FAILED: u32 = 1 << 4;
const OUTCOME_FAULTED: u32 = 2 << 4;
const OUTCOME_NOT_RUN: u32 = 3 << 4;
const OUTCOME_IN_DOUBT: u32 = 4 << 4;
const OUTCOME_CANCELED: u32 = 5 << 4;
/// `NotRunReason`, `InDoubtReason` and `CancelCause`'s variant counts.
const NOT_RUN_REASONS: u32 = 5;
const IN_DOUBT_REASONS: u32 = 3;
const CANCEL_CAUSES: u32 = 4;
/// `RunError`'s variants.
const NOT_RUN: u32 = 0;
const IN_DOUBT: u32 = 1;
const FAILED: u32 = 2;
const FAULTED: u32 = 3;
const CANCELED: u32 = 4;

/// `RunnerReply`'s tags.
const REPLY_OK: u64 = 0;
const REPLY_ERROR: u64 = 1;
const REPLY_FAILED: u64 = 2;

/// The core nominal `name` over `arguments`, a row of `store`.
fn named(store: &TypeInterner, name: &str, arguments: Vec<GenericArgument>) -> TypeId {
    store.intern(Type::with_kind(TypeKind::Named {
        definition: hirsel_syntax::DefId::intern(&DefPath::core(name)),
        arguments: arguments.into(),
    }))
}

/// The callable `F` of an `Entry[F]` row.
fn entry_callable(store: &TypeInterner, entry: TypeId) -> R<TypeId> {
    match store.get(entry).map(|t| &t.kind) {
        Some(TypeKind::Named { arguments, .. }) => match arguments.first() {
            Some(GenericArgument::Type(f)) => Ok(*f),
            _ => Err(PassError::Malformed("an Entry without its callable".into())),
        },
        _ => Err(PassError::Malformed(
            "a runner call's entry is not an Entry".into(),
        )),
    }
}

/// Call `entry`; a failure propagates after `loans` end.
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

fn produced(entry: &str, value: Option<ValueId>) -> R<ValueId> {
    value.ok_or_else(|| PassError::Malformed(format!("{entry} returns a value")))
}

fn push_codec(b: &mut Body<'_, '_>, plan: hirsel_lir::codec::CodecPlan) -> CodecId {
    b.ed.unit.tables.codecs.push(plan);
    CodecId(b.ed.unit.tables.codecs.len() as u32 - 1)
}

/// The canonical bytes of the value of type `ty` at `address`, which stays in place.
fn encode(b: &mut Body<'_, '_>, ty: TypeId, address: AddressId) -> R<ValueId> {
    let root = b.address_layout(address);
    let bytes = b.cx.primitive(Primitive::Bytes)?;
    let plan = super::codec::body_plan(b.cx, ty, root, bytes, None)?;
    let codec = push_codec(b, plan);
    let (value, loan) = b.borrow(address, false)?;
    let result = b.fresh(bytes);
    b.push(Inst::Encode {
        result,
        value,
        codec,
    });
    b.end_loan(loan);
    Ok(result)
}

/// The value of type `ty` the Bytes at `bytes` hold, as fresh storage. The bytes are a codec
/// encoding of that type made in this execution, so an error is an invariant fault.
fn decode(b: &mut Body<'_, '_>, ty: TypeId, bytes: AddressId) -> R<ValueId> {
    let root = b.cx.layout(ty)?;
    let error = named(b.cx.types, "DecodeError", Vec::new());
    let result = named(
        b.cx.types,
        "Result",
        vec![GenericArgument::Type(ty), GenericArgument::Type(error)],
    );
    let (result, error) = (b.cx.layout(result)?, b.cx.layout(error)?);
    let bytes_layout = b.address_layout(bytes);
    let plan = super::codec::body_plan(b.cx, ty, root, bytes_layout, Some((result, error)))?;
    let codec = push_codec(b, plan);
    let (value, loan) = b.borrow(bytes, false)?;
    let decoded = b.fresh(result);
    b.push(Inst::Decode {
        result: decoded,
        value,
        codec,
    });
    b.end_loan(loan);
    let (slot, address) = local(b, decoded)?;
    let (tag, tag_loan) = b.borrow(address, false)?;
    let ok = b.block(BlockKind::Body, &[]);
    let broken = b.block(BlockKind::Body, &[]);
    let ok_edge = b.edge(ok, &[]);
    let broken_edge = b.edge(broken, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm {
            value: RESULT_OK,
            edge: ok_edge,
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
        reason: "a runner call's bytes do not decode".into(),
    });
    b.at(ok);
    b.end_loan(tag_loan);
    let field = b.place(
        AddressBase::Address(address),
        &[ProjectionStep::VariantField {
            variant: RESULT_OK,
            field: 0,
        }],
        root,
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

/// The caller's waiting entries.
struct Waiting {
    wait: Intrinsic,
    take: Intrinsic,
}

impl Waiting {
    fn new(cx: &mut Cx<'_>) -> R<Self> {
        Ok(Self {
            wait: intrinsic(cx, "RunnerWait", Vec::new())?,
            take: intrinsic(cx, "RunnerTake", Vec::new())?,
        })
    }
}

/// The types an outcome decodes: the call's `R` and `E` and core `Failure`.
struct Decoded {
    result: TypeId,
    error: TypeId,
    failure: TypeId,
}

/// Wait for the call `ticket` names and build its `Result[R, RunError[E]]` at `returned`.
fn outcome(
    b: &mut Body<'_, '_>,
    waiting: &Waiting,
    ticket: ValueId,
    returned: LayoutId,
    types: &Decoded,
) -> R<ValueId> {
    let code = call(
        b,
        &waiting.wait,
        vec![argument(0, Passing::Borrow, ticket)],
        &[],
    );
    let code = produced("RunnerWait", code)?;
    let bytes = call(
        b,
        &waiting.take,
        vec![argument(0, Passing::Borrow, ticket)],
        &[],
    );
    let bytes = produced("RunnerTake", bytes)?;
    let (bytes_slot, bytes_address) = local(b, bytes)?;
    let run_error = payload(b, returned, RESULT_ERROR)
        .ok_or_else(|| PassError::Malformed("a runner call's Result has no error".into()))?;
    let reason = |b: &Body<'_, '_>, variant: u32| {
        payload(b, run_error, variant)
            .ok_or_else(|| PassError::Malformed(format!("RunError variant {variant}")))
    };
    let (not_run, in_doubt, canceled) = (
        reason(b, NOT_RUN)?,
        reason(b, IN_DOUBT)?,
        reason(b, CANCELED)?,
    );
    let value = b.fresh(returned);
    let joined = b.block(BlockKind::Body, &[value]);
    let held = b.held_state();
    // Each code's arm, built after the switch.
    enum Arm {
        Ok,
        Failed,
        Faulted,
        Nullary {
            run_error: u32,
            layout: LayoutId,
            name: &'static str,
            ordinal: u32,
        },
    }
    let mut arms: Vec<(u32, Arm)> = vec![
        (OUTCOME_OK, Arm::Ok),
        (OUTCOME_FAILED, Arm::Failed),
        (OUTCOME_FAULTED, Arm::Faulted),
    ];
    for (base, count, run_error, layout, name) in [
        (
            OUTCOME_NOT_RUN,
            NOT_RUN_REASONS,
            NOT_RUN,
            not_run,
            "NotRunReason",
        ),
        (
            OUTCOME_IN_DOUBT,
            IN_DOUBT_REASONS,
            IN_DOUBT,
            in_doubt,
            "InDoubtReason",
        ),
        (
            OUTCOME_CANCELED,
            CANCEL_CAUSES,
            CANCELED,
            canceled,
            "CancelCause",
        ),
    ] {
        for ordinal in 0..count {
            arms.push((
                base | ordinal,
                Arm::Nullary {
                    run_error,
                    layout,
                    name,
                    ordinal,
                },
            ));
        }
    }
    let blocks: Vec<BlockId> = arms.iter().map(|_| b.block(BlockKind::Body, &[])).collect();
    let invalid = b.block(BlockKind::Body, &[]);
    let edges: Vec<SwitchArm> = arms
        .iter()
        .zip(&blocks)
        .map(|((code, _), &block)| SwitchArm {
            value: *code,
            edge: b.edge(block, &[]),
        })
        .collect();
    let otherwise = b.edge(invalid, &[]);
    let switch = PoolRange::push(&mut b.ed.unit.pools.switch_arms, edges);
    b.terminate(Terminator::Switch {
        value: code,
        arms: switch,
        otherwise,
    });
    let error = |b: &mut Body, variant_ordinal: u32, value: ValueId| {
        let error = variant(b, run_error, "RunError", variant_ordinal, &[value]);
        variant(b, returned, "Result", RESULT_ERROR, &[error])
    };
    for ((_, arm), block) in arms.into_iter().zip(blocks) {
        b.at(block);
        b.restore_held(&held);
        let result = match arm {
            Arm::Ok => {
                let value = decode(b, types.result, bytes_address)?;
                variant(b, returned, "Result", RESULT_OK, &[value])
            }
            Arm::Failed => {
                let value = decode(b, types.error, bytes_address)?;
                error(b, FAILED, value)
            }
            Arm::Faulted => {
                let value = decode(b, types.failure, bytes_address)?;
                error(b, FAULTED, value)
            }
            Arm::Nullary {
                run_error: variant_ordinal,
                layout,
                name,
                ordinal,
            } => {
                let reason = variant(b, layout, name, ordinal, &[]);
                error(b, variant_ordinal, reason)
            }
        };
        b.jump(joined, &[result]);
    }
    b.at(invalid);
    b.terminate(Terminator::InvariantFault {
        reason: "a runner call's outcome code is unknown".into(),
    });
    b.at(joined);
    b.restore_held(&held);
    // The result is held across the bytes' drop, whose failure then releases it.
    let (slot, address) = local(b, value)?;
    drop_local(b, bytes_slot, bytes_address);
    let value = b.take(address)?;
    b.emptied(slot);
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
    Ok(value)
}

/// The same checked decoder/catch/reply body used by RunnerSend, rooted by a
/// native callee replay instead of an enclosing caller.
pub fn replay_main(program: &crate::instance::Program, path: &DefPath) -> R<Request> {
    let signature = program
        .signatures
        .get(path)
        .ok_or_else(|| PassError::Malformed("missing callee signature".into()))?;
    let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
        return Err(PassError::Malformed("callee is not a function".into()));
    };
    let f = crate::instance::instantiate_function(
        &program.types,
        &program.memo,
        function,
        hirsel_types::Modes::global(),
        &crate::instance::Actuals::none(),
    )
    .map_err(|e| PassError::Malformed(format!("callee callable: {e:?}")))?;
    let send_path = DefPath::core("$intrinsic.RunnerSend");
    let send = Request::operation(program, 0, &send_path, vec![GenericArgument::Type(f)])
        .map_err(|e| PassError::Malformed(format!("callee send signature: {e:?}")))?;
    let main_ty = parameter(&program.types, send.callable, 3)?;
    let path = native_path("runner_main");
    Ok(Request::native(
        super::body_index(&path).expect("registered runner main"),
        &path,
        vec![GenericArgument::Type(f)],
        main_ty,
    ))
}

/// The callee's `main` for entry callable `f`: `$native.runner_main[F]`, a static
/// `fn() -> ()`, RunnerSend's parameter 3.
fn main_request(cx: &mut Cx<'_>, send: &Intrinsic, f: TypeId) -> R<(Direct, LayoutId)> {
    let path = native_path("runner_main");
    let main_ty = parameter(cx.types, send.callable, 3)?;
    let request = Request::native(
        super::body_index(&path).expect("registered runner main"),
        &path,
        vec![GenericArgument::Type(f)],
        main_ty,
    );
    let direct = cx.require(request)?;
    let layout = cx.layouts.table().fn_abis[send.abi.0 as usize].parameters[3].layout;
    Ok((direct, layout))
}

/// Send the call of `f` on `r` with `a`'s bytes: its ticket.
#[allow(clippy::too_many_arguments)]
fn send(
    b: &mut Body<'_, '_>,
    entry: &Intrinsic,
    main: &(Direct, LayoutId),
    r: AddressId,
    f: AddressId,
    a: AddressId,
    a_ty: TypeId,
    form: u64,
) -> R<ValueId> {
    let r = b.read_bits(r)?;
    let bytes = encode(b, a_ty, a)?;
    let (bytes_slot, bytes_address) = local(b, bytes)?;
    let symbol = b.symbol(main.0.symbol.clone());
    let code = b.fresh(main.1);
    b.push(Inst::FunctionRef {
        result: code,
        symbol,
    });
    let (main_slot, main_address) = local(b, code)?;
    let form = b.u64(form)?;
    let (f, f_loan) = b.borrow(f, false)?;
    let (bytes, bytes_loan) = b.borrow(bytes_address, false)?;
    let (code, code_loan) = b.borrow(main_address, false)?;
    let ticket = call(
        b,
        entry,
        vec![
            argument(0, Passing::Borrow, r),
            argument(1, Passing::Borrow, f),
            argument(2, Passing::Borrow, bytes),
            argument(3, Passing::Borrow, code),
            argument(4, Passing::Borrow, form),
        ],
        &[f_loan, bytes_loan, code_loan],
    );
    let ticket = produced("RunnerSend", ticket)?;
    drop_local(b, main_slot, main_address);
    drop_local(b, bytes_slot, bytes_address);
    Ok(ticket)
}

fn compute(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    sent(cx, FORM_COMPUTE)
}

fn call_form(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    sent(cx, FORM_CALL)
}

/// `runner.compute(r, f, a)` and `runner.call(r, f, a)`: send, then wait for the outcome.
fn sent(cx: &mut Cx<'_>, form: u64) -> R<LirUnit<Value>> {
    let (a_ty, r_ty, e_ty) = (cx.type_actual(0)?, cx.type_actual(1)?, cx.type_actual(2)?);
    let entry_ty = parameter(cx.types, cx.callable, 1)?;
    let f_ty = entry_callable(cx.types, entry_ty)?;
    let entry = intrinsic(cx, "RunnerSend", vec![GenericArgument::Type(f_ty)])?;
    let main = main_request(cx, &entry, f_ty)?;
    let waiting = Waiting::new(cx)?;
    let failure = named(cx.types, "Failure", Vec::new());
    let returned_layout = cx
        .fn_abi()
        .result
        .map(|r| r.layout)
        .ok_or_else(|| PassError::Malformed("a runner call returns a Result".into()))?;
    let mut b = Body::new(cx)?;
    let &[r, f, a] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("runner call parameters".into()));
    };
    let ticket = send(
        &mut b, &entry, &main, r.address, f.address, a.address, a_ty, form,
    )?;
    let types = Decoded {
        result: r_ty,
        error: e_ty,
        failure,
    };
    let value = outcome(&mut b, &waiting, ticket, returned_layout, &types)?;
    b.ret(Some(value));
    Ok(b.finish())
}

/// `runner.spawn(s, r, f, a)`: send from the spawner, then `task.async(s, awaited)` over a
/// closure of the ticket.
fn spawn(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let (a_ty, r_ty, e_ty) = (cx.type_actual(0)?, cx.type_actual(1)?, cx.type_actual(2)?);
    let entry_ty = parameter(cx.types, cx.callable, 2)?;
    let f_ty = entry_callable(cx.types, entry_ty)?;
    let entry = intrinsic(cx, "RunnerSend", vec![GenericArgument::Type(f_ty)])?;
    let main = main_request(cx, &entry, f_ty)?;
    let ticket_ty = cx
        .types
        .intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let run_error = named(cx.types, "RunError", vec![GenericArgument::Type(e_ty)]);
    let result_ty = named(
        cx.types,
        "Result",
        vec![
            GenericArgument::Type(r_ty),
            GenericArgument::Type(run_error),
        ],
    );
    let spawn_path = DefPath::core("task.async");
    let spawn_request = Request::operation(
        cx.program,
        super::body_index(&spawn_path).expect("registered task.async"),
        &spawn_path,
        vec![GenericArgument::Type(result_ty)],
    )?;
    let child_ty = parameter(cx.types, spawn_request.callable, 1)?;
    let path = native_path("runner_await");
    let environment = cx
        .layouts
        .environment(
            cx.types,
            &EnvironmentShape {
                definition: hirsel_syntax::DefId::intern(&path),
                body: 0,
                captures: &[(ticket_ty, Passing::Take)],
                sizes: &[],
                movable: &[],
            },
        )
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    let mut request = Request::native(
        super::body_index(&path).expect("registered runner await"),
        &path,
        vec![GenericArgument::Type(r_ty), GenericArgument::Type(e_ty)],
        child_ty,
    );
    request.shape = CallableShape::Closure;
    request.environment = Some(environment);
    let awaited = cx.require(request)?;
    let spawner = cx.require(spawn_request)?;
    let child_layout = cx.layouts.table().fn_abis[spawner.abi.0 as usize].parameters[1].layout;
    let mut b = Body::new(cx)?;
    let &[s, r, f, a] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("runner.spawn parameters".into()));
    };
    let s = b.read_bits(s.address)?;
    let ticket = send(
        &mut b, &entry, &main, r.address, f.address, a.address, a_ty, FORM_SPAWN,
    )?;
    let code = b.symbol(awaited.symbol);
    let captures = PoolRange::push(
        &mut b.ed.unit.pools.captures,
        [CaptureValue {
            value: ticket,
            usage: CaptureUse::Take,
        }],
    );
    let closure = ClosureId(b.ed.unit.tables.closures.len() as u32);
    b.ed.unit.tables.closures.push(ClosureSite {
        receipts: None,
        scoped: false,
        code: Some(ClosureCode {
            symbol: code,
            abi: awaited.abi,
        }),
        environment,
        captures,
        destination: DestinationClass::Inferred,
        placement: None,
        anchor: b.anchor,
    });
    let child = b.fresh(child_layout);
    let failure = b.failure(None);
    b.push(Inst::Closure {
        result: child,
        closure,
        failure,
    });
    // A take argument transfers at entry: a failed spawn releases the child itself.
    let failure = b.failure(None);
    let callee = b.symbol(spawner.symbol);
    let task = invoke(
        &mut b,
        (Callee::Direct(callee), Vec::new()),
        spawner.abi,
        vec![
            argument(0, Passing::Borrow, s),
            argument(1, Passing::Take, child),
        ],
        &[],
        failure,
        BlockKind::Body,
    );
    b.ret(task);
    Ok(b.finish())
}

/// `$native.runner_await[R, E]`: `runner.spawn`'s child, a once closure of its call's ticket.
fn awaited(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let (r_ty, e_ty) = (cx.type_actual(0)?, cx.type_actual(1)?);
    let waiting = Waiting::new(cx)?;
    let failure = named(cx.types, "Failure", Vec::new());
    let returned_layout = cx
        .fn_abi()
        .result
        .map(|r| r.layout)
        .ok_or_else(|| PassError::Malformed("a runner await returns a Result".into()))?;
    let environment = cx
        .environment()
        .ok_or_else(|| PassError::Malformed("a runner await without environment".into()))?;
    let ticket_layout = cx
        .record(environment)
        .fields
        .first()
        .map(|f| f.layout)
        .ok_or_else(|| PassError::Malformed("a runner await captures no ticket".into()))?;
    let mut b = Body::new(cx)?;
    let ticket = b.capture(0, ticket_layout)?;
    let ticket = b.read_bits(ticket.address)?;
    let types = Decoded {
        result: r_ty,
        error: e_ty,
        failure,
    };
    let value = outcome(&mut b, &waiting, ticket, returned_layout, &types)?;
    b.ret(Some(value));
    Ok(b.finish())
}

/// Encode the value at `address` of type `ty` and reply with it under `tag`.
fn reply(
    b: &mut Body<'_, '_>,
    entry: &Intrinsic,
    tag: u64,
    ty: TypeId,
    address: AddressId,
) -> R<()> {
    let bytes = encode(b, ty, address)?;
    let (slot, bytes_address) = local(b, bytes)?;
    let tag = b.u64(tag)?;
    let (bytes, loan) = b.borrow(bytes_address, false)?;
    call(
        b,
        entry,
        vec![
            argument(0, Passing::Borrow, tag),
            argument(1, Passing::Borrow, bytes),
        ],
        &[loan],
    );
    drop_local(b, slot, bytes_address);
    Ok(())
}

/// `$native.runner_main[F]`: the callee run's root task. It decodes its argument, calls its
/// entry inside a catch and replies with the outcome's bytes.
fn main(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let f = cx.type_actual(0)?;
    let Some(TypeKind::Function(function)) = cx.types.get(f).map(|t| t.kind.clone()) else {
        return Err(PassError::Malformed(
            "a runner main's entry is not callable".into(),
        ));
    };
    let call_form = function.parameters.len() == 2;
    let a_ty = function
        .parameters
        .last()
        .map(|p| p.ty)
        .ok_or_else(|| PassError::Malformed("a runner entry takes an argument".into()))?;
    let (r_ty, e_ty) = match cx.types.get(function.result).map(|t| &t.kind) {
        Some(TypeKind::Named { arguments, .. }) => match arguments.as_ref() {
            [GenericArgument::Type(r), GenericArgument::Type(e)] => (*r, *e),
            _ => {
                return Err(PassError::Malformed(
                    "a runner entry returns a Result".into(),
                ));
            }
        },
        _ => {
            return Err(PassError::Malformed(
                "a runner entry returns a Result".into(),
            ));
        }
    };
    let f_abi = cx
        .layouts
        .fn_abi(cx.types, f, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let shape = cx.layouts.table().fn_abis[f_abi.0 as usize].clone();
    if shape
        .parameters
        .iter()
        .any(|p| p.convention != Passing::Borrow)
    {
        return Err(PassError::Unsupported(
            "a runner entry whose parameters are not borrowed".into(),
        ));
    }
    let entry = intrinsic(cx, "RunnerEntry", vec![GenericArgument::Type(f)])?;
    let argument_entry = intrinsic(cx, "RunnerArgument", Vec::new())?;
    let io_entry = intrinsic(cx, "RunnerIo", Vec::new())?;
    let tasks_entry = intrinsic(cx, "RunnerTasks", Vec::new())?;
    let reply_entry = intrinsic(cx, "RunnerReply", Vec::new())?;
    let isolation = super::isolate::entries(cx)?;
    let failure_ty = named(cx.types, "Failure", Vec::new());
    let failure_layout = cx.layout(failure_ty)?;
    let result_layout = cx.layout(function.result)?;
    let mut b = Body::new(cx)?;
    let failure_shape = super::isolate::shape(&b, failure_layout)?;
    // The entry, its argument and the root Io, fresh in this run.
    let value = call(&mut b, &entry, Vec::new(), &[]);
    let value = produced("RunnerEntry", value)?;
    let (f_slot, f_address) = local(&mut b, value)?;
    let bytes = call(&mut b, &argument_entry, Vec::new(), &[]);
    let bytes = produced("RunnerArgument", bytes)?;
    let (bytes_slot, bytes_address) = local(&mut b, bytes)?;
    let value = decode(&mut b, a_ty, bytes_address)?;
    let (a_slot, a_address) = local(&mut b, value)?;
    drop_local(&mut b, bytes_slot, bytes_address);
    let io = if call_form {
        let value = call(&mut b, &io_entry, Vec::new(), &[]);
        let value = produced("RunnerIo", value)?;
        Some(local(&mut b, value)?)
    } else {
        None
    };
    let tasks = call(&mut b, &tasks_entry, Vec::new(), &[]);
    let tasks = produced("RunnerTasks", tasks)?;
    if b.owns(b.layout_of(tasks)) {
        return Err(PassError::Malformed(
            "a Tasks word that owns storage".into(),
        ));
    }
    let start = super::isolate::begin(&mut b, &isolation, tasks)?;
    let held = b.held_state();
    // A failed entry has unwound; the catch resumes ordinary code.
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
    let (callee, f_loan) = b.borrow(f_address, false)?;
    let mut arguments = Vec::new();
    let mut loans = vec![f_loan];
    if let Some((_, io_address)) = io {
        let (io, loan) = b.borrow(io_address, false)?;
        arguments.push(argument(0, Passing::Borrow, io));
        loans.push(loan);
    }
    let (a, a_loan) = b.borrow(a_address, false)?;
    arguments.push(argument(arguments.len() as u32, Passing::Borrow, a));
    loans.push(a_loan);
    let value = invoke(
        &mut b,
        (Callee::Indirect(callee), Vec::new()),
        f_abi,
        arguments,
        &loans,
        unwound,
        BlockKind::Body,
    );
    let value = produced("a runner entry", value)?;
    // The entry's `Ok` or `Error` payload.
    let (result_slot, result_address) = local(&mut b, value)?;
    let (tag, tag_loan) = b.borrow(result_address, false)?;
    let ok = b.block(BlockKind::Body, &[]);
    let error = b.block(BlockKind::Body, &[]);
    let replied = b.block(BlockKind::Body, &[]);
    let ok_edge = b.edge(ok, &[]);
    let error_edge = b.edge(error, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm {
            value: RESULT_OK,
            edge: ok_edge,
        }],
    );
    b.terminate(Terminator::Switch {
        value: tag,
        arms,
        otherwise: error_edge,
    });
    let returned_held = b.held_state();
    for (block, ordinal, ty, tag) in [
        (ok, RESULT_OK, r_ty, REPLY_OK),
        (error, RESULT_ERROR, e_ty, REPLY_ERROR),
    ] {
        b.at(block);
        b.restore_held(&returned_held);
        b.end_loan(tag_loan);
        let layout = payload(&b, result_layout, ordinal)
            .ok_or_else(|| PassError::Malformed("a runner entry's Result payload".into()))?;
        let field = b.place(
            AddressBase::Address(result_address),
            &[ProjectionStep::VariantField {
                variant: ordinal,
                field: 0,
            }],
            layout,
            None,
        )?;
        reply(&mut b, &reply_entry, tag, ty, field)?;
        b.jump(replied, &[]);
    }
    b.at(replied);
    b.restore_held(&returned_held);
    drop_local(&mut b, result_slot, result_address);
    finish(&mut b, a_slot, a_address, io, f_slot, f_address);

    // The caught chain as one `Failure`.
    b.at(caught);
    b.restore_held(&held);
    let failure = super::isolate::chain(&mut b, &isolation, &failure_shape, tasks)?;
    let (failure_slot, failure_address) = local(&mut b, failure)?;
    reply(
        &mut b,
        &reply_entry,
        REPLY_FAILED,
        failure_ty,
        failure_address,
    )?;
    drop_local(&mut b, failure_slot, failure_address);
    finish(&mut b, a_slot, a_address, io, f_slot, f_address);
    Ok(b.finish())
}

/// Drop the callee's argument, root Io and entry, and return.
fn finish(
    b: &mut Body<'_, '_>,
    a_slot: SlotId,
    a_address: AddressId,
    io: Option<(SlotId, AddressId)>,
    f_slot: SlotId,
    f_address: AddressId,
) {
    drop_local(b, a_slot, a_address);
    if let Some((slot, address)) = io {
        drop_local(b, slot, address);
    }
    drop_local(b, f_slot, f_address);
    let unit =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(unit);
}

/// `runner.grants(r) -> GrantSet` (RUN-2): six flags from `RunnerGrants`, one bit each in
/// field order, and empty lists: a native runner's grant set names no directory, network
/// rule or program (launch refuses one that does).
fn grants(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let entry = intrinsic(cx, "RunnerGrants", Vec::new())?;
    let returned_layout = cx
        .fn_abi()
        .result
        .map(|r| r.layout)
        .ok_or_else(|| PassError::Malformed("runner.grants returns a GrantSet".into()))?;
    let mut b = Body::new(cx)?;
    let &[r] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("runner.grants parameters".into()));
    };
    let r = b.read_bits(r.address)?;
    let flags = call(&mut b, &entry, vec![argument(0, Passing::Borrow, r)], &[]);
    let flags = produced("RunnerGrants", flags)?;
    let zero = b.u64(0)?;
    let mut fields = Vec::new();
    for bit in 0..6 {
        let mask = b.u64(1 << bit)?;
        let result = b.value(b.layout_of(flags), Holding::Plain);
        let failure = b.failure(None);
        b.push(Inst::Binary {
            result,
            op: BinaryOp::BitAnd,
            left: flags,
            right: mask,
            failure,
        });
        fields.push(b.compare(BinaryOp::Ne, result, zero)?);
    }
    // Each empty list is held until the record takes it, so a failed construct drops it.
    let mut lists = Vec::new();
    for ordinal in 6..9 {
        let layout = b
            .record(returned_layout)
            .fields
            .iter()
            .find(|f| f.ordinal == ordinal)
            .map(|f| f.layout)
            .ok_or_else(|| PassError::Malformed(format!("GrantSet field {ordinal}")))?;
        let list = construct(&mut b, layout, ConstructorKind::Sequence, &[])?;
        lists.push(local(&mut b, list)?);
    }
    for (slot, address) in lists {
        fields.push(b.take(address)?);
        b.emptied(slot);
        b.push(Inst::StorageDead {
            slot,
            if_live: false,
        });
    }
    let symbol = nominal(&mut b, "GrantSet");
    let value = construct(
        &mut b,
        returned_layout,
        ConstructorKind::Record(symbol),
        &fields,
    )?;
    b.ret(Some(value));
    Ok(b.finish())
}
