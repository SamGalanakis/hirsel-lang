//! Receipt call contracts. These are local claims: matching a declaration does
//! not prove original-source demand, SSA custody, slot pairing or entered-edge
//! writeback. Executable C/H/backend support remains unavailable until those
//! obligations are independently checked.
use crate::abi_types::FnAbi;
use crate::ir::{AddressId, ArgumentValue, CallSite, Pools, Ref, ValueId};
use crate::sealed_payload_source::{ReceiptContract, ReceiptMode};
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_syntax::ContentHash;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ReceiptArgument {
    Borrow {
        parameter: u32,
        receipt: ValueId,
    },
    Take {
        parameter: u32,
        receipt: ValueId,
    },
    /// The paired receipt-slot writeback version on normal and entered-failure
    /// edges. Rejected admission publishes neither this nor the ordinary version.
    Mut {
        parameter: u32,
        address: AddressId,
        version: ValueId,
    },
}

impl ReceiptArgument {
    pub fn parameter(&self) -> u32 {
        match *self {
            Self::Borrow { parameter, .. }
            | Self::Take { parameter, .. }
            | Self::Mut { parameter, .. } => parameter,
        }
    }
    pub fn mode(&self) -> ReceiptMode {
        match self {
            Self::Borrow { .. } => ReceiptMode::Borrow,
            Self::Take { .. } => ReceiptMode::Take,
            Self::Mut { .. } => ReceiptMode::MutSlot,
        }
    }
    fn visit(&self, f: &mut impl FnMut(Ref)) {
        match *self {
            Self::Borrow { receipt, .. } | Self::Take { receipt, .. } => f(Ref::Value(receipt)),
            Self::Mut {
                address, version, ..
            } => {
                f(Ref::Address(address));
                f(Ref::Value(version));
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptCall {
    /// Complete callee contract, not its callable key or erased machine shape.
    pub contract: ContentHash,
    pub arguments: Vec<ReceiptArgument>,
    /// Defined only on the normal edge, independently of register-returned or
    /// zero-size ordinary results. Callee failure publishes no result receipt.
    pub result: Option<ValueId>,
}

impl ReceiptCall {
    /// Immutable input holders are uses. A Mut version is a writeback
    /// definition, never a use of the previous slot snapshot.
    pub fn inputs(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.arguments
            .iter()
            .filter_map(|argument| match *argument {
                ReceiptArgument::Borrow { receipt, .. } | ReceiptArgument::Take { receipt, .. } => {
                    Some(receipt)
                }
                ReceiptArgument::Mut { .. } => None,
            })
    }

    pub fn writebacks(&self) -> impl Iterator<Item = (AddressId, ValueId)> + '_ {
        self.arguments
            .iter()
            .filter_map(|argument| match *argument {
                ReceiptArgument::Mut {
                    address, version, ..
                } => Some((address, version)),
                _ => None,
            })
    }

    /// Use the same SSA rewrite as the ordinary half. Contract/selector and
    /// address identities remain unchanged when value names are replaced.
    pub fn remap_values(&mut self, map: &impl Fn(ValueId) -> ValueId) {
        for argument in &mut self.arguments {
            match argument {
                ReceiptArgument::Borrow { receipt, .. } | ReceiptArgument::Take { receipt, .. } => {
                    *receipt = map(*receipt)
                }
                ReceiptArgument::Mut { version, .. } => *version = map(*version),
            }
        }
        if let Some(result) = &mut self.result {
            *result = map(*result);
        }
    }

    /// Compare complete local bindings. Original declaration authority and
    /// ordinary/receipt ownership pairing are separate executable obligations.
    pub fn check_contract(&self, contract: &ReceiptContract) -> Result<(), &'static str> {
        if self.contract != contract.content()? {
            return Err("receipt call names a different complete callee contract");
        }
        if self.arguments.len() != contract.parameters.len()
            || self
                .arguments
                .iter()
                .zip(&contract.parameters)
                .any(|(a, p)| a.parameter() != p.source || a.mode() != p.mode)
        {
            return Err(
                "receipt call arguments disagree with canonical parameter selectors or modes",
            );
        }
        if self.result.is_some() != contract.result.is_some() {
            return Err("receipt call result disagrees with callee demand");
        }
        let mut taken = std::collections::BTreeSet::new();
        let mut mutated = std::collections::BTreeSet::new();
        for argument in &self.arguments {
            match argument {
                ReceiptArgument::Take { receipt, .. } if !taken.insert(*receipt) => {
                    return Err("receipt call transfers one holder twice");
                }
                ReceiptArgument::Mut { address, .. } if !mutated.insert(*address) => {
                    return Err("receipt call duplicates an exclusive receipt slot");
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn visit(&self, f: &mut impl FnMut(Ref)) {
        for argument in &self.arguments {
            argument.visit(f);
        }
        if let Some(result) = self.result {
            f(Ref::Value(result));
        }
    }
}

/// Bind each demanded selector to the actual ordinary argument as well as its
/// metadata carrier. This local check cannot prove alias disjointness, typed
/// reaching versions, source authority or metadata ownership/lifetime.
pub fn check_call_contract(
    abi: &FnAbi,
    call: &CallSite,
    pools: &Pools,
) -> Result<(), &'static str> {
    let Some(contract) = &abi.receipts else {
        if abi.key.receipt_contract.is_some() {
            return Err("receipt ABI key without its complete contract");
        }
        return if call.receipts.is_none() {
            Ok(())
        } else {
            Err("receipt ABI transport: call and callee disagree on contract presence")
        };
    };
    if abi.key.receipt_contract != Some(contract.content()?) {
        return Err("receipt ABI key disagrees with its complete contract");
    }
    let receipt = call
        .receipts
        .as_ref()
        .ok_or("receipt ABI transport: call and callee disagree on contract presence")?;
    receipt.check_contract(contract)?;
    let ordinary = call
        .arguments
        .get(&pools.call_arguments)
        .ok_or("receipt call has an invalid ordinary argument range")?;
    for input in &receipt.arguments {
        let mut paired = ordinary.iter().filter(|argument| {
            abi.parameters
                .get(argument.parameter as usize)
                .is_some_and(|parameter| parameter.source == input.parameter())
        });
        let argument = paired
            .next()
            .ok_or("receipt lacks its paired ordinary argument")?;
        if paired.next().is_some() {
            return Err("receipt has duplicate ordinary parameter bindings");
        }
        let passing = match input.mode() {
            ReceiptMode::Borrow => hirsel_core_contract::Passing::Borrow,
            ReceiptMode::Take => hirsel_core_contract::Passing::Take,
            ReceiptMode::MutSlot => hirsel_core_contract::Passing::Mut,
        };
        if argument.passing != passing {
            return Err("receipt differs from ordinary call ownership");
        }
        if matches!(input, ReceiptArgument::Mut { .. })
            && !matches!(argument.value, ArgumentValue::Address { .. })
        {
            return Err("mutable receipt lacks its ordinary mutable address");
        }
    }
    // A unit or other zero-size ordinary result has no physical ValueId.
    // Its independent metadata result must still be published on success.
    if contract.result.is_some() && abi.result.is_some() && call.result.is_none() {
        return Err("receipt result lacks its ordinary non-unit result");
    }
    Ok(())
}

snapshot_enum!(ReceiptArgument {
    Borrow { parameter, receipt },
    Take { parameter, receipt },
    Mut { parameter, address, version }
});
snapshot_struct!(ReceiptCall {
    contract,
    arguments,
    result
});

impl CallSite {
    /// Results exist only on successful return. Unit ordinary results may
    /// still carry an independently demanded metadata result.
    pub fn normal_results(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.result
            .iter()
            .copied()
            .chain(self.receipts.iter().filter_map(|r| r.result))
    }

    /// Both ordinary and metadata slot versions commit only after entry, on
    /// normal and entered-failure outcomes. Rejected admission defines neither.
    pub fn writebacks<'a>(
        &'a self,
        pools: &'a Pools,
    ) -> impl Iterator<Item = (AddressId, ValueId)> + 'a {
        self.arguments
            .get(&pools.call_arguments)
            .unwrap_or(&[])
            .iter()
            .filter_map(|argument| match argument.value {
                ArgumentValue::Address { address, version } => Some((address, version)),
                ArgumentValue::Value(_) => None,
            })
            .chain(self.receipts.iter().flat_map(ReceiptCall::writebacks))
    }
}
