//! Published definition summaries, retained independently of checked bodies.
use super::*;

impl QueryEngine {
    /// Demand the settled summary output identity. A provisional computation
    /// has no reusable output identity, even when its current bytes are equal.
    pub fn definition_summary_identity(
        &mut self,
        definition: DefId,
    ) -> Result<Option<ContentHash>, QueryError> {
        let summary = self.definition_summary(definition)?;
        Ok(self
            .graph
            .output(&MemoKey::new(QueryKind::Summary, definition))
            .and_then(|_| {
                summary_archive_identity(
                    self.types.clone(),
                    &summary.summary,
                    &summary.growth,
                    summary.checked.as_deref(),
                )
            }))
    }

    fn summary_is_settled(&self) -> bool {
        self.s1.is_empty() && self.s1.failure_is_own() && self.signature_stack.is_empty()
    }

    /// Extend a completed compact summary only when a program-level storage
    /// obligation demands its body equations. Ordinary compilation retains no
    /// extra CFG for standard-library bodies without such an obligation.
    pub(crate) fn storage_definition_summary(
        &mut self,
        definition: DefId,
    ) -> Result<Arc<PublishedSummary>, QueryError> {
        let summary = self.definition_summary(definition)?;
        if summary.storage.is_some() {
            return Ok(summary);
        }
        let path = definition.path();
        let analyzed = self.mode_query(path)?;
        if has_errors(&analyzed.diagnostics) {
            return Err(QueryError::Diagnostics(analyzed.diagnostics.clone()));
        }
        let signature = self.signature_output(path)?;
        let detail = self
            .summaries
            .get(&definition)
            .map(|entry| entry.detail.clone())
            .unwrap_or_default();
        let adapter = self
            .adapters
            .modes
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let (types, memo) = (self.types.clone(), self.memo.clone());
        let mut recorder = Recorder {
            owner: path.clone(),
            private_shapes: true,
            observed: Observed::inheriting(Some(&summary.reads)),
            engine: self,
            reads: detail,
            groups: BTreeMap::new(),
        };
        let output = adapter.storage(
            &types,
            &memo,
            &analyzed.value,
            &signature.value,
            &mut recorder,
        )?;
        if has_errors(&output.diagnostics) {
            return Err(QueryError::Diagnostics(output.diagnostics));
        }
        recorder.reads.signatures.extend(output.dependencies);
        let detail = recorder.reads;
        let observed = recorder.observed;
        let reads = self.typed_reads(path, &detail, &observed).map_err(|_| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "source storage publication".into(),
                message: "body and declaration observations disagree".into(),
            })
        })?;
        let mut extended = (*summary).clone();
        extended.storage = Some(Arc::new(output.value));
        extended.reads = Arc::new(reads);
        let extended = Arc::new(extended);
        self.record_summary_products(extended.clone(), detail, false, observed.products);
        Ok(extended)
    }
    /// Publish the compact summary before releasing intermediate bodies whose
    /// S1 work has settled. A later body demand uses the ordinary miss path.
    pub fn publish_definition(
        &mut self,
        definition: DefId,
        retention: crate::memo::Retention,
    ) -> Result<std::sync::Arc<PublishedSummary>, QueryError> {
        let mut summary = self.definition_summary(definition)?;
        if retention == crate::memo::Retention::Batch {
            if summary.checked.is_none() {
                let (checked, growth) = self.checked_definition_summary(definition)?;
                let mut extended = (*summary).clone();
                extended.checked = Some(Arc::new(CheckedSummary {
                    summary: checked,
                    growth,
                }));
                let mut reads = ReadsBuilder::new();
                reads.merge(&extended.reads);
                if let Some(entry) = self.checked.get(&definition) {
                    reads.merge(entry.reads());
                }
                extended.reads = Arc::new(reads.finish().map_err(|_| {
                    QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "definition publication".into(),
                        message: "checked and final summary inputs disagree".into(),
                    })
                })?);
                summary = Arc::new(extended);
                let detail = self
                    .summaries
                    .get(&definition)
                    .map(|entry| entry.detail.clone())
                    .unwrap_or_default();
                self.record_summary(summary.clone(), detail, true);
            }
            for kind in [QueryKind::Check, QueryKind::Modes] {
                self.forget(&crate::memo::MemoKey::new(kind, definition));
            }
        }
        Ok(summary)
    }

    /// A definition's compact published summary, independently of its body.
    pub fn definition_summary(
        &mut self,
        definition: DefId,
    ) -> Result<std::sync::Arc<PublishedSummary>, QueryError> {
        let path = definition.path();
        let text = self.key_text(path, QueryKind::Summary)?;
        if let Some(entry) = self.summaries.get(&definition).cloned()
            && self.memo_valid(path, &entry, text)
        {
            return entry.result().map(|output| output.value.clone());
        }
        if let Some(outcome) = self.load_publication(path)
            && let Ok(published) = outcome.result
        {
            return Ok(published.summary);
        }

        self.mode_query(path)?;
        // Only the final mode result contains every planned call. Its complete
        // reads include the checked body's, as well as mode refinements.
        let completed = self.modes.get(&definition).cloned().ok_or_else(|| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "definition summary".into(),
                message: "mode result did not produce a coherent memo".into(),
            })
        })?;
        self.publish_mode_summary(definition, &completed)
    }

    /// Complete retained front diagnostics, without demanding an analyzed body
    /// when the ordinary summary and all of its reads remain current.
    pub(crate) fn definition_diagnostics(
        &mut self,
        path: &DefPath,
    ) -> Result<Vec<hirsel_diag::Diagnostic>, QueryError> {
        let id = DefId::intern(path);
        self.definition_summary(id)?;
        Ok(self
            .summaries
            .get(&id)
            .ok_or(QueryError::NotImplemented)?
            .result()?
            .diagnostics
            .clone())
    }

    /// Publish from the already completed phase result; P does not demand the
    /// body a second time just to retain its compact products.
    pub(super) fn publish_mode_summary(
        &mut self,
        definition: DefId,
        completed: &Arc<Entry<AnalyzedBody>>,
    ) -> Result<Arc<PublishedSummary>, QueryError> {
        let path = definition.path();
        let text = self.key_text(path, QueryKind::Summary)?;
        if let Some(entry) = self.summaries.get(&definition).cloned()
            && self.memo_valid(path, &entry, text)
        {
            return entry.result().map(|o| o.value.clone());
        }
        let analyzed = completed.result()?;
        let products = Arc::new(PhaseProducts {
            mode_diagnostics: analyzed
                .diagnostics
                .iter()
                .filter_map(|diagnostic| {
                    self.diagnostic_phase(diagnostic)
                        .map(|phase| (diagnostic.clone(), phase))
                })
                .collect(),
            ..Default::default()
        });
        self.start(path, QueryKind::Summary)?;
        let (summary, growth) = summarize(self.types(), &analyzed.value.typed);
        let reads = completed.memo.reads.clone();
        let checked = self.retained_checked_summary(definition).or_else(|| {
            self.checked
                .get(&definition)
                .and_then(|entry| entry.result().ok())
                .map(|body| {
                    let (summary, growth) = summarize(self.types(), &body.value);
                    Arc::new(CheckedSummary { summary, growth })
                })
        });
        let published = std::sync::Arc::new(PublishedSummary {
            definition,
            summary,
            growth,
            storage: None,
            checked,
            reads,
            text,
        });
        self.record_summary_products(published.clone(), completed.detail.clone(), false, products);
        self.active.remove(&(path.clone(), QueryKind::Summary));
        Ok(published)
    }

    pub(super) fn publish_front_result(
        &mut self,
        definition: &DefPath,
        result: Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
    ) -> Result<Arc<QueryOutput<AnalyzedBody>>, QueryError> {
        let saved = (self.last_phase, self.unavailable);
        let id = DefId::intern(definition);
        if let Some(checked) = self.checked.get(&id).cloned()
            && checked.consistent
            && !self.graph.is_dirty(&checked.memo.key)
            && self.key_text(definition, QueryKind::CheckedSummary).ok() == Some(checked.memo.text)
            && let Ok(output) = checked.result()
            && !self.checked_summaries.get(&id).is_some_and(|entry| {
                entry.memo.text == checked.memo.text && !self.graph.is_dirty(&entry.memo.key)
            })
        {
            let (summary, growth) = summarize(self.types(), &output.value);
            self.start(definition, QueryKind::CheckedSummary)?;
            self.record_checked_summary(
                id,
                Arc::new(CheckedSummary { summary, growth }),
                checked.memo.text,
                checked.memo.reads.clone(),
                checked.detail.clone(),
                false,
            );
            self.active
                .remove(&(definition.clone(), QueryKind::CheckedSummary));
        }
        let publication = if result.is_ok()
            && let Some(entry) = self.modes.get(&id).cloned()
            && entry.consistent
        {
            self.publish_mode_summary(id, &entry).map(|_| ())
        } else {
            Ok(())
        };
        (self.last_phase, self.unavailable) = saved;
        publication?;
        result
    }

    /// Seed an image/store summary only when its complete key and reads still
    /// hold. Inserting through the graph restores reverse subscriptions too.
    pub fn seed_definition_summary(&mut self, summary: std::sync::Arc<PublishedSummary>) -> bool {
        let owner = summary.definition.path();
        if self.key_text(owner, QueryKind::Summary).ok() != Some(summary.text)
            || !self.execution_valid(owner, &summary.reads)
        {
            return false;
        }
        self.record_summary(summary, Default::default(), true);
        true
    }

    pub(super) fn record_summary(
        &mut self,
        summary: std::sync::Arc<PublishedSummary>,
        detail: DependencyReads,
        adopted: bool,
    ) {
        self.record_summary_products(summary, detail, adopted, Default::default());
    }

    pub(super) fn record_summary_products(
        &mut self,
        summary: Arc<PublishedSummary>,
        detail: DependencyReads,
        adopted: bool,
        products: Arc<PhaseProducts>,
    ) {
        let definition = summary.definition;
        let durability = summary
            .reads
            .iter()
            .map(|(read, _)| self.durability_of(read))
            .min()
            .unwrap_or(crate::memo::Durability::High);
        let reads = summary.reads.clone();
        let key = crate::memo::MemoKey::new(QueryKind::Summary, definition);
        let text = summary.text;
        let output = self
            .summary_is_settled()
            .then(|| crate::summary::SettledSummary::published(&summary, &self.types));
        let value = Ok(std::sync::Arc::new(QueryOutput {
            value: summary,
            diagnostics: vec![],
            dependencies: detail.signatures.clone(),
        }));
        let memo = if adopted {
            self.graph
                .adopt_output(key, text, value, reads, durability, output)
        } else {
            self.graph
                .insert_output(key, text, value, reads, durability, output)
        };
        self.summaries.insert(
            definition,
            std::sync::Arc::new(crate::queries::Entry {
                memo,
                detail,
                products,
                covered: vec![],
                depth: None,
                acceptance: None,
                verification: Default::default(),
                selections: vec![],
                consistent: true,
            }),
        );
    }

    /// GEN-5's input before mode checking and S1 expansion. The final published
    /// summary is produced separately, after mode refinements add planned calls.
    pub fn checked_definition_summary(
        &mut self,
        definition: DefId,
    ) -> Result<(DefinitionSummary, ParameterGrowth), QueryError> {
        if let Some(checked) = self.retained_checked_summary(definition) {
            return Ok((checked.summary.clone(), checked.growth.clone()));
        }
        let path = definition.path();
        // A durable final summary carries its original pre-mode input. Restore
        // it under its own query key rather than allowing later mode metadata
        // to silently replace GEN-5's result.
        if let Some(entry) = self.summaries.get(&definition).cloned()
            && let Ok(text) = self.key_text(path, QueryKind::Summary)
            && self.memo_valid(path, &entry, text)
            && let Ok(output) = entry.result()
            && let Some(checked) = &output.value.checked
        {
            let checked = checked.clone();
            self.record_checked_summary(
                definition,
                checked.clone(),
                text,
                entry.memo.reads.clone(),
                entry.detail.clone(),
                true,
            );
            return Ok((checked.summary.clone(), checked.growth.clone()));
        }
        let checked = self.check_query(path)?;
        let (summary, growth) = summarize(self.types(), &checked.value);
        let product = CheckedSummary { summary, growth };
        if let Some(entry) = self.checked.get(&definition).cloned() {
            let text = self.key_text(path, QueryKind::CheckedSummary)?;
            self.start(path, QueryKind::CheckedSummary)?;
            self.record_checked_summary(
                definition,
                Arc::new(product.clone()),
                text,
                entry.memo.reads.clone(),
                entry.detail.clone(),
                false,
            );
            self.active
                .remove(&(path.clone(), QueryKind::CheckedSummary));
        }
        Ok((product.summary, product.growth))
    }
    fn retained_checked_summary(&mut self, definition: DefId) -> Option<Arc<CheckedSummary>> {
        let entry = self.checked_summaries.get(&definition)?.clone();
        let text = self
            .key_text(definition.path(), QueryKind::CheckedSummary)
            .ok()?;
        self.memo_valid(definition.path(), &entry, text)
            .then(|| entry.result().ok().map(|output| output.value.clone()))
            .flatten()
    }
    pub(super) fn record_checked_summary(
        &mut self,
        definition: DefId,
        summary: Arc<CheckedSummary>,
        text: ContentHash,
        reads: Arc<Reads>,
        detail: DependencyReads,
        adopted: bool,
    ) {
        let key = MemoKey::new(QueryKind::CheckedSummary, definition);
        let durability = reads.durability(|read| self.durability_of(read));
        let output = self
            .summary_is_settled()
            .then(|| crate::summary::SettledSummary::checked(&summary, &self.types));
        let value = Ok(Arc::new(QueryOutput {
            value: summary,
            diagnostics: vec![],
            dependencies: detail.signatures.clone(),
        }));
        let memo = if adopted {
            self.graph
                .adopt_output(key, text, value, reads, durability, output)
        } else {
            self.graph
                .insert_output(key, text, value, reads, durability, output)
        };
        self.checked_summaries.insert(
            definition,
            Arc::new(Entry {
                memo,
                detail,
                products: Default::default(),
                covered: vec![],
                depth: None,
                acceptance: None,
                verification: Default::default(),
                selections: vec![],
                consistent: true,
            }),
        );
    }

    /// The summary and growth edges of every program definition, from its
    /// checked body; prechecked packages carry their own verdicts.
    pub fn definition_summaries(
        &mut self,
    ) -> Result<Vec<(DefinitionSummary, ParameterGrowth)>, QueryError> {
        self.checked_definition_summaries().map(|summaries| {
            summaries
                .into_iter()
                .map(|(_, checked)| (checked.summary, checked.growth))
                .collect()
        })
    }

    /// Checked inputs keyed by the queried definition. Generated bodies may
    /// retain their source declaration's lexical owner in the symbolic summary;
    /// that owner is not the instance's memo identity.
    pub fn checked_definition_summaries(
        &mut self,
    ) -> Result<Vec<(DefId, CheckedSummary)>, QueryError> {
        self.invalidate_front_products();
        let paths = self.item_paths().into_iter().collect::<Vec<_>>();
        paths
            .into_iter()
            .map(|path| {
                self.work.gen5_visits += 1;
                let definition = DefId::intern(&path);
                if let Some(checked) = self.front_products.checked.get(&definition) {
                    return Ok((definition, checked.clone()));
                }
                let (summary, growth) = self.checked_definition_summary(definition)?;
                let checked = CheckedSummary { summary, growth };
                self.front_products
                    .checked
                    .insert(definition, checked.clone());
                Ok((definition, checked))
            })
            .collect()
    }
}

/// Explicit portable identity for native/durable consumers. Resident memo
/// cutoff uses exact typed fields and never calls this encoder.
pub(crate) fn summary_archive_identity(
    types: Arc<hirsel_types::TypeInterner>,
    summary: &DefinitionSummary,
    growth: &ParameterGrowth,
    checked: Option<&CheckedSummary>,
) -> Option<ContentHash> {
    // Use the portable codec's canonical type graph positions. Diagnostic
    // locations remain in the published record and its position witnesses;
    // source text, reads and locations are outside the semantic output.
    let value = crate::SummaryView {
        summary,
        growth,
        checked,
    };
    let bytes = hirsel_product_codec::canonical_bytes(types, &value, |_| None)?;
    Some(ContentHash::of_parts(
        "hirsel.summary.output.v1",
        [bytes.as_slice()],
    ))
}
