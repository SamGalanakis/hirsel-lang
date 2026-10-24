//! Body-free receipt observations captured from the original Lower/V boundary.
//! These retain current source identity and full original actuals, not a native
//! contract, complete target/profile account, provider row or C authority.
use hirsel_engine::NativeOriginalBody;
use hirsel_layout::instance::InstanceKey;
use hirsel_rc::instance::Actuals;
use hirsel_rc::sealed_source::{
    SourceCallableEntry, SourceDemandProjection, SourceError, SourceInput,
};
use hirsel_types::{Signature, TypeInterner, TypeMemo};

/// Original named entry and compact dependencies, with no DefinitionRef or
/// candidate native IR. A query owner must still independently resolve every
/// call/closure and qualified profile before deriving a boundary contract.
pub struct ReceiptSourceObservation {
    entry: SourceCallableEntry,
    demand: SourceDemandProjection,
}
impl ReceiptSourceObservation {
    pub fn entry(&self) -> &SourceCallableEntry {
        &self.entry
    }
    pub fn demand(&self) -> &SourceDemandProjection {
        &self.demand
    }
}

/// Capture during an actual original-body read, before physical rewriting or
/// body release. Missing V is unavailable, never a fabricated empty demand.
/// The selected original actuals are checked against the complete base key;
/// original source modes remain beside that key's native type erasure.
pub fn named(
    original: &NativeOriginalBody,
    base: &InstanceKey,
    types: &TypeInterner,
    signature: &Signature,
    actuals: &Actuals,
    memo: &TypeMemo,
) -> Result<ReceiptSourceObservation, SourceError> {
    let output = original.output().ok_or(SourceError::Unsupported(
        "receipt source observation needs the current original V output",
    ))?;
    if output.owner() != original.definition().path {
        return Err(SourceError::Malformed(
            "receipt source observation names another original owner",
        ));
    }
    let source = SourceInput::capture(
        output.observation_identity(),
        original.implementation_revision(),
        original.definition().clone(),
    );
    Ok(ReceiptSourceObservation {
        entry: source.named_entry_selected(base, types, signature, actuals, memo)?,
        demand: source.demand_projection(base)?,
    })
}
