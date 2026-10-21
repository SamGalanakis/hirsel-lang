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
        let summary = self.definition_summary(definition)?;
        if retention == crate::memo::Retention::Batch {
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
        let typed = self.check_query(path)?;
        // An incoherent check cannot publish a summary with missing reads.
        // It remains a miss and can be retried under a coherent snapshot.
        let checked = self.checked.get(&definition).cloned().ok_or_else(|| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "definition summary".into(),
                message: "checked body did not produce a coherent memo".into(),
            })
        })?;
        self.start(path, QueryKind::Summary)?;
        let (summary, growth) = summarize(self.types(), &typed.value);
        let reads = checked.memo.reads.clone();
        let published = std::sync::Arc::new(PublishedSummary {
            summary,
            growth,
            reads,
            text,
        });
        self.record_summary(published.clone(), checked.detail.clone());
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
        self.record_summary(summary, Default::default());
        true
    }

    fn record_summary(
        &mut self,
        summary: std::sync::Arc<PublishedSummary>,
        detail: DependencyReads,
    ) {
        let definition = summary.summary.owner;
        let durability = summary
            .reads
            .iter()
            .map(|(read, _)| self.durability_of(read))
            .min()
            .unwrap_or(crate::memo::Durability::High);
        let reads = summary.reads.clone();
        let memo = self.graph.insert(
            crate::memo::MemoKey::new(QueryKind::Summary, definition),
            summary.text,
            Ok(std::sync::Arc::new(QueryOutput {
                value: summary,
                diagnostics: vec![],
                dependencies: detail.signatures.clone(),
            })),
            reads,
            durability,
            false,
        );
        self.summaries.insert(
            definition,
            std::sync::Arc::new(crate::queries::Entry {
                memo,
                detail,
                covered: vec![],
                depth: None,
                consistent: true,
            }),
        );
    }

    /// The summary and growth edges of every program definition, from its
    /// checked body; prechecked packages carry their own verdicts.
    pub fn definition_summaries(
        &mut self,
    ) -> Result<Vec<(DefinitionSummary, ParameterGrowth)>, QueryError> {
        let paths = self
            .item_paths()
            .into_iter()
            .filter(|path| !self.prechecked(&path.0.package))
            .collect::<Vec<_>>();
        paths
            .into_iter()
            .map(|path| {
                let published = self.definition_summary(DefId::intern(&path))?;
                Ok((published.summary.clone(), published.growth.clone()))
            })
            .collect()
    }
}
