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
    pub(crate) lowering_inputs: DefMap<crate::queries::lowering::LoweringUse>,
    verify: Option<Verify>,
    invalidated: DefMap<u64>,
}
/// The body facts the package's dead-error-arm lint consumes, independently
/// of the H body that produced them.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct DeadArmUse {
    pub(crate) errors: std::collections::BTreeSet<DefPath>,
    pub(crate) constructed: std::collections::BTreeSet<(DefPath, u32)>,
}
#[derive(Clone)]
struct LowerProduct {
    text: hirsel_syntax::ContentHash,
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
        // Keep diagnostic facts available if a later edit restores their reads.
        // Every use rechecks their text and pending inputs; Lower publication
        // retires them independently through forget().
        self.selection.remove(&definition);
        self.dead_arms.remove(&definition);
        self.lowering_inputs.remove(&definition);
        self.invalidated.insert(definition, generation);
    }
    pub(crate) fn forget(&mut self, key: &crate::memo::MemoKey) {
        match key.kind {
            QueryKind::Summary => {
                self.outcomes.remove(&key.def);
                self.selection.remove(&key.def);
                self.lowering_inputs.remove(&key.def);
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
        self.invalidate_front_changes(changes)
    }
    pub(crate) fn invalidate_front_definition(
        &mut self,
        definition: DefId,
    ) -> crate::queries::FrontChanges {
        let changes = self.frontier_change(definition);
        self.invalidate_front_changes(changes)
    }
    fn invalidate_front_changes(
        &mut self,
        changes: crate::queries::FrontChanges,
    ) -> crate::queries::FrontChanges {
        if self.front_products.verify != Some(self.verify) {
            self.front_products.lowered.clear();
            self.front_products.verify = Some(self.verify);
        }
        for (definition, generation) in changes.generations() {
            if self.front_products.invalidated.get(&definition) == Some(&generation) {
                continue;
            }
            // S1 can already have settled the phase memo that caused this
            // notification. Revalidate its complete earlier P product before
            // dropping compact lint/lowering inputs with the old generation.
            // Retained Lower facts recheck their own execution/V inputs on use.
            let previous = self.front_products.outcomes.get(&definition).cloned();
            let previous = previous.and_then(|mut outcome| {
                let published = outcome.result.as_mut().ok()?;
                published.summary =
                    self.revalidate_front_summary(&published.summary, &outcome.reads)?;
                Some((
                    outcome,
                    self.front_products.checked.get(&definition).cloned(),
                    self.front_products.dead_arms.get(&definition).cloned(),
                    self.front_products
                        .lowering_inputs
                        .get(&definition)
                        .cloned(),
                ))
            });
            if self.front_products.invalidated.get(&definition) != Some(&generation) {
                self.work.front_invalidations += 1;
            }
            self.front_products.invalidate(definition, generation);
            if let Some((outcome, checked, dead_arms, lowering_inputs)) = previous {
                if let Ok(published) = &outcome.result {
                    self.front_products
                        .selection
                        .insert(definition, published.summary.clone());
                }
                self.front_products.outcomes.insert(definition, outcome);
                if let Some(checked) = checked {
                    self.front_products.checked.insert(definition, checked);
                }
                if let Some(dead_arms) = dead_arms {
                    self.front_products.dead_arms.insert(definition, dead_arms);
                }
                if let Some(lowering_inputs) = lowering_inputs {
                    self.front_products
                        .lowering_inputs
                        .insert(definition, lowering_inputs);
                }
            }
        }
        let removed: Vec<_> = changes.removed.iter().copied().collect();
        self.acknowledge_frontier(&changes, &removed);
        for definition in removed {
            self.front_products.invalidated.remove(&definition);
            self.front_products.lowered.remove(&definition);
        }
        changes
    }

    /// Reuse completed diagnostics without confirming or adopting a Lower
    /// memo. Direct changes still demand Lower; broad V notifications can be
    /// discharged for these facts only when their exact inputs are unchanged.
    fn current_lower_facts(&mut self, definition: DefId) -> Option<LowerProduct> {
        let product = self.front_products.lowered.get(&definition)?.clone();
        let reads = self
            .graph
            .pending_context_reads(&crate::memo::MemoKey::new(QueryKind::Lower, definition))?;
        if self.key_text(definition.path(), QueryKind::Lower).ok()? != product.text
            || reads.into_iter().any(|read| {
                product
                    .reads
                    .get(&read)
                    .is_none_or(|expected| self.observe(definition.path(), &read) != Some(expected))
            })
        {
            return None;
        }
        Some(product)
    }

    /// Compact input of program selection. A hit reads no body memo, but
    /// contributes exactly the same complete closure to an executing caller.
    pub(crate) fn front_summary(
        &mut self,
        definition: DefId,
    ) -> Result<std::sync::Arc<PublishedSummary>, QueryError> {
        self.work.front_summary_visits += 1;
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

    /// Classify source publication barriers without demanding bodies. A static
    /// prefix settles before staged/generic roots can publish declarations;
    /// consecutive dynamic roots still share the compact worker schedule.
    pub(crate) fn static_constant_frontier(&mut self, root: &DefPath) -> bool {
        if self.adapters.phases.is_none() {
            return false;
        }
        self.invalidate_front_products();
        if self.current_lower_facts(DefId::intern(root)).is_some() {
            return true;
        }
        let mut pending = vec![DefId::intern(root)];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(definition) = pending.pop() {
            if !seen.insert(definition) {
                continue;
            }
            let Some(syntax) = self
                .items
                .get(definition.path())
                .and_then(|items| items.first())
                .cloned()
            else {
                return false;
            };
            if crate::queries::generation::serial(&syntax)
                || crate::queries::generation::instance_bodied(&syntax)
            {
                return false;
            }
            // Eligibility is read-only: unavailable or dirty compact inputs
            // cannot run another query before an earlier file prefix settles.
            let Some(published) = self
                .front_products
                .selection
                .get(&definition)
                .cloned()
                .filter(|summary| self.current_front_summary(summary))
            else {
                return false;
            };
            let summary = &published.summary;
            if !summary.staged.is_empty() || summary.frames.iter().any(|frame| frame.generic) {
                return false;
            }
            // Indirect, intrinsic and constructor calls absent from the
            // symbolic static-call list require a dynamic publication barrier.
            if syntax.tree.expressions.iter().any(|expression| {
                matches!(expression.kind, hirsel_syntax::ExprKind::Call { .. })
                    && !summary
                        .calls
                        .iter()
                        .any(|call| call.site == expression.origin.span)
            }) {
                return false;
            }
            for call in &summary.calls {
                if !call.arguments.is_empty() {
                    return false;
                }
                if call.target_frame.is_none() {
                    pending.push(call.callee);
                }
            }
            for (read, _) in published.reads.iter() {
                if let crate::memo::ReadKey::Signature(dependency) = read
                    && self
                        .items
                        .get(dependency.path())
                        .and_then(|items| items.first())
                        .is_some_and(|item| {
                            matches!(item.item.kind, hirsel_syntax::ItemKind::Const { .. })
                        })
                {
                    pending.push(*dependency);
                }
            }
        }
        true
    }

    /// Consume complete dynamic lower facts after GEN-5. Cached roots remain
    /// ordered boundaries: preceding misses settle before their receipts are
    /// rechecked against any newly published declarations.
    pub(crate) fn front_lowerings(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(), QueryError> {
        // Image writers still need closure-local scalar evidence. Fake phase
        // adapters keep the scalar contract without a complete frozen source.
        if self.adapters.phases.is_none() {
            for definition in definitions {
                diagnostics.extend(self.front_lower(&definition)?);
            }
            return Ok(());
        }
        let mut pending = vec![];
        let mut static_prefix = None;
        for definition in definitions {
            self.invalidate_front_products();
            let static_root = self.static_constant_frontier(&definition);
            if static_prefix.is_some_and(|previous| previous != static_root) {
                self.front_lower_frontier(std::mem::take(&mut pending), jobs, diagnostics)?;
            }
            static_prefix = Some(static_root);
            if self
                .current_lower_facts(DefId::intern(&definition))
                .is_some()
            {
                self.front_lower_frontier(std::mem::take(&mut pending), jobs, diagnostics)?;
                self.invalidate_front_products();
                if self
                    .current_lower_facts(DefId::intern(&definition))
                    .is_some()
                {
                    diagnostics.extend(self.front_lower(&definition)?);
                } else {
                    self.front_lower_frontier(vec![definition], jobs, diagnostics)?;
                }
            } else {
                pending.push(definition);
            }
        }
        self.front_lower_frontier(pending, jobs, diagnostics)
    }

    fn front_lower_frontier(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(), QueryError> {
        self.work.front_lower_visits += definitions.len() as u64;
        if definitions.is_empty() {
            return Ok(());
        }
        let outcomes = self.check_lower_definition_prefix(definitions, jobs);
        let changes = self.invalidate_front_products();
        for outcome in outcomes {
            self.checked_scope
                .lowered
                .insert(outcome.definition.clone());
            if outcome.result.is_err() {
                self.checked_scope
                    .failed_lowered
                    .insert(outcome.definition.clone());
            }
            // Keep the delivered failure's reads even when no Lower memo can
            // retain it. Receipt collection never retries that settled root.
            let reads = self
                .checked_scope
                .front
                .entry(outcome.definition.clone())
                .or_default();
            *reads = std::sync::Arc::new(
                Reads::from_vec(
                    reads
                        .iter()
                        .chain(outcome.reads.iter())
                        .map(|(key, value)| (*key, *value))
                        .collect(),
                )
                .map_err(|error| {
                    QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "candidate reads".into(),
                        message: format!(
                            "conflicting complete read values for {}: {error:?}",
                            outcome.definition
                        ),
                    })
                })?,
            );
            if outcome.worker_recomputed {
                // This counts the admitted logical miss, independently of
                // whether its facts survive. Scalar fallbacks already count
                // through start(); discarded speculative attempts are separate.
                self.record_worker_lower_recompute(&outcome.definition);
            }
            let cacheable = self.retain_lower_facts(&outcome);
            self.last_phase = outcome.phase;
            self.unavailable = matches!(outcome.result, Err(QueryError::NotImplemented))
                .then_some(outcome.phase)
                .flatten();
            self.last_comptime_limit = outcome.limit;
            self.merge_execution(&outcome.reads);
            let result = outcome.result.map(|output| output.diagnostics);
            if cacheable {
                let id = DefId::intern(&outcome.definition);
                let text = self.key_text(&outcome.definition, QueryKind::Lower)?;
                self.front_products.lowered.insert(
                    id,
                    LowerProduct {
                        text,
                        result: result.clone(),
                        phase: outcome.phase,
                        reads: outcome.reads,
                    },
                );
                self.acknowledge_frontier(&changes, &[id]);
            }
            diagnostics.extend(result?);
        }
        Ok(())
    }

    /// Program-level lowering diagnostics. Lowered IR stays in the ordinary
    /// Lower memo, whose complete execution/V receipts mark this owner on edits.
    pub(crate) fn front_lower(
        &mut self,
        definition: &DefPath,
    ) -> Result<Vec<Diagnostic>, QueryError> {
        self.checked_scope.lowered.insert(definition.clone());
        self.work.front_lower_visits += 1;
        let changes = self.invalidate_front_products();
        let id = DefId::intern(definition);
        if let Some(product) = self.current_lower_facts(id) {
            self.merge_execution(&product.reads);
            self.last_phase = product.phase;
            self.unavailable = matches!(product.result, Err(QueryError::NotImplemented))
                .then_some(product.phase)
                .flatten();
            if product.result.is_err() {
                self.checked_scope.failed_lowered.insert(definition.clone());
            }
            self.acknowledge_frontier(&changes, &[id]);
            return product.result;
        }
        let result = self
            .lower_to_tir(definition)
            .map(|output| output.diagnostics);
        if result.is_err() {
            self.checked_scope.failed_lowered.insert(definition.clone());
        }
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
            && let Ok(text) = self.key_text(definition, QueryKind::Lower)
        {
            self.front_products.lowered.insert(
                id,
                LowerProduct {
                    text,
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
        self.prepare_private_summaries();
        self.work.front_outcome_visits += definitions.len() as u64;
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
        let refreshed = self.publish_definitions(missing, jobs, self.retention);
        let mut current = DefMap::default();
        let mut acknowledged = vec![];
        for outcome in refreshed {
            let definition = DefId::intern(&outcome.definition);
            if !outcome.reads.is_empty()
                && (outcome
                    .result
                    .as_ref()
                    .is_ok_and(|published| self.current_front_summary(&published.summary))
                    || outcome.phase.is_some_and(|phase| {
                        self.graph
                            .memos_of(definition)
                            .any(|key| key.kind == phase && !self.graph.is_dirty(key))
                    }))
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

    /// S1 body tasks publish the same complete diagnostic/summary product as
    /// P. Keep their refreshed result when its summary is current, so a later
    /// unchanged P request does not demand a deliberately released body.
    pub(crate) fn retain_front_completion(
        &mut self,
        outcome: &PublishedResult,
        changes: &crate::queries::FrontChanges,
    ) {
        let id = DefId::intern(&outcome.definition);
        if let Ok(published) = &outcome.result
            && !outcome.reads.is_empty()
            && self.current_front_summary(&published.summary)
        {
            self.front_products.outcomes.insert(id, outcome.clone());
            self.front_products
                .selection
                .insert(id, published.summary.clone());
            self.acknowledge_frontier(changes, &[id]);
        }
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
