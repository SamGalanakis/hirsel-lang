//! Body-dependent private headers are settled before immutable front snapshots.
//! The public callable rows are unchanged; ordinary signature subscribers carry
//! invalidation when a body's proved bits change.
use super::*;
use hirsel_types::{SignatureKind, UniqueInference};

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

    pub(crate) fn prepare_private_summaries(&mut self) {
        self.note_environment();
        if self.private_summary_active
            || self.private_summary_revision == Some(self.graph.revision())
        {
            return;
        }
        let Some(adapter) = self.adapters.modes.clone() else {
            return;
        };
        if !adapter.infers_private_unique() {
            return;
        }
        self.private_summary_active = true;
        let result = self.compute_private_summaries(adapter);
        // A failed source still gets its ordinary per-definition diagnostics.
        // No failed computation publishes optimistic candidate bits.
        {
            if let Err(error) = &result {
                eprintln!("[DEBUG-private59] inference aborted: {error:?}");
            }
            let inferred = result.unwrap_or_default();
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
                    let mut output = (*output).clone();
                    let mut signature = (*output.value).clone();
                    signature.metadata.inferred_unique = summary.clone();
                    if let Ok(fingerprint) =
                        hirsel_types::encoding::fingerprint(&self.types, &signature, &entry.covered)
                    {
                        signature.fingerprint = fingerprint;
                        output.value = Arc::new(signature);
                        let mut observed = Observed::default();
                        for (key, value) in entry.reads().iter() {
                            if !matches!(key, crate::memo::ReadKey::NameSet(..))
                                && !matches!(key, crate::memo::ReadKey::CitedText(d)
                                if hirsel_types::unique_summary::same_module(d.path(), id.path()))
                            {
                                observed.merged.record(*key, *value);
                            }
                        }
                        observed.merged.record(
                            crate::memo::ReadKey::NameSet(
                                crate::memo::PackageKey::of(&id.path().0.package),
                                crate::memo::ModuleKey::of(&id.path().0.modules),
                            ),
                            ContentHash::of_parts(
                                "hirsel.private-unique.members",
                                self.private_summary_bodies
                                    .get(&id)
                                    .into_iter()
                                    .flatten()
                                    .map(|(p, _)| p.0.name.as_bytes()),
                            ),
                        );
                        for (body, _) in self.private_summary_bodies.get(&id).into_iter().flatten()
                        {
                            observed.merged.record(
                                crate::memo::ReadKey::CitedText(DefId::intern(body)),
                                self.citation_source().cited_text(DefId::intern(body)),
                            );
                        }
                        let mut next = self.record(
                            entry.memo.key,
                            entry.memo.text,
                            Ok(Arc::new(output)),
                            entry.detail.clone(),
                            &observed,
                        );
                        next.covered = entry.covered.clone();
                        self.signatures.insert(id, Arc::new(next));
                        if summary != previous {
                            if id.path().0.name == "epoch_day" {
                                eprintln!(
                                    "[DEBUG-private59] epoch_day {:?}: {:?} -> {:?}",
                                    self.graph.revision(),
                                    previous,
                                    summary
                                );
                            }
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
        self.private_summary_active = false;
        self.private_summary_revision = Some(self.graph.revision());
    }

    fn compute_private_summaries(
        &mut self,
        adapter: Arc<dyn ModesPhase>,
    ) -> Result<hirsel_syntax::DefMap<UniqueInference>, QueryError> {
        let mut bodies = Vec::new();
        for path in self.source_paths() {
            self.materialize(&path)?;
            let syntax = self.snapshot(&path)?;
            if syntax.item.visibility != hirsel_syntax::Visibility::Private
                || syntax
                    .item
                    .attributes
                    .iter()
                    .any(|a| a.name.as_str() == "test")
                || !matches!(&syntax.item.kind, ItemKind::Function { body: Some(_), .. })
            {
                continue;
            }
            let signature = self.signature_query(&path)?;
            if !matches!(&signature.value.kind, SignatureKind::Function(f) if f.foreign.is_none()) {
                continue;
            }
            // Earlier source errors must not make unrelated definitions unavailable.
            if let Ok(checked) = self.check_query(&path) {
                bodies.push((Arc::new(checked.value.clone()), signature.value.clone()));
            }
        }
        self.private_summary_bodies.clear();
        for (_, signature) in &bodies {
            let members = bodies
                .iter()
                .filter(|(_, s)| {
                    hirsel_types::unique_summary::same_module(
                        s.definition.path(),
                        signature.definition.path(),
                    )
                })
                .map(|(body, s)| (s.definition.path().clone(), body.syntax.text_hash))
                .collect();
            self.private_summary_bodies
                .insert(signature.definition, members);
        }
        let (t, memo) = (self.types.clone(), self.memo.clone());
        Ok(adapter
            .infer_unique(&t, &memo, &bodies, self)?
            .into_iter()
            .collect())
    }
}
