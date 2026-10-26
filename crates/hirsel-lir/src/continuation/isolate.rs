//! Independently derived callback identity for a specialized isolate call.
//!
//! This certificate alone never enables compact entry: completed begin and
//! every reachable cleanup still need their own continuation protocols. Code
//! identity is reconstructed from the caller's SSA, independently of RC's
//! specialization discovery and instance naming.
use std::collections::BTreeSet;

use crate::abi_types::FnAbiId;
use crate::ir::*;
use crate::layout_types::{LayoutId, LayoutKind, LayoutTable};
use crate::stage::Ready;

mod admission;
pub use admission::{CallbackBody, IsolateEvidence};
pub(crate) use admission::{candidate, evidence, intrinsic};
mod begin;
mod cleanup;
mod frame;
pub use cleanup::{CleanupOwner, SynchronousCleanup, SynchronousCleanupPlan};
pub use frame::IsolatePlan;
mod protocol;
pub use protocol::{CallPoint, CleanupPoint, HeldResult, InstructionPoint, IsolateProtocol};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct IsolateCallback {
    pub wrapper: hirsel_syntax::ContentHash,
    pub wrapper_digest: hirsel_syntax::ContentHash,
    pub callback_digest: hirsel_syntax::ContentHash,
    pub invocation: CallId,
    pub callback: CallId,
    pub target: Symbol,
    pub abi: FnAbiId,
    pub environment: Option<LayoutId>,
    pub closure: SlotId,
    pub loan: LoanId,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct Target {
    symbol: Symbol,
    abi: FnAbiId,
    environment: Option<LayoutId>,
}

fn whole(unit: &LirUnit<Ready>, address: AddressId) -> Option<SlotId> {
    let mut at = address;
    let mut seen = BTreeSet::new();
    while seen.insert(at) {
        let address = unit.addresses.get(at.index())?;
        if address.projection.len != 0 || !address.unchecked.is_empty() {
            return None;
        }
        match address.base {
            AddressBase::Slot(slot) => return Some(slot),
            AddressBase::Address(parent) => at = parent,
            AddressBase::Value(_) => return None,
        }
    }
    None
}

/// Trace only immutable, whole callable values. Unknown parameters, mutable
/// storage, divergent incoming identities and cycles remain unproved.
fn target(
    unit: &LirUnit<Ready>,
    value: ValueId,
    body: &LirUnit<Ready>,
    seed: Option<(ValueId, &Target)>,
    visiting: &mut BTreeSet<ValueId>,
) -> Option<Target> {
    if let Some((parameter, target)) = seed
        && parameter == value
    {
        return Some(target.clone());
    }
    if !visiting.insert(value) {
        return None;
    }
    let mut incoming = Vec::new();
    let mut found = None;
    for block in &unit.blocks {
        for inst in block.instructions.get(&unit.instructions)? {
            if !inst.defs().contains(&value) {
                continue;
            }
            match *inst {
                Inst::Closure {
                    result, closure, ..
                } if result == value => {
                    let closure = unit.tables.closures.get(closure.index())?;
                    let code = closure.code?;
                    found = Some(Target {
                        symbol: unit.tables.symbols.get(code.symbol.index())?.clone(),
                        abi: code.abi,
                        environment: Some(closure.environment),
                    });
                }
                Inst::FunctionRef { result, symbol } if result == value => {
                    let symbol = unit.tables.symbols.get(symbol.index())?;
                    if symbol.definition != body.header.definition
                        || symbol.instance != Some(body.header.instance)
                    {
                        return None;
                    }
                    found = Some(Target {
                        symbol: symbol.clone(),
                        abi: body.header.abi,
                        environment: None,
                    });
                }
                Inst::Weaken {
                    result,
                    value: from,
                } if result == value => incoming.push(from),
                Inst::Read {
                    result,
                    address,
                    reaching,
                    ..
                }
                | Inst::Copy {
                    result,
                    address,
                    reaching,
                    ..
                } if result == value => {
                    let slot = whole(unit, address)?;
                    if unit
                        .tables
                        .loans
                        .iter()
                        .any(|loan| loan.mutable && whole(unit, loan.address) == Some(slot))
                    {
                        return None;
                    }
                    incoming.push(reaching);
                }
                Inst::Store {
                    address,
                    value: from,
                    version,
                } if version == value => {
                    whole(unit, address)?;
                    incoming.push(from);
                }
                _ => return None,
            }
        }
    }
    for (index, block) in unit.blocks.iter().enumerate() {
        let parameters = block.parameters.get(&unit.pools.values)?;
        if let Some(ordinal) = parameters.iter().position(|p| *p == value) {
            for edge in &unit.tables.edges {
                if edge.target.index() == index {
                    incoming.push(*edge.arguments.get(&unit.pools.values)?.get(ordinal)?);
                }
            }
        }
    }
    for from in incoming {
        let next = target(unit, from, body, seed, visiting)?;
        if found.as_ref().is_some_and(|old| *old != next) {
            return None;
        }
        found = Some(next);
    }
    visiting.remove(&value);
    found
}

impl IsolateCallback {
    /// Prove one actual call's specialized callback against all three Ready
    /// bodies and the admitted callback environment descriptor. No operation
    /// classification, instance-name prefix or optimizer flag supplies identity.
    pub fn prove(
        caller: &LirUnit<Ready>,
        invocation: CallId,
        wrapper: &LirUnit<Ready>,
        callback: &LirUnit<Ready>,
        environment: Option<LayoutId>,
        layouts: &LayoutTable,
    ) -> Option<Self> {
        if wrapper.header.origin != UnitOrigin::Library
            || !wrapper.header.definition.is_core("task.isolate")
        {
            return None;
        }
        let call = caller.tables.calls.get(invocation.index())?;
        let Callee::Direct(symbol) = call.callee else {
            return None;
        };
        let symbol = caller.tables.symbols.get(symbol.index())?;
        if symbol.definition != wrapper.header.definition
            || symbol.instance != Some(wrapper.header.instance)
            || call.abi != wrapper.header.abi
        {
            return None;
        }
        let arguments = call.arguments.get(&caller.pools.call_arguments)?;
        let actual = arguments.iter().find(|arg| arg.parameter == 1)?;
        let ArgumentValue::Value(value) = actual.value else {
            return None;
        };
        if actual.passing != hirsel_core_contract::Passing::Take {
            return None;
        }
        if !matches!(
            layouts
                .layout(caller.types[caller.values[value.index()].ty.index()].layout)?
                .kind,
            LayoutKind::Callable { .. }
        ) {
            return None;
        }
        let actual = target(caller, value, callback, None, &mut BTreeSet::new())?;
        if actual.symbol.definition != callback.header.definition
            || actual.symbol.instance != Some(callback.header.instance)
            || actual.abi != callback.header.abi
            || actual.environment != environment
        {
            return None;
        }
        let mut callbacks =
            wrapper.tables.calls.iter().enumerate().filter(|(_, call)| {
                matches!(call.callee, Callee::Indirect(_) | Callee::Bound { .. })
            });
        let (at, call) = callbacks.next()?;
        if callbacks.next().is_some() || call.abi != actual.abi {
            return None;
        }
        let Callee::Bound { code, closure } = call.callee else {
            return None;
        };
        if actual.environment.is_none()
            || wrapper.tables.symbols.get(code.index())? != &actual.symbol
        {
            return None;
        }
        let Holding::Borrowed(loan) = wrapper.values.get(closure.index())?.holding else {
            return None;
        };
        let parameter = layouts
            .fn_abis
            .get(wrapper.header.abi.0 as usize)?
            .parameters
            .iter()
            .position(|p| p.source == 1)?;
        let parameter = *wrapper.parameters.get(parameter)?;
        if target(
            wrapper,
            closure,
            callback,
            Some((parameter, &actual)),
            &mut BTreeSet::new(),
        )? != actual
        {
            return None;
        }
        let borrowed = wrapper.tables.loans.get(loan.index())?;
        if borrowed.mutable {
            return None;
        }
        let closure = whole(wrapper, borrowed.address)?;
        Some(Self {
            wrapper: wrapper.header.instance,
            wrapper_digest: wrapper.digest(),
            callback_digest: callback.digest(),
            invocation,
            callback: CallId(at as u32),
            target: actual.symbol,
            abi: actual.abi,
            environment,
            closure,
            loan,
        })
    }
}
