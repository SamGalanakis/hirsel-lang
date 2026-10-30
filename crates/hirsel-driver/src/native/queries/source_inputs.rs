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
use std::sync::{Arc, Mutex};

pub type SourceObservation = (
    ContentHash,
    ContentHash,
    Option<(Reference, ContentHash)>,
    Option<Reference>,
);
pub type OriginalSource = (
    Arc<hirsel_engine::NativeOriginalBody>,
    Option<hirsel_rc::sealed_source::SourceInput>,
);

pub trait NativeSourceProvider: NativeRecordSource {
    fn declarations(&self) -> &NativeDeclarations;
    fn verification(&self) -> Verify;
    /// Current original reader for independently reconstructed receipt graphs.
    fn receipt_provider(&self) -> Option<crate::native::receipt_source::provider::Front<'_>> {
        None
    }
    fn receipt_required(&self) -> bool {
        self.declarations()
            .sources
            .values()
            .filter_map(|source| source.lower.as_ref())
            .any(receipt_required)
    }
    fn query_policy(&self) -> NativeQueryPolicy;
    /// The exact selected V output, including a task-local front Lower binding.
    fn source_output(
        &self,
        path: &DefPath,
    ) -> Result<Option<&hirsel_validate::VerifiedDefinitionOutput>, String>;
    /// Independent original call evidence. A V envelope supplies compact rows;
    /// without one, the ordinary reader rechecks the original TIR. Neither path
    /// reads candidate LIR or manufactures a verifier-owned source token.
    fn source_invocations(
        &mut self,
        path: &DefPath,
        selected: &hirsel_lir::ir::Symbol,
        body: hirsel_tir::BodyId,
    ) -> Result<
        (
            hirsel_rc::invocations::SourceInstance,
            Option<OriginalSource>,
        ),
        String,
    > {
        let (mut evidence, mut original) = if let Some(output) = self.source_output(path)? {
            let evidence = hirsel_rc::invocations::source_instance_from_output(
                &output.invocations(),
                path,
                &self.declarations().program.operations,
                selected,
                body,
            )
            .map_err(str::to_owned)?;
            (evidence, None)
        } else {
            let original = self.original_source(path)?;
            let program = &self.declarations().program;
            let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
            signatures.extend(
                program
                    .signatures
                    .iter()
                    .map(|(path, signature)| (path, (**signature).clone())),
            );
            let evidence = hirsel_rc::invocations::source_instance(
                &program.types,
                original.0.definition(),
                &signatures,
                &program.operations,
                selected,
                body,
            )
            .map_err(str::to_owned)?;
            (evidence, Some(original))
        };
        // A direct CT-5 call executes its registered concrete body. Derive
        // that identity from the original call's closed arguments and the
        // captured instance table, before C observes any candidate symbol.
        let instances = &self.declarations().program.instance_bodies;
        if evidence.invocations.iter().any(|row| {
            row.callee.as_ref().is_some_and(|p| {
                instances
                    .declarations
                    .contains(&hirsel_syntax::DefId::intern(p))
            })
        }) {
            if original.is_none() {
                original = Some(self.original_source(path)?);
            }
            let definition = original.as_ref().unwrap().0.definition();
            let program = &self.declarations().program;
            for row in &mut evidence.invocations {
                let Some(callee) = &row.callee else {
                    continue;
                };
                let callee = hirsel_syntax::DefId::intern(callee);
                if !program.instance_bodies.declarations.contains(&callee) {
                    continue;
                }
                let call = definition
                    .bodies
                    .get(row.body as usize)
                    .and_then(|body| body.blocks.get(row.block as usize))
                    .ok_or("generated invocation is outside original source")?;
                let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, arguments },
                    ..
                } = &call.terminator
                else {
                    return Err("generated invocation has no original direct call".into());
                };
                if path != callee.path() {
                    return Err("generated invocation differs from original call".into());
                }
                let selected = program
                    .instance_bodies
                    .resolve(callee, &program.types, arguments)
                    .map_err(|error| format!("generated invocation selection: {error:?}"))?
                    .ok_or("generated invocation has no selected body")?;
                row.callee = Some(selected.path().clone());
            }
        }
        Ok((evidence, original))
    }
    fn source_observation(&self, path: &DefPath) -> Result<SourceObservation, String>;
    fn original_source(&mut self, path: &DefPath) -> Result<OriginalSource, String>;
}

fn receipt_required(output: &hirsel_validate::VerifiedDefinitionOutput) -> bool {
    output.invocations().calls().iter().any(|call| {
        call.callee
            .as_ref()
            .is_some_and(|path| path.is_core("arena.seal") || path.is_core("region.with"))
    })
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
    metadata_bytes: u64,
    metadata_owner: Mutex<Option<Arc<hirsel_engine::runtime::MemoryReservation>>>,
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
            let selected = lower.input(path).and_then(|(output, _, _, _)| output);
            if source.lower.as_ref() != selected {
                Arc::make_mut(source).lower = selected.cloned();
            }
        }
        let codec = lower.record_context();
        let metadata_bytes = declarations
            .retained_metadata_bytes()
            .map_err(invalid)?
            .saturating_add(lower.retained_metadata_bytes())
            .saturating_add(size_of::<Self>() as u64)
            .saturating_add((2 * size_of::<usize>()) as u64);
        Ok(Arc::new(Self {
            declarations,
            lower,
            codec,
            metadata_bytes,
            metadata_owner: Mutex::new(None),
        }))
    }
    /// Copied bodyless metadata only; shared front allocations retain their
    /// original owners. The constructor measures this before publishing the Arc.
    pub fn retained_metadata_bytes(&self) -> u64 {
        self.metadata_bytes
    }
    /// Transfer the copied metadata's one charge out of the supplied native
    /// allowance. The last input owner releases it, independently of task life.
    pub(super) fn retain_metadata(
        &self,
        allowance: &mut hirsel_engine::runtime::MemoryReservation,
    ) -> Result<(), String> {
        let mut owner = self
            .metadata_owner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if owner.is_none() {
            *owner =
                Some(Arc::new(allowance.split(self.metadata_bytes).ok_or(
                    "native source metadata exceeds retained allowance",
                )?));
        }
        Ok(())
    }
    pub(super) fn metadata_allowance(
        &self,
    ) -> Option<Arc<hirsel_engine::runtime::MemoryReservation>> {
        self.metadata_owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
    ) -> Result<FrozenNativeSource, String> {
        let reader = context
            .reader(products)
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
    fn receipt_provider(&self) -> Option<crate::native::receipt_source::provider::Front<'_>> {
        Some(crate::native::receipt_source::provider::Front::new(
            self.reader(),
        ))
    }
    fn verification(&self) -> Verify {
        self.reader().verification()
    }
    fn receipt_required(&self) -> bool {
        self.inputs
            .declarations
            .sources
            .iter()
            .any(|(path, source)| {
                self.reader()
                    .input(path)
                    .and_then(|(output, _, _, _)| output)
                    .or(source.lower.as_ref())
                    .is_some_and(receipt_required)
            })
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
        Ok((Arc::new(body), source))
    }
}
