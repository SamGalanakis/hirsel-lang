//! Readiness returns immutable products, never a mutable compiler session.
//! The request lease and these products retain their admitted owners; a host
//! cannot make a producer descriptor into V, source or native authority.
use super::*;
use hirsel_engine::{CheckOutput, ToolError, UnsafeReach, WorkStats};
use hirsel_span::SourceMap;
use hirsel_tir::{DefinitionRef, MaterializedRef, TirProvider};
use hirsel_types::{GenericArgument, QueryError, SignatureRef};
use std::sync::Arc;

/// Work of this completed request, independently of a transport's delivery
/// revision. Equal concurrent requests can share products without claiming
/// that every caller performed their transformations.
#[derive(Clone, Debug, Default)]
pub struct RequestWork {
    /// Retained-body allowance of the selected front owner.
    pub hold_bytes: u64,
    pub store_adoptions: u64,
    pub recomputations: u64,
    pub front: WorkStats,
    pub touched_memos: Vec<hirsel_engine::memo::MemoKey>,
    pub memo_operations: u64,
    pub input_operations: u64,
    pub input_bytes: u64,
}
/// Actual cumulative producer work for explicitly named definitions.
#[derive(Clone, Debug)]
pub struct DefinitionWork {
    pub definition: DefPath,
    pub recomputations: Vec<(hirsel_engine::QueryKind, u64)>,
    pub body_validations: u64,
    pub lower_calls: u64,
}
/// Read-only observations of the selected engine graph. Producing this value
/// neither runs compilation nor changes memo validity or retention.
#[derive(Clone)]
pub struct CompileInspection {
    pub stats: hirsel_engine::memo::MemoStats,
    pub memos: Vec<(hirsel_engine::memo::MemoKey, Arc<hirsel_engine::Reads>)>,
    pub definitions: Vec<DefinitionWork>,
    observations: Option<Arc<hirsel_engine::FrontLowerQueries>>,
    owner: Arc<dyn Send + Sync>,
}
impl CompileInspection {
    /// The current value of a source/Lower input, including inputs whose memo
    /// a rejected Check never demanded. This performs no compiler work.
    pub fn observed_read_value(
        &self,
        owner: &DefPath,
        read: &hirsel_engine::ReadKey,
    ) -> Option<hirsel_syntax::ContentHash> {
        self.observations.as_ref()?.observed_read_value(owner, read)
    }
    pub(crate) fn with_observations(
        mut self,
        lower: Arc<hirsel_engine::FrontLowerQueries>,
    ) -> Self {
        self.observations = Some(lower);
        self
    }
    pub(crate) fn new(
        stats: hirsel_engine::memo::MemoStats,
        memos: Vec<(hirsel_engine::memo::MemoKey, Arc<hirsel_engine::Reads>)>,
        definitions: Vec<DefinitionWork>,
    ) -> Self {
        Self {
            stats,
            memos,
            definitions,
            observations: None,
            owner: Arc::new(()),
        }
    }
}
impl std::fmt::Debug for CompileInspection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompileInspection")
            .field("stats", &self.stats)
            .field("memos", &self.memos)
            .field("definitions", &self.definitions)
            .finish()
    }
}
/// One completed operation. Its metadata belongs to the same immutable input
/// snapshot as its result and does not ask the disk for newer observations.
pub struct CompileOutput {
    pub entry: DefPath,
    pub inputs: FrozenInputs,
    pub work: RequestWork,
    pub result: CompileResult,
    owner: Arc<dyn Send + Sync>,
}
impl CompileOutput {
    pub fn new(
        entry: DefPath,
        inputs: FrozenInputs,
        work: RequestWork,
        result: CompileResult,
    ) -> Self {
        Self {
            entry,
            inputs,
            work,
            result,
            owner: Arc::new(()),
        }
    }
    pub(crate) fn retain_owner(mut self, owner: Arc<dyn Send + Sync>) -> Self {
        match &mut self.result {
            CompileResult::Check(checked)
            | CompileResult::SafeCheck { checked, .. }
            | CompileResult::CheckedProducts { checked, .. } => {
                checked.sources =
                    super::host_selector::retain_sources(checked.sources.clone(), owner.clone());
            }
            CompileResult::Tir(execution) => {
                let execution = Arc::make_mut(execution);
                execution.sources =
                    super::host_selector::retain_sources(execution.sources.clone(), owner.clone());
            }
            CompileResult::TirBatch(batch) => {
                Arc::make_mut(batch).owner = owner.clone();
            }
            CompileResult::Inspect(inspection) => {
                Arc::make_mut(inspection).owner = owner.clone();
            }
            _ => {}
        }
        self.owner = owner;
        self
    }
}
#[derive(Clone)]
pub enum CompileResult {
    Inspect(Arc<CompileInspection>),
    Artifacts(std::collections::BTreeMap<String, Vec<u8>>),
    ExportTir(String),
    Expansion(Arc<str>),
    Hashes(Arc<std::collections::BTreeMap<DefPath, hirsel_syntax::ContentHash>>),
    /// Native pipeline failures retain their existing inner error category.
    NativeError(crate::native::NativeError),
    Check(CheckOutput),
    Precomputed {
        definitions: u64,
        digest: hirsel_syntax::ContentHash,
        sources: Arc<PrecomputedSources>,
    },
    SafeCheck {
        checked: CheckOutput,
        reach: Option<(DefPath, Option<UnsafeReach>)>,
    },
    CheckedProducts {
        checked: CheckOutput,
        products: Arc<hirsel_engine::CheckedProducts>,
    },
    RecomputeCounts(Vec<(DefPath, hirsel_engine::QueryKind, u64)>),
    Tir(Arc<TirExecution>),
    TirBatch(Arc<TirBatchExecution>),
    Native(Arc<NativeExecution>),
    Ownership(Arc<super::OwnershipExecution>),
    NativeObjects(Arc<NativeObjectOutput>),
    NativeSources(Arc<hirsel_engine::FrontNativeProducts>),
}
/// Read completed library source products without an entry, runtime metadata
/// or a mutable compiler. Decoding consumes only the reader's explicit grant.
pub struct PrecomputedSources {
    pub(super) run: Arc<hirsel_engine::FrontRunQueries>,
    pub(super) lower: Arc<hirsel_engine::FrontLowerQueries>,
}
impl PrecomputedSources {
    fn product(&self, path: &DefPath) -> Result<hirsel_engine::runtime::ProductRef, QueryError> {
        self.run
            .completed_semantic_product(path)
            .ok_or(QueryError::NotImplemented)
    }
    pub fn definitions(&self) -> impl Iterator<Item = &DefPath> {
        self.run.source_definitions()
    }
    pub fn signatures(&self) -> Arc<hirsel_engine::SignatureSnapshot> {
        self.lower.signatures()
    }
    pub fn sources(&self) -> &SourceMap {
        self.lower.sources()
    }
    pub fn semantic_node(
        &self,
        path: &DefPath,
    ) -> Result<hirsel_run_schema::ProgramNode, QueryError> {
        self.run.semantic_node(self.product(path)?)
    }
    pub fn semantic_graph_decode_bytes(&self) -> Result<u64, QueryError> {
        self.run.semantic_graph_decode_bytes()
    }
    pub fn semantic_graph(
        &self,
        allowance: hirsel_engine::runtime::MemoryReservation,
    ) -> Result<hirsel_engine::FrontSemanticGraph, QueryError> {
        self.run.semantic_graph(allowance)
    }

    pub fn source_product_bytes(&self, path: &DefPath) -> Result<Option<u64>, QueryError> {
        self.run.source_product_bytes(self.product(path)?)
    }
    pub fn source_products(
        &self,
        path: &DefPath,
        allowance: hirsel_engine::runtime::MemoryReservation,
    ) -> Result<Option<hirsel_engine::FrontSourceProducts>, QueryError> {
        self.run
            .source_products_with_allowance(self.product(path)?, allowance)
    }
    pub fn dependency_decode_bytes(&self, path: &DefPath) -> Result<u64, QueryError> {
        self.lower.settled_dependency_decode_bytes(path)
    }
    pub fn dependencies(
        &self,
        path: &DefPath,
        allowance: hirsel_engine::runtime::MemoryReservation,
    ) -> Result<hirsel_engine::FrontLowerDependencies, QueryError> {
        self.lower.settled_dependencies(path, allowance)
    }
    pub fn definition_decode_bytes(&self, path: &DefPath) -> Result<u64, QueryError> {
        self.lower.settled_definition_decode_bytes(path)
    }
    pub fn definition(
        &self,
        path: &DefPath,
        allowance: &hirsel_engine::runtime::MemoryReservation,
    ) -> Result<DefinitionRef, QueryError> {
        self.lower
            .settled_definition_with_allowance(path, allowance)
    }
}

/// Materialize only already settled, checked execution products. These reads
/// may decode an owned product but never run a compiler query, evaluate a
/// constant or mutate registration. Missing/corrupt content remains an error.
/// This owner has no second semantic invalidation or verifier-receipt table.
pub trait TirProducts: Send + Sync {
    fn definition(&self, path: &DefPath) -> Result<DefinitionRef, QueryError>;
    fn constant(&self, path: &DefPath, actuals: &[u64]) -> Result<MaterializedRef, QueryError>;
    fn constant_definitions(&self) -> Result<Vec<DefPath>, QueryError>;
    fn instance_body(
        &self,
        path: &DefPath,
        actuals: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError>;
}
/// Completed independently bound entry products from one source-closure pass.
#[derive(Clone)]
pub struct TirBatchExecution {
    pub(super) entries: Arc<hirsel_engine::FrontRunBatchOutput>,
    pub(super) lower: Arc<hirsel_engine::FrontLowerQueries>,
    pub(super) owner: Arc<dyn Send + Sync>,
}
impl TirBatchExecution {
    pub fn entry(&self, path: &DefPath) -> Result<Arc<TirExecution>, ToolError> {
        let facts = self
            .entries
            .get(path)
            .ok_or_else(|| ToolError::Runtime("entry is outside the selected TIR batch".into()))?;
        let mut execution = super::tir_products::materialize(facts.clone(), self.lower.clone())?;
        let selected = Arc::make_mut(&mut execution);
        selected.sources = selected.sources.clone().retain_owner(self.owner.clone());
        Ok(execution)
    }
}

/// The selected TIR root and the immutable closure needed to execute it. The
/// runtime contains the exact type, private-view and checked-H metadata for
/// this snapshot; semantic revision remains independent of native artifacts.
#[derive(Clone)]
pub struct TirExecution {
    pub entry: DefPath,
    pub definition: DefinitionRef,
    pub signature: SignatureRef,
    pub sources: SourceMap,
    pub runtime: Arc<hirsel_interp::aggregate::RuntimeTypes>,
    pub checked_runtime: Arc<hirsel_interp::checked::CheckedRuntime>,
    pub code: hirsel_run_schema::CodeReach,
    pub callees: std::collections::BTreeMap<String, hirsel_run_schema::CalleeFacts>,
    pub entries: std::collections::BTreeMap<
        String,
        (hirsel_run_schema::CalleeFacts, hirsel_run_schema::CallForm),
    >,
    pub revision: hirsel_run_schema::ProgramRevision,
    pub foreign_unreplayable: bool,
    pub foreign: hirsel_engine::ForeignUse,
    pub(crate) products: Arc<dyn TirProducts>,
}
impl TirExecution {
    pub fn provider(&self) -> impl TirProvider + '_ {
        ExecutionProvider(self.products.as_ref())
    }
}
struct ExecutionProvider<'a>(&'a dyn TirProducts);
impl TirProvider for ExecutionProvider<'_> {
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.0.definition(path)
    }
    fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError> {
        self.0.constant(path, &[])
    }
    fn constant_instance(
        &mut self,
        path: &DefPath,
        actuals: &[u64],
    ) -> Result<MaterializedRef, QueryError> {
        self.0.constant(path, actuals)
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        self.0.constant_definitions()
    }
    fn instance_body(
        &mut self,
        path: &DefPath,
        actuals: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.0.instance_body(path, actuals)
    }
}
/// Compact linked observation tables. Site ordinals use the selected source
/// anchors, including generated definitions; no Ready/Value body is retained.
#[derive(Clone, Debug, Default)]
pub struct NativeObservation {
    pub failure_rules: Vec<String>,
    pub sites: std::collections::BTreeMap<u64, (hirsel_syntax::DefId, hirsel_lir::ir::SiteRange)>,
    pub anchors: std::collections::BTreeMap<DefPath, Vec<hirsel_span::Anchor>>,
}
/// Executable readiness includes a materialized executable and frozen launch
/// metadata. Construction follows successful Link/materialization checks; the
/// admission/scratch owner lives until the last output user releases it.
pub struct NativeExecution {
    pub executable: PathBuf,
    pub product: hirsel_engine::runtime::ProductRef,
    pub(crate) platform: Arc<super::native_link::FrozenPlatform>,
    pub store: Arc<dyn hirsel_engine::store::ResultStore>,
    pub observation: NativeObservation,
    pub report: Vec<String>,
    pub entry: DefPath,
    pub signature: SignatureRef,
    pub sources: SourceMap,
    pub revision: hirsel_run_schema::ProgramRevision,
    pub request: NativeRequest,
    pub types: Arc<hirsel_types::TypeInterner>,
    pub code: hirsel_run_schema::CodeReach,
    pub callees: std::collections::BTreeMap<String, hirsel_run_schema::CalleeFacts>,
    pub toolchain: Arc<crate::native::Toolchain>,
    pub run: Option<Arc<hirsel_run_config::ValidatedRun>>,
    pub foreign_unreplayable: bool,
    pub foreign: hirsel_engine::ForeignUse,
    pub(crate) owner: Arc<dyn Send + Sync>,
}
impl NativeExecution {
    /// Keep admitted executable/launch owners alive through a child process.
    pub fn retain(&self) -> Arc<dyn Send + Sync> {
        self.owner.clone()
    }
}

/// Engine-selected materialization after query completion. The implementation
/// is installed by the selector; hosts receive this only inside SelectedRequest.
pub struct CompilerReadiness {
    pub(crate) read_scope: Option<CompileStoreOperation>,
    pub(crate) materialize: Arc<Materialize>,
    pub(crate) failure: Arc<dyn Fn(hirsel_engine::store::Reference) -> ToolError + Send + Sync>,
}
pub(crate) struct CompileStoreOperation {
    pub scope: hirsel_engine::store::StoreOperation,
    pub completed: Arc<std::sync::atomic::AtomicBool>,
}
impl CompileStoreOperation {
    fn finish(self) -> std::io::Result<()> {
        let result = self.scope.finish();
        if result.is_ok() {
            self.completed
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }
}
pub(crate) type Materialize = dyn Fn(&[hirsel_engine::runtime::ProductRef]) -> Result<Arc<CompileOutput>, ToolError>
    + Send
    + Sync;

impl hirsel_engine::requests::OutputReadiness for CompilerReadiness {
    type Output = Arc<CompileOutput>;
    type Error = ToolError;
    fn failed(&mut self, failure: hirsel_engine::store::Reference) -> Option<Self::Error> {
        let _read_scope = self.read_scope.take();
        Some((self.failure)(failure))
    }
    fn ready(
        &mut self,
        products: &[hirsel_engine::runtime::ProductRef],
    ) -> Result<Self::Output, Self::Error> {
        let operation = self.read_scope.take();
        let output = (self.materialize)(products)?;
        let complete = match &output.result {
            CompileResult::Check(checked) => {
                checked.completion == hirsel_engine::CheckCompletion::Complete
            }
            _ => true,
        };
        if complete && let Some(operation) = operation {
            let publication = operation.finish();
            if matches!(output.result, CompileResult::Precomputed { .. }) {
                publication.map_err(|error| ToolError::Runtime(error.to_string()))?;
            }
        }
        Ok(output)
    }
}

/// Settled object products. The retained owner holds the same admitted source
/// and native producers; the store provides ordinary content, never authority.
pub struct NativeObjectOutput {
    pub products: Vec<hirsel_engine::runtime::ProductRef>,
    pub instances: usize,
    pub store: Arc<dyn hirsel_engine::store::ResultStore>,
    pub(super) _owner: Arc<dyn Send + Sync>,
}
