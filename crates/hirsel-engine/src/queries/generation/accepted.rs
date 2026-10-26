//! Accepted CT-5 syntax. The private generator output and its admission owner
//! survive every front that consumes the normalized declaration.
use super::*;
use crate::runtime::MemoryReservation;

/// A rejected registration retains the sources its diagnostics cite without
/// publishing generated syntax into the engine's accepted namespace.
pub struct FrontRegistrationFailure {
    pub error: QueryError,
    pub sources: SourceMap,
    _allowance: Arc<MemoryReservation>,
}
impl std::fmt::Debug for FrontRegistrationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrontRegistrationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

pub struct AcceptedFrontRegistration {
    pub(crate) syntax: Arc<ItemSyntax>,
    pub(crate) reads: Arc<Reads>,
    output: FrontRegistrationOutput,
    original: (hirsel_span::Anchor, u32, ContentHash),
    _allowance: Arc<MemoryReservation>,
    retained: u64,
}
impl AcceptedFrontRegistration {
    pub fn definition(&self) -> &DefPath {
        self.output.definition()
    }
    pub fn retained_bytes(&self) -> u64 {
        self.retained
    }
    pub(crate) fn text(&self) -> &str {
        self.output
            .text()
            .expect("accepted successful generator text")
    }
    pub(crate) fn depth(&self) -> &crate::memo::DepthEvidence<Span> {
        self.output.depth()
    }
    pub(crate) fn acceptance(&self) -> Option<&hirsel_validate::ExecutionReceipt> {
        self.output.acceptance()
    }
    pub(crate) fn validate(&self, engine: &mut QueryEngine) -> Result<(), QueryError> {
        let invalid = |reason: String| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "generated syntax validation".into(),
                message: reason,
            })
        };
        let syntax = engine.snapshot(self.definition())?;
        if syntax.anchor != self.original.0
            || syntax.occurrence != self.original.1
            || syntax.text_hash != self.original.2
        {
            return Err(invalid("original declaration changed".into()));
        }
        if let Some(key) = self
            .reads
            .first_changed(|key| engine.observe(self.definition(), key))
        {
            return Err(invalid(format!(
                "generator or normalization read changed: {key:?}"
            )));
        }
        if (engine.verify != Verify::Off && self.acceptance().is_none())
            || !engine.lower_acceptance_holds(self.acceptance())
        {
            return Err(invalid(
                "private generator acceptance no longer holds".into(),
            ));
        }
        Ok(())
    }
}
impl QueryEngine {
    /// Validate a completed opaque generator result, normalize its literal,
    /// and retain it separately from the original declaration. This performs
    /// no body checking, lowering, interpretation or S1 registration round.
    pub fn accept_front_registration(
        &mut self,
        output: FrontRegistrationOutput,
        allowance: MemoryReservation,
    ) -> Result<Arc<AcceptedFrontRegistration>, FrontRegistrationFailure> {
        let refusal = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "generated syntax acceptance".into(),
                message: "generator output is stale, unaccepted, or exceeds its reservation".into(),
            })
        };
        let definition = output.definition().clone();
        let previous_sources = self.sources.clone();
        let source_before = self.sources.retained_payload_bytes();
        let previous = self.generated.get(&definition).cloned();
        let mut allowance = allowance;
        let result = (|| -> Result<Arc<AcceptedFrontRegistration>, QueryError> {
            let text = output.text()?.to_owned();
            let snapshot = self.snapshot(&definition)?;
            let body = generator(&snapshot).ok_or_else(refusal)?;
            if output
                .reads()
                .first_changed(|key| self.observe(&definition, key))
                .is_some()
                || (self.verify != Verify::Off && output.acceptance().is_none())
                || !self.lower_acceptance_holds(output.acceptance())
            {
                return Err(refusal());
            }
            let site = snapshot.tree.expressions[body.0 as usize].origin.span;
            let logical = format!("<generated:{definition}>");
            let file = self
                .sources
                .add_logical(&logical, &logical, text.clone())
                .ok_or_else(refusal)?;
            self.generated
                .insert(definition.clone(), GeneratedSource { file, text, site });
            let mut recording = Recording {
                reads: DependencyReads::default(),
                observed: Observed::inheriting(Some(output.reads())),
            };
            let syntax = self
                .generated_syntax(&definition, &snapshot, body, file, &mut recording)
                .map_err(|error| label(error, self, file, site))?;
            let reads = Arc::new(
                self.typed_reads(&definition, &recording.reads, &recording.observed)
                    .map_err(|_| refusal())?,
            );
            let mut weight = self.front_registration_source_weight(&definition, source_before);
            weight.add(size_of::<AcceptedFrontRegistration>() + 2 * size_of::<usize>());
            weight.add(
                crate::memo::retained::retained_syntax_bytes(&syntax)
                    .try_into()
                    .map_err(|_| refusal())?,
            );
            for (_, bytes) in reads.allocations() {
                weight.add(bytes);
            }
            let retained = weight.bytes();
            // The caller admits an output ceiling before generation. Keep only
            // the measured successful product through source/view clones; the
            // unused part releases when this acceptance call returns.
            let retained_allowance = Arc::new(allowance.split(retained).ok_or_else(refusal)?);
            self.sources = self
                .sources
                .clone()
                .retain_owner(retained_allowance.clone());
            Ok(Arc::new(AcceptedFrontRegistration {
                syntax,
                reads,
                output,
                original: (snapshot.anchor, snapshot.occurrence, snapshot.text_hash),
                _allowance: retained_allowance,
                retained,
            }))
        })();
        match result {
            Ok(accepted) => Ok(accepted),
            Err(mut error) => {
                let allowance = Arc::new(allowance);
                let mut weight = self.front_registration_source_weight(&definition, source_before);
                weight.add(
                    crate::memo::retained::retained_query_error_bytes(&error)
                        .try_into()
                        .unwrap_or(usize::MAX),
                );
                // Diagnostic snapshots are a failed product, not accepted syntax.
                // If their own grant is short, return the admission refusal and
                // discard the new source rather than exposing unowned anchors.
                let sources = if matches!(error, QueryError::Diagnostics(_))
                    && weight.bytes() <= allowance.bytes()
                {
                    self.sources.clone().retain_owner(allowance.clone())
                } else {
                    if matches!(error, QueryError::Diagnostics(_)) {
                        error = refusal();
                    }
                    previous_sources.clone()
                };
                self.sources = previous_sources;
                if let Some(previous) = previous {
                    self.generated.insert(definition, previous);
                } else {
                    self.generated.remove(&definition);
                }
                Err(FrontRegistrationFailure {
                    error,
                    sources,
                    _allowance: allowance,
                })
            }
        }
    }
    fn front_registration_source_weight(
        &self,
        definition: &DefPath,
        previous_payload: u64,
    ) -> crate::NativeMetadataWeight {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<MemoryReservation>() + 2 * size_of::<usize>());
        weight.tree::<DefPath, GeneratedSource>(1);
        weight.path(definition);
        if let Some(source) = self.generated.get(definition) {
            weight.add(source.text.capacity());
        }
        weight.add(
            self.sources
                .retained_payload_bytes()
                .saturating_sub(previous_payload)
                .try_into()
                .unwrap_or(usize::MAX),
        );
        weight.source_snapshot(&self.sources);
        // Adding a generated file/parse anchor COWs the source anchor map. Old
        // front graphs retain the predecessor; count the complete copied tree.
        weight.tree::<hirsel_span::Anchor, (FileId, u32)>(self.sources.anchors().count());
        weight
    }
}
