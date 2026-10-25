//! Original-source access used by receipt context assembly. The default reads
//! settled front products; alternate providers can simulate unavailable/stale
//! source without substituting candidate native IR or a fabricated V output.
use super::{ReceiptSourceObservation, SourceError};
use hirsel_engine::{NativeOriginalBody, NativeOriginalReader, NativeRecordSource};
use hirsel_layout::instance::InstanceKey;
use hirsel_rc::instance::Actuals;
use hirsel_syntax::{ContentHash, DefId, DefPath};
use hirsel_types::{SignatureRef, TypeInterner, TypeMemo};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Current {
    pub source: ContentHash,
    pub revision: ContentHash,
    pub signature: ContentHash,
}

/// Source observations only. Complete profiles, original target expansion,
/// demand solving and C admission remain separate factory obligations.
pub trait Provider {
    fn types(&self) -> &TypeInterner;
    fn current(&self, path: &DefPath) -> Result<Current, SourceError>;
    fn signature(&self, path: &DefPath) -> Result<SignatureRef, SourceError>;
    fn original(&self, path: &DefPath) -> Result<Arc<NativeOriginalBody>, SourceError>;
}

pub struct Front<'a> {
    reader: &'a NativeOriginalReader,
    types: Arc<TypeInterner>,
}
impl<'a> Front<'a> {
    pub fn new(reader: &'a NativeOriginalReader) -> Self {
        Self {
            reader,
            types: reader.record_context().record_types().clone(),
        }
    }
}
impl Provider for Front<'_> {
    fn types(&self) -> &TypeInterner {
        &self.types
    }
    fn current(&self, path: &DefPath) -> Result<Current, SourceError> {
        let (output, _, revision, signature) = self.reader.input(path).ok_or(
            SourceError::Unsupported("receipt source is not selected by the front"),
        )?;
        let output = output.ok_or(SourceError::Unsupported(
            "receipt source needs current V output",
        ))?;
        Ok(Current {
            source: output.observation_identity(),
            revision,
            signature,
        })
    }
    fn signature(&self, path: &DefPath) -> Result<SignatureRef, SourceError> {
        self.reader
            .signatures()
            .signature(DefId::intern(path))
            .ok_or(SourceError::Unsupported(
                "receipt source signature is unavailable",
            ))?
            .cloned()
            .map_err(|_| SourceError::Unsupported("receipt source signature failed"))
    }
    fn original(&self, path: &DefPath) -> Result<Arc<NativeOriginalBody>, SourceError> {
        self.reader
            .read(path)
            .map(Arc::new)
            .ok_or(SourceError::Unsupported(
                "receipt original source is unavailable",
            ))
    }
}

/// Bind the compact entry/demand observation to the current selected source
/// before its body is released. Native keys and cached metadata cannot select
/// an older original body, signature, or V observation.
pub(super) fn original(
    provider: &dyn Provider,
    path: &DefPath,
) -> Result<(SignatureRef, Arc<NativeOriginalBody>), SourceError> {
    let current = provider.current(path)?;
    let signature = provider.signature(path)?;
    let original = provider.original(path)?;
    let output = original.output().ok_or(SourceError::Unsupported(
        "receipt original source needs V output",
    ))?;
    if original.definition().path.path() != path
        || signature.definition.path() != path
        || signature.fingerprint != current.signature
        || original.definition().signature_hash != current.signature
        || original.implementation_revision() != current.revision
        || output.observation_identity() != current.source
        || output.owner() != original.definition().path
    {
        return Err(SourceError::Malformed(
            "receipt source differs from current front selection",
        ));
    }
    Ok((signature, original))
}

pub fn named(
    provider: &dyn Provider,
    base: &InstanceKey,
    actuals: &Actuals,
    memo: &TypeMemo,
) -> Result<ReceiptSourceObservation, SourceError> {
    let (signature, original) = original(provider, base.definition())?;
    super::named(&original, base, provider.types(), &signature, actuals, memo)
}

pub fn closure(
    provider: &dyn Provider,
    base: &InstanceKey,
    parent: hirsel_tir::BodyId,
    block: hirsel_tir::BlockId,
    statement: u32,
) -> Result<ReceiptSourceObservation, SourceError> {
    let (_, original) = original(provider, base.definition())?;
    super::closure(&original, base, provider.types(), parent, block, statement)
}

/// Original generated-call selection, kept separate from body/source access.
/// Unknown or unselected generated actuals are errors, never ordinary fallback.
pub trait SelectionProvider: Provider {
    fn selected_instance(
        &self,
        declaration: DefId,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefId>, SourceError>;
}
impl SelectionProvider for Front<'_> {
    fn selected_instance(
        &self,
        declaration: DefId,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefId>, SourceError> {
        self.reader
            .instance_bodies()
            .resolve(declaration, self.types(), arguments)
            .map_err(|error| SourceError::Type(format!("{error:?}")))
    }
}
