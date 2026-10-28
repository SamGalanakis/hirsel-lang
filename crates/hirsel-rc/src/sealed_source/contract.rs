//! Assemble canonical boundary requirements from original entry observations
//! and the complete compact demand graph. This produces DATA, not a source/V/C
//! verdict, provider Some, selected native instance, or physical carrier schema.
//!
//! The query owner must independently observe profiles/target expansion and
//! select one unioned row per qualified ClosedStorageProfile context. This
//! module does not create contexts or specialize by runtime origin/history.
use super::demand::{self, CallDemand, CompactBody, Requirements};
use super::{SourceCallableEntry, SourceDemandProjection, SourceError};
use hirsel_core_contract::Passing;
use hirsel_lir::operation::NativeType;
use hirsel_lir::sealed_payload_source::{
    ClosedStorageProfile, ClosedStorageType, ReceiptCapture, ReceiptContract, ReceiptGrant,
    ReceiptMode, ReceiptParameter, ReceiptResult, ReceiptType,
};
use std::collections::BTreeMap;

pub mod registry;

/// Original entry selector, not a filtered ABI or environment ordinal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Port {
    Parameter(u32),
    Result,
    Capture(u32),
}

pub struct Input<'a> {
    pub entry: &'a SourceCallableEntry,
    pub source: &'a SourceDemandProjection,
    pub calls: &'a BTreeMap<u32, CallDemand>,
    pub closures: &'a BTreeMap<u32, usize>,
    /// Independently observed complete profiles. Structural matching here
    /// cannot establish source/CT5 completeness or authorize their use by C.
    pub profiles: &'a BTreeMap<Port, ClosedStorageProfile>,
}

/// Body-free output. An observed empty graph requirement is None; an internal
/// demand with no boundary carriers is Some(empty contract), not ordinary.
/// Neither case manufactures an independent source observation or C verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Declaration {
    entry: SourceCallableEntry,
    requirements: Requirements,
    contract: Option<ReceiptContract>,
}
impl Declaration {
    pub fn entry(&self) -> &SourceCallableEntry {
        &self.entry
    }
    pub fn requirements(&self) -> &Requirements {
        &self.requirements
    }
    pub fn contract(&self) -> Option<&ReceiptContract> {
        self.contract.as_ref()
    }
}

fn original_type(
    entry: &SourceCallableEntry,
    port: Port,
) -> Result<&ClosedStorageType, SourceError> {
    match port {
        Port::Parameter(ordinal) => entry
            .parameters()
            .iter()
            .find(|p| p.ordinal == ordinal)
            .map(|p| &p.ty),
        Port::Result => Some(entry.result()),
        Port::Capture(ordinal) => entry
            .captures()
            .iter()
            .find(|c| c.ordinal == ordinal)
            .map(|c| &c.ty),
    }
    .ok_or(SourceError::Malformed(
        "receipt profile has no original entry selector",
    ))
}

fn receipt_type(input: &Input<'_>, port: Port) -> Result<ReceiptType, SourceError> {
    let profile = input.profiles.get(&port).ok_or(SourceError::Unsupported(
        "receipt demand needs independently observed closed profile",
    ))?;
    let original = original_type(input.entry, port)?;
    if &profile.root != original {
        return Err(SourceError::Malformed(
            "receipt profile differs from full original source type",
        ));
    }
    Ok(ReceiptType {
        source_type: original.source_type,
        profile: profile.content().map_err(SourceError::Malformed)?,
    })
}

fn binding(input: &Input<'_>) -> Result<(), SourceError> {
    if input.entry.reference() != input.source.reference()
        || input.entry.source_identity() != input.source.source_identity()
        || input.entry.implementation_revision() != input.source.implementation_revision()
    {
        return Err(SourceError::Malformed(
            "receipt entry and demand source observation differ",
        ));
    }
    let projection = input.source.projection();
    if input
        .entry
        .parameters()
        .iter()
        .map(|p| p.local)
        .ne(projection.parameters().iter().copied())
        || input
            .entry
            .captures()
            .iter()
            .map(|c| c.local)
            .ne(projection.captures().iter().copied())
    {
        return Err(SourceError::Malformed(
            "receipt entry and demand original locals differ",
        ));
    }
    for (&port, profile) in input.profiles {
        profile.check_structure().map_err(SourceError::Malformed)?;
        if &profile.root != original_type(input.entry, port)? {
            return Err(SourceError::Malformed(
                "receipt profile differs from full original source type",
            ));
        }
    }
    Ok(())
}

fn assemble(input: &Input<'_>, requirements: Requirements) -> Result<Declaration, SourceError> {
    if requirements.result_inventory {
        return Err(SourceError::Unsupported(
            "receipt result needs original grant transport",
        ));
    }
    let mut parameters = Vec::new();
    let mut inventory = Vec::new();
    for parameter in input.entry.parameters() {
        if requirements.payload_locals.contains(&parameter.local)
            && !hirsel_lir::sealed_payload_source::is_shared_boundary(&parameter.ty.ty)
        {
            parameters.push(ReceiptParameter {
                source: parameter.ordinal,
                ty: receipt_type(input, Port::Parameter(parameter.ordinal))?,
                mode: match parameter.passing {
                    Passing::Borrow => ReceiptMode::Borrow,
                    Passing::Take => ReceiptMode::Take,
                    Passing::Mut => ReceiptMode::MutSlot,
                },
            });
        }
        if requirements.inventory_locals.contains(&parameter.local) {
            if !matches!(&parameter.ty.ty, NativeType::Named(path, args) if path.is_core("Arena") && args.is_empty())
            {
                return Err(SourceError::Unsupported(
                    "receipt inventory needs exact original Arena grant",
                ));
            }
            inventory.push(ReceiptGrant::Parameter(parameter.ordinal));
        }
    }
    let mut captures = Vec::new();
    for capture in input.entry.captures() {
        let payload = requirements.payload_locals.contains(&capture.local);
        let retained = requirements.inventory_locals.contains(&capture.local);
        if (payload || retained) && capture.usage == hirsel_types::UseKind::Mut {
            return Err(SourceError::Unsupported(
                "receipt mutable capture needs exclusive environment transport",
            ));
        }
        if payload && !hirsel_lir::sealed_payload_source::is_shared_boundary(&capture.ty.ty) {
            captures.push(ReceiptCapture {
                ordinal: capture.ordinal,
                ty: receipt_type(input, Port::Capture(capture.ordinal))?,
                // Copy and Take both install an owning ordinary capture. The
                // companion has its own metadata holder in either case; the
                // original usage remains bound by the observed closure entry.
                mode: match capture.usage {
                    hirsel_types::UseKind::Borrow => ReceiptMode::Borrow,
                    hirsel_types::UseKind::Copy | hirsel_types::UseKind::Take => ReceiptMode::Take,
                    hirsel_types::UseKind::Mut => unreachable!("checked demanded capture usage"),
                },
            });
        }
        if retained {
            if capture.usage != hirsel_types::UseKind::Borrow {
                return Err(SourceError::Unsupported(
                    "receipt owning inventory capture needs original grant transport",
                ));
            }
            if !matches!(&capture.ty.ty, NativeType::Named(path, args) if path.is_core("Arena") && args.is_empty())
            {
                return Err(SourceError::Unsupported(
                    "receipt inventory needs exact original Arena grant",
                ));
            }
            inventory.push(ReceiptGrant::Capture(capture.ordinal));
        }
    }
    let result = if requirements.result
        && !hirsel_lir::sealed_payload_source::is_shared_boundary(&input.entry.result().ty)
    {
        Some(ReceiptResult {
            ty: receipt_type(input, Port::Result)?,
        })
    } else {
        None
    };
    let empty = requirements.payload_locals.is_empty()
        && requirements.inventory_locals.is_empty()
        && !requirements.result;
    let contract = if empty {
        None
    } else {
        Some(
            ReceiptContract::new(parameters, result, captures, inventory)
                .map_err(SourceError::Malformed)?,
        )
    };
    Ok(Declaration {
        entry: input.entry.clone(),
        requirements,
        contract,
    })
}

/// Recompute demand, then derive selectors/modes/full type/profile products.
/// Missing original expansion/profile is an error, never an empty fallback.
/// Source observations remain separate from equality-backdated semantic output.
pub fn derive(inputs: &[Input<'_>]) -> Result<Vec<Declaration>, SourceError> {
    for input in inputs {
        binding(input)?;
    }
    let graph = inputs
        .iter()
        .map(|input| CompactBody {
            source: input.source.projection(),
            calls: input.calls,
            closures: input.closures,
        })
        .collect::<Vec<_>>();
    let requirements = demand::analyze_compact(&graph)?;
    inputs
        .iter()
        .zip(requirements)
        .map(|(input, requirements)| assemble(input, requirements))
        .collect()
}

#[cfg(test)]
mod tests;
