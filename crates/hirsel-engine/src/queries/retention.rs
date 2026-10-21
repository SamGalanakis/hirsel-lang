//! Bounded scalar TIR ownership. Eviction drops artifacts, not the compact
//! receipts or the Lower graph record that program fronts subscribe to.
use super::*;
use crate::memo::{HoldStats, Retention, retained_lower_entry_bytes};

pub(super) struct LowerReceipt {
    entry: Entry<()>,
    verdict: Option<Vec<hirsel_validate::VRead>>,
    /// Worker check facts carry their complete independently observed V
    /// universe in Reads. They contain no portable or executable TIR.
    validated_universe: bool,
}
impl std::ops::Deref for LowerReceipt {
    type Target = Entry<()>;
    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

impl Entry<hirsel_tir::DefinitionRef> {
    fn lower_receipt(&self) -> Entry<()> {
        Entry {
            memo: self.memo.map_value(|output| QueryOutput {
                value: (),
                diagnostics: output.diagnostics.clone(),
                dependencies: output.dependencies.clone(),
            }),
            detail: self.detail.clone(),
            products: self.products.clone(),
            covered: self.covered.clone(),
            depth: self.depth.clone(),
            consistent: self.consistent,
        }
    }
}

impl QueryEngine {
    pub(crate) fn revalidate_front_summary(
        &mut self,
        summary: &Arc<PublishedSummary>,
        reads: &Reads,
    ) -> Option<Arc<PublishedSummary>> {
        let id = summary.definition;
        let path = id.path();
        // An absent generated declaration is a miss; revalidation must not
        // materialize it ahead of an earlier input's operational boundary.
        self.items.get(path)?.first()?;
        let text = self.key_text(path, QueryKind::Summary).ok()?;
        if text != summary.text || !self.execution_valid(path, reads) {
            return None;
        }
        let entry = self.summaries.get(&id)?.clone();
        if !self.memo_valid(path, &entry, text) {
            return None;
        }
        Some(entry.result().ok()?.value.clone())
    }

    /// Physical LoweringPhase::lower calls in this engine, including worker
    /// retries, discarded speculative roots and scalar artifact demands.
    /// LoweringPhase::lower_evaluation is a separate operation.
    pub fn lower_adapter_calls(&self, path: &DefPath) -> u64 {
        self.scalar_lower_calls
            .get(&DefId::intern(path))
            .copied()
            .unwrap_or(0)
            + self.s1_lower_work(path).adapter_calls
    }

    pub(crate) fn record_worker_lower_recompute(&mut self, path: &DefPath) {
        *self
            .recomputes
            .entry((path.clone(), QueryKind::Lower))
            .or_default() += 1;
        timings::count(Count::Recomputes);
        self.graph.record_recompute(QueryKind::Lower);
    }

    /// Semantic traversal consumes facts, not TIR. A completed lower receipt
    /// covers its inherited mode/resolver inputs and complete execution reads.
    /// Nested evaluators still demand bodies and replay their request traces.
    pub(super) fn semantic_lower_ready(&mut self, path: &DefPath) -> Result<bool, QueryError> {
        if self.retention != Retention::Batch || !self.s1.is_empty() {
            return Ok(false);
        }
        // Generated analyzed syntax contains the generated body; the indexed
        // declaration still contains `= comptime ...`. Until token evidence is
        // retained independently, use the original analyzed-syntax path.
        if self.generated.contains_key(path)
            || self
                .items
                .get(path)
                .and_then(|items| items.first())
                .is_some_and(|syntax| generation::generator(syntax).is_some())
        {
            return Ok(false);
        }
        let id = DefId::intern(path);
        let receipt = self.lower_receipts.get(&id).cloned().or_else(|| {
            self.lowered.get(&id).map(|entry| {
                Arc::new(LowerReceipt {
                    entry: entry.lower_receipt(),
                    verdict: self.lower_verdicts.get(&id).map(|(_, reads)| reads.clone()),
                    validated_universe: false,
                })
            })
        });
        let Some(receipt) = receipt else {
            return Ok(false);
        };
        let text = self.key_text(path, QueryKind::Lower)?;
        if !receipt.consistent
            || receipt
                .depth
                .as_ref()
                .is_some_and(|depth| depth.limit() != self.depth_limit())
            || !self.memo_valid(path, &receipt, text)
        {
            return Ok(false);
        }
        let Ok(output) = receipt.result() else {
            return Ok(false);
        };
        if self.verify == Verify::On && !receipt.validated_universe {
            let Some(reads) = &receipt.verdict else {
                return Ok(false);
            };
            let snapshot = self.signature_snapshot();
            if !reads
                .iter()
                .all(|read| hirsel_validate::read_holds(read, snapshot.validation(&path.0.package)))
            {
                return Ok(false);
            }
        }
        if has_errors(&output.diagnostics) {
            return Err(QueryError::Diagnostics(output.diagnostics.clone()));
        }
        self.merge_execution(receipt.reads());
        Ok(true)
    }

    /// Retain a completed root's facts without manufacturing an artifact or
    /// charging another phase recomputation. Execution inputs are authoritative
    /// even if the frozen origin predates a later root's signature publication.
    pub(crate) fn retain_lower_facts(&mut self, outcome: &s1::LoweringResult<()>) -> bool {
        let path = &outcome.definition;
        if outcome.reads.is_empty()
            || outcome.limit.is_some()
            // Failed root products do not carry context-free depth evidence.
            // Return them in order; a later request takes the ordinary path.
            || outcome.result.is_err()
            || !self.execution_valid(path, &outcome.reads)
        {
            return false;
        }
        let validated_universe = self.verify == Verify::On && outcome.reads.iter().any(|(key, _)| {
            matches!(key, ReadKey::ValidationUniverse(package) if package.id() == path.0.package)
        });
        if self.verify == Verify::On && !validated_universe {
            return false;
        }
        let Ok(text) = self.key_text(path, QueryKind::Lower) else {
            return false;
        };
        let id = DefId::intern(path);
        let key = MemoKey::new(QueryKind::Lower, id);
        let durability = outcome.reads.durability(|read| self.durability_of(read));
        let mut detail = self
            .summaries
            .get(&id)
            .map(|entry| entry.detail.clone())
            .unwrap_or_default();
        if let Ok(output) = &outcome.result {
            detail.signatures = output.dependencies.clone();
        }
        let memo = self.graph.adopt_value(
            key,
            text,
            outcome.result.clone().map(Arc::new),
            outcome.reads.clone(),
            durability,
        );
        self.lowered.remove(&id);
        self.lower_verdicts.remove(&id);
        self.lower_holds.forget(&key);
        self.lower_receipts.insert(
            id,
            Arc::new(LowerReceipt {
                entry: Entry {
                    memo,
                    detail,
                    products: Default::default(),
                    covered: vec![],
                    depth: outcome.depth.clone(),
                    consistent: true,
                },
                verdict: None,
                validated_universe,
            }),
        );
        true
    }

    /// Retained scalar lowering occupancy, including wrappers and V receipts.
    /// Active caller Arcs are in-flight memory and can outlive cache eviction.
    pub fn lower_hold_stats(&self) -> HoldStats {
        self.lower_holds.stats()
    }

    /// Worker frontiers share this session's allowance with scalar cache holds.
    pub fn available_hold_bytes(&self) -> u64 {
        self.hold_bytes
            .saturating_sub(self.lower_holds.stats().bytes)
    }

    pub(crate) fn set_hold_bytes(&mut self, bytes: u64) {
        self.hold_bytes = bytes;
        let evicted = self.lower_holds.set_budget(bytes);
        self.evict_lowerings(evicted);
    }

    pub(super) fn retain_lowering(&mut self, id: DefId) {
        if self.retention != Retention::Batch {
            return;
        }
        let Some(entry) = self.lowered.get(&id).cloned() else {
            return;
        };
        self.lower_receipts.remove(&id);
        let weight = retained_lower_entry_bytes(
            &entry,
            self.lower_verdicts.get(&id).map(|(_, reads)| reads),
        );
        let evicted = self.lower_holds.hold(entry.memo.key, (), weight);
        self.evict_lowerings(evicted);
    }

    pub(super) fn rebuild_lower_holds(&mut self) {
        self.lower_holds.clear();
        // DefIds are session ordinals: use semantic order when changing policy.
        let mut ids: Vec<_> = self.lowered.keys().copied().collect();
        ids.sort_by(|a, b| a.path().cmp(b.path()));
        for id in ids {
            self.retain_lowering(id);
        }
    }

    fn evict_lowerings(&mut self, keys: Vec<MemoKey>) {
        for key in keys {
            let verdict = self.lower_verdicts.remove(&key.def).map(|(_, reads)| reads);
            if let Some(entry) = self.lowered.remove(&key.def) {
                self.lower_receipts.insert(
                    key.def,
                    Arc::new(LowerReceipt {
                        entry: entry.lower_receipt(),
                        verdict,
                        validated_universe: false,
                    }),
                );
            }
        }
    }
}
