//! Immutable settled Lower inputs for worker-local native instance misses.
//! Source V and native C retain separate authority; transport creates neither.
use super::declarations::NativeDeclarations;
use hirsel_engine::store::Reference;
use hirsel_engine::{
    NativeOriginalReader, NativeQueryPolicy, NativeRecordContext, NativeRecordSource, QueryError,
    Verify,
};
use hirsel_span::Anchor;
use hirsel_syntax::{ContentHash, DefPath};
use std::sync::Arc;

pub type SourceObservation = (
    ContentHash,
    ContentHash,
    Option<(Reference, ContentHash)>,
    Option<Reference>,
);
pub type OriginalSource = (
    hirsel_tir::DefinitionRef,
    Option<hirsel_rc::sealed_source::SourceInput>,
);

pub trait NativeSourceProvider: NativeRecordSource {
    fn declarations(&self) -> &NativeDeclarations;
    fn verification(&self) -> Verify;
    fn query_policy(&self) -> NativeQueryPolicy;
    /// The exact selected V output, including a task-local front Lower binding.
    fn source_output(
        &self,
        path: &DefPath,
    ) -> Result<Option<&hirsel_validate::VerifiedDefinitionOutput>, String>;
    fn source_observation(&self, path: &DefPath) -> Result<SourceObservation, String>;
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String>;
}

/// The front owner resolves its settled descriptor without decoding a body.
pub trait NativeLowerProducts: Send + Sync {
    fn lower_product(
        &self,
        definition: &DefPath,
        selected: hirsel_engine::runtime::ProductRef,
    ) -> Result<hirsel_engine::NativeOriginalProduct, String>;
}

pub struct NativeFrontLower {
    pub context: Arc<hirsel_engine::NativeOriginalContext>,
    pub bindings: std::collections::BTreeMap<DefPath, hirsel_engine::runtime::QueryId>,
    pub products: Arc<dyn NativeLowerProducts>,
}

/// The declaration snapshot and independent reader retain no Definition or Value.
pub struct NativeInstanceInputs {
    pub declarations: NativeDeclarations,
    lower: NativeOriginalReader,
    codec: Arc<NativeRecordContext>,
}
fn check_binding(
    declarations: &NativeDeclarations,
    reader: &NativeOriginalReader,
) -> Result<(), String> {
    if !declarations.program.definitions.is_empty()
        || !Arc::ptr_eq(
            &declarations.program.types,
            reader.record_context().record_types(),
        )
    {
        return Err("settled native source has bodies or another interner".into());
    }
    for (path, source) in &declarations.sources {
        if let Some((_, _, revision, signature)) = reader.input(path)
            && (revision != source.source.implementation_revision
                || signature != source.signature.fingerprint)
        {
            return Err("settled native source differs from its declaration".into());
        }
    }
    Ok(())
}
impl NativeInstanceInputs {
    pub fn from_products(
        mut declarations: NativeDeclarations,
        lower: NativeOriginalReader,
    ) -> Result<Arc<Self>, QueryError> {
        check_binding(&declarations, &lower).map_err(invalid)?;
        for (path, source) in &mut declarations.sources {
            source.lower = lower
                .input(path)
                .and_then(|(output, _, _, _)| output.cloned());
        }
        let codec = lower.record_context();
        Ok(Arc::new(Self {
            declarations,
            lower,
            codec,
        }))
    }
    pub fn provider(self: &Arc<Self>) -> FrozenNativeSource {
        FrozenNativeSource {
            inputs: self.clone(),
            selected: None,
            body_demands: 0,
            source_verifications: 0,
        }
    }
    /// Bind only the products read by this task's explicit front dependencies.
    pub fn provider_from_products(
        self: &Arc<Self>,
        context: &hirsel_engine::NativeOriginalContext,
        products: impl IntoIterator<Item = hirsel_engine::NativeOriginalProduct>,
        store: Arc<dyn hirsel_engine::store::ResultStore>,
    ) -> Result<FrozenNativeSource, String> {
        let reader = context
            .reader(products, store)
            .map_err(|e| format!("settled source: {e:?}"))?;
        check_binding(&self.declarations, &reader)?;
        Ok(FrozenNativeSource {
            inputs: self.clone(),
            selected: Some(reader),
            body_demands: 0,
            source_verifications: 0,
        })
    }
}
fn invalid(message: String) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "native source snapshot".into(),
        message,
    })
}

/// Counters and a selected reader are task-local; construction performs no work.
pub struct FrozenNativeSource {
    inputs: Arc<NativeInstanceInputs>,
    selected: Option<NativeOriginalReader>,
    pub body_demands: u64,
    pub source_verifications: u64,
}
impl FrozenNativeSource {
    fn reader(&self) -> &NativeOriginalReader {
        self.selected.as_ref().unwrap_or(&self.inputs.lower)
    }
}
impl NativeRecordSource for FrozenNativeSource {
    fn record_types(&self) -> &Arc<hirsel_types::TypeInterner> {
        self.inputs.codec.record_types()
    }
    fn record_owner(&self, anchor: Anchor) -> Option<(DefPath, u32)> {
        self.inputs.codec.record_owner(anchor)
    }
    fn record_source_anchor(&self, owner: &DefPath, occurrence: u32) -> Option<Anchor> {
        self.inputs.codec.record_source_anchor(owner, occurrence)
    }
}
impl NativeSourceProvider for FrozenNativeSource {
    fn declarations(&self) -> &NativeDeclarations {
        &self.inputs.declarations
    }
    fn verification(&self) -> Verify {
        self.reader().verification()
    }
    fn query_policy(&self) -> NativeQueryPolicy {
        self.reader().query_policy()
    }
    fn source_output(
        &self,
        path: &DefPath,
    ) -> Result<Option<&hirsel_validate::VerifiedDefinitionOutput>, String> {
        self.reader()
            .input(path)
            .map(|(output, _, _, _)| output)
            .ok_or_else(|| "current source output is unavailable to the native worker".into())
    }
    fn source_observation(&self, path: &DefPath) -> Result<SourceObservation, String> {
        let (output, ordinary, revision, signature) = self
            .reader()
            .input(path)
            .ok_or("current source output is unavailable to the native worker")?;
        Ok((
            revision,
            signature,
            output.map(|output| (output.reference(), output.observation_identity())),
            ordinary,
        ))
    }
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String> {
        self.body_demands += 1;
        let body = self
            .reader()
            .read(path)
            .ok_or("current original Lower refused by worker source reader")?;
        self.source_verifications += u64::from(body.verification_ran());
        let original = body.definition().clone();
        let source = body.output().map(|output| {
            hirsel_rc::sealed_source::SourceInput::capture(
                output.observation_identity(),
                body.implementation_revision(),
                original.clone(),
            )
        });
        Ok((original, source))
    }
}
