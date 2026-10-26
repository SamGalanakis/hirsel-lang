use crate::memo::{MemoGraph, MemoKey, ReadKey, Reads, ReadsBuilder};
use crate::*;
mod candidate_products;
mod checked_signatures;
pub use checked_signatures::FrontCheckedSignatures;
pub(crate) mod catalog;
pub use candidate_products::{CheckedProductEncoder, CheckedProducts};
mod arm_fixes;
mod frontier;
mod source_index;
pub use source_index::SourceIndexState;
pub(crate) mod recipes;
pub use frontier::FrontChanges;
mod parallel;
pub(crate) mod s1;
mod scheduled_check;
mod scheduled_native;
mod scheduled_native_root;
mod scheduled_run;
pub use scheduled_native::{
    FRONT_NATIVE_DEFINITION_KIND, FrontNativeDefinitionInputs, FrontNativeDefinitionOutput,
    FrontNativeDefinitionQueries,
};
pub use scheduled_native_root::{
    FRONT_NATIVE_ROOT_KIND, FrontNativeOutput, FrontNativePlan, FrontNativeProducts,
    FrontNativeQueries,
};
pub use scheduled_run::{
    FRONT_RUN_KIND, FrontBuiltProof, FrontPreparedProof, FrontProofExport, FrontProofInput,
    FrontProofText, FrontRunInputs, FrontRunOutput, FrontRunQueries,
};

mod scheduled_storage;
pub use scheduled_storage::{
    FRONT_STORAGE_GRAPH_KIND, FrontStorageGraphOutput, FrontStorageGraphQueries,
};
mod scheduled_lower;
mod scheduled_stage;
pub use scheduled_check::{
    FRONT_CHECK_KIND, FrontCheckOutput, FrontCheckPlan, FrontCheckQueries, FrontCheckScope,
    FrontCheckedInput,
};
pub use scheduled_stage::{FRONT_STAGE_KIND, FrontStageOutput, FrontStageQueries};
mod scheduled_growth;
pub use generation::{
    AcceptedFrontRegistration, FrontInstanceBootstrap, FrontInstanceBootstrapAdmission,
    FrontInstanceBootstrapError, FrontInstanceDiscovery, FrontInstanceRequest, FrontInstanceSyntax,
    FrontRegistrationBootstrap, FrontRegistrationBootstrapAdmission,
    FrontRegistrationBootstrapError, FrontRegistrationFailure,
};
pub use parallel::{
    FRONT_BODY_KIND, FRONT_STORAGE_KIND, FrontBodyOutput, FrontBodyQueries, FrontCarry,
    FrontCheckedOutput, FrontInputFootprint, FrontStorageOutput, FrontStorageQueries, ModeResult,
    PublishedDefinition, PublishedResult,
};
pub use s1::{LoweringResult, S1LowerWork};
pub use scheduled_growth::{FRONT_GROWTH_KIND, FrontGrowthOutput, FrontGrowthQueries};
pub use scheduled_lower::{
    DefinitionDecodeLayout, FRONT_EVALUATE_KIND, FRONT_LOWER_KIND, FRONT_REGISTRATION_KIND,
    FrontLowerOutput, FrontLowerQueries, FrontLowerWork, FrontRegistrationOutput,
    FrontRegistrationQueries, LowerCarry,
};
/// Engine-owned complete canonical input. Only admitted source wrappers
/// implement this lifetime/size seam; it supplies no validation authority.
pub(crate) trait AdmittedProofInput {
    fn export_input(&self) -> &hirsel_proof_export::ExportInput;
    fn export_module_bytes(&self) -> usize;
    fn export_clone_bytes(&self) -> u64;
    fn into_owned(self: std::rc::Rc<Self>) -> Result<OwnedProofInput, QueryError>;
    fn checked_receipt(&self) -> Option<(&FrontCheckScope, &FrontCheckedSignatures)> {
        None
    }
}
/// Source owners and the original physical input grant move with an export.
/// This private seam cannot be implemented by an artifact consumer.
pub(crate) struct OwnedProofInput {
    input: hirsel_proof_export::ExportInput,
    source: OwnedProofSource,
    allowance: crate::runtime::MemoryReservation,
}
pub(crate) struct OwnedProofSource {
    _proof: Option<Arc<FrontProofExport>>,
    checked: Option<(Arc<FrontCheckScope>, Arc<FrontCheckedSignatures>)>,
}
/// One S1 root's captured outcome. Phase, tooling limit and complete execution
/// reads belong to this root, including when another root finished afterward.
#[derive(Clone, Debug)]
pub struct MaterializationResult {
    pub definition: DefPath,
    pub result: Result<hirsel_tir::Materialized, QueryError>,
    pub phase: Option<QueryKind>,
    pub limit: Option<hirsel_tir::ToolingLimit>,
    pub reads: Arc<Reads>,
}

mod dead_arms;
mod definition_hash;
pub use definition_hash::DefinitionHashes;
mod dependencies;
pub(crate) mod generation;
pub use generation::GeneratedSource;
pub(crate) mod lowering;
pub(crate) mod memos;
pub(crate) use memos::{Entry, Observed, Table};
mod program_revision;
pub use program_revision::ForeignUse;
pub(crate) use program_revision::SemanticProgram;
pub(crate) mod native_discovery;
pub(crate) mod native_records;
mod persistent;
mod retention;
mod safety;
mod summaries;
pub use safety::{Escape, UnsafeReach};
mod private_unique;
mod type_groups;
use hirsel_diag::Severity;
use hirsel_intern::timings::{self, Count, Interval, Phase};
use hirsel_span::Span;
use hirsel_syntax::{DefId, ItemKind, ItemPath, ItemSyntax};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Signature normalization only reads the declared header and its contracts (LANG-6).
/// Revision-1 construction seam; the driver publishes its owned output as SignatureRef.
pub trait SignaturePhase: Send + Sync {
    fn signature_items(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        items: &[Arc<ItemSyntax>],
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let syntax = items.first().ok_or(QueryError::NotImplemented)?;
        self.signature(t, m, syntax.clone(), declarations, products, provider)
    }
    fn signature(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError>;
    /// A recursive type group member's provisional signature at `stage`,
    /// which another member reads; it publishes nothing.
    fn staged_signature(
        &self,
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        _items: &[Arc<ItemSyntax>],
        _declarations: &Declarations,
        _products: &mut PhaseProducts,
        _provider: &mut dyn SignatureProvider,
        _stage: hirsel_types::GroupStage,
    ) -> Result<Signature, QueryError> {
        Err(QueryError::NotImplemented)
    }
}

/// Optional adapter slots support deliberately partial sessions in tooling tests.
/// Installed leaf adapters may also return unavailable or operational errors.
#[derive(Default)]
pub struct PhaseAdapters {
    pub lexer: Option<Arc<dyn LexerPhase>>,
    pub parser: Option<Arc<dyn ParserPhase>>,
    pub formatter: Option<Arc<dyn FormatterPhase>>,
    pub resolver: Option<Arc<dyn ResolverPhase>>,
    pub signature: Option<Arc<dyn SignaturePhase>>,
    pub checker: Option<Arc<dyn CheckerPhase>>,
    pub modes: Option<Arc<dyn ModesPhase>>,
    pub lowering: Option<Arc<dyn LoweringPhase>>,
    pub interpreter: Option<Box<dyn InterpreterPhase>>,
    /// The front phases a complete compiler was built from (`PhaseAdapters::compiler`).
    pub phases: Option<Arc<dyn Phases>>,
}

/// Reads survive errors and missing phases, rather than being lost in QueryError.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct DependencyReads {
    pub operational_failure: Option<hirsel_types::OperationalFailure>,
    pub signatures: Vec<SignatureDependency>,
    pub failed_signatures: Vec<DefPath>,
    pub failed_signature_keys: Vec<(DefPath, Option<ContentHash>)>,
    pub lookups: Vec<(DefPath, bool)>,
    pub manifest_lookups: Vec<hirsel_types::DependencyRead>,
    pub resolver: Arc<hirsel_types::SharedPath<hirsel_resolve::DependencyRead>>,
    /// Other items' name sites a result cites, as read (`cite_name`).
    pub citations: Vec<(DefPath, Option<Span>)>,
}

/// A file's parse result under its package's query environment.
pub(crate) type ParsedFile = (
    FileContext,
    QueryEnvironment,
    Result<QueryOutput<SyntaxFile>, QueryError>,
);
/// A single coherent session. Items are item-local syntax at anchors: moving an
/// item moves its anchor, never its IDs, spans or cached results.
pub struct QueryEngine {
    pub(crate) work: crate::WorkStats,
    pub(crate) checked_scope: candidate_products::CheckedScope,
    pub(crate) store_identity: ContentHash,
    store_adoptions: u64,
    pub(crate) verify_store: crate::store::VerifyStore,
    pending_store_checks: BTreeMap<(DefId, QueryKind), persistent::Expected>,
    store_check_spool: std::sync::Weak<persistent::Spool>,
    pub(crate) result_store: Option<Arc<dyn crate::store::ResultStore>>,
    /// Successful indexing contexts survive source-only carry independently
    /// of parse product/body tables. FileId identifies an immutable snapshot.
    pub(crate) indexed_sources: BTreeMap<FileId, (FileContext, QueryEnvironment)>,
    /// Whether V validates what this session lowers; `run_v` is its only reader.
    pub verify: Verify,
    pub(crate) retention: crate::memo::Retention,
    pub(crate) hold_bytes: u64,
    pub(crate) configured_jobs: Option<hirsel_intern::jobs::Jobs>,
    pub(crate) configured_lowering: LoweringPolicy,
    /// A `Compiler` opened this engine as a session (`free_sessions`).
    pub(crate) session: bool,
    /// Image construction requires closure-local evidence. Until worker V
    /// receipts are closure-qualified, its source materializations stay scalar.
    pub comptime_limits: crate::ComptimeLimits,
    pub last_comptime_limit: Option<hirsel_tir::ToolingLimit>,
    /// This engine's S1 stack: the open evaluations, generators and staged
    /// instances with their requests' depth evidence, claimed through the
    /// session's coordinator (design §1c).
    pub(crate) s1: crate::memo::S1Worker<DefPath, Span>,
    pub(crate) constant_values: BTreeMap<DefPath, crate::comptime::EvaluatedConstant>,
    materialization_environment: Option<(QueryEnvironment, crate::ComptimeLimits, Verify)>,
    pub(crate) staged_constants: BTreeMap<DefPath, crate::comptime::StagedConstant>,
    pub(crate) staged_values: BTreeMap<(DefPath, Vec<u64>), crate::comptime::EvaluatedConstant>,
    /// Generated function bodies (CT-5), by declaration.
    generated: BTreeMap<DefPath, GeneratedSource>,
    /// Selected instances of generic generated declarations (CT-5).
    pub(crate) instance_bodies: hirsel_tir::InstanceBodies,
    pub sources: SourceMap,
    pub environment: QueryEnvironment,
    pub adapters: PhaseAdapters,
    /// Each definition's candidates in occurrence order.
    pub(crate) items: BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    /// The items the last index dropped, which the next can adopt.
    detached: Vec<Arc<ItemSyntax>>,
    parsed: BTreeMap<FileId, ParsedFile>,
    pub(crate) registry: crate::registry::BuiltinRegistry,
    pub(crate) builtins: BTreeMap<DefPath, hirsel_types::SignatureRef>,
    syntax_keys: BTreeMap<(DefPath, QueryKind), ContentHash>,
    signatures: Table<hirsel_types::SignatureRef>,
    resolutions: Table<Resolution>,
    checked: Table<TypedBody>,
    modes: Table<AnalyzedBody>,
    lowered: Table<hirsel_tir::DefinitionRef>,
    lower_receipts: hirsel_syntax::DefMap<Arc<retention::LowerReceipt>>,
    lower_holds: crate::memo::Holds<()>,
    pub(crate) summaries: Table<Arc<PublishedSummary>>,
    pub(crate) checked_summaries: Table<Arc<CheckedSummary>>,
    portable_type_records: std::sync::Mutex<
        Option<(
            Arc<dyn crate::store::ResultStore>,
            Arc<persistent::TypeRecords>,
        )>,
    >,
    /// The memo records, the reverse subscriber index and the dirty marks.
    pub(crate) graph: MemoGraph,
    front_changes: frontier::FrontTracker,
    pub(crate) front_products: crate::tools::FrontProducts,
    /// Each indexed definition's inputs as last indexed (S0's diff).
    inputs: hirsel_syntax::DefMap<memos::DefInputs>,
    /// The environment and limits the memos were last checked under.
    environment_seen: Option<(QueryEnvironment, crate::ComptimeLimits, Option<ContentHash>)>,
    /// The manifest revision indexing last published; any other value was
    /// set explicitly by the caller (its build inputs changed).
    published_manifest: Option<ContentHash>,
    /// Detached definitions whose readers were told they are gone.
    vanished: BTreeSet<DefId>,
    /// Every indexed item, by file and by definition.
    pub(crate) declarations: crate::snapshot::DeclarationIndex,
    /// Dependency packages key on their own closure's manifests, not the whole
    /// graph's, so their queries are shared across programs (package images).
    pub(crate) package_manifests: BTreeMap<hirsel_syntax::PackageId, ContentHash>,
    /// The selected `unsafe ffi` packages' link inputs, in package order (PROG-4).
    pub foreign_links: Vec<crate::LinkInput>,
    /// The checked entry reaches an extern outside a deterministic wrapper, so its
    /// executables refuse to record or replay (PROG-4).
    pub foreign_unreplayable: bool,
    /// Each selected `unsafe ffi` package's `threads` (S-13).
    pub foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
    /// Open evaluation frames: each collects the complete reads of what it
    /// executes or materializes (execution closure by union).
    pub(crate) execution_stack: Vec<ReadsBuilder>,
    pub(crate) evaluation_acceptance: Vec<Option<Vec<hirsel_validate::AcceptedArtifact>>>,
    pub(crate) evaluation_selections: Vec<Vec<crate::comptime::Selection>>,
    pub(crate) evaluation_validation: Vec<Option<BTreeMap<hirsel_syntax::PackageId, ContentHash>>>,
    active: BTreeSet<(DefPath, QueryKind)>,
    /// The runtime types compile-time evaluation runs with, and the epoch of
    /// the signature snapshot they were built from.
    comptime_runtime: Option<(
        crate::memo::Revision,
        Arc<hirsel_interp::aggregate::RuntimeTypes>,
    )>,
    pub(crate) source_growth: crate::generic_graph::SourceGrowth,
    /// The last signature snapshot and the inputs it was built from.
    pub(crate) snapshot: Option<(
        crate::snapshot::SnapshotStamp,
        Arc<crate::snapshot::SignatureSnapshot>,
    )>,
    pub(crate) snapshot_epoch: crate::memo::Revision,
    /// Producer identity for rows in the retained frozen snapshot. Weak
    /// references establish provenance without retaining old memo generations.
    pub(crate) snapshot_sources:
        hirsel_syntax::DefMap<std::sync::Weak<Entry<hirsel_types::SignatureRef>>>,
    /// Signatures published or retired outside declaration indexing (S1).
    pub(crate) snapshot_inputs_revision: u64,
    /// The session's one type interner (design §1(a)), over the core rows.
    pub(crate) types: Arc<hirsel_types::TypeInterner>,
    /// The session's structural memo over `types`.
    pub(crate) memo: Arc<hirsel_types::TypeMemo>,
    private_summary_bodies: hirsel_syntax::DefMap<Vec<(DefPath, ContentHash)>>,
    private_summaries: hirsel_syntax::DefMap<hirsel_types::UniqueSummary>,
    private_unique_failures: BTreeMap<DefId, Vec<hirsel_types::UniqueFailure>>,
    private_summary_revision: Option<crate::memo::Revision>,
    private_summary_active: bool,
    signature_stack: Vec<DefPath>,
    recomputes: BTreeMap<(DefPath, QueryKind), usize>,
    scalar_lower_calls: hirsel_syntax::DefMap<u64>,
    scalar_validation_runs: hirsel_syntax::DefMap<u64>,
    s1_lower_work: hirsel_syntax::DefMap<S1LowerWork>,
    catalog: catalog::Catalog,
    pub unavailable: Option<QueryKind>,
    pub last_phase: Option<QueryKind>,
    /// Interpretation refuses to enter a definition outside its entry's
    /// semantic closure (`run_snapshot`, `run_observed_*`). On in builds with
    /// debug assertions; the conformance runner turns it on in every build.
    pub closure_assertions: bool,
}
const _: () = {
    const fn movable<T: Send>() {}
    movable::<QueryEngine>();
    const fn shared<T: Send + Sync>() {}
    shared::<PhaseProducts>();
};
/// Dropping an engine drops its tables as the teardown phase. With a heap
/// meter (`--mem-usage`) it records the bytes each table frees; tables still
/// shared with a package image free nothing here, and the image's bytes are
/// recorded when the package cache is released (`release_shared_caches`).
/// A session the process does not free (`free_sessions`) leaks its tables.
impl Drop for QueryEngine {
    fn drop(&mut self) {
        use hirsel_intern::memory::released;
        use std::mem::take;
        // Scratch resources must close even when process-exit policy leaves
        // ordinary query allocations to the operating system.
        self.pending_store_checks.clear();
        if self.session && !crate::policy::frees_sessions() {
            let vacant = QueryEngine::new(self.environment.clone(), PhaseAdapters::default());
            std::mem::forget(std::mem::replace(self, vacant));
            return;
        }
        let _timing = timings::interval(Interval::Merge);
        released("engine: lowered", take(&mut self.lowered));
        released("engine: lower receipts", take(&mut self.lower_receipts));
        let holds = self.lower_holds.stats();
        timings::count_by(Count::S1LowerHoldEvictions, holds.evictions);
        timings::count_by(Count::S1LowerHoldRecomputes, holds.recomputes);
        timings::count_by(Count::S1LowerHoldPeakBytes, holds.peak_bytes);
        released(
            "engine: lower holds",
            std::mem::replace(&mut self.lower_holds, crate::memo::Holds::new(0)),
        );
        released("engine: modes", take(&mut self.modes));
        released("engine: checked", take(&mut self.checked));
        released("engine: summaries", take(&mut self.summaries));
        released(
            "engine: checked summaries",
            take(&mut self.checked_summaries),
        );
        released("engine: resolutions", take(&mut self.resolutions));
        released("engine: signatures", take(&mut self.signatures));
        released("engine: memo graph", take(&mut self.graph));
        released(
            "engine: pending front changes",
            take(&mut self.front_changes),
        );
        released(
            "engine: compact front products",
            take(&mut self.front_products),
        );
        released("engine: inputs", take(&mut self.inputs));
        released("engine: syntax keys", take(&mut self.syntax_keys));
        released("engine: items", take(&mut self.items));
        released("engine: parsed", take(&mut self.parsed));
        released("engine: signature snapshot", take(&mut self.snapshot));
        released(
            "engine: snapshot row provenance",
            take(&mut self.snapshot_sources),
        );
        released("engine: instance bodies", take(&mut self.instance_bodies));
        released("engine: staged values", take(&mut self.staged_values));
        released("engine: generated", take(&mut self.generated));
        released("engine: recomputes", take(&mut self.recomputes));
        released(
            "engine: scalar lower calls",
            take(&mut self.scalar_lower_calls),
        );
        released(
            "engine: validation work",
            take(&mut self.scalar_validation_runs),
        );
        released("engine: S1 lower work", take(&mut self.s1_lower_work));
        released("engine: constant values", take(&mut self.constant_values));
        released("engine: sources", take(&mut self.sources));
        released("engine: adapters", take(&mut self.adapters));
        released("engine: builtins", take(&mut self.builtins));
    }
}
impl QueryEngine {
    /// The configured product provider shared by front/native/object queries.
    /// Access grants no verifier or native-validator proof authority.
    pub fn product_store(&self) -> Option<Arc<dyn crate::store::ResultStore>> {
        self.result_store.clone()
    }
    /// The session's type interner, which every phase and thread shares.
    pub fn types(&self) -> &Arc<hirsel_types::TypeInterner> {
        &self.types
    }
    /// The session's structural type memo over [`QueryEngine::types`].
    pub fn type_memo(&self) -> &Arc<hirsel_types::TypeMemo> {
        &self.memo
    }
    /// This engine over `types` instead of its own fresh interner, so
    /// sessions that exchange type ids share one table. Only before the
    /// first query: every memo is keyed by ids from one interner.
    pub fn with_types(mut self, types: Arc<hirsel_types::TypeInterner>) -> Self {
        assert!(
            self.graph.is_empty(),
            "an engine changes interner only before its first query"
        );
        self.types = types;
        self.memo = Arc::new(hirsel_types::TypeMemo::new());
        self
    }
}
impl QueryEngine {
    pub(crate) fn prepare_signature_snapshot(&mut self) {
        self.note_environment();
        self.settle_signatures();
        self.prepare_private_summaries();
    }
    pub(crate) fn signature_memo_source(
        &self,
        definition: DefId,
    ) -> Option<&Arc<Entry<hirsel_types::SignatureRef>>> {
        self.signatures.get(&definition)
    }
    /// Indexed source declarations, including derived declaration helpers.
    /// Listing these names does not demand syntax, signatures or bodies.
    pub fn indexed_definitions(&self) -> Vec<DefPath> {
        self.source_paths()
    }
    /// Physical backing of the materialized item index, excluding source text,
    /// header products and session interners. Source-only revision owners add
    /// their independently measured source/header/read-index components.
    /// This visits immutable capacities without parsing or querying a provider.
    pub fn indexed_syntax_bytes(&self) -> Option<u64> {
        let mut metadata = crate::NativeMetadataWeight::default();
        metadata.tree::<DefPath, Vec<Arc<ItemSyntax>>>(self.items.len());
        let mut payload = 0u64;
        for (path, candidates) in &self.items {
            metadata.path(path);
            metadata.flat_vec(candidates);
            for syntax in candidates {
                payload =
                    payload.checked_add(crate::memo::retained::retained_syntax_bytes(syntax))?;
            }
        }
        payload.checked_add(metadata.bytes())
    }
    /// Every indexed source definition.
    pub(crate) fn item_paths(&self) -> Vec<DefPath> {
        self.source_paths()
    }
    /// The source definitions a native export selects: the semantic closure
    /// (`semantic_program`, the closure check-time lowering and the program
    /// revision use) of every definition outside the dependency packages. A
    /// dependency definition the program never reaches is not compiled. An
    /// engine without dependency packages selects every definition.
    pub(crate) fn native_paths(&mut self) -> Result<BTreeSet<DefPath>, QueryError> {
        let roots: Vec<_> = self
            .source_paths()
            .into_iter()
            .filter(|path| !self.package_manifests.contains_key(&path.0.package))
            .collect();
        self.native_paths_from(roots)
    }
    pub(crate) fn native_paths_from(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<BTreeSet<DefPath>, QueryError> {
        let mut selected = self.semantic_program(roots)?.definitions;
        selected.retain(|path| self.declared(path));
        // CT-3: a non-generic compile-time-only function never runs: no
        // run-time code can hold the TypeShape or Body its signature mentions.
        let mut compile_time = vec![];
        for path in &selected {
            let signature = self.item_signature(path)?.value;
            if let hirsel_types::SignatureKind::Function(f) = &signature.kind
                && f.generics.is_empty()
                && f.parameters
                    .iter()
                    .map(|p| p.ty)
                    .chain([f.result])
                    .any(|t| hirsel_types::mentions_compile_time_only(&self.types, t))
            {
                compile_time.push(path.clone());
            }
        }
        for path in compile_time {
            selected.remove(&path);
        }
        Ok(selected)
    }
    pub(crate) fn native_builtin_signatures(
        &self,
    ) -> BTreeMap<DefPath, hirsel_types::SignatureRef> {
        self.registry
            .signatures()
            .map(|s| (s.definition.path().clone(), s.clone()))
            .chain(self.builtins.iter().map(|(p, s)| (p.clone(), s.clone())))
            .collect()
    }
    pub(crate) fn registry_dictionary_value(
        &self,
        path: &DefPath,
    ) -> Result<Option<hirsel_tir::Materialized>, QueryError> {
        if self.definition_items(path).is_ok() {
            return Ok(None);
        }
        self.registry
            .dictionary_value(&self.types, &self.memo, path)
    }
    pub(crate) fn prepare_materialization_environment(&mut self) {
        let now = (self.environment.clone(), self.comptime_limits, self.verify);
        if self.materialization_environment.as_ref() != Some(&now) {
            // A verification-policy transition invalidates executed values,
            // but an unchanged source body with no execution inputs remains
            // the same artifact. Its next demand still runs actual V. Keeping
            // that artifact avoids rebuilding and then separately validating
            // the identical original supplied to native import.
            let unchanged_source: BTreeSet<_> = self
                .materialization_environment
                .as_ref()
                .filter(|(environment, limits, _)| environment == &now.0 && limits == &now.1)
                .into_iter()
                .flat_map(|_| self.lowered.iter())
                .filter(|(id, entry)| {
                    self.source_facts(id.path())
                        .is_some_and(|facts| !facts.execution)
                        && !self.lower_requires_execution(id.path(), entry.reads())
                })
                .map(|(id, _)| *id)
                .collect();
            self.constant_values.clear();
            self.staged_values.clear();
            self.lowered.retain(|id, _| unchanged_source.contains(id));
            self.lower_receipts.clear();
            self.lower_holds.clear();
            self.materialization_environment = Some(now);
        }
    }

    pub(crate) fn validate_comptime(
        &mut self,
        definition: &hirsel_tir::Definition,
        artifact: Signature,
    ) -> Result<(), QueryError> {
        if self.verify == Verify::Off {
            return Ok(());
        }
        let snapshot = self.signature_snapshot();
        crate::comptime::validate_evaluation(&snapshot, definition, artifact, self.verify, None)
            .map(|_| ())
    }
    pub fn new(environment: QueryEnvironment, adapters: PhaseAdapters) -> Self {
        Self::new_with_hold_bytes(
            environment,
            adapters,
            crate::config::configured_hold_bytes(),
        )
    }
    /// Explicit hosts must not consult process defaults even transiently while
    /// constructing an engine whose policy is already captured.
    pub(crate) fn new_with_hold_bytes(
        mut environment: QueryEnvironment,
        adapters: PhaseAdapters,
        hold_bytes: u64,
    ) -> Self {
        let registry = crate::registry::BuiltinRegistry::shared();
        environment.builtin_signatures_hash = registry.revision();
        Self {
            work: crate::WorkStats::default(),
            store_adoptions: 0,
            store_identity: ContentHash::default(),
            verify_store: crate::store::VerifyStore::Off,
            checked_scope: Default::default(),
            pending_store_checks: BTreeMap::new(),
            store_check_spool: std::sync::Weak::new(),
            result_store: None,
            indexed_sources: BTreeMap::new(),
            verify: Verify::On,
            retention: crate::memo::Retention::Resident,
            hold_bytes,
            configured_jobs: None,
            configured_lowering: LoweringPolicy::Comptime,
            session: false,
            comptime_limits: crate::ComptimeLimits::default(),
            last_comptime_limit: None,
            s1: crate::memo::S1Coordinator::new().worker(),
            constant_values: BTreeMap::new(),
            materialization_environment: None,
            staged_constants: BTreeMap::new(),
            staged_values: BTreeMap::new(),
            generated: BTreeMap::new(),
            instance_bodies: hirsel_tir::InstanceBodies::default(),
            sources: SourceMap::default(),
            environment,
            adapters,
            items: BTreeMap::new(),
            detached: vec![],
            parsed: BTreeMap::new(),
            registry,
            builtins: BTreeMap::new(),
            syntax_keys: BTreeMap::new(),
            signatures: Default::default(),
            resolutions: Default::default(),
            checked: Default::default(),
            modes: Default::default(),
            lowered: Default::default(),
            lower_receipts: Default::default(),
            lower_holds: crate::memo::Holds::new(hold_bytes),
            summaries: Default::default(),
            checked_summaries: Default::default(),
            portable_type_records: Default::default(),
            graph: MemoGraph::default(),
            front_changes: Default::default(),
            front_products: Default::default(),
            inputs: Default::default(),
            environment_seen: None,
            vanished: BTreeSet::new(),
            published_manifest: None,
            declarations: Default::default(),
            package_manifests: BTreeMap::new(),
            foreign_links: vec![],
            foreign_unreplayable: false,
            foreign_threads: BTreeMap::new(),
            execution_stack: vec![],
            evaluation_validation: vec![],
            evaluation_acceptance: vec![],
            evaluation_selections: vec![],
            active: BTreeSet::new(),
            comptime_runtime: None,
            source_growth: Default::default(),
            snapshot: None,
            snapshot_epoch: crate::memo::Revision::from_u64(1).expect("a nonzero revision"),
            snapshot_sources: Default::default(),
            snapshot_inputs_revision: 0,
            types: crate::registry::session_types(),
            memo: Arc::new(hirsel_types::TypeMemo::new()),
            private_summary_bodies: Default::default(),
            private_summaries: Default::default(),
            private_unique_failures: BTreeMap::new(),
            private_summary_revision: None,
            private_summary_active: false,
            signature_stack: vec![],
            recomputes: BTreeMap::new(),
            scalar_lower_calls: Default::default(),
            scalar_validation_runs: Default::default(),
            s1_lower_work: Default::default(),
            catalog: Default::default(),
            unavailable: None,
            last_phase: None,
            closure_assertions: cfg!(debug_assertions),
        }
    }
    /// The environment a package's queries and parses key on.
    pub(crate) fn package_environment(
        &self,
        package: &hirsel_syntax::PackageId,
    ) -> QueryEnvironment {
        let mut environment = self.environment.clone();
        if let Some(manifest) = self.package_manifests.get(package) {
            environment.package_manifest_hash = *manifest;
        }
        environment
    }
    /// Every definition this session has computed `kind` for.
    pub fn recomputed(&self, kind: QueryKind) -> BTreeSet<DefPath> {
        self.recomputes
            .keys()
            .filter(|(_, k)| *k == kind)
            .map(|(path, _)| path.clone())
            .collect()
    }
    pub fn recompute_count(&self, definition: &DefPath, kind: QueryKind) -> usize {
        self.recomputes
            .get(&(definition.clone(), kind))
            .copied()
            .unwrap_or(0)
    }
    pub fn dependency_reads(
        &self,
        definition: &DefPath,
        kind: QueryKind,
    ) -> Option<&DependencyReads> {
        let id = DefId::intern(definition);
        match kind {
            QueryKind::Signature => self.signatures.get(&id).map(|e| &e.detail),
            QueryKind::Resolve => self.resolutions.get(&id).map(|e| &e.detail),
            QueryKind::Check => self.checked.get(&id).map(|e| &e.detail),
            QueryKind::Modes => self.modes.get(&id).map(|e| &e.detail),
            QueryKind::Lower => self
                .lowered
                .get(&id)
                .map(|e| &e.detail)
                .or_else(|| self.lower_receipts.get(&id).map(|e| &e.detail)),
            QueryKind::Summary => self.summaries.get(&id).map(|e| &e.detail),
            QueryKind::CheckedSummary => self.checked_summaries.get(&id).map(|e| &e.detail),
            _ => None,
        }
    }
    /// The typed reads of `definition`'s memo of `kind`.
    pub fn memo_reads(&self, definition: &DefPath, kind: QueryKind) -> Option<&Reads> {
        let id = DefId::intern(definition);
        match kind {
            QueryKind::Signature => self.signatures.get(&id).map(|e| e.reads()),
            QueryKind::Resolve => self.resolutions.get(&id).map(|e| e.reads()),
            QueryKind::Check => self.checked.get(&id).map(|e| e.reads()),
            QueryKind::Modes => self.modes.get(&id).map(|e| e.reads()),
            QueryKind::Lower => self
                .lowered
                .get(&id)
                .map(|e| e.reads())
                .or_else(|| self.lower_receipts.get(&id).map(|e| e.reads())),
            QueryKind::Summary => self.summaries.get(&id).map(|e| e.reads()),
            QueryKind::CheckedSummary => self.checked_summaries.get(&id).map(|e| e.reads()),
            _ => None,
        }
    }
    /// The recorded read allocation of a completed query, shared with its memo.
    pub fn shared_memo_reads(&self, definition: &DefPath, kind: QueryKind) -> Option<Arc<Reads>> {
        let id = DefId::intern(definition);
        match kind {
            QueryKind::Signature => self.signatures.get(&id).map(|e| e.memo.reads.clone()),
            QueryKind::Resolve => self.resolutions.get(&id).map(|e| e.memo.reads.clone()),
            QueryKind::Check => self.checked.get(&id).map(|e| e.memo.reads.clone()),
            QueryKind::Modes => self.modes.get(&id).map(|e| e.memo.reads.clone()),
            QueryKind::Lower => self
                .lowered
                .get(&id)
                .map(|e| e.memo.reads.clone())
                .or_else(|| self.lower_receipts.get(&id).map(|e| e.memo.reads.clone())),
            QueryKind::Summary => self.summaries.get(&id).map(|e| e.memo.reads.clone()),
            QueryKind::CheckedSummary => self
                .checked_summaries
                .get(&id)
                .map(|e| e.memo.reads.clone()),
            _ => None,
        }
    }
    /// The memo graph's counters: memos touched and invalidations by read kind.
    pub fn memo_stats(&self) -> &crate::memo::MemoStats {
        self.graph.stats()
    }
    /// Read-only memo/read inventory for independent affected-set checks.
    pub fn memo_snapshot(&self) -> Vec<(crate::memo::MemoKey, Arc<Reads>)> {
        self.graph.snapshot()
    }
    /// Physical body V runs, including cache-hit work and discarded S1 attempts.
    pub fn body_validation_runs(&self, definition: &DefPath) -> u64 {
        self.scalar_validation_runs
            .get(&DefId::intern(definition))
            .copied()
            .unwrap_or(0)
            + self.s1_lower_work(definition).validation_runs
    }

    /// Inventory the distinct query records read or written by one host request.
    /// Raw per-operation `memo_stats().touched` remains unchanged.
    pub fn begin_memo_touch_tracking(&mut self) {
        self.graph.begin_touch_tracking();
    }
    pub fn take_touched_memos(&mut self) -> Vec<crate::memo::MemoKey> {
        self.graph.take_touched_memos()
    }

    fn missing<T>(&mut self, kind: QueryKind) -> Result<T, QueryError> {
        self.unavailable = Some(kind);
        Err(QueryError::NotImplemented)
    }
    fn snapshot(&self, definition: &DefPath) -> Result<Arc<ItemSyntax>, QueryError> {
        self.items
            .get(definition)
            .and_then(|items| items.first())
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
    /// The hash of one header item: its text and its offset in its item,
    /// which positions the header's spans relative to the item's anchor.
    pub(crate) fn header_hash(&self, snapshot: &ItemSyntax) -> ContentHash {
        let header = header_span(snapshot);
        ContentHash::of_parts(
            "hirsel.signature.header.v2",
            [
                self.sources.slice(header).unwrap_or_default().as_bytes(),
                header.start.to_le_bytes().as_slice(),
            ],
        )
    }
    /// The own content a query of `kind` keys on: the definition's headers
    /// (signatures) or its first item's text (every body query), with the
    /// package's closure manifest. Header hashes are taken once per index.
    pub(crate) fn key_text(
        &mut self,
        definition: &DefPath,
        kind: QueryKind,
    ) -> Result<ContentHash, QueryError> {
        let text = if let Some(body) = self.catalog.body(definition) {
            if kind == QueryKind::Signature {
                let headers = self.declarations.headers(definition).unwrap_or_default();
                ContentHash::of_parts(
                    "hirsel.signature.inputs.v2",
                    headers.iter().map(|h| h.0.as_slice()),
                )
            } else {
                body
            }
        } else {
            self.materialize(definition)?;
            let snapshot = self.snapshot(definition)?;
            if kind == QueryKind::Signature {
                match self.inputs.get(&DefId::intern(definition)) {
                    Some(inputs) => inputs.header(),
                    None => self.headers_hash(definition),
                }
            } else {
                snapshot.text_hash
            }
        };
        let manifest = self
            .package_manifests
            .get(&definition.0.package)
            .copied()
            .unwrap_or(self.environment.package_manifest_hash);
        Ok(ContentHash::of_parts(
            "hirsel.query.text.v1",
            [text.0.as_slice(), manifest.0.as_slice()],
        ))
    }
    /// Every candidate's header hash, combined.
    pub(crate) fn headers_hash(&self, definition: &DefPath) -> ContentHash {
        let hashes: Vec<_> = self
            .items
            .get(definition)
            .into_iter()
            .flatten()
            .map(|snapshot| self.header_hash(snapshot))
            .collect();
        ContentHash::of_parts(
            "hirsel.signature.inputs.v2",
            hashes.iter().map(|h| h.0.as_slice()),
        )
    }
    fn signature_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<hirsel_types::SignatureRef>>, QueryError> {
        self.signature_query_id(DefId::intern(definition))
    }

    fn signature_query_id(
        &mut self,
        id: DefId,
    ) -> Result<Arc<QueryOutput<hirsel_types::SignatureRef>>, QueryError> {
        // Indexing marks each changed header/candidate and its subscribers.
        // An unmarked memo needs neither a text hash nor a path-based probe;
        // the graph stamps its own verified-at revision when it is read.
        if let Some(entry) = self.signatures.get(&id).cloned() {
            self.note_environment();
            if matches!(self.graph.check(&entry.memo), crate::memo::Verdict::Valid) {
                return entry.result();
            }
        }
        let definition = id.path();
        if let Some(constant) = self.staged_constants.get(definition) {
            return Ok(Arc::new(output(constant.signature.clone())));
        }

        if !self.items.contains_key(definition) {
            if let Some(signature) = self.registry.get(definition) {
                return Ok(Arc::new(output(signature)));
            }
            if let Some(signature) = self.builtins.get(definition) {
                return Ok(Arc::new(output(signature.clone())));
            }
        }
        let _timing = timings::phase(Phase::Signatures);
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Signature)?;
        if let Some(entry) = self.signatures.get(&id).cloned() {
            if self
                .active
                .contains(&(definition.clone(), QueryKind::Signature))
            {
                return self.reentrant(definition, QueryKind::Signature);
            }
            self.active
                .insert((definition.clone(), QueryKind::Signature));
            let valid = self.memo_valid(definition, &entry, text);
            self.active
                .remove(&(definition.clone(), QueryKind::Signature));
            if valid {
                return entry.result();
            }
        }
        // Read the complete previous action before invoking normalization or
        // reconstructing its private view. Active cycles retain the ordinary
        // provisional-group/reentrant path and are never restored as settled.
        if !self
            .active
            .contains(&(definition.clone(), QueryKind::Signature))
        {
            self.active
                .insert((definition.clone(), QueryKind::Signature));
            let stored = self.load_signature(definition);
            self.active
                .remove(&(definition.clone(), QueryKind::Signature));
            if let Some(entry) = stored.filter(|entry| {
                self.private_summaries.get(&id).is_none_or(|summary| {
                    entry
                        .result()
                        .is_ok_and(|output| output.value.metadata.inferred_unique == *summary)
                })
            }) {
                self.signatures.insert(id, entry.clone());
                return entry.result();
            }
        }
        // A miss can still have declaration heads without expression arenas.
        // Materialize the demanded source before opening normalization's active
        // query, so an unavailable body cannot leave a false recursive frame.
        self.materialize(definition)?;
        let candidates = self.definition_items(definition)?;
        self.start(definition, QueryKind::Signature)?;
        let declarations = self.frozen_declarations();
        let adapter = self.adapters.signature.clone();
        let mut products = PhaseProducts::default();
        let mut recorder = Recorder {
            owner: definition.clone(),
            private_shapes: false,
            observed: Observed::default(),
            engine: self,
            reads: DependencyReads::default(),
            groups: BTreeMap::new(),
        };
        recorder.engine.signature_stack.push(definition.clone());
        let (types, memo) = (recorder.engine.types.clone(), recorder.engine.memo.clone());
        let result = match adapter {
            Some(adapter) => adapter.signature_items(
                &types,
                &memo,
                &candidates,
                &declarations,
                &mut products,
                &mut recorder,
            ),
            None => recorder.engine.missing(QueryKind::Signature),
        };
        recorder.engine.signature_stack.pop();
        recorder.observed.products = Arc::new(products);
        let mut observed = recorder.observed;
        let reads = recorder.reads;
        let mut result = result;
        if let Ok(output) = &mut result {
            self.attach_private_summary(&mut output.value, &output.dependencies);
            self.observe_private_summary(id, &mut observed);
        }
        let covered = result
            .as_ref()
            .map(|output| output.dependencies.clone())
            .unwrap_or_default();
        let key = MemoKey::new(QueryKind::Signature, id);
        let mut entry = self.finish(key, text, reads, result.map(shared_output), observed);
        entry.covered = covered;
        let entry = Arc::new(entry);
        self.store_signature(definition, &entry)?;
        keep(
            &mut self.signatures,
            &mut self.graph,
            entry.clone(),
            self.s1.failure_is_own(),
        );
        entry.result()
    }
    /// The name site of `path`'s first item in the file of `owner`'s first
    /// item, relative to that item's anchor.
    fn cited_name(&self, owner: &DefPath, path: &DefPath) -> Option<Span> {
        self.citation_source()
            .cited_name(DefId::intern(owner), DefId::intern(path))
    }
    /// Citation reads of the current source revision, borrowed independently of
    /// retained signature snapshots so ordinary body edits stay observable.
    pub fn citation_source(&self) -> impl CitationSource + '_ {
        crate::snapshot::SourceContext {
            unique_failures: &self.private_unique_failures,
            sources: &self.sources,
            items: &self.items,
            declarations: &self.declarations,
            catalog: &self.catalog,
        }
    }
    /// A failed signature's key: its own content and what it read.
    pub(crate) fn failed_signature_key(&self, path: &DefPath) -> Option<ContentHash> {
        self.signatures.get(&DefId::intern(path)).map(|entry| {
            ContentHash::of_parts(
                "hirsel.failed-signature.v1",
                [
                    entry.memo.text.0.as_slice(),
                    entry.memo.reads.digest().0.as_slice(),
                ],
            )
        })
    }
    fn reentrant<T>(&mut self, definition: &DefPath, kind: QueryKind) -> Result<T, QueryError> {
        if kind == QueryKind::Signature
            && self
                .signature_stack
                .last()
                .is_some_and(|owner| owner != definition)
        {
            let snapshot = self.snapshot(definition)?;
            if let ItemKind::Type {
                body: hirsel_syntax::TypeBody::Alias(target),
                ..
            } = &snapshot.item.kind
            {
                return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                    const { hirsel_diag::RuleId::of("LANG-7") },
                    snapshot.tree.types[target.0 as usize].origin.span,
                    "cyclic type aliases have no representation",
                )]));
            }
        }
        // Direct self-reference remains the signature/layout phase's obligation;
        // cache receipt replay also retains its existing deterministic rejection.
        self.missing(kind)
    }
    fn start(&mut self, definition: &DefPath, kind: QueryKind) -> Result<(), QueryError> {
        self.last_phase = Some(kind);
        if !self.active.insert((definition.clone(), kind)) {
            return self.reentrant(definition, kind);
        }
        *self
            .recomputes
            .entry((definition.clone(), kind))
            .or_default() += 1;
        timings::count(Count::Recomputes);
        Ok(())
    }
    fn finish<T>(
        &mut self,
        key: MemoKey,
        text: ContentHash,
        mut reads: DependencyReads,
        mut result: Result<QueryOutput<T>, QueryError>,
        observed: Observed,
    ) -> Entry<T> {
        prepare_result(&mut reads, &mut result, &observed.products);
        self.finish_prepared(key, text, reads, result.map(Arc::new), observed)
    }
    /// Worker results have already applied the same diagnostic/dependency
    /// completion as serial phases; record them without adding reads twice.
    fn finish_prepared<T>(
        &mut self,
        key: MemoKey,
        text: ContentHash,
        reads: DependencyReads,
        result: Result<Arc<QueryOutput<T>>, QueryError>,
        observed: Observed,
    ) -> Entry<T> {
        let definition = key.def.path();
        self.active.remove(&(definition.clone(), key.kind));
        if result.is_err() {
            self.last_phase = Some(key.kind);
        }
        self.record(key, text, result, reads, &observed)
    }
    /// The file an item stands in.
    pub(crate) fn item_file(&self, syntax: &ItemSyntax) -> FileId {
        self.sources
            .anchor(syntax.anchor)
            .expect("an indexed item's anchor is placed")
            .0
    }
    /// Where an item stands: its file's path and its anchor's base, the order
    /// of a definition's candidates.
    fn position(&self, syntax: &ItemSyntax) -> (std::path::PathBuf, u32) {
        let (file, base) = self
            .sources
            .anchor(syntax.anchor)
            .expect("an indexed item's anchor is placed");
        (self.sources.file(file).unwrap().path.clone(), base)
    }
    /// Index a new snapshot of one source. Each definition's candidates are
    /// ordered by (file path, position); an item's occurrence is its ordinal
    /// there. An item unchanged in (definition, text, occurrence) is the same
    /// `ItemSyntax`: only its anchor moves, so its IDs, spans and queries stay.
    /// A changed item takes over its predecessor's anchor (same definition and
    /// occurrence), so a signature surviving a body edit still names it. The
    /// items this snapshot dropped stay adoptable until the next index, for a
    /// definition moving to another file.
    fn index(&mut self, file: &mut SyntaxFile, _context: &FileContext) -> Result<(), QueryError> {
        self.prepare_catalog_edit(file.file)?;
        let new_paths: BTreeSet<_> = file
            .items
            .iter()
            .map(|item| item.item.definition.clone())
            .collect();
        for path in new_paths {
            self.materialize(&path)?;
        }
        self.comptime_runtime = None;
        self.catalog.clear_facts();
        let source_path = self.sources.file(file.file).unwrap().path.clone();
        // Every snapshot of this source, each at its own FileId.
        let mut snapshots = self.declarations.snapshots(&source_path);
        snapshots.insert(file.file);
        let from_source = |engine: &Self, old: &ItemSyntax| {
            engine
                .sources
                .anchor(old.anchor)
                .is_some_and(|(file, _)| snapshots.contains(&file))
        };
        // Only definitions this source owned or now owns can change; every
        // other entry, including a shared image's, is left untouched.
        let source_items = self.declarations.source_definitions(&source_path);
        self.work.index_owner_visits += source_items.len() as u64;
        let owned: Vec<_> = source_items
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|path| self.items.contains_key(path))
            .collect();
        // A detached item stands where it was dropped until this index adopts it.
        let carried = std::mem::take(&mut self.detached);
        // Fact helpers are derived again from their sources below.
        let mut previous: Vec<Arc<ItemSyntax>> = owned
            .iter()
            .flat_map(|path| &self.items[path])
            .filter(|old| {
                from_source(self, old)
                    && hirsel_check_protocol::fact::fact_owner(&old.item.definition).is_none()
                    && !old.item.definition.0.name.starts_with("$dictionary$")
                    && !crate::families::derived(&old.item.definition)
            })
            .cloned()
            .collect();
        previous.extend(carried.iter().cloned());
        // CT-5: a retired declaration's instances are made again from its new
        // text when a call selects them. Their disappearance is an ordinary
        // input change too: retire growth roots and invalidate all subscribers.
        let mut retired_instances = BTreeSet::new();
        for path in &owned {
            let id = DefId::intern(path);
            self.instance_bodies.declarations.remove(&id);
            for (_, instance) in self
                .instance_bodies
                .instances
                .remove(&id)
                .unwrap_or_default()
            {
                self.items.remove(instance.definition.path());
                self.generated.remove(instance.definition.path());
                retired_instances.insert(instance.definition.path().clone());
            }
        }
        for path in &owned {
            let kept: Vec<_> = self.items[path]
                .iter()
                .filter(|old| !from_source(self, old))
                .cloned()
                .collect();
            if kept.is_empty() {
                self.items.remove(path);
            } else {
                self.items.insert(path.clone(), kept);
            }
        }
        let mut affected: BTreeSet<DefPath> = owned.into_iter().chain(retired_instances).collect();
        // Occurrences: this source's items take their places among each
        // definition's other candidates.
        let new_positions: Vec<_> = file.items.iter().map(|new| self.position(new)).collect();
        let mut occurrences = vec![0u32; file.items.len()];
        let mut reordered = vec![];
        let mut ordinals: BTreeMap<DefPath, Vec<usize>> = BTreeMap::new();
        for (i, new) in file.items.iter().enumerate() {
            ordinals
                .entry(new.item.definition.clone())
                .or_default()
                .push(i);
        }
        for (path, indices) in &ordinals {
            let mut order: Vec<(std::path::PathBuf, u32, Result<usize, Arc<ItemSyntax>>)> = self
                .items
                .get(path)
                .into_iter()
                .flatten()
                .map(|old| {
                    let (p, base) = self.position(old);
                    (p, base, Err(old.clone()))
                })
                .collect();
            for &i in indices {
                self.work.index_occurrence_visits += 1;
                let (p, base) = new_positions[i].clone();
                order.push((p, base, Ok(i)));
            }
            order.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
            for (occurrence, (_, _, entry)) in order.into_iter().enumerate() {
                match entry {
                    Ok(i) => occurrences[i] = occurrence as u32,
                    Err(old) if old.occurrence != occurrence as u32 => {
                        reordered.push((old, occurrence as u32));
                    }
                    Err(_) => {}
                }
            }
        }
        // Another file's candidate whose ordinal changed is the same item at
        // its new occurrence.
        for (old, occurrence) in reordered {
            let path = old.item.definition.clone();
            let moved = Arc::new(ItemSyntax {
                occurrence,
                ..(*old).clone()
            });
            for candidate in self.items.get_mut(&path).unwrap().iter_mut() {
                if Arc::ptr_eq(candidate, &old) {
                    *candidate = moved.clone();
                }
            }
            if let Some(helper) = crate::facts::snapshot(&moved) {
                let helper_path = helper.item.definition.clone();
                for candidate in self.items.get_mut(&helper_path).into_iter().flatten() {
                    if candidate.anchor == moved.anchor {
                        *candidate = Arc::new(ItemSyntax {
                            occurrence,
                            ..(*helper).clone()
                        });
                    }
                }
                affected.insert(helper_path);
            }
            affected.insert(path);
        }
        let file_id = file.file;
        let mut predecessor_slots: BTreeMap<(DefPath, u32), Vec<usize>> = BTreeMap::new();
        for (i, old) in previous.iter().enumerate() {
            predecessor_slots
                .entry((old.item.definition.clone(), old.occurrence))
                .or_default()
                .push(i);
        }
        let mut previous: Vec<_> = previous.into_iter().map(Some).collect();
        for (i, new) in file.items.iter_mut().enumerate() {
            let occurrence = occurrences[i];
            let base = new_positions[i].1;
            let definition = new.item.definition.clone();
            let slots = predecessor_slots
                .get(&(definition, occurrence))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let unchanged = slots.iter().copied().find(|&index| {
                self.work.index_anchor_visits += 1;
                previous[index]
                    .as_ref()
                    .is_some_and(|old| old.text_hash == new.text_hash)
            });
            if let Some(index) = unchanged {
                let old = previous[index].take().unwrap();
                self.sources.move_anchor(old.anchor, file_id, base);
                *new = old;
            } else {
                let syntax = Arc::make_mut(new);
                syntax.occurrence = occurrence;
                if let Some(index) = slots.iter().copied().find(|&index| {
                    self.work.index_anchor_visits += 1;
                    previous[index].is_some()
                }) {
                    let old = previous[index].take().unwrap();
                    self.sources.move_anchor(old.anchor, file_id, base);
                    syntax.reanchor(old.anchor);
                }
            }
        }
        // Items this source dropped stay adoptable through the next index, with
        // their queries; a carried item nobody adopted expires now.
        let carried_identity: std::collections::HashSet<_> =
            carried.iter().map(Arc::as_ptr).collect();
        let (expired, dropped): (Vec<_>, Vec<_>) = previous
            .into_iter()
            .flatten()
            .partition(|old| carried_identity.contains(&Arc::as_ptr(old)));
        for old in &expired {
            affected.insert(old.item.definition.clone());
        }
        self.detached = dropped;
        for new in &file.items {
            // Duplicate paths keep one semantic identity and multiple addressable items.
            // Task E diagnoses overloading as LANG-7 at the later definition's name.
            if let Some(helper) = crate::facts::snapshot(new) {
                affected.insert(helper.item.definition.clone());
                self.items
                    .entry(helper.item.definition.clone())
                    .or_default()
                    .push(helper);
            }
            for helper in crate::dictionaries::literal_snapshots(new)
                .into_iter()
                .chain(crate::families::snapshots(new))
            {
                affected.insert(helper.item.definition.clone());
                self.items
                    .entry(helper.item.definition.clone())
                    .or_default()
                    .push(helper);
            }
            affected.insert(new.item.definition.clone());
            self.items
                .entry(new.item.definition.clone())
                .or_default()
                .push(new.clone());
        }
        for path in &affected {
            if let Some(mut candidates) = self.items.get(path).cloned() {
                candidates.sort_by_key(|candidate| candidate.occurrence);
                self.items.insert(path.clone(), candidates);
            }
        }
        self.declarations
            .update(file_id, &affected, &self.items, &self.sources);
        let revision = self.manifest_revision();
        if self.declarations.inputs().manifest_revision != revision {
            self.declarations.inputs_mut().manifest_revision = revision;
        }
        self.published_manifest = Some(revision);
        // A definition whose only item this source just dropped keeps its
        // memos while that item waits to be adopted.
        let waiting: BTreeSet<DefPath> = self
            .detached
            .iter()
            .map(|old| old.item.definition.clone())
            .filter(|path| !self.items.contains_key(path))
            .collect();
        // S0's change set: each affected definition's inputs against what was
        // last indexed. Unchanged headers survive body edits.
        self.note_inputs(
            affected
                .iter()
                .filter(|path| !waiting.contains(*path))
                .cloned(),
        );
        self.note_detached(waiting);
        let candidates: Vec<_> = self
            .staged_constants
            .iter()
            .filter(|(_, constant)| affected.contains(&constant.owner))
            .map(|(path, constant)| (path.clone(), constant.owner.clone()))
            .collect();
        let retired: Vec<DefPath> = candidates
            .into_iter()
            .filter(|(_, owner)| {
                let current = self.key_text(owner, QueryKind::Modes).ok();
                !self.modes.get(&DefId::intern(owner)).is_some_and(|entry| {
                    !self.graph.is_dirty(&entry.memo.key) && current == Some(entry.memo.text)
                })
            })
            .map(|(path, _)| path)
            .collect();
        for path in &retired {
            self.staged_constants.remove(path);
        }
        self.staged_values
            .retain(|(path, _), _| self.staged_constants.contains_key(path));
        if !retired.is_empty() {
            // Reinstalling the same hidden signature does not restore its
            // materialized instances. Retained callers must reconstruct those
            // products even if a later registration has the same fingerprint.
            let retired: BTreeSet<_> = retired.iter().map(DefId::intern).collect();
            let uses_retired = |reads: &Reads| {
                reads
                    .iter()
                    .any(|(read, _)| matches!(read, ReadKey::Signature(id) if retired.contains(id)))
            };
            self.constant_values
                .retain(|_, value| !uses_retired(&value.executed));
            let mut lowers = BTreeSet::new();
            for id in &retired {
                lowers.extend(
                    self.graph
                        .subscribers(&ReadKey::Signature(*id))
                        .filter(|key| key.kind == QueryKind::Lower)
                        .copied(),
                );
            }
            for key in lowers {
                self.forget(&key);
            }
            self.retire_snapshot();
            let retired: Vec<_> = retired.iter().map(|id| id.path().clone()).collect();
            self.note_signatures(&retired);
        }
        self.settle_signatures();
        Ok(())
    }
    /// All source items sharing a semantic path, in occurrence order. Ordinals stay
    /// local; neither duplicates nor file positions are ever appended to DefPath identity.
    pub fn definition_items(
        &self,
        definition: &DefPath,
    ) -> Result<Vec<Arc<ItemSyntax>>, QueryError> {
        self.items
            .get(definition)
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
    /// The indexed definition's first item: its own arenas, at its anchor.
    pub fn parse_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<Arc<ItemSyntax>>, QueryError> {
        self.syntax_view(definition, QueryKind::Parse)?;
        Ok(output(self.snapshot(definition)?))
    }
    fn syntax_view(&mut self, definition: &DefPath, kind: QueryKind) -> Result<(), QueryError> {
        self.materialize(definition)?;
        let key = self.key_text(definition, kind)?;
        let identity = (definition.clone(), kind);
        if self.syntax_keys.get(&identity) != Some(&key) {
            *self.recomputes.entry(identity.clone()).or_default() += 1;
            self.syntax_keys.insert(identity, key);
        }
        Ok(())
    }
    pub fn resolve_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<Resolution>, QueryError> {
        self.resolve_query(definition)
            .map(|output| (*output).clone())
    }
    /// The lexing and parsing views resolution reads, without copying them.
    fn syntax_views(&mut self, definition: &DefPath) -> Result<(), QueryError> {
        self.syntax_view(definition, QueryKind::Lex)?;
        self.snapshot(definition)?;
        self.syntax_view(definition, QueryKind::Parse)
    }
    /// The signature's shared output: cache hits never copy it.
    /// The dependency list `definition`'s fingerprint covers (PS-9): a
    /// declaration's normalized dependencies, a fact helper's owner, nothing
    /// for a registry row. The query output's dependencies are its reads,
    /// which also name what only resolution or a private representation read.
    pub fn signature_coverage(
        &mut self,
        definition: &DefPath,
    ) -> Result<Vec<SignatureDependency>, QueryError> {
        let output = self.signature_output(definition)?;
        Ok(match self.signatures.get(&DefId::intern(definition)) {
            Some(entry)
                if self.items.contains_key(definition)
                    || self.catalog.body(definition).is_some() =>
            {
                entry.covered.clone()
            }
            _ => output.dependencies.clone(),
        })
    }
    fn signature_output(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<hirsel_types::SignatureRef>>, QueryError> {
        self.signature_query(definition)
    }
    fn resolve_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<Resolution>>, QueryError> {
        self.syntax_views(definition)?;
        let _timing = timings::phase(Phase::Resolve);
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Resolve)?;
        if let Some(entry) = self.resolutions.get(&id).cloned()
            && self.memo_valid(definition, &entry, text)
        {
            return entry.result();
        }
        self.start(definition, QueryKind::Resolve)?;
        let snapshot = self.snapshot(definition)?;
        let declarations = self.frozen_declarations();
        let mut products = PhaseProducts::default();
        let adapter = self.adapters.resolver.clone();
        let mut recorder = Recorder {
            owner: definition.clone(),
            private_shapes: true,
            observed: Observed::default(),
            engine: self,
            reads: DependencyReads::default(),
            groups: BTreeMap::new(),
        };
        let (types, memo) = (recorder.engine.types.clone(), recorder.engine.memo.clone());
        let result = match adapter {
            Some(adapter) => adapter.resolve(
                &types,
                &memo,
                snapshot.clone(),
                &declarations,
                &mut products,
                &mut recorder,
            ),
            None => recorder.engine.missing(QueryKind::Resolve),
        };
        recorder.observed.products = Arc::new(products);
        let observed = recorder.observed;
        let reads = recorder.reads;
        let key = MemoKey::new(QueryKind::Resolve, id);
        let entry = Arc::new(self.finish(key, text, reads, result.map(|o| o), observed));
        keep(
            &mut self.resolutions,
            &mut self.graph,
            entry.clone(),
            self.s1.failure_is_own(),
        );
        entry.result()
    }
    pub fn mode_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
        self.mode_query(definition).map(|output| (*output).clone())
    }
    pub(crate) fn mode_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<AnalyzedBody>>, QueryError> {
        self.prepare_private_summaries();
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Modes)?;
        if let Some(entry) = self.modes.get(&id).cloned()
            && self.memo_valid(definition, &entry, text)
        {
            // Checking a generated declaration owns an S1 frame even when
            // its analyzed body is retained. Preserve that request on a hit.
            if let Some(syntax) = self
                .items
                .get(definition)
                .and_then(|rows| rows.first())
                .cloned()
                && let Some(body) = generation::generator(&syntax)
                && !generation::instance_bodied(&syntax)
            {
                self.generated_check_query(definition, syntax, body)?;
            }
            return self.mode_result(entry);
        }
        if let Some(entry) = self.load_mode(definition) {
            self.modes.insert(id, entry.clone());
            return self.mode_result(entry);
        }
        let checked = self.check_query(definition)?;
        let signature = self.signature_output(definition)?;
        let timing = timings::phase(Phase::Modes);
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Modes)?;
        let cached = self.modes.get(&id).cloned();
        let entry = match cached {
            Some(entry) if self.memo_valid(definition, &entry, text) => entry,
            _ => {
                // An unavailable phase performs no transformation. Preserve
                // earlier Check/signature diagnostics without counting a Modes
                // recomputation or opening a memo that cannot be produced.
                let Some(adapter) = self.adapters.modes.clone() else {
                    return self.missing(QueryKind::Modes);
                };
                self.start(definition, QueryKind::Modes)?;
                let mut products = PhaseProducts::default();
                let previous = self.checked.get(&id).cloned();
                let reads = previous
                    .as_ref()
                    .map(|c| c.detail.clone())
                    .unwrap_or_else(|| DependencyReads {
                        signatures: checked.dependencies.clone(),
                        ..Default::default()
                    });
                let mut recorder = Recorder {
                    owner: definition.clone(),
                    private_shapes: true,
                    observed: Observed::inheriting(previous.as_ref().map(|c| c.reads())),
                    engine: self,
                    reads,
                    groups: BTreeMap::new(),
                };
                let (types, memo) = (recorder.engine.types.clone(), recorder.engine.memo.clone());
                let result = adapter.check_modes(
                    &types,
                    &memo,
                    Arc::new(checked.value.clone()),
                    &signature.value,
                    &mut products,
                    &mut recorder,
                );
                recorder.observed.products = Arc::new(products);
                let observed = recorder.observed;
                let reads = recorder.reads;
                let key = MemoKey::new(QueryKind::Modes, id);
                let entry = Arc::new(self.finish(key, text, reads, result, observed));
                self.store_mode(definition, &entry)?;
                keep(
                    &mut self.modes,
                    &mut self.graph,
                    entry.clone(),
                    self.s1.failure_is_own(),
                );
                entry
            }
        };
        drop(timing);
        self.mode_result(entry)
    }
    fn mode_result(
        &mut self,
        entry: Arc<Entry<AnalyzedBody>>,
    ) -> Result<Arc<QueryOutput<AnalyzedBody>>, QueryError> {
        // A materialization executes this checked body regardless of whether
        // Modes was computed now, retained, or restored. Preserve its entire
        // receipt (including negative lookups and diagnostic citations).
        self.merge_execution(entry.reads());
        // Compile-time users also depend on private shapes used by a body,
        // including when its public signature and source text are unchanged.
        let representations: Vec<_> = entry
            .reads()
            .iter()
            .filter(|(k, _)| matches!(k, ReadKey::Representation(_)))
            .map(|(k, _)| k.definition().unwrap().path().clone())
            .collect();
        for path in representations {
            self.read_layout_body(&path);
        }
        self.label_generated_result(entry.result())
    }
    fn check_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<TypedBody>>, QueryError> {
        self.materialize(definition)?;
        let snapshot = self.snapshot(definition)?;
        if let Some(body) = generation::generator(&snapshot) {
            return self.generated_check_query(definition, snapshot, body);
        }
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Check)?;
        if let Some(entry) = self.checked.get(&id).cloned()
            && self.memo_valid(definition, &entry, text)
        {
            return entry.result();
        }
        // GEN-6 members share their source owner's syntax and are omitted
        // from the ordinary per-file front roots. Direct Check demand must
        // still restore their checked facts from a current ordinary Modes row.
        if crate::families::derived(definition)
            && let Some(modes) = self.load_mode(definition)
        {
            self.modes.insert(id, modes);
            if let Some(entry) = self.checked.get(&id).cloned()
                && self.memo_valid(definition, &entry, text)
            {
                return entry.result();
            }
        }
        let resolution = self.resolve_query(definition)?;
        let signature = self.signature_output(definition)?;
        let _timing = timings::phase(Phase::Check);
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Check)?;
        if let Some(entry) = self.checked.get(&id).cloned()
            && self.memo_valid(definition, &entry, text)
        {
            return entry.result();
        }
        timings::count(Count::Definitions);
        let previous = self.resolutions.get(&id).cloned();
        let mut initial = previous
            .as_ref()
            .map(|c| c.detail.clone())
            .unwrap_or_default();
        initial.signatures.extend(resolution.dependencies.clone());
        initial.signatures.extend(signature.dependencies.clone());
        initial.signatures.push(SignatureDependency {
            definition: DefId::intern(definition),
            fingerprint: signature.value.fingerprint,
        });
        self.start(definition, QueryKind::Check)?;
        let snapshot = self.snapshot(definition)?;
        let adapter = self.adapters.checker.clone();
        let mut recorder = Recorder {
            owner: definition.clone(),
            private_shapes: true,
            observed: Observed::inheriting(previous.as_ref().map(|c| c.reads())),
            engine: self,
            reads: initial,
            groups: BTreeMap::new(),
        };
        let (types, memo) = (recorder.engine.types.clone(), recorder.engine.memo.clone());
        let result = match adapter {
            Some(adapter) => adapter.check(
                &types,
                &memo,
                snapshot.clone(),
                resolution.value.clone(),
                &signature.value,
                &mut recorder,
            ),
            None => recorder.engine.missing(QueryKind::Check),
        };
        let mut observed = recorder.observed;
        let reads = recorder.reads;
        let mut result = result;
        if let Ok(output) = &mut result {
            self.place_arm_fixes(output, &mut observed.representations);
        }
        let key = MemoKey::new(QueryKind::Check, id);
        let entry = Arc::new(self.finish(key, text, reads, result, observed));
        keep(
            &mut self.checked,
            &mut self.graph,
            entry.clone(),
            self.s1.failure_is_own(),
        );
        entry.result()
    }
    /// GEN-5 owns these body snapshots independently of per-definition cache keys.
    pub(crate) fn instantiation_bodies(
        &mut self,
    ) -> Result<Vec<(DefPath, Arc<QueryOutput<TypedBody>>)>, QueryError> {
        let mut bodies = vec![];
        // An image package reaches no definition outside its closure, so it closes no
        // instantiation cycle through one, and its own verdict is in its image.
        for path in self.items.keys().cloned().collect::<Vec<_>>() {
            let checked = self.check_query(&path)?;
            bodies.push((path, checked));
        }
        Ok(bodies)
    }
    pub(crate) fn signature_products(&self, definition: DefId) -> Option<&PhaseProducts> {
        self.signatures
            .get(&definition)
            .map(|entry| &*entry.products)
    }
    pub fn resolver_snapshot(
        &self,
        definition: &DefPath,
        kind: QueryKind,
    ) -> Option<&ResolverSnapshot> {
        let id = DefId::intern(definition);
        let entry = match kind {
            QueryKind::Resolve => self.resolutions.get(&id).map(|e| &e.products),
            QueryKind::Signature => self.signatures.get(&id).map(|e| &e.products),
            _ => None,
        }?;
        entry.resolution.as_ref()
    }

    pub fn diagnostic_phase(&self, diagnostic: &Diagnostic) -> Option<String> {
        for entry in self.modes.values() {
            if let Some((_, phase)) = entry
                .products
                .mode_diagnostics
                .iter()
                .find(|(d, _)| d.rule == diagnostic.rule && d.primary == diagnostic.primary)
            {
                return Some(phase.clone());
            }
        }
        for (id, entry) in &self.summaries {
            // A detached or edited owner's receipt no longer attributes a
            // warning, even if its old summary awaits adoption/retirement.
            if entry.products.mode_diagnostics.is_empty()
                || !entry.consistent
                || self.graph.is_dirty(&entry.memo.key)
            {
                continue;
            }
            let Some(body) = self.implementation_hash(id.path()) else {
                continue;
            };
            let manifest = self
                .package_manifests
                .get(&id.path().0.package)
                .copied()
                .unwrap_or(self.environment.package_manifest_hash);
            if entry.memo.text
                != ContentHash::of_parts(
                    "hirsel.query.text.v1",
                    [body.0.as_slice(), manifest.0.as_slice()],
                )
            {
                continue;
            }
            if let Some((_, phase)) = entry
                .products
                .mode_diagnostics
                .iter()
                .find(|(d, _)| d.rule == diagnostic.rule && d.primary == diagnostic.primary)
            {
                return Some(phase.clone());
            }
        }
        fn contains<T>(entry: &Entry<T>, diagnostic: &Diagnostic) -> bool {
            let diagnostics = match &entry.memo.value {
                Ok(o) => &o.diagnostics,
                Err(QueryError::Diagnostics(ds)) => ds,
                _ => return false,
            };
            diagnostics
                .iter()
                .any(|d| d.rule == diagnostic.rule && d.primary == diagnostic.primary)
        }
        if self.resolutions.values().any(|c| contains(c, diagnostic)) {
            return Some("name resolution".into());
        }
        if self.signatures.values().any(|c| contains(c, diagnostic)) {
            return Some("signature normalization".into());
        }
        if self.checked.values().any(|c| contains(c, diagnostic)) {
            return Some("body checking".into());
        }
        self.last_phase.map(|p| p.phase_name().into())
    }
    /// The runtime types a program runs with: every item's view as its own
    /// package's bodies see it, with each owned drop body lowered.
    pub fn runtime_types(&mut self) -> Result<hirsel_interp::aggregate::RuntimeTypes, QueryError> {
        let mut views: Vec<_> = self
            .registry
            .signatures()
            .map(|s| (s.clone(), ContentHash::default()))
            .collect();
        for path in self.source_paths() {
            let public = self.item_signature(&path)?.value;
            views.push((
                self.body_signature(&path.0.package, public)?.0,
                ContentHash::default(),
            ));
        }
        self.build_runtime_types(views, true)
    }
    /// Runtime metadata plus the declared facts checked execution enforces:
    /// `@noalloc` definitions, library `in ..` results and `arena.with`.
    pub fn checked_runtime_types(
        &mut self,
    ) -> Result<hirsel_interp::aggregate::RuntimeTypes, QueryError> {
        let mut runtime = self.runtime_types()?;
        let mut checked = hirsel_interp::checked::CheckedRuntime {
            arena_with: Some(hirsel_syntax::DefPath::core("arena.with")),
            ..Default::default()
        };
        let library: Vec<_> = self.registry.signatures().cloned().collect();
        let mut signatures: Vec<_> = library.iter().cloned().collect();
        for path in self.source_paths() {
            signatures.push(self.item_signature(&path)?.value);
        }
        for signature in signatures {
            if let hirsel_types::SignatureKind::Type(t) = &signature.kind {
                if t.layout.flat {
                    checked.flat.insert(signature.definition.path().clone());
                }
                if t.loop_source.is_some() {
                    checked
                        .operations
                        .insert(signature.definition.path().clone(), signature.clone());
                }
                if t.destructor_allocation == hirsel_types::Allocation::NoAlloc {
                    checked
                        .noalloc_drop
                        .insert(signature.definition.path().clone());
                    checked.noalloc.insert(signature.definition.path().clone());
                }
            }
            let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
                continue;
            };
            if function.allocation == hirsel_types::Allocation::NoAlloc {
                checked.noalloc.insert(signature.definition.path().clone());
            }
            checked
                .operations
                .insert(signature.definition.path().clone(), signature);
        }
        if let Some(forget) = &runtime.forget {
            checked.noalloc.insert(forget.clone());
        }
        runtime.checked = Some(checked);
        Ok(runtime)
    }
    /// The runtime types compile-time evaluation runs with, built once per
    /// signature snapshot from its body views. Reflection and codecs read
    /// them through the recorded serialization view.
    pub(crate) fn comptime_runtime_types(
        &mut self,
    ) -> Result<Arc<hirsel_interp::aggregate::RuntimeTypes>, QueryError> {
        let snapshot = self.signature_snapshot();
        if let Some((epoch, runtime)) = &self.comptime_runtime
            && *epoch == snapshot.epoch()
        {
            return Ok(runtime.clone());
        }
        let mut views: Vec<_> = self
            .registry
            .signatures()
            .map(|s| {
                let digest = snapshot
                    .representation(s.definition)
                    .and_then(Result::ok)
                    .map_or_else(ContentHash::default, |(_, digest)| digest.0);
                (s.clone(), digest)
            })
            .collect();
        for path in self.source_paths() {
            match snapshot.representation(hirsel_syntax::DefId::intern(&path)) {
                Some(Ok((view, digest))) => views.push((view.clone(), digest.0)),
                Some(Err(error)) => return Err(error.clone()),
                None => {}
            }
        }
        let runtime = Arc::new(self.build_runtime_types(views, false)?);
        self.comptime_runtime = Some((snapshot.epoch(), runtime.clone()));
        Ok(runtime)
    }
    /// The digest a reflection of `path` now records: its view in its own
    /// package, from the current snapshot when one is kept.
    pub(crate) fn representation_digest(&mut self, path: &DefPath) -> ContentHash {
        let d = hirsel_syntax::DefId::intern(path);
        if let Some((stamp, snapshot)) = &self.snapshot
            && *stamp == self.snapshot_stamp()
            && let Some(view) = snapshot.representation(d)
        {
            return view.map_or_else(|_| ContentHash::default(), |(_, digest)| digest.0);
        }
        let view = match self.registry.get(d) {
            Some(signature) => Ok(signature.clone()),
            None if self.declared(path) => self
                .item_signature(path)
                .and_then(|public| self.body_signature(&path.0.package, public.value))
                .map(|(view, _)| view),
            None => return ContentHash::default(),
        };
        view.map_or_else(
            |_| ContentHash::default(),
            |view| hirsel_validate::digest(self.types(), &view).0,
        )
    }
    /// Runtime types over `views` (each with the digest a reflection of it
    /// records). A program's run lowers each owned drop body; compile-time
    /// evaluation names the entry body and lowers a finalizer when it runs.
    fn build_runtime_types(
        &mut self,
        views: Vec<(hirsel_types::SignatureRef, ContentHash)>,
        eager: bool,
    ) -> Result<hirsel_interp::aggregate::RuntimeTypes, QueryError> {
        let mut runtime = hirsel_interp::aggregate::RuntimeTypes {
            trace_mode: hirsel_interp::TraceMode::Effects,
            forget: Some(hirsel_syntax::DefPath::core(
                hirsel_interp::library::FORGET_OPERATION,
            )),
            ..hirsel_interp::aggregate::RuntimeTypes::new(self.types().clone())
        };
        runtime.serialization = hirsel_interp::aggregate::Serialization::new(
            views
                .iter()
                .map(|(s, digest)| (s.definition.path().clone(), s.clone(), *digest)),
        );
        for (signature, _) in views {
            let hirsel_types::SignatureKind::Type(t) = &signature.kind else {
                continue;
            };
            let fields = t
                .representation
                .and_then(|id| self.types().get(id))
                .map(|r| match &r.kind {
                    hirsel_types::TypeKind::Record(fs) => fs.len(),
                    hirsel_types::TypeKind::Variant(vs) => {
                        vs.iter().map(|v| v.fields.len()).max().unwrap_or(0)
                    }
                    _ => 1,
                })
                .unwrap_or(0);
            let finalizer = match &t.drop {
                Some(drop) if self.declared(drop) => {
                    let entry = if eager {
                        self.lower_to_tir(drop)?.value.entry
                    } else {
                        match &self.adapters.lowering {
                            Some(lowering) => lowering.entry_body(),
                            None => return self.missing(QueryKind::Lower),
                        }
                    };
                    Some((drop.clone(), entry))
                }
                // The registry's File/AtomicFile are native handles: Library's
                // drop_handle hook owns their cleanup, with no Hirsel body.
                Some(drop) if self.registry.get(drop).is_some() => None,
                Some(drop) => {
                    return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "runtime types".into(),
                        message: format!("missing finalizer {drop:?}"),
                    }));
                }
                None => None,
            };
            runtime.nominals.push(hirsel_interp::aggregate::Nominal {
                definition: signature.definition.path().clone(),
                resource: t.resource,
                distinct: t.distinct,
                finalizer,
                fields,
                representation: t.representation,
            });
        }
        Ok(runtime)
    }
    pub fn manifest_revision(&self) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.manifest.v1",
            [
                self.environment.package_manifest_hash.0.as_slice(),
                self.declarations.identity().0.as_slice(),
            ],
        )
    }
}

impl QueryEngine {
    /// Whether this engine retained a clean parse of `file`.
    pub(crate) fn parsed_cleanly(&self, file: FileId) -> bool {
        matches!(self.parsed.get(&file), Some((_, _, Ok(_))))
    }
    /// Parse `file` in this engine, discarding a retained outcome first.
    pub(crate) fn reparse_file(
        &mut self,
        file: FileId,
        context: &FileContext,
    ) -> Result<(), QueryError> {
        self.parsed.remove(&file);
        self.parse_file(file, context).map(|_| ())
    }
}
impl QueryEngine {
    /// Lex and parse `file` under `context`, with the diagnostics std's
    /// shared module heads add; indexing is the caller's.
    fn parse_syntax(
        &mut self,
        file: FileId,
        context: &FileContext,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError> {
        if self.package_manifests.contains_key(&context.package) {
            timings::count(Count::DependencyParses);
        }
        let _timing = timings::phase(Phase::Parse);
        let Some(lexer) = self.adapters.lexer.clone() else {
            return self.missing(QueryKind::Lex);
        };
        self.last_phase = Some(QueryKind::Lex);
        self.work.parsed_bytes += self
            .sources
            .file(file)
            .map_or(0, |source| source.text().len() as u64);
        let tokens = lexer.lex(file, &self.sources)?;
        if has_errors(&tokens.diagnostics) {
            return Err(QueryError::Diagnostics(tokens.diagnostics));
        }
        let Some(parser) = self.adapters.parser.clone() else {
            return self.missing(QueryKind::Parse);
        };
        self.last_phase = Some(QueryKind::Parse);
        let mut output = parser.parse(file, tokens.value, context, &mut self.sources)?;
        self.work.parsed_definitions += output.value.items.len() as u64;
        let mut diagnostics = tokens.diagnostics;
        diagnostics.append(&mut output.diagnostics);
        if context.package.name_is("std") {
            for syntax in &output.value.items {
                let item = &syntax.item;
                let mut core = item.definition.clone();
                core.0.package = DefPath::core("Result").0.package;
                if !core.0.modules.is_empty() && self.registry.get(&core).is_some() {
                    diagnostics.push(Diagnostic::error(
                        const { hirsel_diag::RuleId::of("LANG-4") },
                        item.name_span,
                        format!(
                            "std definition {} duplicates {core} in their shared module head",
                            item.definition
                        ),
                    ));
                }
            }
        }
        output.diagnostics = diagnostics;
        Ok(output)
    }
}
impl QueryEngine {
    fn lower_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<hirsel_tir::DefinitionRef>, QueryError> {
        self.prepare_materialization_environment();

        if self
            .source_facts(definition)
            .is_some_and(|facts| !facts.executable)
        {
            let diagnostics = self.definition_diagnostics(definition)?;
            if has_errors(&diagnostics) {
                return Err(QueryError::Diagnostics(diagnostics));
            }
            return Err(QueryError::NotImplemented);
        }
        let _timing = timings::phase(Phase::Lower);
        let id = DefId::intern(definition);
        let text = self.key_text(definition, QueryKind::Lower)?;
        if let Some(entry) = self.lowered.get(&id).cloned()
            && entry
                .depth
                .as_ref()
                .is_none_or(|depth| depth.limit() == self.depth_limit())
            && self.memo_valid(definition, &entry, text)
            && self.lower_acceptance_holds(entry.acceptance.as_ref())
            && self.replay_selections(&entry.selections).is_some()
        {
            // Staged declarations still need their registration product. Ordinary
            // immutable lower hits do not demand the checked/Modes bodies.
            if entry
                .result()
                .is_ok_and(|output| crate::comptime::has_staged_reads(&output.value))
            {
                self.register_staged_definition(definition)?;
            }
            if self.verify == Verify::On {
                if let Ok(output) = entry.result() {
                    self.ensure_lowered_verdict(&output.value)?;
                }
            }
            if let Some(depth) = &entry.depth {
                self.depth_replay_requests(depth)?;
            }
            let required = self.lower_requires_execution(definition, entry.reads());
            self.merge_lower_acceptance(entry.acceptance.as_ref(), required);
            self.record_selections(&entry.selections);
            self.merge_execution(entry.reads());
            return entry.output();
        }
        self.lower_holds
            .demand_evicted(&MemoKey::new(QueryKind::Lower, id));
        if let Some(entry) = self.load_lower(definition) {
            self.depth_replay_requests(entry.depth.as_ref().expect("stored depth receipt"))?;
            let required = self.lower_requires_execution(definition, entry.reads());
            self.merge_lower_acceptance(entry.acceptance.as_ref(), required);
            self.record_selections(&entry.selections);
            self.merge_execution(entry.reads());
            self.lowered.insert(id, entry.clone());
            return entry.output();
        }
        // Include generator requests made while obtaining the analyzed body.
        // A Lower hit must replay the same trace as computing it from source.
        self.s1.begin_requests();
        self.begin_evaluation_validation();
        self.execution_stack.push(ReadsBuilder::new());
        let front = (|| {
            let modes = self.mode_query(definition)?;
            self.register_staged(&modes.value)?;
            let signature = self.signature_output(definition)?;
            self.read_execution_signatures(&modes.dependencies);
            self.lower_receipts.remove(&id);
            timings::count(Count::Lowered);
            self.start(definition, QueryKind::Lower)?;
            Ok((modes, signature))
        })();
        let (modes, signature) = match front {
            Ok(front) => front,
            Err(error) => {
                self.s1.finish_requests(self.depth_limit());
                self.execution_stack
                    .pop()
                    .expect("failed lowering execution frame");
                self.end_evaluation_validation();
                return Err(error);
            }
        };
        let previous = self.modes.get(&id).cloned();
        let mut reads = previous
            .as_ref()
            .map(|c| c.detail.clone())
            .unwrap_or_default();
        reads.signatures.extend(modes.dependencies.clone());
        let mut observed = Observed::inheriting(previous.as_ref().map(|c| c.reads()));
        let prepared = self.prepare_comptime(&modes.value, &signature.value);
        let result = prepared.and_then(|comptime| match self.adapters.lowering.clone() {
            Some(adapter) => {
                *self.scalar_lower_calls.entry(id).or_default() += 1;
                adapter.lower(
                    &self.types,
                    &self.memo,
                    &modes.value,
                    &comptime,
                    &signature.value,
                )
            }
            None => self.missing(QueryKind::Lower),
        });
        let mut result = result.map(shared_output);
        let mut verification = None;
        if let Ok(lowered) = &result {
            if self.verify == Verify::On
                && self.lower_requires_execution(definition, &Reads::empty())
            {
                let package = definition.0.package.clone();
                let digest = self.signature_snapshot().validation_input_digest(&package);
                self.record_evaluation_validation(package, digest);
            }
            let (report, evidence) = self.validate_lowered_artifact(&lowered.value);
            verification = evidence;
            if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
                result = Err(error);
            } else if !report.violations.is_empty() {
                result = Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "TIR validation".into(),
                    message: format!("{definition:?}: {:?}", report.violations),
                }));
            }
        }
        let executed = self
            .execution_stack
            .pop()
            .expect("lowering execution frame");
        observed.merged.extend(executed);
        let depth = self.s1.finish_requests(self.depth_limit());
        let (_, acceptance, selections) = self.end_evaluation_validation();
        let key = MemoKey::new(QueryKind::Lower, id);
        let mut entry = self.finish(key, text, reads, result, observed);
        entry.depth = entry.result().is_ok().then_some(depth);
        entry.acceptance = entry.result().is_ok().then_some(acceptance).flatten();
        entry.selections = selections;
        entry.verification = std::sync::Mutex::new(verification);
        let entry = Arc::new(entry);
        self.store_lower(definition, &entry)?;
        keep(
            &mut self.lowered,
            &mut self.graph,
            entry.clone(),
            self.s1.failure_is_own(),
        );
        if !self.lowered.contains_key(&id) {
            self.lower_holds.forget(&key);
        }
        // Execution closure by union: whoever executes this body reads all it read.
        self.merge_execution(entry.reads());
        entry.output()
    }
}

impl DefinitionQueries for QueryEngine {
    fn parse_file(
        &mut self,
        file: FileId,
        context: &FileContext,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError> {
        // An unedited source of the adopted image keeps the image.
        self.materialize_source(file)?;
        let package_environment = self.package_environment(&context.package);
        if let Some((old_context, environment, result)) = self.parsed.get(&file) {
            if old_context == context && environment == &package_environment {
                return result.clone();
            }
        }
        let mut output = match self.parse_syntax(file, context) {
            Ok(output) => output,
            Err(error) => {
                // A lexing failure, and this fixed implementation limit, are
                // deterministic for an exact source snapshot. Retain only the
                // failure, never partial syntax.
                if matches!(&error, QueryError::Diagnostics(_))
                    || matches!(&error, QueryError::Operational(failure) if failure.operation == "parser-tooling-limit")
                {
                    self.parsed.insert(
                        file,
                        (context.clone(), package_environment, Err(error.clone())),
                    );
                }
                return Err(error);
            }
        };
        let result = if has_errors(&output.diagnostics) {
            Err(QueryError::Diagnostics(output.diagnostics.clone()))
        } else {
            let _timing = timings::phase(Phase::Declarations);
            self.index(&mut output.value, context).map(|_| output)
        };
        if let Ok(output) = &result {
            self.store_parse_syntax(file, context, output)?;
            self.indexed_sources
                .insert(file, (context.clone(), package_environment.clone()));
        }
        self.parsed.insert(
            file,
            (context.clone(), package_environment.clone(), result.clone()),
        );
        result
    }
    fn item_signature(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<hirsel_types::SignatureRef>, QueryError> {
        self.signature_query(definition)
            .map(|output| (*output).clone())
    }
    fn check_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        self.check_query(definition).map(|output| (*output).clone())
    }
    fn lower_to_tir(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<hirsel_tir::DefinitionRef>, QueryError> {
        let result = self.lower_query(definition);
        let released = self.release_batch_front(definition);
        self.retain_lowering(DefId::intern(definition));
        result.and_then(|output| released.map(|()| output))
    }
}
impl SignatureProvider for QueryEngine {
    fn signature(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        self.signature_query_id(definition)
            .map(|output| output.value.clone())
    }
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
    ) -> hirsel_types::LookupOutput {
        hirsel_types::LookupOutput {
            manifest: ContentHash::of_parts(
                "hirsel.provider.registry.v1",
                [
                    self.manifest_revision().0.as_slice(),
                    self.registry.revision().0.as_slice(),
                ],
            ),
            result: (|| {
                if path
                    .segments
                    .iter()
                    .any(|segment| segment.as_str().starts_with('$'))
                {
                    return Err(QueryError::NotImplemented);
                }
                let definition = lookup_path(context, path).ok_or(QueryError::NotImplemented)?;
                if self.declared(&definition) || self.builtins.contains_key(&definition) {
                    Ok(definition)
                } else {
                    self.registry
                        .lookup_name(path)
                        .ok_or(QueryError::NotImplemented)
                }
            })(),
        }
    }
}
impl TirProvider for QueryEngine {
    fn constant(&mut self, path: &DefPath) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        self.materialize_value(path).map(std::sync::Arc::new)
    }
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        if arguments.is_empty() {
            return self.constant(path);
        }
        self.materialize_staged(path, arguments)
            .map(|value| Arc::new(value.value))
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        // #63: dependency-first; ties use source file path and declaration position.
        let mut declarations: Vec<_> = self
            .source_paths()
            .into_iter()
            .filter_map(|path| {
                let (constant, name) = match self.items.get(&path) {
                    Some(items) => {
                        let item = items.first()?;
                        (
                            matches!(item.item.kind, ItemKind::Const { .. }),
                            item.item.name_span,
                        )
                    }
                    None if self.catalog.body(&path).is_some() => (
                        self.catalog.facts(&path)?.constant,
                        self.declarations.sites(&path).first()?.name_span,
                    ),
                    None => return None,
                };
                constant.then(|| {
                    let site = self.sources.resolve(name);
                    (
                        self.sources.file(site.file).unwrap().path.clone(),
                        site.start,
                        path,
                    )
                })
            })
            .collect();
        declarations.sort();
        let mut paths: Vec<_> = self
            .registry
            .signatures()
            .filter(|s| matches!(s.kind, hirsel_types::SignatureKind::Const { ty: _, .. }))
            .map(|s| s.definition.path().clone())
            .collect();
        paths.extend(declarations.into_iter().map(|(_, _, path)| path));
        scheduled_native_root::order_constants(paths, |path| {
            let value = self.evaluate_constant(path)?;
            Ok(value
                .executed
                .iter()
                .filter_map(|(read, _)| match read {
                    ReadKey::ExecutedBody(dependency) => Some(dependency.path().clone()),
                    _ => None,
                })
                .collect())
        })
    }

    fn definition(&mut self, path: &DefPath) -> Result<hirsel_tir::DefinitionRef, QueryError> {
        self.lower_to_tir(path).map(|o| o.value)
    }
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.resolve_instance(path, arguments)
    }
}
struct Recorder<'a> {
    owner: DefPath,
    private_shapes: bool,
    observed: Observed,
    engine: &'a mut QueryEngine,
    reads: DependencyReads,
    /// Recursive type groups found so far, by member.
    groups: BTreeMap<DefPath, Option<Arc<type_groups::TypeGroup>>>,
}
impl SignatureProvider for Recorder<'_> {
    fn signature(&mut self, id: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        let definition = id.path();
        let result = self.engine.signature(id);
        if let Err(QueryError::Operational(error)) = &result {
            self.reads.operational_failure = Some(error.clone());
        }
        match &result {
            Ok(signature) => self.reads.signatures.push(SignatureDependency {
                definition: DefId::intern(definition),
                fingerprint: signature.fingerprint,
            }),
            // A group member's published signature may be the one being
            // built; the group key covers it (the resolver only probes it).
            Err(_) if self.in_owner_group(definition) => {}
            Err(_) => {
                self.reads.failed_signatures.push(definition.clone());
                if definition != &self.owner
                    || !self
                        .engine
                        .active
                        .contains(&(definition.clone(), QueryKind::Signature))
                {
                    self.reads.failed_signature_keys.push((
                        definition.clone(),
                        self.engine.failed_signature_key(definition),
                    ));
                }
            }
        }
        if self.private_shapes {
            let public = result?;
            let (view, revision) = self.engine.body_signature(&self.owner.0.package, public)?;
            if let Some(digest) = revision {
                self.observed
                    .merged
                    .record(ReadKey::Representation(id), digest);
            }
            Ok(view)
        } else {
            result
        }
    }
    fn cite_unique_failure(
        &mut self,
        definition: &DefPath,
        claim: hirsel_types::UniqueClaim,
    ) -> Option<hirsel_types::UniqueFailure> {
        let failure = self.engine.citation_source().unique_failure(
            DefId::intern(&self.owner),
            DefId::intern(definition),
            claim,
        );
        self.reads
            .citations
            .push((definition.clone(), failure.as_ref().map(|f| f.span)));
        failure
    }
    fn cite_name(&mut self, definition: &DefPath) -> Option<Span> {
        let cited = self.engine.cited_name(&self.owner, definition);
        self.reads.citations.push((definition.clone(), cited));
        cited
    }
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
    ) -> hirsel_types::LookupOutput {
        let result = self.engine.lookup(context, path);
        self.reads
            .manifest_lookups
            .push(hirsel_types::DependencyRead::ProviderLookup {
                context: context.clone(),
                package: path.package.clone(),
                segments: path
                    .segments
                    .iter()
                    .map(|s| s.as_str().to_owned())
                    .collect(),
                output: result.clone(),
            });
        if let Err(QueryError::Operational(error)) = &result.result {
            self.reads.operational_failure = Some(error.clone());
        }
        if let Some(path) = lookup_path(context, path) {
            self.reads.lookups.push((path, result.result.is_ok()));
        }
        result
    }
    fn recursive_group(
        &mut self,
        definition: &DefPath,
        stage: hirsel_types::GroupStage,
    ) -> Result<hirsel_types::RecursiveGroup, QueryError> {
        self.group_at(definition, stage)
    }
    fn in_recursive_group(&mut self, owner: &DefPath, member: &DefPath) -> bool {
        let Some(group) = self.type_group(owner) else {
            return false;
        };
        // Membership is a semantic input even when the header only probes a
        // peer's nominal name. Use the same coverage as group normalization.
        for path in &group.members {
            if let Some(text) = self.engine.implementation_hash(path) {
                self.observed.representations.insert(path.clone(), text);
            }
        }
        group.members.iter().any(|path| path == member)
    }
}
pub(crate) fn lookup_path(context: &FileContext, path: &hirsel_syntax::Path) -> Option<DefPath> {
    let mut segments: Vec<_> = path
        .segments
        .iter()
        .map(|s| s.as_str().to_owned())
        .collect();
    let name = segments.pop()?;
    let modules = if path.package.is_none() && segments.is_empty() {
        context.modules.clone()
    } else {
        segments
    };
    Some(DefPath(ItemPath {
        package: path
            .package
            .clone()
            .unwrap_or_else(|| context.package.clone()),
        modules,
        name,
    }))
}
pub(crate) fn output<T>(value: T) -> QueryOutput<T> {
    QueryOutput {
        value,
        diagnostics: vec![],
        dependencies: vec![],
    }
}
pub(crate) fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity == Severity::Error)
}

/// Complete a phase before the next phase consumes it. Shared by serial and
/// frozen workers so diagnostic rejection and dependency order stay identical.
fn prepare_result<T>(
    reads: &mut DependencyReads,
    result: &mut Result<QueryOutput<T>, QueryError>,
    products: &PhaseProducts,
) {
    if let Ok(output) = result {
        reads.signatures.extend(output.dependencies.clone());
        output.dependencies = reads.signatures.clone();
        if has_errors(&output.diagnostics) {
            *result = Err(QueryError::Diagnostics(output.diagnostics.clone()));
        }
    }
    if let Some(error) = &reads.operational_failure {
        *result = Err(QueryError::Operational(error.clone()));
    }
    if let Some(resolution) = &products.resolution {
        reads.resolver = resolution.reads.clone();
    }
}

/// Keeps a recorded memo in its table, or forgets it to be recomputed:
/// inconsistent reads, an uncacheable result (an operational failure or a
/// missing phase), or a failure that met a compile-time cycle or the depth
/// limit outside its own frames (`own` false), which another stack would not
/// meet.
fn keep<T>(table: &mut Table<T>, graph: &mut MemoGraph, entry: Arc<Entry<T>>, own: bool) {
    let key = entry.memo.key;
    let value = entry.memo.value.as_ref();
    if !entry.consistent || value.is_err_and(|e| !e.cacheable()) || (value.is_err() && !own) {
        graph.remove(&key);
        table.remove(&key.def);
    } else {
        table.insert(key.def, entry);
    }
}

fn shared_output<T>(output: QueryOutput<T>) -> QueryOutput<std::sync::Arc<T>> {
    QueryOutput {
        value: std::sync::Arc::new(output.value),
        diagnostics: output.diagnostics,
        dependencies: output.dependencies,
    }
}

pub(crate) fn header_span(syntax: &ItemSyntax) -> Span {
    let item = &syntax.item;
    let end = match item.kind {
        ItemKind::Function {
            body: Some(body), ..
        } => syntax.tree.expressions[body.0 as usize].origin.span.start,
        _ => item.origin.span.end,
    };
    Span {
        end,
        ..item.origin.span
    }
}
