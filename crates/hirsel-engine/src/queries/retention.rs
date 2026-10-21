//! Bounded scalar TIR ownership. Eviction drops artifacts, not the compact
//! receipts or the Lower graph record that program fronts subscribe to.
use super::*;
use crate::memo::{HoldStats, Retention, retained_lower_entry_bytes};

pub(super) struct LowerReceipt {
    entry: Entry<()>,
    verdict: Option<Vec<hirsel_validate::VRead>>,
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
        if self.verify == Verify::On {
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
                    }),
                );
            }
        }
    }
}
