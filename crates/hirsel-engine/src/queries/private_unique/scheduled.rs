//! Private module proofs use the ordinary S5 runtime and canonical read codec.
//! The SCC/fixed-point algorithm remains the Modes adapter's implementation.
use super::*;
use crate::runtime::*;
use crate::store::{Entry as StoredEntry, EntryKey, Reference, VariantRef};
use std::sync::{Mutex, OnceLock};

const KIND: u32 = 0x4602_0001;
type Proofs = Vec<(DefId, UniqueInference)>;
#[derive(Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub(super) struct Facts {
    pub proofs: Proofs,
    pub reads: Reads,
}
struct Input {
    syntax: Arc<ItemSyntax>,
    signature: Arc<QueryOutput<hirsel_types::SignatureRef>>,
    reads: Arc<Reads>,
}
struct Inference {
    query: QueryId,
    key: EntryKey,
    inputs: Vec<Input>,
    base: Reads,
    types: Arc<hirsel_types::TypeInterner>,
    memo: Arc<hirsel_types::TypeMemo>,
    snapshot: Arc<SignatureSnapshot>,
    declarations: Arc<Declarations>,
    citations: Arc<crate::snapshot::OwnedSourceContext>,
    resolver: Arc<dyn ResolverPhase>,
    checker: Arc<dyn CheckerPhase>,
    modes: Arc<dyn ModesPhase>,
    environment: ContentHash,
    result: Mutex<Option<Result<Facts, QueryError>>>,
    expected: Option<Facts>,
    complete: bool,
    reference: Reference,
    store: Option<Arc<dyn crate::store::ResultStore>>,
    records: crate::NativeRecords,
    weight: MemoryWeight,
}
struct Factory(Arc<Inference>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if query != self.0.query {
            return Err(self.0.reference);
        }
        Ok(Box::new(Task {
            inference: self.0.clone(),
            terminal: None,
        }))
    }
}
struct Task {
    inference: Arc<Inference>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.inference.query
    }
    fn weight(&self) -> MemoryWeight {
        self.inference.weight
    }
    fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
        if let Some(terminal) = self.terminal {
            return match terminal {
                Ok(product) => QueryPoll::Complete(product),
                Err(reference) => QueryPoll::Failed(reference),
            };
        }
        let result = self.inference.compute().and_then(|facts| {
            if self
                .inference
                .expected
                .as_ref()
                .is_some_and(|expected| *expected != facts)
            {
                if std::env::var_os("HIRSEL_TRACE_PRIVATE_INFERENCE").is_some() {
                    eprintln!(
                        "private inference cached: {:#?}\nprivate inference recomputed: {facts:#?}",
                        self.inference.expected.as_ref().unwrap()
                    );
                }
                Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "verify-cache".into(),
                    message: "private inference differs from its cached result".into(),
                }))
            } else {
                Ok(facts)
            }
        });
        let product = result.as_ref().ok().and_then(|facts| {
            let bytes = self
                .inference
                .records
                .encode(&*self.inference.citations.owners, facts)?;
            let record =
                crate::store::product_domain(QueryKind::PrivateInference).reference(&bytes)?;
            if self.inference.complete
                && let Some(store) = &self.inference.store
            {
                let variant =
                    VariantRef::publish(&**store, &self.inference.key, facts.reads.clone(), &bytes);
                if let Some(variant) = variant {
                    let mut entry = StoredEntry::load(&**store, &self.inference.key);
                    entry.insert(variant);
                    store.put_query(
                        self.inference.key.2,
                        &self.inference.key,
                        entry.action(&self.inference.key),
                        &entry.encode_for(&self.inference.key),
                    );
                }
            }
            Some(ProductRef {
                address: self.inference.query.address,
                action: crate::store::query_action(
                    &self.inference.key,
                    self.inference.key.1,
                    &facts.reads,
                ),
                record,
            })
        });
        let result = if product.is_none() && result.is_ok() {
            Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "private inference transport".into(),
                message: "cannot encode module proof facts".into(),
            }))
        } else {
            result
        };
        *self
            .inference
            .result
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(result);
        self.terminal = Some(product.ok_or(self.inference.reference));
        match product {
            Some(product) => QueryPoll::Complete(product),
            None => QueryPoll::Failed(self.inference.reference),
        }
    }
}
impl Inference {
    fn record_reads<'a>(
        &self,
        reads: &mut ReadsBuilder,
        observed: impl Iterator<Item = &'a (ReadKey, ContentHash)>,
        candidates: &BTreeSet<DefId>,
    ) {
        for &(key, value) in observed {
            if matches!(key, ReadKey::Signature(id) | ReadKey::DeclaredSignature(id) | ReadKey::PrivateSummary(id) if candidates.contains(&id))
            {
                continue;
            }
            if let ReadKey::Signature(id) = key {
                if let Some(signature) = self.snapshot.signature(id).and_then(Result::ok)
                    && let Some(value) = memos::declared_signature_value(&self.types, signature)
                {
                    reads.record(ReadKey::DeclaredSignature(id), value);
                } else {
                    reads.record(key, value);
                }
            } else {
                reads.record(key, value);
            }
        }
    }
    fn compute(&self) -> Result<Facts, QueryError> {
        let worker = parallel::Worker {
            types: &self.types,
            memo: &self.memo,
            snapshot: &self.snapshot,
            declarations: &self.declarations,
            citations: &*self.citations,
            sources: &self.citations.sources,
            items: &self.citations.items,
            resolver: Some(&*self.resolver),
            checker: Some(&*self.checker),
            modes: None,
        };
        let mut reads = ReadsBuilder::new();
        reads.merge(&self.base);
        let mut bodies = Vec::new();
        let candidates: BTreeSet<DefId> = self
            .inputs
            .iter()
            .map(|input| input.signature.value.definition)
            .collect();
        for input in &self.inputs {
            let path = &input.syntax.item.definition;
            // Published signatures also carry the previous inference proof.
            // This computation reads every candidate body and module membership
            // anew in `base`; do not inherit those old receipts through a
            // declaration whose normalized header survived a body-only edit.
            let signature_reads = Arc::new(
                Reads::from_vec(
                    input
                        .reads
                        .iter()
                        .filter(|(key, _)| {
                            self.base.get(key).is_none()
                                && !matches!(key, ReadKey::PrivateSummary(id) if candidates.contains(id))
                        })
                        .copied()
                        .collect(),
                )
                .map_err(|_| QueryError::NotImplemented)?,
            );
            let done = worker.run_to(
                parallel::Task {
                    index: 0,
                    definition: path.clone(),
                    syntax: input.syntax.clone(),
                    signature: Ok(input.signature.clone()),
                    signature_reads: signature_reads.clone(),
                    text: input.syntax.text_hash,
                    resolution: None,
                    checked: None,
                },
                true,
            );
            let mut source = parallel::FrozenReadSource {
                declarations: &self.declarations,
                citations: &*self.citations,
                candidates: self.snapshot.declaration_candidates(DefId::intern(path)),
                environment: self.environment,
            };
            let observed = done.complete_reads(&mut source).map_err(|e| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "private inference reads".into(),
                    message: format!("{path}: {e:?}"),
                })
            })?;
            self.record_reads(
                &mut reads,
                observed.iter().chain(signature_reads.iter()),
                &candidates,
            );
            if let Some(checked) = done.checked.and_then(|p| p.result().ok()) {
                bodies.push((
                    Arc::new(checked.value.clone()),
                    input.signature.value.clone(),
                ));
            }
        }
        let owner = self
            .inputs
            .first()
            .ok_or(QueryError::NotImplemented)?
            .signature
            .value
            .definition;
        let mut observed = ReadsBuilder::new();
        let mut provider =
            FrozenProvider::for_definition(&self.snapshot, owner, &*self.citations, &mut observed);
        let result = self
            .modes
            .infer_unique(&self.types, &self.memo, &bodies, &mut provider);
        let observed = observed.finish().map_err(|e| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "private inference reads".into(),
                message: format!("{owner}: {e:?}"),
            })
        })?;
        let proofs = result.map_err(|e| match e {
            QueryError::NotImplemented => {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "private inference dependency".into(),
                    message: format!(
                        "{owner}: {:?}",
                        observed
                            .iter()
                            .filter(|(key, _)| matches!(key, ReadKey::FailedSignature(..)))
                            .collect::<Vec<_>>()
                    ),
                })
            }
            other => other,
        })?;
        self.record_reads(&mut reads, observed.iter(), &candidates);
        Ok(Facts {
            proofs,
            reads: reads.finish().map_err(|e| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "private inference reads".into(),
                    message: format!("{owner}: {e:?}"),
                })
            })?,
        })
    }
}

impl QueryEngine {
    fn inference_key(&self, path: &DefPath) -> EntryKey {
        let mut owner = path.clone();
        owner.0.name = "$private-unique".into();
        EntryKey::new(
            ContentHash::of_parts(
                "hirsel.private-inference.v3",
                [self.environment_value().0.as_slice()],
            ),
            &owner,
            ContentHash::default(),
            QueryKind::PrivateInference,
            LoweringPolicy::Reachable,
        )
    }
    fn inference_groups(
        &self,
    ) -> BTreeMap<(crate::memo::PackageKey, crate::memo::ModuleKey), Vec<DefPath>> {
        let mut groups = BTreeMap::<_, Vec<_>>::new();
        for path in self.source_paths() {
            groups
                .entry((
                    crate::memo::PackageKey::of(&path.0.package),
                    crate::memo::ModuleKey::of(&path.0.modules),
                ))
                .or_default()
                .push(path);
        }
        groups
    }
    fn local_inference(
        &self,
    ) -> BTreeMap<(crate::memo::PackageKey, crate::memo::ModuleKey), Facts> {
        let mut groups = BTreeMap::<_, Facts>::new();
        for (id, summary) in &self.private_summaries {
            let Some(reads) = self.private_summary_reads.get(id) else {
                continue;
            };
            let path = id.path();
            let facts = groups
                .entry((
                    crate::memo::PackageKey::of(&path.0.package),
                    crate::memo::ModuleKey::of(&path.0.modules),
                ))
                .or_insert_with(|| Facts {
                    proofs: vec![],
                    reads: (**reads).clone(),
                });
            facts.proofs.push((
                *id,
                UniqueInference {
                    summary: summary.clone(),
                    failures: self
                        .private_unique_failures
                        .get(id)
                        .cloned()
                        .unwrap_or_default(),
                },
            ));
        }
        for facts in groups.values_mut() {
            facts.proofs.sort_by_key(|(id, _)| *id);
        }
        groups
    }
    fn load_inference(&mut self, path: &DefPath) -> Option<Facts> {
        let key = self.inference_key(path);
        let store = self.result_store.clone()?;
        let variant = StoredEntry::load(&*store, &key)
            .resolve_current(&*store, &key, |key| self.observe(path, key))?;
        let facts: Facts = self.decode_record(&variant.payload)?;
        (facts.reads == variant.reads
            && facts
                .proofs
                .iter()
                .all(|(id, _)| hirsel_types::unique_summary::same_module(id.path(), path)))
        .then_some(facts)
    }
    pub(in crate::queries) fn restored_private_summary(
        &mut self,
        id: DefId,
    ) -> Option<hirsel_types::UniqueSummary> {
        if let Some(summary) = self.private_summaries.get(&id) {
            return Some(summary.clone());
        }
        // A body-free consumer can validate the producer's complete module
        // reads and adopt its proof without installing a Modes adapter.
        let facts = self.load_inference(id.path())?;
        let reads = Arc::new(facts.reads);
        for (definition, proof) in facts.proofs {
            self.private_summaries.insert(definition, proof.summary);
            self.private_unique_failures
                .insert(definition, proof.failures);
            self.private_summary_reads.insert(definition, reads.clone());
        }
        self.private_summaries.get(&id).cloned()
    }
    fn adopt_inference(
        &mut self,
        facts: Facts,
        inferred: &mut hirsel_syntax::DefMap<UniqueInference>,
    ) {
        let reads = Arc::new(facts.reads);
        for (id, proof) in facts.proofs {
            self.private_summary_reads.insert(id, reads.clone());
            inferred.insert(id, proof);
        }
    }
    /// Settle module inference before immutable front inputs are captured,
    /// lending and returning the caller's existing process/runtime partition.
    pub fn prepare_private_inference(
        &mut self,
        reservation: MemoryReservation,
    ) -> Result<MemoryReservation, QueryError> {
        self.private_inference_hosted = true;
        self.prepare_private_inference_with_registrations(reservation, &[])
    }
    /// Infer accepted generated bodies along with their written module peers.
    /// Before bootstrap, only written bodies can contribute provisional facts.
    pub fn prepare_private_inference_with_registrations(
        &mut self,
        reservation: MemoryReservation,
        registrations: &[Arc<AcceptedFrontRegistration>],
    ) -> Result<MemoryReservation, QueryError> {
        self.note_environment();
        if self.private_summary_active
            || (registrations.is_empty()
                && self.private_summary_revision == Some(self.graph.revision()))
        {
            return Ok(reservation);
        }
        self.private_summary_active = true;
        self.private_summary_failure = None;
        let result = self
            .infer_on_runtime(reservation, registrations)
            .and_then(|reservation| {
                // Normalization may visit other modules before their proofs have
                // settled. Publish and verify those signatures only after all
                // module facts have been installed, never as provisional headers.
                while !self.private_signature_publications.is_empty() {
                    for id in std::mem::take(&mut self.private_signature_publications) {
                        self.signature_query_id(id)?;
                        if let Some(entry) = self.signatures.get(&id).cloned() {
                            if self.verify_store == crate::store::VerifyStore::Recompute {
                                // A conservative declaration and its settled
                                // proof have different observed inputs. Select
                                // the oracle variant in the completed universe.
                                self.pending_store_checks
                                    .remove(&(id, QueryKind::Signature));
                                self.load_signature(id.path());
                            }
                            self.store_signature(id.path(), &entry)?;
                        }
                        self.private_signature_publications.remove(&id);
                    }
                }
                self.private_summary_revision = Some(self.graph.revision());
                Ok(reservation)
            });
        self.private_summary_active = false;
        if result.is_err() {
            let mut changes = crate::memo::ChangeSet::default();
            for id in self.private_summaries.keys() {
                for read in [
                    ReadKey::PrivateSummary(*id),
                    ReadKey::Signature(*id),
                    ReadKey::ExecutedSignature(*id),
                ] {
                    changes.read(read, self.durability_of(&read));
                }
            }
            self.apply_changes(&changes);
            self.retire_snapshot();
        }
        self.private_summary_failure = result
            .as_ref()
            .err()
            .map(|error| (self.graph.revision(), error.clone()));
        result
    }
    fn infer_on_runtime(
        &mut self,
        mut reservation: MemoryReservation,
        registrations: &[Arc<AcceptedFrontRegistration>],
    ) -> Result<MemoryReservation, QueryError> {
        let Some(modes) = self
            .adapters
            .modes
            .clone()
            .filter(|m| m.infers_private_unique())
        else {
            return Ok(reservation);
        };
        for registration in registrations {
            registration.validate(self)?;
        }
        let mut local = self.local_inference();
        let mut inferred = hirsel_syntax::DefMap::default();
        let mut record_context = None;
        let mut pending = Vec::new();
        self.private_summary_reads.clear();
        for ((package, module), paths) in self.inference_groups() {
            let mut generated = Vec::new();
            for path in &paths {
                if self.source_facts(path).is_some_and(|f| f.generator) {
                    self.materialize(path)?;
                    let syntax = self.snapshot(path)?;
                    if syntax.item.visibility == hirsel_syntax::Visibility::Private
                        && !generation::instance_bodied(&syntax)
                        && generation::generator(&syntax).is_some()
                    {
                        generated.push(DefId::intern(path));
                    }
                }
            }
            let previous = local.remove(&(package, module));
            let previous = previous.filter(|facts| {
                generated
                    .iter()
                    .all(|id| facts.proofs.iter().any(|(proved, _)| proved == id))
            });
            let expected = previous
                .filter(|facts| {
                    facts
                        .reads
                        .first_changed(|key| self.observe(&paths[0], key))
                        .is_none()
                })
                .or_else(|| self.load_inference(&paths[0]))
                .filter(|facts| {
                    generated
                        .iter()
                        .all(|id| facts.proofs.iter().any(|(proved, _)| proved == id))
                });
            if let Some(facts) = &expected {
                for (id, proof) in &facts.proofs {
                    self.private_summaries.insert(*id, proof.summary.clone());
                    self.private_summary_reads
                        .insert(*id, Arc::new(facts.reads.clone()));
                }
            }

            if self.verify_store == crate::store::VerifyStore::Off
                && let Some(facts) = expected
            {
                self.adopt_inference(facts, &mut inferred);
                continue;
            }

            let declarations = self.frozen_declarations();
            let potential: Vec<_> = paths
                .iter()
                .filter(|path| {
                    declarations
                        .metadata()
                        .definitions
                        .get(*path)
                        .is_some_and(|m| !m.public)
                        && self
                            .source_facts(path)
                            .is_some_and(|f| (f.body || f.generator) && !f.constant)
                })
                .cloned()
                .collect();
            if potential.is_empty() {
                continue;
            }
            pending.push((package, module, paths, potential, expected));
        }
        // Discover all missed private bodies before freezing the universe.
        // Registering one module at a time would repeatedly rebuild every
        // signature/private view when another module introduces an instance.
        let potential: Vec<_> = pending
            .iter()
            .flat_map(|(_, _, _, paths, _)| paths.iter().cloned())
            .collect();
        self.register_specializations(&potential);
        for path in &potential {
            self.materialize(path)?;
        }
        let snapshot = (!pending.is_empty()).then(|| self.signature_inputs());
        let declarations = self.frozen_declarations();
        for (package, module, paths, potential, expected) in pending {
            let mut complete = true;
            let snapshot = snapshot.as_ref().expect("pending inference snapshot");
            let mut base = ReadsBuilder::new();
            base.record(
                ReadKey::NameSet(package, module),
                declarations.name_set(package, module),
            );
            base.record(ReadKey::Environment, self.environment_value());
            let mut inputs = Vec::new();
            for path in &potential {
                // Eligibility comes from declaration/source heads, without
                // normalizing unrelated public or generated declarations.
                if declarations
                    .metadata()
                    .definitions
                    .get(path)
                    .is_some_and(|m| m.public)
                    || self
                        .source_facts(path)
                        .is_some_and(|f| (!f.body && !f.generator) || f.constant)
                {
                    continue;
                }
                let id = DefId::intern(path);
                let retained = self
                    .unmarked_signature(id)
                    .and_then(|_| self.signatures.get(&id).map(|entry| entry.result()));
                let signature = match retained.unwrap_or_else(|| self.signature_query(path)) {
                    Ok(signature) => signature,
                    Err(QueryError::Diagnostics(_) | QueryError::NotImplemented) => continue,
                    Err(error) => return Err(error),
                };
                if signature.value.metadata.visibility != hirsel_syntax::Visibility::Private
                    || !matches!(&signature.value.kind, SignatureKind::Function(f) if f.foreign.is_none())
                    || self
                        .source_facts(path)
                        .is_some_and(|f| !f.body && !f.generator)
                {
                    continue;
                }
                self.materialize(path)?;
                let original = self.snapshot(path)?;
                let registration = registrations.iter().find(|r| r.definition() == path);
                if generation::instance_bodied(&original) {
                    continue;
                }
                let generated_text = self.key_text(path, QueryKind::Check)?;
                let mut generated_reads = None;
                let syntax = if generation::generator(&original).is_some() {
                    if let Some(registration) = registration {
                        registration.syntax.clone()
                    } else if let Some(entry) = self.checked.get(&DefId::intern(path)).cloned()
                        && self.memo_valid(path, &entry, generated_text)
                        && self.lower_acceptance_holds(entry.acceptance.as_ref())
                        && let Ok(checked) = entry.result()
                    {
                        generated_reads = Some(entry.memo.reads.clone());
                        checked.value.syntax.clone()
                    } else {
                        complete = false;
                        continue;
                    }
                } else {
                    original
                };
                if syntax
                    .item
                    .attributes
                    .iter()
                    .any(|a| a.name.as_str() == "test")
                    || !matches!(syntax.item.kind, ItemKind::Function { body: Some(_), .. })
                {
                    continue;
                }
                base.record(
                    ReadKey::CitedText(DefId::intern(path)),
                    self.cited_value(path),
                );
                inputs.push(Input {
                    syntax,
                    signature: Arc::new(QueryOutput {
                        value: snapshot
                            .signature(DefId::intern(path))
                            .and_then(Result::ok)
                            .cloned()
                            .ok_or(QueryError::NotImplemented)?,
                        dependencies: vec![],
                        diagnostics: vec![],
                    }),
                    reads: {
                        let mut reads = ReadsBuilder::new();
                        if let Some(signature) = self.shared_memo_reads(path, QueryKind::Signature)
                        {
                            reads.merge(&signature);
                        }
                        if let Some(registration) = registration {
                            reads.merge(registration.reads());
                        }
                        if let Some(generated_reads) = generated_reads {
                            reads.merge(&generated_reads);
                        }
                        Arc::new(reads.finish().map_err(|_| QueryError::NotImplemented)?)
                    },
                });
            }
            if inputs.is_empty() {
                continue;
            }
            let key = self.inference_key(&paths[0]);
            let stamp = (self.declarations.revision(), self.sources.anchors().count());
            let citations = match &record_context {
                Some((previous, context)) if *previous == stamp => Arc::clone(context),
                _ => {
                    let cited = self
                        .source_paths()
                        .into_iter()
                        .map(|path| {
                            let id = DefId::intern(&path);
                            (id, self.cited_value(&path))
                        })
                        .collect();
                    let context = Arc::new(crate::snapshot::OwnedSourceContext {
                        unique_failures: BTreeMap::new(),
                        sources: self.sources.clone(),
                        items: Arc::new(self.items.clone()),
                        declarations: self.declarations.observations(),
                        catalog: self.catalog.clone(),
                        owners: Arc::new(crate::NativeRecordContext::capture(self)),
                        cited,
                        arm_fixes: BTreeMap::new(),
                    });
                    record_context = Some((stamp, context.clone()));
                    context
                }
            };
            // The same per-module fixed point runs with declarations captured
            // by the existing worker. It never enters a scalar body query.
            let reference = crate::store::Domain {
                kind: KIND,
                schema: 1,
            }
            .reference(b"private inference")
            .ok_or(QueryError::NotImplemented)?;
            let errors = RuntimeErrors {
                resource: reference,
                cycle: reference,
                cancelled: reference,
                protocol: reference,
                panicked: reference,
            };
            let working = reservation
                .bytes()
                .saturating_sub(hirsel_intern::jobs::COMPILER_STACK_BYTES as u64 + 16384);
            let mut metadata = crate::NativeMetadataWeight::default();
            metadata.add(size_of::<Inference>() + inputs.capacity() * size_of::<Input>());
            metadata.tree::<DefPath, Vec<Arc<ItemSyntax>>>(2 * self.items.len());
            for (path, values) in &self.items {
                metadata.path(path);
                metadata.path(path);
                metadata.add(2 * values.capacity() * size_of::<Arc<ItemSyntax>>());
            }
            if metadata.bytes() > working / 2 {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "private inference admission".into(),
                    message: "module inputs exceed the retained partition".into(),
                }));
            }
            let inference = Arc::new(Inference {
                query: QueryId {
                    kind: KIND,
                    address: hirsel_artifact_store::Address(key.0),
                },
                key,
                inputs,
                base: base.finish().map_err(|_| QueryError::NotImplemented)?,
                types: self.types.clone(),
                memo: self.memo.clone(),
                snapshot: snapshot.clone(),
                declarations: declarations.clone(),
                citations,
                resolver: self
                    .adapters
                    .resolver
                    .clone()
                    .ok_or(QueryError::NotImplemented)?,
                checker: self
                    .adapters
                    .checker
                    .clone()
                    .ok_or(QueryError::NotImplemented)?,
                modes: modes.clone(),
                environment: self.environment_value(),
                result: Mutex::new(None),
                // Written-only bootstrap has a different proof inventory.
                // Compare the settled cache after generated registrations
                // restore the full module, as required for publication below.
                expected: expected.filter(|_| complete),
                complete,
                reference,
                store: self.result_store.clone(),
                records: crate::NativeRecords::new(
                    self.types.clone(),
                    self.result_store
                        .clone()
                        .unwrap_or_else(|| Arc::new(crate::store::NoneStore)),
                ),
                weight: MemoryWeight {
                    stack: 0,
                    scratch: working / 2,
                    retained: working / 2,
                },
            });
            let mut runtime = AdmittedRuntime::with_query_capacity(
                hirsel_intern::jobs::Jobs::new(1),
                reservation,
                errors,
                2,
            )
            .map_err(|e| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "private inference admission".into(),
                    message: e.to_string(),
                })
            })?;
            runtime
                .register(KIND, Arc::new(Factory(inference.clone())))
                .map_err(|_| QueryError::NotImplemented)?;
            let request = runtime.request(&[inference.query]);
            let completed = runtime.wait(request);
            reservation = runtime
                .shutdown_with_reservation()
                .expect("one runtime reservation");
            let facts = inference
                .result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .unwrap_or_else(|| {
                    Err(QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "private inference".into(),
                        message: format!("admitted module proof did not complete: {completed:?}"),
                    }))
                })?;
            for input in &inference.inputs {
                *self
                    .recomputes
                    .entry((
                        input.syntax.item.definition.clone(),
                        QueryKind::PrivateInference,
                    ))
                    .or_default() += 1;
            }
            self.adopt_inference(facts, &mut inferred);
        }
        self.publish_private_summaries(inferred)?;
        self.private_summary_revision = Some(self.graph.revision());
        Ok(reservation)
    }
    pub(super) fn standalone_private_inference(&mut self) {
        if self
            .adapters
            .modes
            .as_ref()
            .is_none_or(|modes| !modes.infers_private_unique())
        {
            return;
        }
        static SERIAL: Mutex<()> = Mutex::new(());
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        static BUDGET: OnceLock<Arc<ProcessBudget>> = OnceLock::new();
        let budget = BUDGET.get_or_init(|| ProcessBudget::new(512 << 20));
        let available = budget.limit().saturating_sub(budget.reserved());
        if let Ok(reservation) = budget.reserve(available) {
            let _ = self.standalone_inference(reservation).inspect_err(|error| {
                self.private_summary_failure = Some((self.graph.revision(), error.clone()));
            });
        }
    }
    fn standalone_inference(&mut self, reservation: MemoryReservation) -> Result<(), QueryError> {
        let mut reservation =
            self.prepare_private_inference_with_registrations(reservation, &[])?;
        if self.private_inference_hosted {
            return Ok(());
        }
        let definitions = self.front_source_roots();
        let generators = self.front_generator_roots(&definitions)?;
        let missing = generators.iter().any(|path| {
            self.snapshot(path).is_ok_and(|syntax| {
                !generation::instance_bodied(&syntax)
                    && syntax.item.visibility == hirsel_syntax::Visibility::Private
                    && !self.private_summaries.contains_key(&DefId::intern(path))
            })
        });
        if !missing {
            return Ok(());
        }
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "private generated inference".into(),
                message: "standalone bootstrap exceeds its existing partition".into(),
            })
        };
        let mut split = |bytes| reservation.split(bytes).ok_or_else(refused);
        let admission = FrontInstanceBootstrapAdmission {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            weight: MemoryWeight {
                scratch: 8 << 20,
                retained: 2 << 20,
                stack: 0,
            },
            max_reads: definitions.len().saturating_mul(3).min(4096),
            runtime: split(384 << 20)?,
            front: split(32 << 20)?,
            lower: split(32 << 20)?,
            text: split(16 << 20)?,
            syntax: split(16 << 20)?,
            discovery: split(8 << 20)?,
            metadata: split(8 << 20)?,
        };
        let reference = crate::store::Domain {
            kind: KIND,
            schema: 1,
        }
        .reference(b"private generated inference")
        .ok_or_else(refused)?;
        let errors = RuntimeErrors {
            resource: reference,
            cycle: reference,
            cancelled: reference,
            protocol: reference,
            panicked: reference,
        };
        let previous = self.result_store.clone();
        if previous.as_ref().is_none_or(|store| !store.enabled()) {
            self.result_store = Some(Arc::new(crate::store::MemStore::default()));
        }
        let result = self.bootstrap_front_instances(
            &definitions,
            &definitions,
            &generators,
            admission,
            errors,
        );
        self.result_store = previous;
        result.map(|_| ()).map_err(|error| match error {
            FrontInstanceBootstrapError::Bootstrap(FrontRegistrationBootstrapError::Admission(
                error,
            ))
            | FrontInstanceBootstrapError::Discovery { error, .. }
            | FrontInstanceBootstrapError::Front {
                error: Some(error), ..
            } => error,
            FrontInstanceBootstrapError::Bootstrap(
                FrontRegistrationBootstrapError::Acceptance(error),
            ) => error.error,
            FrontInstanceBootstrapError::Bootstrap(FrontRegistrationBootstrapError::Runtime {
                error: Some(error),
                ..
            }) => error,
            _ => refused(),
        })
    }
}
