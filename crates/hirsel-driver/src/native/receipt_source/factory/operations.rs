//! Exact reviewed source operation expansion. Unknown operations never supply
//! an empty requirement, even if their current native body is unavailable.
use super::{SourceError, provider};
use hirsel_core_contract::OperationId;
use hirsel_rc::sealed_source::demand::{ExternalDemand, SourceCall};
use hirsel_tir::Callee;
use hirsel_types::{GenericArgument, UseKind};

pub(super) fn expand(
    provider: &dyn provider::Provider,
    call: &SourceCall,
) -> Result<Option<(OperationId, ExternalDemand)>, SourceError> {
    let Callee::Definition { path, arguments } = &call.callee else {
        return Ok(None);
    };
    if !path.is_core("arena.seal") {
        return Ok(None);
    }
    let registry = hirsel_engine::registry::BuiltinRegistry::shared();
    let declaration = provider.signature(path)?;
    let reviewed = registry
        .get(declaration.definition)
        .filter(|signature| signature.definition.path() == path)
        .ok_or(SourceError::Malformed(
            "receipt operation lacks its reviewed core declaration",
        ))?;
    let actual = hirsel_types::view_digest(provider.types(), &declaration)
        .map_err(|error| SourceError::Type(format!("{error:?}")))?;
    let expected = hirsel_types::view_digest(registry.types(), &reviewed)
        .map_err(|error| SourceError::Type(format!("{error:?}")))?;
    if declaration.definition.path() != path
        || declaration.fingerprint != reviewed.fingerprint
        || actual != expected
    {
        return Err(SourceError::Malformed(
            "receipt operation declaration differs from reviewed core",
        ));
    }
    if !matches!(arguments.as_slice(), [GenericArgument::Type(_)])
        || call.environment.is_some()
        || call.arguments.len() != 2
        || ![0, 1].into_iter().all(|ordinal| {
            call.arguments
                .get(&ordinal)
                .is_some_and(|argument| argument.usage == UseKind::Take)
        })
        || call.result.is_none()
    {
        return Err(SourceError::Malformed(
            "receipt arena.seal original call differs",
        ));
    }
    Ok(Some((
        OperationId {
            definition: path.clone(),
            declaration_digest: reviewed.fingerprint,
        },
        ExternalDemand {
            parameters: vec![1],
            inventory: vec![0],
            ..Default::default()
        },
    )))
}
