//! Entered Free intrinsics are admitted by their actual wrapper and carrier,
//! never by an operation-boundary hint or scalar ExclusiveSlot evidence.
use super::*;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TailKind {
    Put = 0,
    Kill = 1,
    Cause = 2,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct TailCarrier {
    pub slot: SlotId,
    pub address: AddressId,
    pub loan: Option<LoanId>,
    pub layout: LayoutId,
    pub offset: u64,
    pub size: u64,
    pub align: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct OwnedTail {
    pub kind: TailKind,
    pub carrier: Option<TailCarrier>,
    pub normal: EdgeId,
    pub failure: FailureId,
}

pub(super) fn known_wrapper(u: &LirUnit<Ready>) -> bool {
    core_name(&u.header.definition).as_deref() == Some("mutex.with")
        && !u
            .tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Indirect(_)))
        && u.tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Bound { .. } | Callee::Direct(_)))
}

fn intrinsic(u: &LirUnit<Ready>, call: CallId) -> Option<String> {
    match u.tables.calls[call.index()].callee {
        Callee::Operation(op) => core_name(&u.tables.operations[op.index()].definition),
        _ => None,
    }
}

/// Every callback-failure path reaches kill before payload/capture cleanup.
/// This is deliberately narrower than arbitrary LIR: unknown wrappers keep
/// their synchronous entry and cannot leak Pending to an opaque caller.
fn kills_first(u: &LirUnit<Ready>, start: BlockId) -> bool {
    let mut seen = BTreeSet::new();
    let mut pending = vec![start];
    while let Some(b) = pending.pop() {
        if !seen.insert(b) {
            return false;
        }
        if u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
            .iter()
            .any(|i| {
                matches!(
                    i,
                    Inst::Drop { .. } | Inst::Release { .. } | Inst::FinalizeBegin { .. }
                )
            })
        {
            return false;
        }
        match u.terminators[b.index()] {
            Terminator::Call { call, .. }
                if intrinsic(u, call).as_deref() == Some("$intrinsic.MutexKill") =>
            {
                continue;
            }
            Terminator::Call { .. }
            | Terminator::Propagate
            | Terminator::Return(_)
            | Terminator::InvariantFault { .. } => return false,
            _ => {
                let next = successors_of(u, b);
                if next.is_empty() {
                    return false;
                }
                pending.extend(next);
            }
        }
    }
    true
}

pub(super) fn derive(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    call: CallId,
    block: BlockId,
    slots: &BTreeSet<SlotId>,
    loans: &BTreeSet<LoanId>,
    storage: &[LiveStorage],
) -> Result<Option<OwnedTail>, String> {
    if !known_wrapper(u) {
        return Ok(None);
    }
    let kind = match intrinsic(u, call).as_deref() {
        Some("$intrinsic.MutexPut") => TailKind::Put,
        Some("$intrinsic.MutexKill") => TailKind::Kill,
        Some("$intrinsic.MutexCause") => TailKind::Cause,
        _ => return Ok(None),
    };
    for (b, term) in u.terminators.iter().enumerate() {
        if let Terminator::Call { call, failure, .. } = *term
            && matches!(
                u.tables.calls[call.index()].callee,
                Callee::Bound { .. } | Callee::Direct(_)
            )
            && !kills_first(
                u,
                u.tables.edges[u.tables.failures[failure.index()].cleanup.index()].target,
            )
        {
            return Err(format!("mutex callback cleanup before kill at {b}"));
        }
    }
    let Terminator::Call {
        normal: Some(normal),
        failure,
        ..
    } = u.terminators[block.index()]
    else {
        return Err("diverging owned tail".into());
    };
    let carrier = if kind == TailKind::Put {
        let args = u.tables.calls[call.index()]
            .arguments
            .get(&u.pools.call_arguments)
            .unwrap_or(&[]);
        let Some(arg) = args.iter().find(|a| a.parameter == 1) else {
            return Err("missing put carrier".into());
        };
        let ArgumentValue::Address { address, .. } = arg.value else {
            return Err("put carrier is not storage".into());
        };
        let at = &u.addresses[address.index()];
        let AddressBase::Slot(slot) = at.base else {
            return Err("put carrier is not a whole frame slot".into());
        };
        if at.projection.len != 0 || !at.unchecked.is_empty() || !slots.contains(&slot) {
            return Err("unstable put carrier".into());
        }
        let (layout, _) = slot_layout(u, slot);
        let (size, align) = fixed(table, layout).ok_or("dynamic put image")?;
        let stored = storage
            .iter()
            .find(|s| s.slot == slot)
            .ok_or("put image outside frame")?;
        let root = |mut a: AddressId| loop {
            match u.addresses[a.index()].base {
                AddressBase::Address(p) => a = p,
                AddressBase::Slot(s) => break Some(s),
                _ => break None,
            }
        };
        let reaching: Vec<_> = loans
            .iter()
            .copied()
            .filter(|l| root(u.tables.loans[l.index()].address) == Some(slot))
            .collect();
        // Library bodies use an exclusive call address directly, without
        // a BeginLoan instruction. Source bodies can carry its explicit loan.
        let loan = match reaching.as_slice() {
            [] => None,
            [loan] => Some(*loan),
            _ => return Err("aliased put carrier".into()),
        };
        if arg.passing != Passing::Mut || align > 16
            || loan.is_some_and(|loan| !u.tables.loans[loan.index()].mutable || u.tables.loans[loan.index()].address != address)
            || args.iter().any(|a| a.parameter != 1 && matches!(a.value, ArgumentValue::Address { address, .. } if root(address) == Some(slot)))
            || stored.layout != layout || stored.offset % align != 0 || u.types[at.ty.index()].layout != layout
            || !matches!(u.slots[slot.index()].kind, SlotKind::Local | SlotKind::Temporary)
        { return Err("unproved put ownership transfer".into()); }
        Some(TailCarrier {
            slot,
            address,
            loan,
            layout,
            offset: stored.offset,
            size,
            align,
        })
    } else {
        None
    };
    Ok(Some(OwnedTail {
        kind,
        carrier,
        normal,
        failure,
    }))
}
