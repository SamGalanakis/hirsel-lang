//! Channels (CONC-6) over the task runtime's entries (`hirsel_rt_v2_channel_*`,
//! `hirsel-rt/src/tasks.rs`).
//!
//! A message crosses as the block of a one-element `List[T]` in an `Option`
//! slot, so the runtime moves block references and never reads `T`'s layout.
//! `channel.send` builds `Some([v])` and `$intrinsic.ChannelSend` takes the
//! block, leaving `None`, unless the channel refuses it; `channel.receive`
//! passes a `None` slot that `$intrinsic.ChannelReceive` fills. A refused or
//! received value comes back out by moving the list's one element out and
//! dropping the then empty list. The entries' status is 0 sent or received,
//! 1 closed and 2 + cause canceled.
use super::LibraryBody;
use super::builder::{Body, Cx};
use super::iter::{drop_local, local, option};
use super::slots::{construct, nominal};
use super::tasks::{exclusive, intrinsic, invoke, operation, option_of};
use crate::PassError;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;
use hirsel_types::{BuiltinType, GenericArgument, Type, TypeKind};

type R<T> = Result<T, PassError>;

pub static SEND: LibraryBody = LibraryBody {
    path: || DefPath::core("channel.send"),
    build: send,
};
pub static RECEIVE: LibraryBody = LibraryBody {
    path: || DefPath::core("channel.receive"),
    build: receive,
};

/// The message carrier `List[T]` of `channel.<op>[T]`: its layout and the
/// layout of its `Option`.
pub(super) fn carrier(cx: &mut Cx<'_>) -> R<(LayoutId, LayoutId)> {
    let element = cx.type_actual(0)?;
    let list = cx.types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(element)].into(),
    }));
    let list_layout = cx.layout(list)?;
    let (option_layout, _) = option_of(cx, list)?;
    Ok((list_layout, option_layout))
}

/// The layout of field `field` of variant `variant` of `layout`.
fn variant_field(b: &Body<'_, '_>, layout: LayoutId, variant: u32, field: usize) -> R<LayoutId> {
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

/// Move the value out of the `Some([v])` message in `slot`, whose storage then ends,
/// and drop the empty list. The value is left in a local the body holds.
pub(super) fn unbox(
    b: &mut Body<'_, '_>,
    slot: SlotId,
    address: AddressId,
    option_layout: LayoutId,
) -> R<ValueId> {
    let list_layout = variant_field(b, option_layout, 1, 0)?;
    let field = b.place(
        AddressBase::Address(address),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        list_layout,
        None,
    )?;
    let list = b.take(field)?;
    b.emptied(slot);
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
    let (list_slot, list_address) = local(b, list)?;
    let zero = b.u64(0)?;
    let at = b.slot_place(list_address, zero)?;
    let value = b.take(at)?;
    let (value_slot, value_address) = local(b, value)?;
    b.seq_set_len(list_address, zero)?;
    drop_local(b, list_slot, list_address);
    let value = b.take(value_address)?;
    b.emptied(value_slot);
    b.push(Inst::StorageDead {
        slot: value_slot,
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

/// `channel.send(tasks, mut s, take v) -> Result[(), (T, ChannelError)]`.
fn send(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let (list_layout, option_layout) = carrier(cx)?;
    let element = cx.type_actual(0)?;
    let entry = intrinsic(cx, "ChannelSend", vec![GenericArgument::Type(element)])?;
    let result = cx.fn_abi().result.unwrap().layout;
    let mut b = Body::new(cx)?;
    let &[_, s, v] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("channel.send parameters".into()));
    };
    let value = b.take(v.address)?;
    let list = construct(&mut b, list_layout, ConstructorKind::Sequence, &[value])?;
    let some = option(&mut b, option_layout, 1, &[list]);
    let (slot, address) = local(&mut b, some)?;
    let status = call(&mut b, &entry, s, slot, address)?;
    let held = b.held_state();
    let (sent, refused) = branch_done(&mut b, status)?;
    let definition = nominal(&mut b, "Result");
    b.at(sent);
    // The runtime took the block and left `None`.
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
    let (_, option_layout) = carrier(cx)?;
    let element = cx.type_actual(0)?;
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
