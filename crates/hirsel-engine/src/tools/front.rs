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
    verify: Option<Verify>,
}
#[derive(Clone)]
struct LowerProduct {
    result: Result<Vec<Diagnostic>, QueryError>,
    phase: Option<QueryKind>,
    reads: std::sync::Arc<Reads>,
}
impl FrontProducts {
    pub(crate) fn forget(&mut self, key: &crate::memo::MemoKey) {
        match key.kind {
            QueryKind::Summary => {
                self.outcomes.remove(&key.def);
            }
            QueryKind::CheckedSummary => {
                self.checked.remove(&key.def);
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
        for definition in changes
            .changed
            .iter()
            .chain(&changes.affected)
            .chain(&changes.removed)
        {
            self.front_products.outcomes.remove(definition);
            self.front_products.checked.remove(definition);
            self.front_products.lowered.remove(definition);
        }
        let removed: Vec<_> = changes.removed.iter().copied().collect();
        self.acknowledge_frontier(&changes, &removed);
        changes
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
