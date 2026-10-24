//! Actual resolve/check/modes body work as admitted S5 tasks. These reuse the
//! existing immutable Worker and scalar canonical codec; no batch pool or
//! mutable engine is invoked by a factory or a poll. Publication is separate.
use super::*;
use crate::runtime::*;
use crate::snapshot::OwnedSourceContext;
use crate::store::{Domain, EntryKey, Reference, ResultStore, VariantRef};
use std::sync::{
    Mutex, Weak,
    atomic::{AtomicU64, Ordering::Relaxed},
};

pub const FRONT_BODY_KIND: u32 = 0x4600_0001;
const FAILURE: Domain = Domain {
    kind: 0x4600_0002,
    schema: 1,
};
type Key = ContentHash;
type CheckedProduct = Option<(Arc<CheckedSummary>, Arc<Reads>)>;
struct FrontOutcome {
    reference: Reference,
    definition: DefPath,
    status: Result<(), QueryError>,
    phase: Option<QueryKind>,
    summary: Option<Arc<PublishedSummary>>,
    reads: Arc<Reads>,
    diagnostic_phases: Vec<(Diagnostic, String)>,
    checked: CheckedProduct,
    // Only active consumers own a decoded body. The settled query retains its
    // canonical archive and compact source facts, including when hold_bytes=0.
    body: Weak<QueryOutput<AnalyzedBody>>,
}
impl FrontOutcome {
    fn capture(reference: Reference, outcome: ModeResult, checked: CheckedProduct) -> Self {
        let ModeResult {
            definition,
            result,
            phase,
            summary,
            reads,
            diagnostic_phases,
        } = outcome;
        let body = result.as_ref().ok().map(Arc::downgrade).unwrap_or_default();
        Self {
            reference,
            definition,
            status: result.map(|_| ()),
            phase,
            summary,
            reads,
            diagnostic_phases,
            checked,
            body,
        }
    }
}
struct Input {
    task: Task,
    key: EntryKey,
    weight: MemoryWeight,
}
/// One immutable namespace/source view. A source registration transaction must
/// select its view before constructing queries; producer completion cannot
/// publish declarations, fuel or a later source root's diagnostics.
pub struct FrontBodyQueries {
    inputs: BTreeMap<Key, Input>,
    types: Arc<hirsel_types::TypeInterner>,
    memo: Arc<hirsel_types::TypeMemo>,
    snapshot: Arc<SignatureSnapshot>,
    declarations: Arc<Declarations>,
    citations: OwnedSourceContext,
    resolver: Option<Arc<dyn ResolverPhase>>,
    checker: Option<Arc<dyn CheckerPhase>>,
    modes: Option<Arc<dyn ModesPhase>>,
    environment: ContentHash,
    candidates: BTreeMap<DefPath, ContentHash>,
    records: Arc<persistent::TypeRecords>,
    store: Arc<dyn ResultStore>,
    work: Mutex<BTreeMap<(DefPath, QueryKind), u64>>,
    table_keys: Mutex<BTreeMap<Key, Arc<crate::table_keys::TableKeyUse>>>,
    memos: Mutex<BTreeMap<crate::memo::MemoKey, Arc<Reads>>>,
    store_adoptions: AtomicU64,
    verify_store: crate::store::VerifyStore,
    outcomes: Mutex<BTreeMap<Key, FrontOutcome>>,
    errors: RuntimeErrors,
    // Own immutable snapshot and output metadata admission until every task and
    // external output reader releases this view.
    _allowance: MemoryReservation,
}
/// A held output shares its snapshot reservation; dropping the runtime cannot
/// release admission while a caller still retains the checked body or sources.
#[derive(Clone)]
pub struct FrontBodyOutput {
    pub outcome: ModeResult,
    _snapshot: Arc<FrontBodyQueries>,
}
/// Checked-only source-growth input. It survives a later Modes failure and
/// carries the checked stage's complete reads rather than a provisional union.
#[derive(Clone)]
pub struct FrontCheckedOutput {
    pub(crate) table_keys: Arc<crate::table_keys::TableKeyUse>,
    pub summary: Arc<CheckedSummary>,
    pub reads: Arc<Reads>,
    _snapshot: Arc<FrontBodyQueries>,
}
impl FrontBodyQueries {
    /// Actual phase invocations, including failed or discarded producers.
    pub fn store_adoptions(&self) -> u64 {
        self.store_adoptions.load(Relaxed)
    }
    fn observe(&self, owner: &DefPath, read: &ReadKey) -> Option<ContentHash> {
        use crate::memo::LookupSource;
        Some(match *read {
            ReadKey::Signature(id) | ReadKey::ExecutedSignature(id) => memos::signature_value(
                self.snapshot
                    .signature(id)
                    .and_then(Result::ok)
                    .map(|signature| signature.fingerprint),
            ),
            ReadKey::FailedSignature(id) => {
                if self
                    .snapshot
                    .signature(id)
                    .is_some_and(|result| result.is_ok())
                {
                    ContentHash::of_parts("hirsel.read.signature-ok", std::iter::empty::<&[u8]>())
                } else {
                    self.snapshot.failed_signature_receipt(id)
                }
            }
            ReadKey::Lookup(key) => {
                let context = key.context();
                let qualifier = key.qualifier();
                let segments = key.segments();
                let path = hirsel_syntax::Path {
                    package: qualifier.clone(),
                    segments: segments.iter().map(|part| Symbol::intern(part)).collect(),
                };
                let index = self.declarations.index(context.package.clone());
                match key.source {
                    LookupSource::Provider => {
                        memos::provider_value(&self.snapshot.lookup(&context, &path).result)
                    }
                    LookupSource::Manifest => {
                        let result = index.lookup(&context, &path);
                        let namespace = result
                            .as_ref()
                            .ok()
                            .and_then(|path| index.entry(path))
                            .map(|entry| entry.namespace);
                        memos::manifest_value(&result, namespace)
                    }
                    LookupSource::Candidates => {
                        index.lookup_candidates_revision(&context, qualifier.as_ref(), &segments)
                    }
                    LookupSource::Head => ContentHash::of_parts(
                        "hirsel.read.head",
                        [&[u8::from(index.has_head(&context, segments.first()?))][..]],
                    ),
                }
            }
            ReadKey::Representation(id) => self
                .snapshot
                .representation(id)
                .and_then(Result::ok)
                .map_or_else(ContentHash::default, |(_, digest)| digest.0),
            ReadKey::CitedText(id) => self.citations.cited_text(id),
            ReadKey::DeclarationCandidates(id) => self
                .snapshot
                .declaration_candidates(id)
                .or_else(|| self.candidates.get(id.path()).copied())
                .unwrap_or_else(memos::no_candidates),
            ReadKey::Environment => self.environment,
            ReadKey::ValidationView(id) => self
                .snapshot
                .validation(&owner.0.package)
                .view(id)
                .map_or_else(ContentHash::default, |input| input.digest().0),
            ReadKey::ValidationUniverse(package) => {
                self.snapshot.validation_input_digest(&package.id())
            }
            ReadKey::UniverseErrors => {
                self.snapshot
                    .validation(&owner.0.package)
                    .universe_errors()
                    .0
            }
            ReadKey::FirstHeap => self
                .snapshot
                .validation(&owner.0.package)
                .first_heap()
                .map_or_else(ContentHash::default, |(id, _)| id.path().fingerprint()),
            // A front product must not execute a dependency to decide a hit.
            // Unknown source/registration facts safely miss this ordinary codec.
            ReadKey::ExecutedBody(_) | ReadKey::LayoutBody(_) | ReadKey::NameSet(..) => {
                return None;
            }
        })
    }
    /// Actual completed phase read rows, captured by the same compact phase
    /// finisher as scalar publication. This inspection executes no query.
    pub fn memo_snapshot(&self) -> Vec<(crate::memo::MemoKey, Arc<Reads>)> {
        self.memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(key, reads)| (*key, reads.clone()))
            .collect()
    }
    pub fn work_snapshot(&self) -> Vec<(DefPath, QueryKind, u64)> {
        self.work
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|((path, kind), count)| (path.clone(), *kind, *count))
            .collect()
    }
    pub fn recompute_count(&self, definition: &DefPath, kind: QueryKind) -> u64 {
        self.work
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(definition.clone(), kind))
            .copied()
            .unwrap_or(0)
    }
    pub fn checked(self: &Arc<Self>, reference: Reference) -> Option<FrontCheckedOutput> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (key, outcome) = outcomes
            .iter()
            .find(|(_, outcome)| outcome.reference == reference)?;
        let (summary, reads) = outcome.checked.as_ref()?;
        Some(FrontCheckedOutput {
            table_keys: self
                .table_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(key)?
                .clone(),
            summary: summary.clone(),
            reads: reads.clone(),
            _snapshot: self.clone(),
        })
    }

    pub fn query(&self, definition: &DefPath) -> Option<QueryId> {
        self.inputs.iter().find_map(|(&address, input)| {
            (input.task.definition == *definition).then_some(QueryId {
                kind: FRONT_BODY_KIND,
                address: hirsel_artifact_store::Address(address),
            })
        })
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    /// An immutable outcome, including exact error-phase and read evidence.
    /// This lookup never adopts a product into a mutable source namespace.
    pub fn outcome(self: &Arc<Self>, reference: Reference) -> Option<FrontBodyOutput> {
        let mut outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (key, selected) = outcomes
            .iter_mut()
            .find(|(_, outcome)| outcome.reference == reference)?;
        let result = match &selected.status {
            Err(error) => Err(error.clone()),
            Ok(()) => {
                let output = if let Some(output) = selected.body.upgrade() {
                    output
                } else {
                    let bytes = self.store.get_content(reference)?;
                    let input = self.inputs.get(key)?;
                    let restored = persistent::restore_frozen_mode(
                        &self.records,
                        &self.types,
                        input.task.syntax.clone(),
                        &self.citations,
                        input.key.1,
                        &bytes,
                        selected.reads.clone(),
                    )?;
                    let output = restored.outcome.result.ok()?;
                    selected.body = Arc::downgrade(&output);
                    output
                };
                Ok(output)
            }
        };
        Some(FrontBodyOutput {
            outcome: ModeResult {
                definition: selected.definition.clone(),
                result,
                phase: selected.phase,
                summary: selected.summary.clone(),
                reads: selected.reads.clone(),
                diagnostic_phases: selected.diagnostic_phases.clone(),
            },
            _snapshot: self.clone(),
        })
    }
    /// Distinct decoded bodies currently owned by active output consumers.
    /// Settled archives and compact checked summaries contain no body owner.
    pub fn live_body_count(&self) -> usize {
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|outcome| outcome.body.strong_count() != 0)
            .count()
    }
    #[cfg(test)]
    pub(super) fn outcomes_for_test(&self) -> Vec<Reference> {
        self.outcomes
            .lock()
            .unwrap()
            .values()
            .map(|outcome| outcome.reference)
            .collect()
    }
    fn worker(&self) -> Worker<'_> {
        Worker {
            types: &self.types,
            memo: &self.memo,
            snapshot: &self.snapshot,
            declarations: &self.declarations,
            citations: &self.citations,
            sources: &self.citations.sources,
            items: &self.citations.items,
            resolver: self.resolver.as_deref(),
            checker: self.checker.as_deref(),
            modes: self.modes.as_deref(),
        }
    }
}
struct Factory(Arc<FrontBodyQueries>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        let input = self
            .0
            .inputs
            .get(&query.address.0)
            .filter(|_| query.kind == FRONT_BODY_KIND)
            .ok_or(self.0.errors.protocol)?;
        Ok(Box::new(BodyTask {
            queries: self.0.clone(),
            query,
            weight: input.weight,
            task: Some(input.task.clone()),
            terminal: None,
        }))
    }
}
struct BodyTask {
    queries: Arc<FrontBodyQueries>,
    query: QueryId,
    weight: MemoryWeight,
    task: Option<Task>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for BodyTask {
    fn id(&self) -> QueryId {
        self.query
    }
    fn weight(&self) -> MemoryWeight {
        self.weight
    }
    fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        let result = self.run();
        self.terminal = Some(result);
        finish(result)
    }
}
fn finish(result: Result<ProductRef, Reference>) -> QueryPoll {
    match result {
        Ok(product) => QueryPoll::Complete(product),
        Err(error) => QueryPoll::Failed(error),
    }
}
impl BodyTask {
    fn load(&self) -> Option<Result<ProductRef, Reference>> {
        let queries = &self.queries;
        let input = queries.inputs.get(&self.query.address.0)?;
        let mut entry = crate::store::Entry::load(&*queries.store, &input.key);
        entry
            .variants
            .retain(|variant| variant.inputs == input.key.1 && variant.valid(&input.key));
        let variant = entry.hit(|read| queries.observe(&input.task.definition, read))?;
        if variant.output.len > self.weight.retained {
            return None;
        }
        let bytes = queries.store.get_content(variant.output)?;
        let restored = persistent::restore_frozen_mode(
            &queries.records,
            &queries.types,
            input.task.syntax.clone(),
            &queries.citations,
            input.key.1,
            &bytes,
            Arc::new(variant.reads.clone()),
        )?;
        if let Some(table_keys) = restored.table_keys {
            queries
                .table_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.query.address.0, table_keys);
        }
        let (outcome, checked) = (restored.outcome, restored.checked);
        let failed = outcome.result.is_err();
        let definition = DefId::intern(&input.task.definition);
        let mut memos = queries.memos.lock().unwrap_or_else(|e| e.into_inner());
        memos.insert(
            crate::memo::MemoKey::new(QueryKind::Modes, definition),
            outcome.reads.clone(),
        );
        if let Some((_, reads)) = &checked {
            memos.insert(
                crate::memo::MemoKey::new(QueryKind::Check, definition),
                reads.clone(),
            );
        }
        drop(memos);
        queries
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                self.query.address.0,
                FrontOutcome::capture(variant.output, outcome, checked),
            );
        queries.store_adoptions.fetch_add(1, Relaxed);
        timings::count(Count::StoreHits);
        if failed {
            return Some(Err(variant.output));
        }
        let action = variant.action(&input.key);
        queries
            .store
            .associate_product(self.query.address, action, variant.output);
        Some(Ok(ProductRef {
            address: self.query.address,
            action,
            record: variant.output,
        }))
    }
    fn run(&mut self) -> Result<ProductRef, Reference> {
        if self.queries.verify_store == crate::store::VerifyStore::Off {
            if let Some(hit) = self.load() {
                return hit;
            }
        }
        let queries = &self.queries;
        let input = &queries.inputs[&self.query.address.0];
        let expected = if queries.verify_store == crate::store::VerifyStore::Recompute {
            let mut entry = crate::store::Entry::load(&*queries.store, &input.key);
            entry.variants.retain(|variant| {
                variant.inputs == input.key.1
                    && variant.valid(&input.key)
                    && variant.output.len <= self.weight.retained
            });
            entry
                .hit(|read| queries.observe(&input.task.definition, read))
                .and_then(|variant| {
                    queries
                        .store
                        .get_content(variant.output)
                        .map(|bytes| (variant.reads.clone(), bytes))
                })
        } else {
            None
        };
        let worker = queries.worker();
        let done = worker.run(self.task.take().ok_or(queries.errors.protocol)?);
        for kind in done.recomputed() {
            *queries
                .work
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry((done.definition.clone(), kind))
                .or_default() += 1;
        }
        if let Some(checked) = done
            .checked
            .as_ref()
            .and_then(|product| product.result().ok())
        {
            queries
                .table_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    self.query.address.0,
                    Arc::new(crate::table_keys::TableKeyUse::capture(&checked.value)),
                );
        }
        let result = done.result();
        let mut source = FrozenReadSource {
            declarations: &queries.declarations,
            citations: &queries.citations,
            candidates: queries.candidates.get(&done.definition).copied(),
            environment: queries.environment,
        };
        let mut completed = CompactCompleted::capture(
            done,
            &worker,
            queries.types.clone(),
            &mut source,
            Some(&queries.records),
        );
        queries
            .memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(completed.memo_snapshot(&mut source));
        let checked = completed.checked_product();
        let published = completed.outcome().clone();
        let outcome = ModeResult {
            definition: published.definition.clone(),
            result: match &published.result {
                Err(error) => Err(error.clone()),
                Ok(_) => result,
            },
            phase: published.phase,
            summary: published.result.as_ref().ok().map(|v| v.summary.clone()),
            reads: published.reads.clone(),
            diagnostic_phases: published.diagnostic_phases.clone(),
        };
        // A front failure is an immutable diagnostic record, never a successful
        // mode output, and includes source/phase/ordered-read provenance.
        let failed = published.result.is_err();
        let bytes = if failed {
            queries
                .records
                .encode_record(
                    &(
                        published.definition,
                        published.result.err(),
                        published.phase.map(QueryKind::phase_name),
                        published.diagnostic_phases,
                        &*published.reads,
                    ),
                    |anchor| persistent::anchor_owner(&queries.citations.items, anchor),
                )
                .ok_or(queries.errors.resource)?
        } else {
            completed.take_payload().ok_or(queries.errors.resource)?
        };
        if bytes.len() as u64 > self.weight.retained {
            return Err(queries.errors.resource);
        }
        if let Some((reads, expected)) = expected {
            if failed || reads != *outcome.reads || expected.as_ref() != bytes.as_slice() {
                let error = QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "verify-cache".into(),
                    message: format!(
                        "{}: Modes differs from its cached result",
                        input.task.definition
                    ),
                });
                let failure = queries
                    .records
                    .encode_record(
                        &(
                            &input.task.definition,
                            &error,
                            QueryKind::Modes.phase_name(),
                            &*outcome.reads,
                        ),
                        |anchor| persistent::anchor_owner(&queries.citations.items, anchor),
                    )
                    .ok_or(queries.errors.resource)?;
                let record = queries
                    .store
                    .put_content(FAILURE, &failure)
                    .ok_or(queries.errors.resource)?;
                let output = ModeResult {
                    definition: input.task.definition.clone(),
                    result: Err(error),
                    phase: Some(QueryKind::Modes),
                    summary: None,
                    reads: outcome.reads,
                    diagnostic_phases: vec![],
                };
                queries
                    .outcomes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        self.query.address.0,
                        FrontOutcome::capture(record, output, None),
                    );
                return Err(record);
            }
        }
        let (record, action) = if failed {
            let record = queries
                .store
                .put_content(FAILURE, &bytes)
                .ok_or(queries.errors.resource)?;
            (
                record,
                hirsel_artifact_store::Action(ContentHash::of_parts(
                    "hirsel.front.failure-action.v1",
                    [
                        input.key.0.0.as_slice(),
                        input.key.1.0.as_slice(),
                        record.output.0.0.as_slice(),
                    ],
                )),
            )
        } else {
            let variant = VariantRef::publish(
                queries.store.as_ref(),
                &input.key,
                (*outcome.reads).clone(),
                &bytes,
            )
            .ok_or(queries.errors.resource)?;
            let mut entry = crate::store::Entry::load(&*queries.store, &input.key);
            entry.insert(variant.clone());
            queries.store.put_query(
                input.key.2,
                &input.key,
                entry.action(&input.key),
                &entry.encode_for(&input.key),
            );
            (variant.output, variant.action(&input.key))
        };
        queries
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                self.query.address.0,
                FrontOutcome::capture(record, outcome, checked),
            );
        if failed {
            return Err(record);
        }
        queries
            .store
            .associate_product(self.query.address, action, record);
        Ok(ProductRef {
            address: self.query.address,
            action,
            record,
        })
    }
}
impl QueryEngine {
    /// Capture only demanded body inputs and immutable phase handles. Body
    /// checking starts on the admitted worker, using the same ordinary worker
    /// and codec as scalar queries. Dynamic declarations require their accepted
    /// registration view first; construction never executes a generator.
    pub fn front_body_queries(
        &mut self,
        definitions: &[DefPath],
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontBodyQueries>, QueryError> {
        let refusal = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "front query admission".into(),
                message:
                    "immutable front inputs exceed admission or need an accepted registration view"
                        .into(),
            })
        };
        // The immutable view retains outcomes independently of hot-node eviction.
        // Conservatively reserve each output bound here as well as its runtime
        // poll weight, until representation-only readers can release bodies.
        let required = weight
            .retained
            .checked_add(8192)
            .and_then(|bytes| bytes.checked_mul(definitions.len() as u64));
        if required.is_none_or(|bytes| bytes > allowance.bytes()) {
            return Err(refusal());
        }
        let store = self
            .result_store
            .clone()
            .filter(|s| s.enabled())
            .ok_or_else(refusal)?;
        let mut selected = Vec::new();
        for definition in definitions {
            if self
                .source_facts(definition)
                .is_some_and(|facts| facts.generator)
            {
                return Err(refusal());
            }
            let syntax = self.snapshot(definition)?;
            if generation::generator(&syntax).is_some() {
                return Err(refusal());
            }
            let text = self.key_text(definition, QueryKind::Modes)?;
            let key = self
                .store_key(definition, QueryKind::Modes)
                .ok_or_else(refusal)?;
            // Immutable S0 variants provide body reuse. The construction
            // inventory itself must never pin a mutable engine's checked body
            // after a request settles, including the explicit zero hold policy.
            let resolution = None;
            let checked = None;
            selected.push((definition.clone(), syntax, text, key, resolution, checked));
        }
        let snapshot = self.signature_inputs();
        let mut inputs = BTreeMap::new();
        for (definition, syntax, text, key, resolution, checked) in selected {
            let id = DefId::intern(&definition);
            let signature = match snapshot.signature(id) {
                Some(Ok(signature)) => self
                    .signatures
                    .get(&id)
                    .and_then(|entry| entry.result().ok())
                    .map(Ok)
                    .unwrap_or_else(|| Ok(Arc::new(output(signature.clone())))),
                Some(Err(error)) => Err(error.clone()),
                None => Err(QueryError::NotImplemented),
            };
            inputs.insert(
                key.0,
                Input {
                    task: Task {
                        index: 0,
                        signature,
                        signature_reads: self
                            .shared_memo_reads(&definition, QueryKind::Signature)
                            .unwrap_or_else(|| Arc::new(Reads::empty())),
                        definition,
                        syntax,
                        text,
                        resolution,
                        checked,
                    },
                    key,
                    weight,
                },
            );
        }
        let items = Arc::new(self.items.clone());
        let candidates = inputs
            .values()
            .map(|input| {
                (
                    input.task.definition.clone(),
                    self.candidates_value(&input.task.definition),
                )
            })
            .collect();
        Ok(Arc::new(FrontBodyQueries {
            inputs,
            types: self.types.clone(),
            memo: self.memo.clone(),
            snapshot,
            declarations: self.frozen_declarations(),
            citations: OwnedSourceContext {
                sources: self.sources.clone(),
                items,
                declarations: self.declarations.clone(),
                catalog: self.catalog.clone(),
            },
            resolver: self.adapters.resolver.clone(),
            checker: self.adapters.checker.clone(),
            modes: self.adapters.modes.clone(),
            environment: self.environment_value(),
            candidates,
            records: self.type_records().ok_or_else(refusal)?,
            store,
            outcomes: Mutex::new(BTreeMap::new()),
            work: Mutex::new(BTreeMap::new()),
            table_keys: Mutex::new(BTreeMap::new()),
            memos: Mutex::new(BTreeMap::new()),
            store_adoptions: AtomicU64::new(0),
            verify_store: self.verify_store,
            errors,
            _allowance: allowance,
        }))
    }
}
