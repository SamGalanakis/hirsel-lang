//! Receipt call contracts. These are local claims: matching a declaration does
//! not prove original-source demand, SSA custody, slot pairing or entered-edge
//! writeback. Executable C/H/backend support remains unavailable until those
//! obligations are independently checked.
use crate::ir::{AddressId, Ref, ValueId};
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
