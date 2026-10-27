//! Body-dependent private headers are settled before immutable front snapshots.
//! The public callable rows are unchanged; ordinary signature subscribers carry
//! invalidation when a body's proved bits change.
use super::*;
use hirsel_types::{SignatureKind, UniqueInference};
mod scheduled;

impl QueryEngine {
    pub(super) fn attach_private_summary(
        &self,
        signature: &mut hirsel_types::Signature,
        dependencies: &[SignatureDependency],
    ) {
        if let Some(summary) = self.private_summaries.get(&signature.definition) {
            signature.metadata.inferred_unique = summary.clone();
            if let Ok(fingerprint) =
                hirsel_types::encoding::fingerprint(&self.types, signature, dependencies)
            {
                signature.fingerprint = fingerprint;
            }
        }
    }

    pub(super) fn observe_private_summary(
        &self,
        signature: &hirsel_types::Signature,
        observed: &mut Observed,
    ) {
        let id = signature.definition;
        if self.private_summaries.contains_key(&id)
            || signature.metadata.visibility == hirsel_syntax::Visibility::Private
                && matches!(&signature.kind, SignatureKind::Function(f) if f.foreign.is_none())
                && self
                    .source_facts(id.path())
                    .is_some_and(|facts| facts.body && !facts.constant)
        {
            // A declaration normalized without a Modes adapter still observes
            // its conservative proof. It cannot be a wildcard cache variant
            // after inference publishes a stronger result for the same header.
            let summary = self.private_summaries.get(&id).cloned().unwrap_or_default();
            observed.merged.record(
                crate::memo::ReadKey::PrivateSummary(id),
                memos::private_summary_value(&summary),
            );
        }
    }

    pub(crate) fn prepare_private_summaries(&mut self) {
        self.note_environment();
        if !self.private_summary_active
            && self.private_summary_revision != Some(self.graph.revision())
        {
            self.standalone_private_inference();
        }
    }

    pub(crate) fn publish_private_summaries(
        &mut self,
        inferred: hirsel_syntax::DefMap<UniqueInference>,
    ) -> Result<(), QueryError> {
        // A failed source still gets its ordinary per-definition diagnostics.
        // No failed computation publishes optimistic candidate bits.
        {
            self.private_unique_failures = inferred
                .iter()
                .map(|(id, proof)| (*id, proof.failures.clone()))
                .collect();
            let summaries = inferred
                .into_iter()
                .map(|(id, proof)| (id, proof.summary))
                .collect();
            let old = std::mem::replace(&mut self.private_summaries, summaries);
            let mut changed = Vec::new();
            let ids: BTreeSet<_> = old
                .keys()
                .chain(self.private_summaries.keys())
                .copied()
                .collect();
            for id in ids {
                let summary = self.private_summaries.get(&id).cloned().unwrap_or_default();
                let previous = old.get(&id).cloned().unwrap_or_default();
                if let Some(entry) = self.signatures.get(&id).cloned()
                    && let Ok(output) = entry.result()
                {
                    if summary == previous
                        && output.value.metadata.inferred_unique == summary
                        && self.memo_valid(id.path(), &entry, entry.memo.text)
                    {
                        continue;
                    }
                    if entry.products.signature_persisted {
                        // A restored header can omit syntax-backed auxiliary
                        // products. Restore or recompute the complete product
                        // under the new proof instead of rewriting that projection.
                        self.signatures.remove(&id);
                        self.private_signature_publications.insert(id);
                        changed.push(id.path().clone());
                        continue;
                    }
                    let mut output = (*output).clone();
                    let mut signature = (*output.value).clone();
                    signature.metadata.inferred_unique = summary.clone();
                    if let Ok(fingerprint) =
                        hirsel_types::encoding::fingerprint(&self.types, &signature, &entry.covered)
                    {
                        signature.fingerprint = fingerprint;
                        let signature_changed = output.value.fingerprint != fingerprint;
                        output.value = Arc::new(signature);
                        let mut observed = Observed::default();
                        observed.products = entry.products.clone();
                        for (key, value) in entry.reads().iter() {
                            if !matches!(key, crate::memo::ReadKey::PrivateSummary(d) if *d == id) {
                                observed.merged.record(*key, *value);
                            }
                        }
                        self.observe_private_summary(&output.value, &mut observed);
                        let mut next = self.record(
                            entry.memo.key,
                            entry.memo.text,
                            Ok(Arc::new(output)),
                            entry.detail.clone(),
                            &observed,
                        );
                        next.covered = entry.covered.clone();
                        self.private_signature_publications.insert(id);
                        self.signatures.insert(id, Arc::new(next));
                        if signature_changed || summary != previous {
                            changed.push(id.path().clone());
                        }
                    }
                }
            }
            if !changed.is_empty() {
                self.snapshot_inputs_revision += 1;
                self.note_signatures(&changed);
            }
        }
        Ok(())
    }
}
