//! Published definition summaries, retained independently of checked bodies.
use super::*;

impl QueryEngine {
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
                extended.checked = Some(CheckedSummary {
                    summary: checked,
                    growth,
                });
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
        if let Some(summary) = self.image_summary(path)
            && self.seed_definition_summary(summary.clone())
        {
            return Ok(summary);
        }
        let analyzed = self.mode_query(path)?;
        // Only the final mode result contains every planned call. Its complete
        // reads include the checked body's, as well as mode refinements.
        let completed = self.modes.get(&definition).cloned().ok_or_else(|| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "definition summary".into(),
                message: "mode result did not produce a coherent memo".into(),
            })
        })?;
        self.start(path, QueryKind::Summary)?;
        let (summary, growth) = summarize(self.types(), &analyzed.value.typed);
        let reads = completed.memo.reads.clone();
        let checked = self
            .checked
            .get(&definition)
            .and_then(|entry| entry.result().ok())
            .map(|body| {
                let (summary, growth) = summarize(self.types(), &body.value);
                CheckedSummary { summary, growth }
            });
        let published = std::sync::Arc::new(PublishedSummary {
            summary,
            growth,
            checked,
            reads,
            text,
        });
        self.record_summary(published.clone(), completed.detail.clone(), false);
        self.active.remove(&(path.clone(), QueryKind::Summary));
        Ok(published)
    }

    /// Seed an image/store summary only when its complete key and reads still
    /// hold. Inserting through the graph restores reverse subscriptions too.
    pub fn seed_definition_summary(&mut self, summary: std::sync::Arc<PublishedSummary>) -> bool {
        let owner = summary.summary.owner.path();
        if self.key_text(owner, QueryKind::Summary).ok() != Some(summary.text)
            || !self.execution_valid(owner, &summary.reads)
        {
            return false;
        }
        self.record_summary(summary, Default::default(), true);
        true
    }

    fn record_summary(
        &mut self,
        summary: std::sync::Arc<PublishedSummary>,
        detail: DependencyReads,
        adopted: bool,
    ) {
        let definition = summary.summary.owner;
        let durability = summary
            .reads
            .iter()
            .map(|(read, _)| self.durability_of(read))
            .min()
            .unwrap_or(crate::memo::Durability::High);
        let reads = summary.reads.clone();
        let key = crate::memo::MemoKey::new(QueryKind::Summary, definition);
        let text = summary.text;
        let value = Ok(std::sync::Arc::new(QueryOutput {
            value: summary,
            diagnostics: vec![],
            dependencies: detail.signatures.clone(),
        }));
        let memo = if adopted {
            self.graph.adopt_value(key, text, value, reads, durability)
        } else {
            self.graph
                .insert(key, text, value, reads, durability, false)
        };
        self.summaries.insert(
            definition,
            std::sync::Arc::new(crate::queries::Entry {
                memo,
                detail,
                products: Default::default(),
                covered: vec![],
                depth: None,
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
        let path = definition.path();
        if let Some(entry) = self.summaries.get(&definition).cloned()
            && let Ok(text) = self.key_text(path, QueryKind::Summary)
            && self.memo_valid(path, &entry, text)
            && let Ok(output) = entry.result()
            && let Some(checked) = &output.value.checked
        {
            return Ok((checked.summary.clone(), checked.growth.clone()));
        }
        let checked = self.check_query(path)?;
        Ok(summarize(self.types(), &checked.value))
    }

    /// The summary and growth edges of every program definition, from its
    /// checked body; prechecked packages carry their own verdicts.
    pub fn definition_summaries(
        &mut self,
    ) -> Result<Vec<(DefinitionSummary, ParameterGrowth)>, QueryError> {
        let paths = self
            .item_paths()
            .into_iter()
            .filter(|path| !self.imaged(&path.0.package))
            .collect::<Vec<_>>();
        paths
            .into_iter()
            .map(|path| self.checked_definition_summary(DefId::intern(&path)))
            .collect()
    }
}
