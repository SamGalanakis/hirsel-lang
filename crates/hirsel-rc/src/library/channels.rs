//! Channels (CONC-6) over the task runtime's entries (`hirsel_rt_v2_channel_*`,
//! `hirsel-rt/src/tasks.rs`).
//!
//! A message crosses inline, as the `Some` image of an `Option[T]` slot the
//! runtime copies. `channel.open` gives `$intrinsic.ChannelOpen` a `None` image
//! and `Option[T]`'s layout descriptor (`ConstantValue::Descriptor`), so the
//! runtime knows a message's size, the `None` a send leaves behind and the drop
//! of a message nobody receives. `channel.send` puts `Some(v)` in a slot and
//! `$intrinsic.ChannelSend` moves it out, leaving `None`, unless the channel
//! refuses it; `channel.receive` passes a `None` slot that
//! `$intrinsic.ChannelReceive` fills. A refused or received value comes back out
//! by moving the `Some` payload out of its slot. The entries' status is 0 sent
//! or received, 1 closed and 2 + cause canceled.
use super::LibraryBody;
use super::builder::{Body, Cx};
use super::iter::{drop_local, local, option};
use super::slots::{construct, nominal};
use super::tasks::{argument, exclusive, intrinsic, invoke, operation, option_of};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;
use hirsel_types::GenericArgument;

type R<T> = Result<T, PassError>;

pub static OPEN: LibraryBody = LibraryBody {
    path: || DefPath::core("channel.open"),
    build: open,
};
pub static SEND: LibraryBody = LibraryBody {
    path: || DefPath::core("channel.send"),
    build: send,
};
pub static RECEIVE: LibraryBody = LibraryBody {
    path: || DefPath::core("channel.receive"),
    build: receive,
};

/// The layout of field `field` of variant `variant` of `layout`.
pub(super) fn variant_field(
    b: &Body<'_, '_>,
    layout: LayoutId,
    variant: u32,
    field: usize,
) -> R<LayoutId> {
    b.record(layout)
        .tag
        .as_ref()
        .and_then(|t| t.variants.iter().find(|v| v.ordinal == variant))
        .and_then(|v| v.fields.get(field))
        .map(|f| f.layout)
        .ok_or_else(|| {
            PassError::Malformed(format!(
                "{layout:?} has no field {field} of variant {variant}"
            ))
        })
}

/// Call `entry` with the endpoint parameter and the message slot; its status.
fn call(
    b: &mut Body<'_, '_>,
    entry: &super::tasks::Intrinsic,
    endpoint: super::builder::Parameter,
    slot: SlotId,
    address: AddressId,
) -> R<ValueId> {
    let failure = b.failure(None);
    let callee = operation(b, entry);
    let arguments = vec![
        exclusive(b, 0, endpoint.slot, endpoint.address),
        exclusive(b, 1, slot, address),
    ];
    invoke(
        b,
        callee,
        entry.abi,
        arguments,
        &[],
        failure,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("a channel entry without a status".into()))
}

/// Whether `status` is 0, and the blocks for both answers.
fn branch_done(b: &mut Body<'_, '_>, status: ValueId) -> R<(BlockId, BlockId)> {
    let layout = b.layout_of(status);
    let zero = b.constant(layout, ConstantValue::Integer(0));
    let done = b.compare(BinaryOp::Eq, status, zero)?;
    let yes = b.block(BlockKind::Body, &[]);
    let no = b.block(BlockKind::Body, &[]);
    b.branch(done, yes, no);
    Ok((yes, no))
}

/// Move the value out of the `Some(v)` message in `slot`, whose storage then ends.
fn unbox(
    b: &mut Body<'_, '_>,
    slot: SlotId,
    address: AddressId,
    option_layout: LayoutId,
) -> R<ValueId> {
    let layout = variant_field(b, option_layout, 1, 0)?;
    let field = b.place(
        AddressBase::Address(address),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        layout,
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

/// The `ChannelError` of a refusal `status` (1 closed, 2 + cause canceled).
fn channel_error(b: &mut Body<'_, '_>, status: ValueId, layout: LayoutId) -> R<ValueId> {
    let cause_layout = variant_field(b, layout, 1, 0)?;
    let error = b.fresh(layout);
    let joined = b.block(BlockKind::Body, &[error]);
    let broken = b.block(BlockKind::Body, &[]);
    let mut arms = Vec::new();
    let channel_error = nominal(b, "ChannelError");
    let cancel_cause = nominal(b, "CancelCause");
    let at = b.current();
    for code in 1..=5u32 {
        let arm = b.block(BlockKind::Body, &[]);
        b.at(arm);
        let value = if code == 1 {
            construct(
                b,
                layout,
                ConstructorKind::Variant {
                    definition: channel_error,
                    variant: 0,
                },
                &[],
            )?
        } else {
            let cause = construct(
                b,
                cause_layout,
                ConstructorKind::Variant {
                    definition: cancel_cause,
                    variant: code - 2,
                },
                &[],
            )?;
            construct(
                b,
                layout,
                ConstructorKind::Variant {
                    definition: channel_error,
                    variant: 1,
                },
                &[cause],
            )?
        };
        b.jump(joined, &[value]);
        let edge = b.edge(arm, &[]);
        arms.push(SwitchArm { value: code, edge });
    }
    b.at(broken);
    b.terminate(Terminator::InvariantFault {
        reason: "a channel entry returned an unknown status".into(),
    });
    b.at(at);
    let otherwise = b.edge(broken, &[]);
    let arms = PoolRange::push(&mut b.ed.unit.pools.switch_arms, arms);
    b.terminate(Terminator::Switch {
        value: status,
        arms,
        otherwise,
    });
    b.at(joined);
    Ok(error)
}

/// `channel.open(tasks) -> (Sender[T], Receiver[T])`: a channel of `Option[T]`
/// images, given their layout descriptor and the `None` a send leaves behind.
fn open(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let element = cx.type_actual(0)?;
    let (option_layout, _) = option_of(cx, element)?;
    let entry = intrinsic(cx, "ChannelOpen", vec![GenericArgument::Type(element)])?;
    let u32_layout = cx.primitive(hirsel_types::Primitive::U32)?;
    let mut b = Body::new(cx)?;
    let &[tasks] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("channel.open parameters".into()));
    };
    let tasks = b.read_bits(tasks.address)?;
    let none = option(&mut b, option_layout, 0, &[]);
    let (none_slot, none_address) = local(&mut b, none)?;
    let (none, none_loan) = b.borrow(none_address, false)?;
    let descriptor = b.constant(u32_layout, ConstantValue::Descriptor(option_layout));
    let failure = b.failure(None);
    let callee = operation(&mut b, &entry);
    let arguments = vec![
        argument(0, Passing::Borrow, tasks),
        argument(1, Passing::Borrow, none),
        argument(2, Passing::Borrow, descriptor),
    ];
    let pair = invoke(
        &mut b,
        callee,
        entry.abi,
        arguments,
        &[none_loan],
        failure,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("a channel open without its endpoints".into()))?;
    // The endpoints are held while the `None` image drops, so its failure edge releases them.
    let (pair_slot, pair_address) = local(&mut b, pair)?;
    drop_local(&mut b, none_slot, none_address);
    let pair = b.take(pair_address)?;
    b.emptied(pair_slot);
    b.push(Inst::StorageDead {
        slot: pair_slot,
        if_live: false,
    });
    b.ret(Some(pair));
    Ok(b.finish())
}

/// `channel.send(tasks, mut s, take v) -> Result[(), (T, ChannelError)]`.
fn send(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let element = cx.type_actual(0)?;
    let (option_layout, _) = option_of(cx, element)?;
    let entry = intrinsic(cx, "ChannelSend", vec![GenericArgument::Type(element)])?;
    let result = cx.fn_abi().result.unwrap().layout;
    let mut b = Body::new(cx)?;
    let &[_, s, v] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("channel.send parameters".into()));
    };
    let value = b.take(v.address)?;
    let some = option(&mut b, option_layout, 1, &[value]);
    let (slot, address) = local(&mut b, some)?;
    let status = call(&mut b, &entry, s, slot, address)?;
    let held = b.held_state();
    let (sent, refused) = branch_done(&mut b, status)?;
    let definition = nominal(&mut b, "Result");
    b.at(sent);
    // The runtime took the image and left `None`.
    drop_local(&mut b, slot, address);
    let unit_layout = variant_field(&b, result, 0, 0)?;
    let unit = b.constant(unit_layout, ConstantValue::Unit);
    let ok = construct(
        &mut b,
        result,
        ConstructorKind::Variant {
            definition,
            variant: 0,
        },
        &[unit],
    )?;
    b.ret(Some(ok));
    b.at(refused);
    b.restore_held(&held);
    let pair_layout = variant_field(&b, result, 1, 0)?;
    let error_layout = b.record(pair_layout).fields[1].layout;
    let error = channel_error(&mut b, status, error_layout)?;
    let value = unbox(&mut b, slot, address, option_layout)?;
    let pair = construct(&mut b, pair_layout, ConstructorKind::Tuple, &[value, error])?;
    let refusal = construct(
        &mut b,
        result,
        ConstructorKind::Variant {
            definition,
            variant: 1,
        },
        &[pair],
    )?;
    b.ret(Some(refusal));
    Ok(b.finish())
}

/// `channel.receive(tasks, mut r) -> Result[T, ChannelError]`.
fn receive(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let element = cx.type_actual(0)?;
    let (option_layout, _) = option_of(cx, element)?;
    let entry = intrinsic(cx, "ChannelReceive", vec![GenericArgument::Type(element)])?;
    let result = cx.fn_abi().result.unwrap().layout;
    let mut b = Body::new(cx)?;
    let &[_, r] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("channel.receive parameters".into()));
    };
    let none = option(&mut b, option_layout, 0, &[]);
    let (slot, address) = local(&mut b, none)?;
    let status = call(&mut b, &entry, r, slot, address)?;
    let held = b.held_state();
    let (received, refused) = branch_done(&mut b, status)?;
    let definition = nominal(&mut b, "Result");
    b.at(received);
    let value = unbox(&mut b, slot, address, option_layout)?;
    let ok = construct(
        &mut b,
        result,
        ConstructorKind::Variant {
            definition,
            variant: 0,
        },
        &[value],
    )?;
    b.ret(Some(ok));
    b.at(refused);
    b.restore_held(&held);
    // The slot still holds `None`.
    drop_local(&mut b, slot, address);
    let error_layout = variant_field(&b, result, 1, 0)?;
    let error = channel_error(&mut b, status, error_layout)?;
    let refusal = construct(
        &mut b,
        result,
        ConstructorKind::Variant {
            definition,
            variant: 1,
        },
        &[error],
    )?;
    b.ret(Some(refusal));
    Ok(b.finish())
}
