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

mod carry;
mod compact_store;
mod lint_facts;
mod retained;
use carry::CarryChanges;
pub use carry::FrontCarry;
use lint_facts::LintFacts;
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
enum FrontSummary {
    Checked {
        definition: DefId,
        text: ContentHash,
    },
    Owned(Arc<PublishedSummary>),
}
impl FrontSummary {
    fn shareable(summary: &PublishedSummary, checked: &CheckedProduct, reads: &Reads) -> bool {
        checked.as_ref().is_some_and(|(value, _)| {
            summary.storage.is_none()
                && summary
                    .checked
                    .as_ref()
                    .is_some_and(|c| Arc::ptr_eq(c, value))
                && summary.summary == value.summary
                && summary.growth == value.growth
                && summary.reads.as_ref() == reads
        })
    }
    fn capture(summary: Arc<PublishedSummary>, checked: &CheckedProduct, reads: &Reads) -> Self {
        if Self::shareable(&summary, checked, reads) {
            Self::Checked {
                definition: summary.definition,
                text: summary.text,
            }
        } else {
            Self::Owned(summary)
        }
    }
    fn materialize(&self, checked: &CheckedProduct, reads: &Arc<Reads>) -> Arc<PublishedSummary> {
        match self {
            Self::Owned(summary) => summary.clone(),
            Self::Checked { definition, text } => {
                let (checked, _) = checked.as_ref().expect("captured checked summary");
                Arc::new(PublishedSummary {
                    definition: *definition,
                    text: *text,
                    summary: checked.summary.clone(),
                    growth: checked.growth.clone(),
                    storage: None,
                    checked: Some(checked.clone()),
                    reads: reads.clone(),
                })
            }
        }
    }
}
struct FrontOutcome {
    reference: Reference,
    definition: DefId,
    status: Result<(), QueryError>,
    phase: Option<QueryKind>,
    summary: Option<FrontSummary>,
    reads: Arc<Reads>,
    diagnostic_phases: Vec<(Diagnostic, String)>,
    checked: CheckedProduct,
    /// The package dead-arm lint's facts of a checked body.
    dead_arms: Option<LintFacts>,
    // Only active consumers own a decoded body. The settled query retains its
    // canonical archive and compact source facts, including when hold_bytes=0.
    body: Weak<QueryOutput<AnalyzedBody>>,
    diagnostics: Vec<Diagnostic>,
    lowering: Option<lowering::LoweringUse>,
}
pub(crate) struct FrontCheckFacts {
    pub result: Result<Vec<Diagnostic>, QueryError>,
    pub phase: Option<QueryKind>,
    pub reads: Arc<Reads>,
    pub source_rules: bool,
    pub lowering: Option<lowering::LoweringUse>,
}
impl FrontOutcome {
    fn capture(
        reference: Reference,
        outcome: ModeResult,
        checked: CheckedProduct,
        dead_arms: Option<crate::tools::DeadArmUse>,
    ) -> Self {
        let ModeResult {
            definition,
            result,
            phase,
            summary,
            reads,
            diagnostic_phases,
        } = outcome;
        let body = result.as_ref().ok().map(Arc::downgrade).unwrap_or_default();
        let diagnostics = result
            .as_ref()
            .ok()
            .map(|output| output.diagnostics.clone())
            .unwrap_or_default();
        let lowering = result
            .as_ref()
            .ok()
            .map(|output| lowering::LoweringUse::capture(&output.value.typed));
        let summary = summary.map(|summary| FrontSummary::capture(summary, &checked, &reads));
        Self {
            reference,
            definition: DefId::intern(&definition),
            status: result.map(|_| ()),
            phase,
            summary,
            reads,
            diagnostic_phases,
            checked,
            dead_arms: dead_arms.map(LintFacts::capture),
            body,
            diagnostics,
            lowering,
        }
    }
}
struct Input {
    definition: DefId,
    syntax: Reference,
    text: ContentHash,
    implementation: ContentHash,
    signature: Result<Arc<QueryOutput<SignatureRef>>, QueryError>,
    signature_reads: Arc<Reads>,
    registration: Option<Arc<AcceptedFrontRegistration>>,
    key: EntryKey,
    weight: MemoryWeight,
}
/// One immutable namespace/source view. A source registration transaction must
/// select its view before constructing queries; producer completion cannot
/// publish declarations, fuel or a later source root's diagnostics.
pub struct FrontBodyQueries {
    inputs: BTreeMap<Key, Input>,
    by_definition: Box<[(DefId, Key)]>,
    body_demands: Box<[(Key, Key)]>,
    types: Arc<hirsel_types::TypeInterner>,
    memo: Arc<hirsel_types::TypeMemo>,
    snapshot: Arc<SignatureSnapshot>,
    declarations: Arc<Declarations>,
    citations: OwnedSourceContext,
    resolver: Option<Arc<dyn ResolverPhase>>,
    checker: Option<Arc<dyn CheckerPhase>>,
    modes: Option<Arc<dyn ModesPhase>>,
    environment: ContentHash,
    candidates: BTreeMap<DefId, ContentHash>,
    records: Arc<persistent::TypeRecords>,
    input_records: crate::NativeRecords,
    anchors: Arc<crate::NativeRecordContext>,
    store: Arc<dyn ResultStore>,
    work: Mutex<BTreeMap<(DefId, QueryKind), u64>>,
    table_keys: Mutex<BTreeMap<Key, Arc<crate::table_keys::TableKeyUse>>>,
    memos: Mutex<BTreeMap<crate::memo::MemoKey, Arc<Reads>>>,
    store_adoptions: AtomicU64,
    /// Full-rescan refusals of stored body entries: by the first changed read
    /// of the newest variant, in `ReadKind::ALL` order, then by own text.
    refusals: [AtomicU64; crate::memo::ReadKind::ALL.len()],
    text_refusals: AtomicU64,
    /// Stored entry validations plus memo rows written.
    memo_operations: AtomicU64,
    verify_store: crate::store::VerifyStore,
    outcomes: Mutex<BTreeMap<Key, FrontOutcome>>,
    carry: Mutex<Option<Arc<FrontCarry>>>,
    /// Subscription generations validated against this immutable source view.
    carry_changes: Mutex<Option<Arc<CarryChanges>>>,
    /// Memo rows adopted from the carry without validation. They were neither
    /// marked nor scanned, so inspection does not count them as touched.
    clean_memos: Mutex<BTreeSet<crate::memo::MemoKey>>,
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
    pub(crate) fn prepare_generator(
        &self,
        syntax: Arc<ItemSyntax>,
        signer: &dyn SignaturePhase,
        instance: bool,
    ) -> s1::DependencyProduct<generation::task::PreparedGenerator> {
        let (Some(resolver), Some(checker), Some(modes)) = (
            self.resolver.as_deref(),
            self.checker.as_deref(),
            self.modes.as_deref(),
        ) else {
            return s1::DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
        };
        let path = syntax.item.definition.clone();
        // A closed instance is a parser-created single-declaration source. Its
        // private generator reads that source, which is not an ordinary body
        // input in this view. Keep the source context explicit and local to
        // preparation, under the admitted worker's existing scratch grant.
        let declarations = if instance {
            self.declarations
                .with_archived_siblings(Arc::new(generation::task::InstanceSiblings(
                    syntax.clone(),
                )))
        } else {
            self.declarations.clone()
        };
        generation::task::prepare(
            &path,
            syntax,
            &self.types,
            &self.memo,
            &self.snapshot,
            &declarations,
            &self.citations,
            self.environment,
            &generation::task::GeneratorPhases {
                signature: signer,
                resolver,
                checker,
                modes,
            },
        )
    }
    pub(crate) fn stage_source(&self) -> crate::queries::scheduled_stage::StageSource {
        crate::queries::scheduled_stage::StageSource {
            snapshot: self.snapshot.clone(),
            memo: self.memo.clone(),
            records: self.input_records.clone(),
            anchors: self.anchors.clone(),
            store: self.store.clone(),
        }
    }
    /// The exact immutable signature universe selected with these P inputs.
    pub fn signatures(&self) -> Arc<SignatureSnapshot> {
        self.snapshot.clone()
    }
    fn syntax(&self, input: &Input) -> Option<Arc<ItemSyntax>> {
        // Accepted literals are parser-created immutable source, and legitimately
        // combine the written header's anchor with generated body anchors. They
        // never enter the ordinary single-source untrusted archive decoder.
        if let Some(registration) = &input.registration {
            let syntax = &registration.syntax;
            // Instance bodies retain the declaring source's lexical owner,
            // as on the scalar path. The opaque registration binds the query
            // identity; the syntax's owner is not a second instance identity.
            return (registration.definition() == input.definition.path()
                && syntax.text_hash == input.implementation)
                .then(|| syntax.clone());
        }
        let bytes = self.store.get_content(input.syntax)?;
        let (mut syntax, occurrence): (ItemSyntax, u32) =
            self.input_records.decode(&*self.anchors, &bytes)?;
        if syntax.item.definition != *input.definition.path()
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
        (definitions.len() as u64)
            .checked_mul(
                (std::mem::size_of::<Input>()
                    + std::mem::size_of::<FrontOutcome>()
                    + 8 * std::mem::size_of::<crate::memo::MemoKey>()
                    + 32 * std::mem::size_of::<usize>()
                    + std::mem::size_of::<(DefId, Key)>()
                    + std::mem::size_of::<(Key, Key)>()) as u64,
            )?
            .checked_add(std::mem::size_of::<Self>() as u64)
    }
    fn retain(
        &self,
        outcome: &ModeResult,
        checked: &CheckedProduct,
        table_keys: Option<&Arc<crate::table_keys::TableKeyUse>>,
        memos: &[(crate::memo::MemoKey, Arc<Reads>)],
    ) -> Option<()> {
        let charge = retained::compact_metadata(outcome, checked, table_keys, memos)?;
        let mut retention = self.retention.lock().unwrap_or_else(|e| e.into_inner());
        *retention = retention.checked_sub(charge)?;
        Some(())
    }
    fn retain_dead_arms(&self, uses: Option<&mut crate::tools::DeadArmUse>) -> Option<()> {
        let Some(uses) = uses else {
            return Some(());
        };
        let bytes = LintFacts::capture_bytes(uses);
        let mut retention = self.retention.lock().unwrap_or_else(|e| e.into_inner());
        *retention = retention.checked_sub(bytes)?;
        Some(())
    }
    /// The package dead-arm lint's facts of `definition`'s settled checked
    /// body, if this view settled one.
    pub(crate) fn dead_arm_use(&self, definition: &DefPath) -> Option<crate::tools::DeadArmUse> {
        let query = self.query(definition)?;
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&query.address.0)?
            .dead_arms
            .as_ref()
            .map(LintFacts::materialize)
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

    /// Immutable producer grant, including input rows and retained outcomes.
    pub fn admitted_bytes(&self) -> u64 {
        self._allowance.bytes()
    }

    pub fn store_adoptions(&self) -> u64 {
        self.store_adoptions.load(Relaxed)
    }
    /// Cumulative refusals, as `MemoStats` invalidation counters, and raw memo
    /// operations. Each refused entry is one memo identity.
    pub fn refusal_stats(&self) -> (crate::memo::MemoStats, u64) {
        let mut stats = crate::memo::MemoStats::default();
        for (total, count) in stats.invalidated.iter_mut().zip(&self.refusals) {
            *total = count.load(Relaxed);
        }
        stats.invalidated_text = self.text_refusals.load(Relaxed);
        (stats, self.memo_operations.load(Relaxed))
    }
    /// Successful lookup targets are semantic closure edges even when a
    /// primitive/alias adds no normalized signature fingerprint dependency.
    pub(crate) fn lookup_definition(&self, key: crate::memo::LookupKey) -> Option<DefPath> {
        Self::lookup_in(&self.snapshot, &self.declarations, key)
    }
    pub(crate) fn lookup_in(
        snapshot: &SignatureSnapshot,
        declarations: &Declarations,
        key: crate::memo::LookupKey,
    ) -> Option<DefPath> {
        use crate::memo::LookupSource;
        let context = key.context();
        let path = hirsel_syntax::Path {
            package: key.qualifier(),
            segments: key
                .segments()
                .iter()
                .map(|part| Symbol::intern(part))
                .collect(),
        };
        match key.source {
            LookupSource::Provider => snapshot.lookup(&context, &path).result.ok(),
            LookupSource::Manifest => declarations
                .index(context.package.clone())
                .lookup(&context, &path)
                .ok(),
            LookupSource::Head | LookupSource::Candidates => None,
        }
    }
    pub(crate) fn observe(&self, owner: &DefPath, read: &ReadKey) -> Option<ContentHash> {
        self.observe_in(&owner.0.package, read)
    }
    fn observe_in(
        &self,
        package: &hirsel_syntax::PackageId,
        read: &ReadKey,
    ) -> Option<ContentHash> {
        use crate::memo::LookupSource;
        Some(match *read {
            ReadKey::PrivateSummary(id) => memos::private_summary_value(
                &self
                    .snapshot
                    .signature(id)
                    .and_then(Result::ok)?
                    .metadata
                    .inferred_unique,
            ),
            ReadKey::DeclaredSignature(id) => {
                let signature = self.snapshot.signature(id).and_then(Result::ok)?;
                memos::declared_signature_value(&self.types, signature)?
            }
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
                        // As the resolver recorded it: a name the manifest index
                        // misses (a prelude name) is the provider's, if visible.
                        let mut result = index.lookup(&context, &path);
                        if let Err(
                            hirsel_resolve::LookupError::Missing
                            | hirsel_resolve::LookupError::MissingMember(_),
                        ) = &result
                        {
                            match self.snapshot.lookup(&context, &path).result {
                                Ok(found) => {
                                    if !matches!(&result, Err(hirsel_resolve::LookupError::MissingMember(candidates)) if !candidates.contains(&found))
                                    {
                                        result = index.access(&context, &found);
                                    }
                                }
                                Err(QueryError::Diagnostics(ds)) if !ds.is_empty() => {
                                    result = Err(hirsel_resolve::LookupError::Provider(ds));
                                }
                                _ => {}
                            }
                        }
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
                .or_else(|| self.candidates.get(&id).copied())
                .unwrap_or_else(memos::no_candidates),
            ReadKey::Environment => self.environment,
            ReadKey::ValidationView(id) => self
                .snapshot
                .validation(package)
                .view(id)
                .map_or_else(ContentHash::default, |input| input.digest().0),
            ReadKey::ValidationUniverse(package) => {
                self.snapshot.validation_input_digest(&package.id())
            }
            ReadKey::UniverseErrors => self.snapshot.validation(package).universe_errors().0,
            ReadKey::FirstHeap => self
                .snapshot
                .validation(package)
                .first_heap()
                .map_or_else(ContentHash::default, |(id, _)| id.path().fingerprint()),
            // A front product must not execute a dependency to decide a hit.
            // Unknown source/registration facts safely miss this ordinary codec.
            ReadKey::NameSet(package, module) => self.declarations.name_set(package, module),
            ReadKey::ExecutedBody(_) | ReadKey::LayoutBody(_) => {
                return None;
            }
        })
    }
    /// Whether every body settled so far checked without an error.
    pub fn settled_clean(&self) -> bool {
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .all(|outcome| outcome.status.is_ok())
    }
    /// Memo rows adopted through unchanged input subscriptions.
    pub fn clean_memos(&self) -> BTreeSet<crate::memo::MemoKey> {
        self.clean_memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    /// Actual completed phase reads. This inspection executes no query.
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
            .map(|((path, kind), count)| (path.path().clone(), *kind, *count))
            .collect()
    }
    pub fn recompute_count(&self, definition: &DefPath, kind: QueryKind) -> u64 {
        self.work
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(DefId::intern(definition), kind))
            .copied()
            .unwrap_or(0)
    }
    /// Discovery consumes only an actual settled product from this admitted view.
    /// Check own text separately: the read set need not contain its owner's body.
    pub(crate) fn instance_discovery_summary(
        self: &Arc<Self>,
        engine: &mut QueryEngine,
        path: &DefPath,
    ) -> Result<Option<FrontCheckedOutput>, QueryError> {
        let Some(input) = self
            .inputs
            .values()
            .find(|input| input.definition.path() == path)
        else {
            return Ok(None);
        };
        let Some(output) = self.checked_definition(path) else {
            return Ok(None);
        };
        if !Arc::ptr_eq(&self.types, &engine.types)
            || engine.key_text(path, QueryKind::Modes)? != input.text
            || output
                .reads
                .first_changed(|key| engine.observe(path, key))
                .is_some()
        {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "generated instance discovery".into(),
                message: format!("stale front summary for {path}"),
            }));
        }
        if let Some(accepted) = &input.registration {
            accepted.validate(engine)?;
        }
        Ok(Some(output))
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
        let address = *index_value(&self.by_definition, &DefId::get(definition)?)?;
        Some(QueryId {
            kind: FRONT_BODY_KIND,
            address: hirsel_artifact_store::Address(address),
        })
    }
    /// A separately admitted body materialization after the compact P settles.
    /// This address is demanded only when a body consumer lost its archive.
    pub fn body_query(&self, definition: &DefPath) -> Option<QueryId> {
        let query = self.query(definition)?;
        Some(QueryId {
            kind: FRONT_BODY_KIND,
            address: hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.body-demand.v1",
                [query.address.0.0.as_slice()],
            )),
        })
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    /// Inspect compact reads without decoding the checked body.
    pub(crate) fn selected_reads(&self, reference: Reference) -> Option<Arc<Reads>> {
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|outcome| outcome.reference == reference)
            .map(|outcome| outcome.reads.clone())
    }
    /// Settled Check inputs own no analyzed body and never read its archive.
    pub(crate) fn check_facts(&self, reference: Reference) -> Option<FrontCheckFacts> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let selected = outcomes
            .values()
            .find(|outcome| outcome.reference == reference)?;
        let source_rules = selected.summary.as_ref().is_some_and(|summary| {
            let summary = summary.materialize(&selected.checked, &selected.reads);
            summary
                .summary
                .calls
                .iter()
                .any(|call| call.callee.path().is_core("region.with"))
        });
        Some(FrontCheckFacts {
            result: selected
                .status
                .clone()
                .map(|()| selected.diagnostics.clone()),
            phase: selected.phase,
            reads: selected.reads.clone(),
            source_rules,
            lowering: selected.lowering.clone(),
        })
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
                    let mut restored = persistent::restore_frozen_mode(
                        &self.records,
                        &self.types,
                        self.syntax(input)?,
                        &self.citations,
                        input.key.1,
                        &bytes,
                        selected.reads.clone(),
                        None,
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
                definition: selected.definition.path().clone(),
                result,
                phase: selected.phase,
                summary: selected
                    .summary
                    .as_ref()
                    .map(|summary| summary.materialize(&selected.checked, &selected.reads)),
                reads: selected.reads.clone(),
                diagnostic_phases: selected.diagnostic_phases.clone(),
            },
            _snapshot: self.clone(),
        })
    }
    pub(crate) fn registration_handle(
        &self,
        path: &DefPath,
    ) -> Option<&Arc<AcceptedFrontRegistration>> {
        self.inputs
            .get(index_value(&self.by_definition, &DefId::get(path)?)?)?
            .registration
            .as_ref()
    }

    /// Immutable ordinary input transport. Decoding starts only on demand;
    /// this performs no compiler phase or namespace publication.
    pub(crate) fn native_input(
        &self,
        path: &DefPath,
    ) -> Option<(
        Arc<ItemSyntax>,
        Arc<QueryOutput<hirsel_types::SignatureRef>>,
    )> {
        let input = self
            .inputs
            .values()
            .find(|input| input.definition.path() == path)?;
        Some((self.syntax(input)?, input.signature.as_ref().ok()?.clone()))
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
    pub(crate) fn registration(&self, path: &DefPath) -> Option<&AcceptedFrontRegistration> {
        self.registration_handle(path).map(Arc::as_ref)
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
// The address maps are frozen at capture. Sorted slices keep their exact
// lookup keys without retaining insertion capacity or B-tree nodes.
fn index_value<'a, K: Ord, V>(rows: &'a [(K, V)], key: &K) -> Option<&'a V> {
    let at = rows
        .binary_search_by(|(candidate, _)| candidate.cmp(key))
        .ok()?;
    Some(&rows[at].1)
}

struct Factory(Arc<FrontBodyQueries>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if query.kind != FRONT_BODY_KIND {
            return Err(self.0.errors.protocol);
        }
        let (key, input, materialize) = if let Some(input) = self.0.inputs.get(&query.address.0) {
            (query.address.0, input, false)
        } else {
            let key = *index_value(&self.0.body_demands, &query.address.0)
                .ok_or(self.0.errors.protocol)?;
            (key, &self.0.inputs[&key], true)
        };
        Ok(Box::new(BodyTask {
            queries: self.0.clone(),
            query,
            input: key,
            materialize,
            weight: input.weight,
            terminal: None,
        }))
    }
}
struct BodyTask {
    queries: Arc<FrontBodyQueries>,
    query: QueryId,
    input: Key,
    materialize: bool,
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
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        if self.materialize {
            let parent = QueryId {
                kind: FRONT_BODY_KIND,
                address: hirsel_artifact_store::Address(self.input),
            };
            match cx.read(parent) {
                std::task::Poll::Pending => return QueryPoll::Waiting(vec![parent]),
                std::task::Poll::Ready(Err(error)) => {
                    self.terminal = Some(Err(error));
                    return finish(Err(error));
                }
                std::task::Poll::Ready(Ok(product)) => {
                    if self.queries.outcome(product.record).is_some() {
                        let result = Ok(ProductRef {
                            address: self.query.address,
                            ..product
                        });
                        self.terminal = Some(result);
                        return finish(result);
                    }
                }
            }
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
        let input = queries.inputs.get(&self.input)?;
        let mut entry = crate::store::Entry::load(&*queries.store, &input.key);
        if entry.variants.is_empty() {
            return None;
        }
        queries.memo_operations.fetch_add(1, Relaxed);
        entry
            .variants
            .retain(|variant| variant.inputs == input.key.1 && variant.valid(&input.key));
        let Some(newest) = entry.variants.first() else {
            queries.text_refusals.fetch_add(1, Relaxed);
            return None;
        };
        let observe = |read: &ReadKey| queries.observe(input.definition.path(), read);
        let Some(variant) = entry.hit(observe) else {
            if let Some(read) = newest.reads.first_changed(observe) {
                queries.refusals[read.kind() as usize].fetch_add(1, Relaxed);
            }
            return None;
        };
        if variant.output.len > self.weight.scratch {
            return None;
        }
        let bytes = queries.store.get_content(variant.output)?;
        let mut restored = persistent::restore_frozen_mode(
            &queries.records,
            &queries.types,
            queries.syntax(input)?,
            &queries.citations,
            input.key.1,
            &bytes,
            Arc::new(variant.reads.clone()),
            queries
                .snapshot
                .signature(input.definition)
                .and_then(Result::ok)
                .map(|signature| &**signature),
        )?;
        let definition = input.definition;
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
        queries.retain_dead_arms(restored.dead_arms.as_mut())?;
        if let Some(table_keys) = restored.table_keys {
            queries
                .table_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.input, table_keys);
        }
        let (outcome, checked) = (restored.outcome, restored.checked);
        let failed = outcome.result.is_err();
        queries
            .memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(memo_rows.iter().cloned());
        queries
            .memo_operations
            .fetch_add(memo_rows.len() as u64, Relaxed);
        queries
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                self.input,
                FrontOutcome::capture(variant.output, outcome, checked, restored.dead_arms),
            );
        queries.store_adoptions.fetch_add(1, Relaxed);
        timings::count(Count::StoreHits);
        timings::count(Count::StoreModeHits);
        if !failed {
            self.remember_carry(variant.output);
        }
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
        let _read_scope = self.queries.store.read_scope();
        let _publication = self.queries.store.publication_scope();
        if !self.materialize
            && let Some(product) = self.queries.completed_product(self.query)
        {
            return product;
        }
        if !self.materialize && self.queries.verify_store == crate::store::VerifyStore::Off {
            if let Some(hit) = self.load_carry() {
                return hit;
            }
            if let Some(hit) = self.load_compact() {
                return hit;
            }
            if let Some(hit) = self.load() {
                return hit;
            }
            timings::count(Count::StoreMisses);
            timings::count(Count::StoreModeMisses);
        }
        let queries = &self.queries;
        let input = &queries.inputs[&self.input];
        let expected = if queries.verify_store == crate::store::VerifyStore::Recompute {
            let mut entry = crate::store::Entry::load(&*queries.store, &input.key);
            entry.variants.retain(|variant| {
                variant.inputs == input.key.1
                    && variant.valid(&input.key)
                    && variant.output.len <= self.weight.scratch
            });
            entry
                .hit(|read| queries.observe(input.definition.path(), read))
                .and_then(|variant| {
                    queries
                        .store
                        .get_content(variant.output)
                        .map(|bytes| (variant.reads.clone(), bytes))
                })
        } else {
            None
        };
        if !self.materialize
            && queries.verify_store == crate::store::VerifyStore::Recompute
            && expected.is_none()
        {
            timings::count(Count::StoreMisses);
            timings::count(Count::StoreModeMisses);
        }
        let worker = queries.worker();
        let syntax = queries.syntax(input).ok_or(queries.errors.protocol)?;
        let mut done = worker.run(Task {
            index: 0,
            definition: input.definition.path().clone(),
            syntax,
            signature: input.signature.clone(),
            signature_reads: input.signature_reads.clone(),
            text: input.text,
            resolution: None,
            checked: None,
        });
        if let Some(registration) = &input.registration {
            registration.explain(&mut done.result, &queries.citations.sources);
            if let Some(Product::Computed(stage)) = &mut done.resolution {
                registration.explain(&mut stage.result, &queries.citations.sources);
            }
            if let Some(Product::Computed(stage)) = &mut done.checked {
                registration.explain(&mut stage.result, &queries.citations.sources);
            }
            if let Some(Product::Computed(stage)) = &mut done.modes {
                registration.explain(&mut stage.result, &queries.citations.sources);
            }
        }
        for kind in done.recomputed() {
            *queries
                .work
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry((DefId::intern(&done.definition), kind))
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
            candidates: queries
                .candidates
                .get(&DefId::intern(&done.definition))
                .copied(),
            environment: queries.environment,
        };
        let mut completed = CompactCompleted::capture(
            done,
            &worker,
            queries.types.clone(),
            &mut source,
            Some(&queries.records),
        );
        let memo_rows = completed.memo_snapshot();
        let checked = completed.checked_product();
        let mut dead_arms = completed.take_dead_arms();
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
        if self.materialize {
            // The accepted compact P is immutable. A body demand may only
            // replenish exactly its original archive; it never replaces facts
            // or debits a second retained copy of their metadata.
            let (reference, reads) = {
                let outcomes = queries.outcomes.lock().unwrap_or_else(|e| e.into_inner());
                let old = outcomes.get(&self.input).ok_or(queries.errors.protocol)?;
                if failed || old.status.is_err() || old.reads.as_ref() != outcome.reads.as_ref() {
                    return Err(queries.errors.protocol);
                }
                (old.reference, old.reads.clone())
            };
            if reference.domain.reference(&bytes) != Some(reference) {
                return Err(queries.errors.protocol);
            }
            if queries.store.put_content(reference.domain, &bytes) != Some(reference) {
                return Err(queries.errors.resource);
            }
            let action = crate::store::query_action(&input.key, input.key.1, &reads);
            queries
                .store
                .associate_product(self.query.address, action, reference);
            return Ok(ProductRef {
                address: self.query.address,
                action,
                record: reference,
            });
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
                            input.definition.path(),
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
                    definition: input.definition.path().clone(),
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
                        self.input,
                        FrontOutcome::capture(record, output, None, None),
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
            .retain_dead_arms(dead_arms.as_mut())
            .ok_or(queries.errors.resource)?;
        queries
            .memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(memo_rows.iter().cloned());
        queries
            .memo_operations
            .fetch_add(memo_rows.len() as u64, Relaxed);
        if let Some(keys) = table_keys {
            queries
                .table_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.input, keys);
        }
        queries
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                self.input,
                FrontOutcome::capture(record, outcome, checked, dead_arms),
            );
        if !failed {
            self.remember_carry(record);
        }
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
pub(in crate::queries) struct SpecializationDependencies {
    pub(in crate::queries) text: ContentHash,
    pub(in crate::queries) reads: Arc<Reads>,
    pub(in crate::queries) paths: Vec<DefPath>,
}
impl QueryEngine {
    fn specialization_dependencies(&mut self, definition: &DefPath) -> Option<Vec<DefPath>> {
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Resolve).ok()?;
        if let Some(kept) = self.specialization_dependencies.remove(&id) {
            let valid = kept.text == text
                && kept
                    .reads
                    .first_changed(|read| self.observe(definition, read))
                    .is_none();
            let paths = valid.then(|| kept.paths.clone());
            self.specialization_dependencies.insert(id, kept);
            if paths.is_some() {
                return paths;
            }
        }
        let output = self.resolve_query(definition).ok()?;
        let paths: Vec<_> = output
            .value
            .dependencies
            .iter()
            .filter(|path| hirsel_types::specialization_parts(path).is_some())
            .cloned()
            .collect();
        if !has_errors(&output.diagnostics)
            && let Some(entry) = self.resolutions.get(&id)
            && entry.consistent
            && !self.graph.is_dirty(&entry.memo.key)
        {
            self.specialization_dependencies.insert(
                id,
                SpecializationDependencies {
                    text,
                    reads: Arc::new(entry.reads().flat_copy()),
                    paths: paths.clone(),
                },
            );
        }
        Some(paths)
    }
    /// The accepted registration view of closed `with` clauses (GEN-6): every
    /// specialization a selected body names, and those its own body closes,
    /// is installed before the immutable signature snapshot is captured, and
    /// each becomes a required source root. Callers register before selecting
    /// their roots. Only resolution runs here; a body that fails to resolve or names an
    /// unavailable specialization reports it from its own worker outcome.
    pub fn register_specializations(&mut self, definitions: &[DefPath]) {
        let mut pending = definitions.to_vec();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(definition) = pending.pop() {
            if !seen.insert(definition.clone()) {
                continue;
            }
            // A source carry can own an exact selection receipt before its
            // expression arenas are materialized. Validate that receipt below;
            // absent With syntax in a body-free head cannot invalidate it.
            if !self
                .specialization_dependencies
                .contains_key(&DefId::intern(&definition))
                && !self
                    .snapshot(&definition)
                    .is_ok_and(|syntax| crate::families::closes(&syntax))
            {
                continue;
            }
            let Some(dependencies) = self.specialization_dependencies(&definition) else {
                continue;
            };
            for dependency in dependencies {
                if self.materialize(&dependency).is_ok() {
                    pending.push(dependency);
                }
            }
        }
    }
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
        self.front_body_queries_with_registrations(definitions, &[], weight, allowance, errors)
    }
    /// Freeze original bodies together with explicitly accepted generated views.
    pub fn front_body_queries_with_registrations(
        &mut self,
        definitions: &[DefPath],
        registrations: &[Arc<AcceptedFrontRegistration>],
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
        self.register_specializations(definitions);
        // Inference can materialize siblings and change private header facts.
        // Settle them before capturing any immutable syntax, keys or receipts.
        self.prepare_private_summaries();
        let anchors = Arc::new(crate::NativeRecordContext::capture(self));
        let input_records = crate::NativeRecords::new(self.types.clone(), store.clone());
        let declarations = self.frozen_declarations();
        let mut siblings = BTreeMap::new();
        let sibling_inputs = self.front_sibling_inputs(definitions)?;
        let mut sibling_index = crate::NativeMetadataWeight::default();
        sibling_index.tree::<FileId, Vec<Reference>>(sibling_inputs.len());
        required = required
            .checked_add(sibling_index.bytes())
            .ok_or_else(refusal)?;
        for (file, original) in sibling_inputs {
            if let std::collections::btree_map::Entry::Vacant(slot) = siblings.entry(file) {
                let written = declarations.siblings(&original);
                required = required
                    .checked_add(
                        (written.len() as u64)
                            .checked_mul(std::mem::size_of::<Reference>() as u64)
                            .ok_or_else(refusal)?,
                    )
                    .ok_or_else(refusal)?;
                if required > allowance.bytes() {
                    return Err(refusal());
                }
                let mut refs = Vec::with_capacity(written.len());
                for sibling in written.iter() {
                    // Item resolution reads variant spellings from siblings.
                    // Retain written callable/constant fallback whenever the
                    // provider does not already have the corresponding kind.
                    let written_value = matches!(
                        sibling.item.kind,
                        hirsel_syntax::ItemKind::Function { .. }
                            | hirsel_syntax::ItemKind::Const { .. }
                    );
                    let normalized_value = self
                        .signatures
                        .get(&DefId::intern(&sibling.item.definition))
                        .filter(|entry| entry.consistent && !self.graph.is_dirty(&entry.memo.key))
                        .and_then(|entry| entry.result().ok())
                        .is_some_and(|output| {
                            matches!(
                                output.value.kind,
                                hirsel_types::SignatureKind::Function(_)
                                    | hirsel_types::SignatureKind::Const { .. }
                            )
                        });
                    let variant = matches!(
                        &sibling.item.kind,
                        hirsel_syntax::ItemKind::Type {
                            body: hirsel_syntax::TypeBody::Variants(_),
                            ..
                        }
                    );
                    if !variant && (!written_value || normalized_value) {
                        continue;
                    }
                    let bytes = input_records
                        .encode(&*anchors, &(sibling, sibling.occurrence))
                        .ok_or_else(refusal)?;
                    refs.push(
                        store
                            .put_content(SOURCE_INPUT, &bytes)
                            .ok_or_else(refusal)?,
                    );
                }
                slot.insert(refs);
            }
        }
        let mut selected = Vec::new();
        for definition in definitions {
            let registration = registrations
                .iter()
                .find(|accepted| accepted.definition() == definition);
            if let Some(accepted) = registration {
                accepted.validate(self)?;
            }
            self.materialize(definition)?;
            let original = self.snapshot(definition)?;
            let declaration = generation::instance_bodied(&original);
            let syntax = if declaration {
                if registration.is_some() {
                    return Err(refusal());
                }
                // As in declaration_check_query, a generic generated
                // declaration checks its header. Only closed instances have
                // executable generated bodies, each with an accepted view.
                let mut header = (*original).clone();
                if let hirsel_syntax::ItemKind::Function { body, .. } = &mut header.item.kind {
                    *body = None;
                }
                Arc::new(header)
            } else if generation::generator(&original).is_some() {
                registration.ok_or_else(refusal)?.syntax.clone()
            } else {
                if registration.is_some() {
                    return Err(refusal());
                }
                original
            };
            let implementation = syntax.text_hash;
            let text = self.key_text(definition, QueryKind::Modes)?;
            let key = match registration {
                Some(accepted) => self.accepted_front_store_key(accepted, QueryKind::Modes)?,
                None if declaration => {
                    self.front_declaration_store_key(definition, QueryKind::Modes)?
                }
                None => self
                    .store_key(definition, QueryKind::Modes)
                    .ok_or_else(refusal)?,
            };
            // Immutable S0 variants provide body reuse. The construction
            // inventory itself must never pin a mutable engine's checked body
            // after a request settles, including the explicit zero hold policy.
            let bytes = input_records
                .encode(&*anchors, &(&syntax, syntax.occurrence))
                .ok_or_else(|| {
                    QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "front syntax archive".into(),
                        message: format!("cannot archive accepted source for {definition}"),
                    })
                })?;
            if required > allowance.bytes() {
                return Err(refusal());
            }
            let syntax = store
                .put_content(SOURCE_INPUT, &bytes)
                .ok_or_else(refusal)?;
            selected.push((
                definition.clone(),
                syntax,
                text,
                implementation,
                key,
                registration.cloned(),
            ));
        }
        let snapshot = self.signature_inputs();
        let mut inputs = BTreeMap::new();
        for (definition, syntax, text, implementation, key, registration) in selected {
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
            let mut signature_reads = ReadsBuilder::new();
            if let Some(reads) = self.shared_memo_reads(&definition, QueryKind::Signature) {
                signature_reads.merge(&reads);
            }
            if let Some(accepted) = &registration {
                signature_reads.merge(&accepted.reads);
            }
            inputs.insert(
                key.0,
                Input {
                    signature,
                    signature_reads: Arc::new(signature_reads.finish().map_err(|read| {
                        QueryError::Operational(hirsel_types::OperationalFailure {
                            operation: "front inherited reads".into(),
                            message: format!("inconsistent registration read: {read:?}"),
                        })
                    })?),
                    registration,
                    definition: id,
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
                    input.definition,
                    self.candidates_value(input.definition.path()),
                )
            })
            .collect();
        let mut by_definition = inputs
            .iter()
            .map(|(&key, input)| (input.definition, key))
            .collect::<Vec<_>>();
        by_definition.sort_unstable_by_key(|&(definition, _)| definition);
        let mut body_demands = inputs
            .keys()
            .map(|&key| {
                (
                    ContentHash::of_parts("hirsel.front.body-demand.v1", [key.0.as_slice()]),
                    key,
                )
            })
            .collect::<Vec<_>>();
        body_demands.sort_unstable_by_key(|&(demand, _)| demand);
        Ok(Arc::new(FrontBodyQueries {
            inputs,
            by_definition: by_definition.into_boxed_slice(),
            body_demands: body_demands.into_boxed_slice(),
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
                unique_failures: self.private_unique_failures.clone(),
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
            carry: Mutex::new(None),
            carry_changes: Mutex::new(None),
            clean_memos: Mutex::new(BTreeSet::new()),
            work: Mutex::new(BTreeMap::new()),
            table_keys: Mutex::new(BTreeMap::new()),
            memos: Mutex::new(BTreeMap::new()),
            store_adoptions: AtomicU64::new(0),
            refusals: Default::default(),
            text_refusals: AtomicU64::new(0),
            memo_operations: AtomicU64::new(0),
            verify_store: self.verify_store,
            errors,
            retention: Mutex::new(allowance.bytes() - required),
            _allowance: allowance,
        }))
    }
}
