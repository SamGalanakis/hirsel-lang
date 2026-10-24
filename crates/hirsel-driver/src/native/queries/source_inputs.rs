//! Source inputs for the sole demand instance producer. Live source completion
//! and immutable worker transport share exactly one native import/C path.
use super::declarations::NativeDeclarations;
use hirsel_artifact_store::Reference;
use hirsel_engine::{
    NativeLowerReader, NativeQueryPolicy, NativeRecordContext, NativeRecordSource, QueryEngine,
    QueryError, Verify,
};
use hirsel_span::Anchor;
use hirsel_syntax::{ContentHash, DefPath};
use std::sync::Arc;

pub type SourceObservation = (ContentHash, ContentHash, Option<(Reference, ContentHash)>);
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
        ))
    }
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String>;
}
pub(super) struct LiveNativeSource<'a> {
    pub engine: &'a mut QueryEngine,
    pub declarations: &'a mut NativeDeclarations,
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
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String> {
        self.declarations
            .original_source(self.engine, path)
            .map_err(|e| format!("native source body: {e:?}"))
    }
}

/// Immutable source query products for worker-local instance producers. No
/// Definition/Value collection, engine, mutable controller or alternate codec.
pub struct NativeInstanceInputs {
    pub declarations: NativeDeclarations,
    lower: NativeLowerReader,
    codec: NativeRecordContext,
    policy: ContentHash,
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
            lower,
            codec: NativeRecordContext::capture(engine),
            policy: engine.native_query_policy().source,
        })))
    }
    pub fn provider(self: &Arc<Self>) -> FrozenNativeSource {
        FrozenNativeSource {
            inputs: self.clone(),
            body_demands: 0,
        }
    }
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
        Verify::On
    }
    fn query_policy(&self) -> NativeQueryPolicy {
        NativeQueryPolicy {
            source: self.inputs.policy,
            reuse: true,
        }
    }
    fn source_observation(&self, path: &DefPath) -> Result<SourceObservation, String> {
        let (output, revision, signature) = self
            .inputs
            .lower
            .input(path)
            .ok_or("current source envelope is unavailable to the native worker")?;
        Ok((
            revision,
            signature,
            Some((output.reference(), output.observation_identity())),
        ))
    }
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String> {
        self.body_demands += 1;
        let body = self
            .inputs
            .lower
            .read(path)
            .ok_or("current original Lower refused by worker source reader")?;
        let original = body.definition().clone();
        let captured = hirsel_rc::sealed_source::SourceInput::capture(
            body.output().observation_identity(),
            body.implementation_revision(),
            original.clone(),
        );
        Ok((original, Some(captured)))
    }
}
