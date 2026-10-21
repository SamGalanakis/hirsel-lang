//! Compact program products. Pending generation-stamped notifications retire
//! products before reuse, even when another query already settled its memo.
//! The aggregate owns values, not body memos; ordinary per-definition demand
//! still uses the graph's normal verification and miss path.
use super::*;
use hirsel_syntax::{DefId, DefMap};

#[derive(Default)]
pub(crate) struct FrontProducts {
    outcomes: DefMap<PublishedResult>,
    pub(crate) checked: DefMap<CheckedSummary>,
    lowered: DefMap<LowerProduct>,
    selection: DefMap<std::sync::Arc<PublishedSummary>>,
    pub(crate) dead_arms: DefMap<DeadArmUse>,
    verify: Option<Verify>,
    invalidated: DefMap<u64>,
}
/// The body facts the package's dead-error-arm lint consumes, independently
/// of the H body that produced them.
#[derive(Clone, Default)]
pub(crate) struct DeadArmUse {
    pub(crate) errors: std::collections::BTreeSet<DefPath>,
    pub(crate) constructed: std::collections::BTreeSet<(DefPath, u32)>,
}
#[derive(Clone)]
struct LowerProduct {
    result: Result<Vec<Diagnostic>, QueryError>,
    phase: Option<QueryKind>,
    reads: std::sync::Arc<Reads>,
}
impl FrontProducts {
    fn invalidate(&mut self, definition: DefId, generation: u64) {
        if self.invalidated.get(&definition) == Some(&generation) {
            return;
        }
        self.outcomes.remove(&definition);
        self.checked.remove(&definition);
        self.lowered.remove(&definition);
        self.selection.remove(&definition);
        self.dead_arms.remove(&definition);
        self.invalidated.insert(definition, generation);
    }
    pub(crate) fn forget(&mut self, key: &crate::memo::MemoKey) {
        match key.kind {
            QueryKind::Summary => {
                self.outcomes.remove(&key.def);
                self.selection.remove(&key.def);
            }
            QueryKind::CheckedSummary => {
                self.checked.remove(&key.def);
                self.dead_arms.remove(&key.def);
            }
            QueryKind::Lower => {
                self.lowered.remove(&key.def);
            }
            _ => {
                if self.outcomes.get(&key.def).is_some_and(|outcome| {
                    outcome.result.is_err() && outcome.phase == Some(key.kind)
                }) {
                    self.outcomes.remove(&key.def);
                }
            }
        }
    }
}
impl QueryEngine {
    pub(crate) fn invalidate_front_products(&mut self) -> crate::queries::FrontChanges {
        let changes = self.frontier_changes();
        if self.front_products.verify != Some(self.verify) {
            self.front_products.lowered.clear();
            self.front_products.verify = Some(self.verify);
        }
        for (definition, generation) in changes.generations() {
            self.front_products.invalidate(definition, generation);
        }
        let removed: Vec<_> = changes.removed.iter().copied().collect();
        self.acknowledge_frontier(&changes, &removed);
        for definition in removed {
            self.front_products.invalidated.remove(&definition);
        }
        changes
    }

    /// Compact input of program selection. A hit reads no body memo, but
    /// contributes exactly the same complete closure to an executing caller.
    pub(crate) fn front_summary(
        &mut self,
        definition: DefId,
    ) -> Result<std::sync::Arc<PublishedSummary>, QueryError> {
        self.invalidate_front_products();
        let summary = match self.front_products.selection.get(&definition) {
            Some(summary) => summary.clone(),
            None => {
                let summary = self.definition_summary(definition)?;
                if self.current_front_summary(&summary) {
                    self.front_products
                        .selection
                        .insert(definition, summary.clone());
                }
                summary
            }
        };
        self.merge_execution(&summary.reads);
        Ok(summary)
    }

    fn current_front_summary(&self, summary: &std::sync::Arc<PublishedSummary>) -> bool {
        !summary.reads.is_empty()
            && self
                .summaries
                .get(&summary.definition)
                .is_some_and(|entry| {
                    entry.consistent
                        && !self.graph.is_dirty(&entry.memo.key)
                        && entry
                            .result()
                            .is_ok_and(|output| std::sync::Arc::ptr_eq(&output.value, summary))
                })
    }

    /// Program-level lowering diagnostics. Lowered IR stays in the ordinary
    /// Lower memo, whose complete execution/V receipts mark this owner on edits.
    pub(crate) fn front_lower(
        &mut self,
        definition: &DefPath,
    ) -> Result<Vec<Diagnostic>, QueryError> {
        let changes = self.invalidate_front_products();
        let id = DefId::intern(definition);
        if let Some(product) = self.front_products.lowered.get(&id).cloned() {
            self.merge_execution(&product.reads);
            self.last_phase = product.phase;
            self.unavailable = matches!(product.result, Err(QueryError::NotImplemented))
                .then_some(product.phase)
                .flatten();
            return product.result;
        }
        let result = self
            .lower_to_tir(definition)
            .map(|output| output.diagnostics);
        let phase = self.unavailable.or(self.last_phase);
        if let Some(reads) = self.shared_memo_reads(definition, QueryKind::Lower)
            && !reads.is_empty()
            && self
                .graph
                .memos_of(id)
                .any(|key| key.kind == QueryKind::Lower)
            && !self
                .graph
                .is_dirty(&crate::memo::MemoKey::new(QueryKind::Lower, id))
            && !result.as_ref().is_err_and(|error| !error.cacheable())
        {
            self.front_products.lowered.insert(
                id,
                LowerProduct {
                    result: result.clone(),
                    phase,
                    reads,
                },
            );
            // P may already have acknowledged this generation. A lower-only
            // subscriber still needs its own refreshed result before settling.
            self.acknowledge_frontier(&changes, &[id]);
        }
        result
    }

    pub(crate) fn front_definitions(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<PublishedResult> {
        let changes = self.invalidate_front_products();
        // Preserve caller order, duplicate occurrences and generator boundaries.
        // Every miss is evaluated by the same fused P implementation.
        let missing = definitions
            .iter()
            .filter(|path| {
                !self
                    .front_products
                    .outcomes
                    .contains_key(&DefId::intern(path))
            })
            .cloned()
            .collect();
        let refreshed = self.publish_definitions(missing, jobs, crate::memo::Retention::Resident);
        let mut current = DefMap::default();
        let mut acknowledged = vec![];
        for outcome in refreshed {
            let definition = DefId::intern(&outcome.definition);
            if !outcome.reads.is_empty()
                && outcome.phase.is_some_and(|phase| {
                    self.graph
                        .memos_of(definition)
                        .any(|key| key.kind == phase && !self.graph.is_dirty(key))
                })
                && !outcome
                    .result
                    .as_ref()
                    .is_err_and(|error| !error.cacheable())
            {
                self.front_products
                    .outcomes
                    .insert(definition, outcome.clone());
                acknowledged.push(definition);
            }
            if let Ok(published) = &outcome.result
                && self.current_front_summary(&published.summary)
            {
                self.front_products
                    .selection
                    .insert(definition, published.summary.clone());
            }
            current.insert(definition, outcome);
        }
        self.acknowledge_frontier(&changes, &acknowledged);
        let outputs: Vec<_> = definitions
            .into_iter()
            .map(|path| {
                let definition = DefId::intern(&path);
                current
                    .get(&definition)
                    .or_else(|| self.front_products.outcomes.get(&definition))
                    .expect("one compact outcome per requested definition")
                    .clone()
            })
            .collect();
        if let Some(last) = outputs.last() {
            self.last_phase = last.phase;
            self.unavailable = matches!(last.result, Err(QueryError::NotImplemented))
                .then_some(last.phase)
                .flatten();
        }
        outputs
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_generation_invalidates_once_without_consuming_another_products_work() {
        let definition = DefId::intern(&entry_path(&default_context()));
        let mut products = FrontProducts::default();
        products.dead_arms.insert(definition, DeadArmUse::default());
        products.invalidate(definition, 1);
        assert!(!products.dead_arms.contains_key(&definition));
        // A summary or lint refresh must survive later observations of the
        // same pending generation while P/S1 has not acknowledged its work.
        products.dead_arms.insert(definition, DeadArmUse::default());
        products.invalidate(definition, 1);
        assert!(products.dead_arms.contains_key(&definition));
        products.invalidate(definition, 2);
        assert!(!products.dead_arms.contains_key(&definition));
    }
    #[test]
    fn retiring_the_receipt_owner_retires_its_compact_failure() {
        let path = entry_path(&default_context());
        let id = DefId::intern(&path);
        let failure = PublishedResult {
            definition: path,
            result: Err(QueryError::Diagnostics(vec![])),
            phase: Some(QueryKind::Modes),
            diagnostic_phases: vec![],
            reads: std::sync::Arc::new(Reads::empty()),
        };
        let mut products = FrontProducts::default();
        products.outcomes.insert(id, failure);
        products.forget(&crate::memo::MemoKey::new(QueryKind::Check, id));
        assert!(products.outcomes.contains_key(&id));
        products.forget(&crate::memo::MemoKey::new(QueryKind::Modes, id));
        assert!(!products.outcomes.contains_key(&id));
    }
}
