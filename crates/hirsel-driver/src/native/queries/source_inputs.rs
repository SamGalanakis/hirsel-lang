//! Source inputs for the sole demand instance producer. Live source completion
//! and immutable worker transport share exactly one native import/C path.
use super::declarations::NativeDeclarations;
use hirsel_artifact_store::Reference;
use hirsel_engine::{
    NativeLowerReader, NativeOriginalReader, NativeQueryPolicy, NativeRecordContext,
    NativeRecordSource, QueryEngine, QueryError, Verify,
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
/// Source-only data and the one current original Lower demand. Implementations
/// cannot grant native authority: the demand producer still runs actual C.
pub trait NativeSourceProvider: NativeRecordSource {
    fn declarations(&self) -> &NativeDeclarations;
    fn verification(&self) -> Verify;
    fn query_policy(&self) -> NativeQueryPolicy;
    fn source_observation(&self, path: &DefPath) -> Result<SourceObservation, String> {
        let source = self
            .declarations()
            .sources
            .get(path)
            .ok_or("missing native source output")?;
        Ok((
            source.source.implementation_revision,
            source.signature.fingerprint,
            source
                .lower
                .as_ref()
                .map(|v| (v.reference(), v.observation_identity())),
            None,
        ))
    }
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String>;
}
pub(super) struct LiveNativeSource<'a> {
    pub engine: &'a mut QueryEngine,
    pub declarations: &'a mut NativeDeclarations,
    pub store: Arc<dyn hirsel_engine::store::ResultStore>,
    pub ordinary: std::collections::BTreeMap<DefPath, Reference>,
}
impl NativeRecordSource for LiveNativeSource<'_> {
    fn record_types(&self) -> &Arc<hirsel_types::TypeInterner> {
        self.engine.record_types()
    }
    fn record_owner(&self, anchor: Anchor) -> Option<(DefPath, u32)> {
        self.engine.record_owner(anchor)
    }
    fn record_source_anchor(&self, owner: &DefPath, occurrence: u32) -> Option<Anchor> {
        self.engine.record_source_anchor(owner, occurrence)
    }
}
impl NativeSourceProvider for LiveNativeSource<'_> {
    fn declarations(&self) -> &NativeDeclarations {
        self.declarations
    }
    fn verification(&self) -> Verify {
        self.engine.verify
    }
    fn query_policy(&self) -> NativeQueryPolicy {
        self.engine.native_query_policy()
    }
    fn source_observation(&self, path: &DefPath) -> Result<SourceObservation, String> {
        let source = self
            .declarations
            .sources
            .get(path)
            .ok_or("missing native source output")?;
        Ok((
            source.source.implementation_revision,
            source.signature.fingerprint,
            source
                .lower
                .as_ref()
                .map(|output| (output.reference(), output.observation_identity())),
            self.ordinary.get(path).copied(),
        ))
    }
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String> {
        let original = self
            .declarations
            .original_source(self.engine, path)
            .map_err(|e| format!("native source body: {e:?}"))?;
        if self.declarations.sources[path].lower.is_none() {
            let records =
                hirsel_engine::NativeRecords::new(self.engine.types().clone(), self.store.clone());
            let bytes = records
                .encode(self.engine, &original.0)
                .ok_or("ordinary source encoding failed")?;
            let reference = self
                .store
                .put_content(hirsel_engine::NATIVE_ORIGINAL_BODY, &bytes)
                .ok_or("ordinary source transport failed")?;
            if hirsel_engine::NATIVE_ORIGINAL_BODY.reference(&bytes) != Some(reference) {
                return Err("ordinary source content identity differs".into());
            }
            self.ordinary.insert(path.clone(), reference);
        }
        Ok(original)
    }
}

/// Immutable source query products for worker-local instance producers. No
/// Definition/Value collection, engine, mutable controller or alternate codec.
pub struct NativeInstanceInputs {
    pub declarations: NativeDeclarations,
    lower: OriginalReader,
    codec: NativeRecordContext,
    policy: NativeQueryPolicy,
    verify: Verify,
}
impl NativeInstanceInputs {
    /// Returns None when immutable V-owned transport is unavailable. Source
    /// production itself remains the ordinary source query's responsibility.
    pub fn capture(
        engine: &mut QueryEngine,
        mut declarations: NativeDeclarations,
    ) -> Result<Option<Arc<Self>>, QueryError> {
        if !declarations.program.definitions.is_empty() {
            return Err(invalid(
                "native declaration snapshot contains source bodies",
            ));
        }
        let Some(lower) = engine.native_lower_reader(&declarations.roots.definitions)? else {
            return Ok(None);
        };
        for (path, source) in &mut declarations.sources {
            if let Some((output, revision, signature)) = lower.input(path) {
                if revision != source.source.implementation_revision
                    || signature != source.signature.fingerprint
                {
                    return Err(invalid(
                        "source declaration changed during native input capture",
                    ));
                }
                source.lower = Some(output.clone());
            } else {
                source.lower = None;
            }
        }
        declarations.roots.sources = engine.sources.clone();
        Ok(Some(Arc::new(Self {
            declarations,
            lower: OriginalReader::Verified(lower),
            codec: NativeRecordContext::capture(engine),
            policy: engine.native_query_policy(),
            verify: engine.verify,
        })))
    }
    /// Complete the ordinary source producer where its V envelope is absent.
    /// Selected ordinary bytes are transported without authority; worker V
    /// runs on their miss and ordinary-only snapshots disable native reuse.
    pub fn complete(
        engine: &mut QueryEngine,
        mut declarations: NativeDeclarations,
        store: Arc<dyn hirsel_engine::store::ResultStore>,
    ) -> Result<Arc<Self>, QueryError> {
        if !declarations.program.definitions.is_empty() {
            return Err(invalid(
                "native declaration snapshot contains source bodies",
            ));
        }
        let lower = engine.native_original_reader(&declarations.roots.definitions, store)?;
        for (path, source) in &mut declarations.sources {
            if let Some((output, _, revision, signature)) = lower.input(path) {
                if revision != source.source.implementation_revision
                    || signature != source.signature.fingerprint
                {
                    return Err(invalid(
                        "source declaration changed during native input capture",
                    ));
                }
                source.lower = output.cloned();
            } else {
                source.lower = None;
            }
        }
        declarations.roots.sources = engine.sources.clone();
        let mut policy = engine.native_query_policy();
        policy.reuse &= !lower.has_ordinary();
        Ok(Arc::new(Self {
            declarations,
            lower: OriginalReader::Complete(lower),
            codec: NativeRecordContext::capture(engine),
            policy,
            verify: engine.verify,
        }))
    }
    pub fn provider(self: &Arc<Self>) -> FrozenNativeSource {
        FrozenNativeSource {
            inputs: self.clone(),
            body_demands: 0,
            source_verifications: 0,
        }
    }
}
enum OriginalReader {
    Verified(NativeLowerReader),
    Complete(NativeOriginalReader),
}
fn invalid(message: &str) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "native source snapshot".into(),
        message: message.into(),
    })
}
/// Counters and one demanded original body are worker-local. Constructing a
/// provider clones immutable inputs only and never executes source/native work.
pub struct FrozenNativeSource {
    inputs: Arc<NativeInstanceInputs>,
    pub body_demands: u64,
    pub source_verifications: u64,
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
        self.inputs.verify
    }
    fn query_policy(&self) -> NativeQueryPolicy {
        NativeQueryPolicy {
            source: self.inputs.policy.source,
            reuse: self.inputs.policy.reuse,
        }
    }
    fn source_observation(&self, path: &DefPath) -> Result<SourceObservation, String> {
        let (output, ordinary, revision, signature) = match &self.inputs.lower {
            OriginalReader::Verified(reader) => reader
                .input(path)
                .map(|(output, revision, signature)| (Some(output), None, revision, signature)),
            OriginalReader::Complete(reader) => reader.input(path),
        }
        .ok_or("current source envelope is unavailable to the native worker")?;
        Ok((
            revision,
            signature,
            output.map(|output| (output.reference(), output.observation_identity())),
            ordinary,
        ))
    }
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String> {
        self.body_demands += 1;
        let (original, output, revision, ran) = match &self.inputs.lower {
            OriginalReader::Verified(reader) => {
                let body = reader
                    .read(path)
                    .ok_or("current original Lower refused by worker source reader")?;
                (
                    body.definition().clone(),
                    Some(body.output().clone()),
                    body.implementation_revision(),
                    false,
                )
            }
            OriginalReader::Complete(reader) => {
                let body = reader
                    .read(path)
                    .ok_or("current original Lower refused by worker source reader")?;
                (
                    body.definition().clone(),
                    body.output().cloned(),
                    body.implementation_revision(),
                    body.verification_ran(),
                )
            }
        };
        self.source_verifications += u64::from(ran);
        let captured = output.map(|output| {
            hirsel_rc::sealed_source::SourceInput::capture(
                output.observation_identity(),
                revision,
                original.clone(),
            )
        });
        Ok((original, captured))
    }
}
