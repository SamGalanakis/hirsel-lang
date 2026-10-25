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

mod retained;
mod source;
mod storage;
pub use source::FrontInputFootprint;
use source::SiblingArchives;
pub use storage::{FRONT_STORAGE_KIND, FrontStorageOutput, FrontStorageQueries};
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
    definition: DefPath,
    syntax: Reference,
    text: ContentHash,
    implementation: ContentHash,
    signature: Result<Arc<QueryOutput<SignatureRef>>, QueryError>,
    signature_reads: Arc<Reads>,
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
    input_records: crate::NativeRecords,
    anchors: Arc<crate::NativeRecordContext>,
    store: Arc<dyn ResultStore>,
    work: Mutex<BTreeMap<(DefPath, QueryKind), u64>>,
    table_keys: Mutex<BTreeMap<Key, Arc<crate::table_keys::TableKeyUse>>>,
    memos: Mutex<BTreeMap<crate::memo::MemoKey, Arc<Reads>>>,
    store_adoptions: AtomicU64,
    verify_store: crate::store::VerifyStore,
    outcomes: Mutex<BTreeMap<Key, FrontOutcome>>,
    errors: RuntimeErrors,
    retention: Mutex<u64>,
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
const SOURCE_INPUT: Domain = Domain {
    kind: 0x4600_0013,
    schema: 1,
};
impl FrontBodyQueries {
    fn syntax(&self, input: &Input) -> Option<Arc<ItemSyntax>> {
        let bytes = self.store.get_content(input.syntax)?;
        let (mut syntax, occurrence): (ItemSyntax, u32) =
            self.input_records.decode(&*self.anchors, &bytes)?;
        if syntax.item.definition != input.definition
            || syntax.text_hash != input.implementation
            || !syntax.structurally_valid(|span| self.citations.sources.slice(span).is_some())
        {
            return None;
        }
        syntax.occurrence = occurrence;
        Some(Arc::new(syntax))
    }

    /// Potential task, result and counter slots. Completed read/summary
    /// metadata consumes the remaining allowance only on demand. Archive bytes
    /// belong to the selected provider, not to these reference-only rows.
    pub fn input_bytes(definitions: &[DefPath]) -> Option<u64> {
        definitions
            .iter()
            .try_fold(std::mem::size_of::<Self>() as u64, |n, path| {
                n.checked_add(bincode::serialized_size(path).ok()?.checked_mul(4)?)?
                    .checked_add(
                        (std::mem::size_of::<Input>()
                            + std::mem::size_of::<FrontOutcome>()
                            + 8 * std::mem::size_of::<crate::memo::MemoKey>()
                            + 32 * std::mem::size_of::<usize>()) as u64,
                    )
            })
    }
    fn retain(
        &self,
        outcome: &ModeResult,
        checked: &CheckedProduct,
        table_keys: Option<&Arc<crate::table_keys::TableKeyUse>>,
        memos: &[(crate::memo::MemoKey, Arc<Reads>)],
    ) -> Option<()> {
        let charge = retained::metadata(outcome, checked, table_keys, memos)?;
        let mut retention = self.retention.lock().unwrap_or_else(|e| e.into_inner());
        *retention = retention.checked_sub(charge)?;
        Some(())
    }

    /// Actual phase invocations, including failed or discarded producers.
    fn completed_product(&self, query: QueryId) -> Option<Result<ProductRef, Reference>> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let outcome = outcomes.get(&query.address.0)?;
        Some(match &outcome.status {
            Err(_) => Err(outcome.reference),
            Ok(()) => Ok(ProductRef {
                address: query.address,
                action: crate::store::query_action(
                    &self.inputs.get(&query.address.0)?.key,
                    self.inputs.get(&query.address.0)?.key.1,
                    &outcome.reads,
                ),
                record: outcome.reference,
            }),
        })
    }
    /// Captured rows and actual settled metadata owned by this producer view.
    /// Provider content bytes and active decode scratch have separate owners.
    pub fn retained_bytes(&self) -> u64 {
        self._allowance.bytes() - *self.retention.lock().unwrap_or_else(|e| e.into_inner())
    }

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
    /// Actual settled checked summary, including after a later Modes failure.
    pub fn checked_definition(self: &Arc<Self>, path: &DefPath) -> Option<FrontCheckedOutput> {
        let query = self.query(path)?;
        let reference = self
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&query.address.0)?
            .reference;
        self.checked(reference)
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
            (input.definition == *definition).then_some(QueryId {
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
                        self.syntax(input)?,
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

            terminal: None,
        }))
    }
}
struct BodyTask {
    queries: Arc<FrontBodyQueries>,
    query: QueryId,
    weight: MemoryWeight,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for BodyTask {
    fn id(&self) -> QueryId {
        self.query
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            retained: (std::mem::size_of::<Self>() + std::mem::size_of::<ProductRef>()) as u64,
            scratch: self.weight.scratch,
            stack: self.weight.stack,
        }
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
        let variant = entry.hit(|read| queries.observe(&input.definition, read))?;
        if variant.output.len > self.weight.scratch {
            return None;
        }
        let bytes = queries.store.get_content(variant.output)?;
        let restored = persistent::restore_frozen_mode(
            &queries.records,
            &queries.types,
            queries.syntax(input)?,
            &queries.citations,
            input.key.1,
            &bytes,
            Arc::new(variant.reads.clone()),
        )?;
        let definition = DefId::intern(&input.definition);
        let mut memo_rows = vec![(
            crate::memo::MemoKey::new(QueryKind::Modes, definition),
            restored.outcome.reads.clone(),
        )];
        if let Some((_, reads)) = &restored.checked {
            memo_rows.push((
                crate::memo::MemoKey::new(QueryKind::Check, definition),
                reads.clone(),
            ));
        }
        queries.retain(
            &restored.outcome,
            &restored.checked,
            restored.table_keys.as_ref(),
            &memo_rows,
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
        queries
            .memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(memo_rows);
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
        if let Some(product) = self.queries.completed_product(self.query) {
            return product;
        }
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
                    && variant.output.len <= self.weight.scratch
            });
            entry
                .hit(|read| queries.observe(&input.definition, read))
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
        let syntax = queries.syntax(input).ok_or(queries.errors.protocol)?;
        let done = worker.run(Task {
            index: 0,
            definition: input.definition.clone(),
            syntax,
            signature: input.signature.clone(),
            signature_reads: input.signature_reads.clone(),
            text: input.text,
            resolution: None,
            checked: None,
        });
        for kind in done.recomputed() {
            *queries
                .work
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry((done.definition.clone(), kind))
                .or_default() += 1;
        }
        let table_keys = done
            .checked
            .as_ref()
            .and_then(|p| p.result().ok())
            .map(|checked| Arc::new(crate::table_keys::TableKeyUse::capture(&checked.value)));
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
        let memo_rows = completed.memo_snapshot(&mut source);
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
                    |anchor| queries.citations.record_owner(anchor),
                )
                .ok_or(queries.errors.resource)?
        } else {
            completed.take_payload().ok_or(queries.errors.resource)?
        };
        if bytes.len() as u64 > self.weight.scratch {
            return Err(queries.errors.resource);
        }
        if let Some((reads, expected)) = expected {
            if failed || reads != *outcome.reads || expected.as_ref() != bytes.as_slice() {
                let error = QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "verify-cache".into(),
                    message: format!("{}: Modes differs from its cached result", input.definition),
                });
                let failure = queries
                    .records
                    .encode_record(
                        &(
                            &input.definition,
                            &error,
                            QueryKind::Modes.phase_name(),
                            &*outcome.reads,
                        ),
                        |anchor| queries.citations.record_owner(anchor),
                    )
                    .ok_or(queries.errors.resource)?;
                let record = queries
                    .store
                    .put_content(FAILURE, &failure)
                    .ok_or(queries.errors.resource)?;
                let output = ModeResult {
                    definition: input.definition.clone(),
                    result: Err(error),
                    phase: Some(QueryKind::Modes),
                    summary: None,
                    reads: outcome.reads,
                    diagnostic_phases: vec![],
                };
                queries
                    .retain(&output, &None, None, &[])
                    .ok_or(queries.errors.resource)?;
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
            .retain(&outcome, &checked, table_keys.as_ref(), &memo_rows)
            .ok_or(queries.errors.resource)?;
        queries
            .memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(memo_rows);
        if let Some(keys) = table_keys {
            queries
                .table_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.query.address.0, keys);
        }
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
        let mut required = FrontBodyQueries::input_bytes(definitions).ok_or_else(refusal)?;
        if required > allowance.bytes() {
            return Err(refusal());
        }
        let store = self
            .result_store
            .clone()
            .filter(|s| s.enabled())
            .ok_or_else(refusal)?;
        // Materialize only the selected ordinary Parse files before capturing
        // their stable source/anchor archive inputs.
        for definition in definitions {
            self.materialize(definition)?;
        }
        let anchors = Arc::new(crate::NativeRecordContext::capture(self));
        let input_records = crate::NativeRecords::new(self.types.clone(), store.clone());
        let declarations = self.frozen_declarations();
        let mut siblings = BTreeMap::new();
        let mut selected = Vec::new();
        for definition in definitions {
            if self
                .source_facts(definition)
                .is_some_and(|facts| facts.generator)
            {
                return Err(refusal());
            }
            self.materialize(definition)?;
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
            let bytes = input_records
                .encode(&*anchors, &(&syntax, syntax.occurrence))
                .ok_or_else(refusal)?;
            if required > allowance.bytes() {
                return Err(refusal());
            }
            let syntax = store
                .put_content(SOURCE_INPUT, &bytes)
                .ok_or_else(refusal)?;
            let original = self.snapshot(definition)?;
            let file = self.sources.anchor(original.anchor).ok_or_else(refusal)?.0;
            if let std::collections::btree_map::Entry::Vacant(slot) = siblings.entry(file) {
                let mut refs = Vec::new();
                for sibling in declarations.siblings(&original).iter() {
                    let bytes = input_records
                        .encode(&*anchors, &(sibling, sibling.occurrence))
                        .ok_or_else(refusal)?;
                    required = required
                        .checked_add(std::mem::size_of::<Reference>() as u64)
                        .ok_or_else(refusal)?;
                    if required > allowance.bytes() {
                        return Err(refusal());
                    }
                    refs.push(
                        store
                            .put_content(SOURCE_INPUT, &bytes)
                            .ok_or_else(refusal)?,
                    );
                }
                slot.insert(refs);
            }
            selected.push((
                definition.clone(),
                syntax,
                text,
                self.snapshot(definition)?.text_hash,
                key,
            ));
        }
        let snapshot = self.signature_inputs();
        let mut inputs = BTreeMap::new();
        for (definition, syntax, text, implementation, key) in selected {
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
                    signature,
                    signature_reads: self
                        .shared_memo_reads(&definition, QueryKind::Signature)
                        .unwrap_or_else(|| Arc::new(Reads::empty())),
                    definition,
                    syntax,
                    text,
                    implementation,
                    key,
                    weight,
                },
            );
        }
        let arm_fixes = arm_fixes::capture(&self.sources, &self.items);
        let items = Arc::new(BTreeMap::new());
        let cited = self
            .source_paths()
            .into_iter()
            .map(|path| {
                let id = DefId::intern(&path);
                (id, self.citation_source().cited_text(id))
            })
            .collect();
        let candidates = inputs
            .values()
            .map(|input| {
                (
                    input.definition.clone(),
                    self.candidates_value(&input.definition),
                )
            })
            .collect();
        Ok(Arc::new(FrontBodyQueries {
            inputs,
            types: self.types.clone(),
            memo: self.memo.clone(),
            snapshot,
            declarations: declarations.with_archived_siblings(Arc::new(SiblingArchives {
                files: siblings,
                sources: self.sources.clone(),
                owners: anchors.clone(),
                records: crate::NativeRecords::new(self.types.clone(), store.clone()),
                store: store.clone(),
            })),
            citations: OwnedSourceContext {
                sources: self.sources.clone(),
                items,
                declarations: self.declarations.observations(),
                catalog: self.catalog.clone(),
                owners: anchors.clone(),
                cited,
                arm_fixes,
            },
            resolver: self.adapters.resolver.clone(),
            checker: self.adapters.checker.clone(),
            modes: self.adapters.modes.clone(),
            environment: self.environment_value(),
            candidates,
            records: self.type_records().ok_or_else(refusal)?,
            input_records,
            anchors,
            store,
            outcomes: Mutex::new(BTreeMap::new()),
            work: Mutex::new(BTreeMap::new()),
            table_keys: Mutex::new(BTreeMap::new()),
            memos: Mutex::new(BTreeMap::new()),
            store_adoptions: AtomicU64::new(0),
            verify_store: self.verify_store,
            errors,
            retention: Mutex::new(allowance.bytes() - required),
            _allowance: allowance,
        }))
    }
}
