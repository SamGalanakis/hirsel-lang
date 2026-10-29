use super::*;
use crate::queries::private_unique::scheduled::Facts;
use std::sync::{Mutex, atomic::AtomicU64};
type Module = (crate::memo::PackageKey, crate::memo::ModuleKey);
pub(super) struct ModuleProduct {
    facts: Facts,
    complete: bool,
    work: BTreeMap<(DefId, QueryKind), usize>,
    pub(super) signatures: hirsel_syntax::DefMap<Arc<Entry<hirsel_types::SignatureRef>>>,
    accounted: AtomicBool,
    _standalone: Option<Arc<crate::runtime::MemoryReservation>>,
}
pub(super) struct PrivateProducts {
    candidates: hirsel_syntax::DefMap<Module>,
    modules: BTreeMap<Module, OnceLock<Result<ModuleProduct, QueryError>>>,
    working: AtomicU64,
    registrations: OnceLock<Vec<Arc<AcceptedFrontRegistration>>>,
    retention: OnceLock<Arc<Mutex<u64>>>,
}
impl PrivateProducts {
    pub(super) fn capture(engine: &QueryEngine) -> Option<Self> {
        if engine.private_summary_active
            || engine.private_summary_revision == Some(engine.graph.revision())
            || engine
                .adapters
                .modes
                .as_ref()
                .is_none_or(|m| !m.infers_private_unique())
        {
            return None;
        }
        let declarations = engine.declarations.frozen();
        let candidates = engine
            .source_paths()
            .into_iter()
            .filter(|path| {
                declarations
                    .metadata()
                    .definitions
                    .get(path)
                    .is_some_and(|m| !m.public)
                    && engine.source_facts(path).is_some_and(|f| {
                        (f.body || f.generator) && !f.constant && !f.instance_bodied
                    })
            })
            .map(|path| {
                let module = (
                    crate::memo::PackageKey::of(&path.0.package),
                    crate::memo::ModuleKey::of(&path.0.modules),
                );
                (DefId::intern(&path), module)
            })
            .collect::<hirsel_syntax::DefMap<_>>();
        let modules = candidates
            .values()
            .map(|module| (*module, OnceLock::new()))
            .collect();
        Some(Self {
            candidates,
            modules,
            working: AtomicU64::new(0),
            registrations: OnceLock::new(),
            retention: OnceLock::new(),
        })
    }
    pub(super) fn requires_proof(&self, id: DefId) -> bool {
        self.candidates.contains_key(&id)
    }
    fn product(&self, id: DefId) -> Option<&Result<ModuleProduct, QueryError>> {
        self.modules.get(self.candidates.get(&id)?)?.get()
    }
}
impl HeaderProducts {
    pub(crate) fn configure_private(
        &self,
        working: u64,
        registrations: &[Arc<AcceptedFrontRegistration>],
        retention: Arc<Mutex<u64>>,
    ) {
        let _ = self.retention.set(retention.clone());
        if let Some(private) = &self.private {
            private.working.store(working, Ordering::Relaxed);
            let _ = private.registrations.set(registrations.to_vec());
            let _ = private.retention.set(retention);
        }
    }
    pub(super) fn private_product(
        self: &Arc<Self>,
        id: DefId,
    ) -> Result<Option<&ModuleProduct>, QueryError> {
        let Some(private) = &self.private else {
            return Ok(None);
        };
        let Some(module) = private.candidates.get(&id) else {
            return Ok(None);
        };
        private.modules[module]
            .get_or_init(|| {
                let working = private.working.load(Ordering::Relaxed);
                if working == 0 {
                    return Err(private_refusal());
                }
                // This fixed point reads declared headers only. It cannot demand
                // another settled module while its own publication is in progress.
                let declared = self.declared();
                let mut engine = declared.execution();
                let (facts, complete) = engine.infer_private_module(
                    working,
                    private.registrations.get().map_or(&[], Vec::as_slice),
                    id,
                )?;
                let mut weight = crate::NativeMetadataWeight::default();
                weight.add(
                    size_of::<ModuleProduct>()
                        + facts.proofs.capacity()
                            * size_of::<(DefId, hirsel_types::UniqueInference)>(),
                );
                for (_, bytes) in facts.reads.allocations() {
                    weight.add(bytes);
                }
                for (_, proof) in &facts.proofs {
                    weight.flat_vec(&proof.summary.preserves_unique);
                    weight.flat_vec(&proof.summary.results);
                    weight.flat_vec(&proof.summary.nonretaining);
                    weight.vec(&proof.failures, |weight, failure| {
                        weight.add(failure.message.capacity())
                    });
                }
                let signatures: hirsel_syntax::DefMap<_> = engine
                    .signatures
                    .iter()
                    .filter(|(definition, entry)| {
                        complete
                            && private.candidates.get(definition) == Some(module)
                            && engine.unmarked_signature(**definition).is_some()
                            && entry.reads().iter().all(|(read, _)| match read {
                                ReadKey::PrivateSummary(dependency)
                                | ReadKey::Signature(dependency)
                                | ReadKey::ExecutedSignature(dependency)
                                | ReadKey::FailedSignature(dependency) => private
                                    .candidates
                                    .get(dependency)
                                    .is_none_or(|dependency_module| dependency_module == module),
                                _ => true,
                            })
                    })
                    .map(|(definition, entry)| (*definition, project(entry)))
                    .collect();
                for entry in signatures.values() {
                    weight.add(
                        crate::memo::retained::retained_signature_entry_bytes(entry)
                            .ok_or_else(private_refusal)? as usize,
                    );
                }
                weight.tree::<DefId, Arc<Entry<hirsel_types::SignatureRef>>>(signatures.len());
                let work: BTreeMap<_, _> = engine
                    .recomputes
                    .iter()
                    .map(|((path, kind), count)| ((DefId::intern(path), *kind), *count))
                    .collect();
                weight.tree::<(DefId, QueryKind), usize>(work.len());
                let standalone = if self.standalone && private.retention.get().is_none() {
                    Some(self.reserve_standalone(weight.bytes())?)
                } else if let Some(retention) = private.retention.get() {
                    let mut remaining = retention.lock().unwrap_or_else(|e| e.into_inner());
                    *remaining = remaining
                        .checked_sub(weight.bytes())
                        .ok_or_else(private_refusal)?;
                    None
                } else {
                    return Err(private_refusal());
                };
                self.generation.fetch_add(1, Ordering::Relaxed);
                Ok(ModuleProduct {
                    facts,
                    complete,
                    work,
                    signatures,
                    accounted: AtomicBool::new(false),
                    _standalone: standalone,
                })
            })
            .as_ref()
            .map(Some)
            .map_err(Clone::clone)
    }
    pub(super) fn demanded_summary(&self, id: DefId) -> Option<&hirsel_types::UniqueSummary> {
        self.private
            .as_ref()?
            .product(id)?
            .as_ref()
            .ok()?
            .facts
            .proofs
            .iter()
            .find(|(definition, _)| *definition == id)
            .map(|(_, proof)| &proof.summary)
    }
    pub(crate) fn unique_failure(
        &self,
        id: DefId,
        claim: hirsel_types::UniqueClaim,
    ) -> Option<hirsel_types::UniqueFailure> {
        let proof = self
            .private
            .as_ref()?
            .product(id)?
            .as_ref()
            .ok()?
            .facts
            .proofs
            .iter()
            .find(|(definition, _)| *definition == id)?;
        proof.1.failures.iter().find(|f| f.claim == claim).cloned()
    }
    pub(crate) fn adopt_private_products(
        &self,
        engine: &mut QueryEngine,
    ) -> Result<(), QueryError> {
        let Some(private) = &self.private else {
            return Ok(());
        };
        let mut inferred = engine
            .private_summaries
            .iter()
            .map(|(id, summary)| {
                (
                    *id,
                    hirsel_types::UniqueInference {
                        summary: summary.clone(),
                        failures: engine
                            .private_unique_failures
                            .get(id)
                            .cloned()
                            .unwrap_or_default(),
                    },
                )
            })
            .collect::<hirsel_syntax::DefMap<_>>();
        let mut changed = false;
        for cell in private.modules.values() {
            let Some(Ok(product)) = cell.get() else {
                continue;
            };
            let reads = Arc::new(product.facts.reads.clone());
            for (id, proof) in &product.facts.proofs {
                engine.private_summary_reads.insert(*id, reads.clone());
                inferred.insert(*id, proof.clone());
            }
            if !product.accounted.swap(true, Ordering::Relaxed) {
                for ((id, kind), count) in &product.work {
                    *engine
                        .recomputes
                        .entry((id.path().clone(), *kind))
                        .or_default() += count;
                }
            }
            changed = true;
        }
        if changed {
            engine.publish_private_summaries(inferred)?;
            if private.modules.values().all(|cell| {
                cell.get()
                    .is_some_and(|result| result.as_ref().is_ok_and(|product| product.complete))
            }) {
                engine.private_summary_revision = Some(engine.graph.revision());
            }
        }
        Ok(())
    }
    pub(in crate::queries) fn private_work(&self, work: &mut Vec<(DefPath, QueryKind, u64)>) {
        if let Some(private) = &self.private {
            for product in private
                .modules
                .values()
                .filter_map(OnceLock::get)
                .filter_map(|r| r.as_ref().ok())
            {
                work.extend(
                    product
                        .work
                        .iter()
                        .map(|((id, kind), count)| (id.path().clone(), *kind, *count as u64)),
                );
            }
        }
    }
    pub(super) fn reserve_standalone(
        &self,
        bytes: u64,
    ) -> Result<Arc<crate::runtime::MemoryReservation>, QueryError> {
        // Legacy scalar inputs already use this shared source-inference partition.
        // The admitted front grant covers its task/output rows, not those inputs.
        let bytes = bytes
            .checked_add(
                (size_of::<crate::runtime::MemoryReservation>() + 2 * size_of::<usize>()) as u64,
            )
            .ok_or_else(private_refusal)?;
        crate::queries::private_unique::scheduled::standalone_budget()
            .reserve(bytes)
            .map(Arc::new)
            .map_err(|_| private_refusal())
    }
    fn declared(&self) -> Arc<Self> {
        Arc::new(Self {
            rows: self.rows.keys().map(|id| (*id, OnceLock::new())).collect(),
            generation: AtomicU64::new(0),
            retention: OnceLock::new(),
            environment: self.environment.clone(),
            adapters: self.adapters.clone(),
            sources: self.sources.clone(),
            owners: self.owners.clone(),
            provisional: true,
            standalone: false,
            failure: None,
            syntax: self.syntax.clone(),
            declarations: self.declarations.clone(),
            catalog: self.catalog.clone(),
            types: self.types.clone(),
            memo: self.memo.clone(),
            builtins: self.builtins.clone(),
            registry: self.registry.clone(),
            package_manifests: self.package_manifests.clone(),
            published_manifest: self.published_manifest,
            store: self.store.clone(),
            identity: self.identity,
            verify_store: self.verify_store,
            limits: self.limits,
            staged: self.staged.clone(),
            summaries: Default::default(),
            failures: Default::default(),
            private: None,
        })
    }
}
fn private_refusal() -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "private inference admission".into(),
        message: "demanded module proof exceeds its existing front partition".into(),
    })
}
