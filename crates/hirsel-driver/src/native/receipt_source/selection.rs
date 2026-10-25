//! Original direct-call selection for receipt context discovery. Source block
//! ordinals select checked TIR calls; candidate LIR and physical ABIs are absent.
use super::{ReceiptSourceObservation, SourceError, provider};
use hirsel_layout::instance::InstanceKey;
use hirsel_rc::instance::{Actuals, SelectionRead, callee_actuals, key_of};
use hirsel_syntax::DefId;
use hirsel_tir::Callee;
use hirsel_types::TypeMemo;

/// Body-free selected original named entry. This does not create a held-callable
/// recipe, a profile, a receipt contract or native admission authority.
pub struct DirectCall {
    pub base: InstanceKey,
    pub actuals: Actuals,
    pub selection: SelectionRead,
    pub source: ReceiptSourceObservation,
}

/// Read one current original call and select its exact lexical specialization.
/// Both the caller observation and the selected callee observation are checked
/// against the provider's current source/signature/revision before release.
pub fn direct(
    provider: &dyn provider::SelectionProvider,
    caller: &InstanceKey,
    actuals: &Actuals,
    block: u32,
    memo: &TypeMemo,
) -> Result<DirectCall, SourceError> {
    let caller = provider::named(provider, caller, actuals, memo)?;
    let call = caller
        .demand()
        .projection()
        .calls()
        .get(&block)
        .ok_or(SourceError::Malformed(
            "receipt direct-call source selector",
        ))?;
    let Callee::Definition { path, arguments } = &call.callee else {
        return Err(SourceError::Unsupported(
            "receipt indirect call needs original callable cases",
        ));
    };
    let closed = callee_actuals(
        provider.types(),
        memo,
        None,
        arguments,
        actuals,
        false,
        path,
    )
    .map_err(|error| SourceError::Type(format!("{error:?}")))?;
    let declaration = DefId::intern(path);
    let selected = provider.selected_instance(declaration, &closed.arguments)?;
    let selection = SelectionRead {
        declaration: path.clone(),
        generated: selected.is_some(),
        arguments: if selected.is_some() {
            Some(
                hirsel_tir::InstanceBodies::key(provider.types(), &closed.arguments)
                    .ok_or(SourceError::Malformed(
                        "receipt generated call needs closed original actuals",
                    ))?
                    .spellings()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            )
        } else {
            None
        },
        selected: selected.map(|definition| definition.path().clone()),
    };
    let target = selected.map_or(path, |definition| definition.path());
    let (signature, original) = provider::original(provider, target)?;
    let definition = original.definition();
    let entry = definition
        .bodies
        .get(definition.entry.0 as usize)
        .ok_or(SourceError::Malformed("receipt direct-call original entry"))?;
    let actuals = if selected.is_some() {
        Actuals::none()
    } else {
        Actuals {
            binder: (!closed.arguments.is_empty()).then_some(entry.parameter_scope),
            arguments: closed.arguments,
            outer: vec![],
        }
    };
    let base = key_of(
        provider.types(),
        target,
        definition.entry,
        &actuals,
        None,
        None,
    )
    .map_err(|error| SourceError::Type(format!("{error:?}")))?;
    let source = super::named(
        &original,
        &base,
        provider.types(),
        &signature,
        &actuals,
        memo,
    )?;
    Ok(DirectCall {
        base,
        actuals,
        selection,
        source,
    })
}
