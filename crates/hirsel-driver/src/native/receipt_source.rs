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

/// Original entry and compact dependencies, with no DefinitionRef or
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
    let source = input(original)?;
    Ok(ReceiptSourceObservation {
        entry: source.named_entry_selected(base, types, signature, actuals, memo)?,
        demand: source.demand_projection(base)?,
    })
}

/// Observe one actual original closure Assign and all of its original capture
/// operands. Only an exact nongeneric lexical base is supported here; selected
/// enclosing binders require their own complete original actuals account.
pub fn closure(
    original: &NativeOriginalBody,
    base: &InstanceKey,
    types: &TypeInterner,
    parent: hirsel_tir::BodyId,
    block: hirsel_tir::BlockId,
    statement: u32,
) -> Result<ReceiptSourceObservation, SourceError> {
    let source = input(original)?;
    let lexical = InstanceKey::new(
        original.definition().path.path(),
        base.body(),
        &[],
        types,
        None,
        None,
    )
    .map_err(|error| SourceError::Type(format!("{error:?}")))?;
    if &lexical != base {
        return Err(SourceError::Unsupported(
            "closure source observation needs an exact nongeneric lexical base",
        ));
    }
    Ok(ReceiptSourceObservation {
        entry: source.closure_entry(base, types, parent, block, statement, Ok)?,
        demand: source.demand_projection(base)?,
    })
}

fn input(original: &NativeOriginalBody) -> Result<SourceInput, SourceError> {
    let output = original.output().ok_or(SourceError::Unsupported(
        "receipt source observation needs the current original V output",
    ))?;
    if output.owner() != original.definition().path {
        return Err(SourceError::Malformed(
            "receipt source observation names another original owner",
        ));
    }
    Ok(SourceInput::capture(
        output.observation_identity(),
        original.implementation_revision(),
        original.definition().clone(),
    ))
}

pub mod provider;
pub mod selection;

pub mod factory;

pub mod declarations;
