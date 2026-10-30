//! Demand-driven ordinary Lower and constant evaluation. Rust futures retain
//! preparation and interpreter state on the admitted home worker. Publication
//! into a source namespace remains a separate ordered transaction.
use super::*;
mod carry;
mod constants;
mod dependencies;
mod depth;
mod evidence;
mod inputs;
mod inspection;
mod reads;
mod registration;
mod staged;
pub(super) mod staged_runtime;
mod stored;
use crate::comptime::{EvaluationInput, PreparingSource, ValuePreparation};
use crate::memo::{DepthEvidence, dependency_limit};
use crate::runtime::*;
use crate::store::{Domain, EntryKey, Reference, ResultStore};
pub use carry::LowerCarry;
use constants::Constants;
use dependencies::Dependencies;
use evidence::Evidence;
use hirsel_tir::{DefinitionRef, Materialized, MaterializedRef};
use inputs::Inputs;
pub use inspection::FrontLowerDependencies;
pub use registration::{
    FRONT_REGISTRATION_KIND, FrontRegistrationOutput, FrontRegistrationQueries,
};
use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    pin::Pin,
    rc::Rc,
    sync::Mutex,
    task::{Context, Poll, Waker},
};

pub const FRONT_LOWER_KIND: u32 = 0x4600_0003;
pub const FRONT_EVALUATE_KIND: u32 = 0x4600_0004;
const FAILURE: Domain = Domain {
    kind: 0x4600_0005,
    schema: 3,
};
const LOWER_DESCRIPTOR: Domain = Domain {
    kind: 0x4600_0006,
    schema: 7,
};
// Ordinary immutable substitution bytes, owned by the selected provider like
// the original body archive. Inspectors decode under their working allowance.
const LOWER_MATERIALIZED: Domain = Domain {
    kind: 0x4600_0032,
    schema: 1,
};
const CONSTANT_DESCRIPTOR: Domain = Domain {
    kind: 0x4600_0007,
    schema: 4,
};
const LOWER_VERIFICATION: Domain = Domain {
    kind: 0x4600_0033,
    schema: 1,
};
#[derive(Clone)]
struct StoredVerification {
    authority: hirsel_validate::StoredDefinitionOutput,
    metadata: Reference,
}
/// Physical producer work, including work before an eventual failure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrontLowerWork {
    pub lowerings: u64,
    pub initializers: u64,
    pub validation_runs: u64,
}
/// One original's retained decoded artifact and temporary decoding workspace.
/// Sequential inspectors retain every artifact but reuse one scratch region.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DefinitionDecodeLayout {
    pub retained: u64,
    pub transient: u64,
}
impl DefinitionDecodeLayout {
    pub fn total(self) -> Option<u64> {
        self.retained.checked_add(self.transient)
    }
}
struct Input {
    lower: EntryKey,
    evaluate: EntryKey,
    implementation: ContentHash,
    site: Span,
}
#[derive(Clone)]
enum Value {
    Lower(Box<LowerProduct>),
    Constant(Box<Constants>),
}
type RawLowerOutput = (
    QueryOutput<DefinitionRef>,
    Vec<(hirsel_syntax::ExprId, Materialized)>,
);
struct Computed {
    value: Value,
    verification: Option<(DefinitionRef, hirsel_validate::VerifiedDefinition)>,
    raw: Option<RawLowerOutput>,
}
struct Publication {
    domain: Domain,
    bytes: hirsel_artifact_store::Bytes,
    persist: bool,
    raw: Option<RawLowerOutput>,
}
#[derive(Clone)]
struct LowerProduct {
    live: Option<WeakLowerProduct>,
    definition: DefId,
    body: Reference,
    diagnostics: Vec<Diagnostic>,
    dependencies: Dependencies,
    archived_metadata: Option<Box<reads::Archive>>,
    verified: Option<StoredVerification>,
    acceptance: Option<hirsel_validate::AcceptedArtifact>,
    materialized: Reference,
    definition_decode_layout: DefinitionDecodeLayout,
    constant_selection_bytes: u64,
}
impl LowerProduct {
    fn verified(
        &self,
        owner: &FrontLowerQueries,
    ) -> Option<Option<hirsel_validate::VerifiedDefinitionOutput>> {
        if let Some(live) = self.live.as_ref().and_then(WeakLowerProduct::upgrade) {
            return Some(live.observations().verified.clone());
        }
        let Some(stored) = &self.verified else {
            return Some(None);
        };
        if stored.authority.decode_bytes() > owner.weight.scratch {
            return None;
        }
        let cache = owner.store.verifier()?;
        let bytes = owner.store.get_content(stored.metadata)?;
        stored
            .authority
            .load(
                &cache,
                owner.snapshot.types(),
                owner.snapshot.validation(&self.definition.path().0.package),
                &bytes,
                owner.weight.scratch,
            )
            .map(Some)
    }
    fn dependencies(&self, owner: &FrontLowerQueries) -> Option<Vec<SignatureDependency>> {
        if let Some(live) = self.live.as_ref().and_then(WeakLowerProduct::upgrade) {
            return Some(live.output().dependencies.clone());
        }
        match &self.archived_metadata {
            None => Some(self.dependencies.to_vec()),
            Some(archive) => archive.dependencies(
                &owner.records,
                &owner.anchors,
                &*owner.store,
                owner.weight.scratch,
            ),
        }
    }
    fn decode(&self, owner: &FrontLowerQueries) -> Option<QueryOutput<DefinitionRef>> {
        if let Some(live) = self.live.as_ref().and_then(WeakLowerProduct::upgrade) {
            return Some(live.output().clone());
        }
        let definition = decode_definition(owner, self.body)?;
        if definition.path != self.definition {
            return None;
        }
        Some(QueryOutput {
            value: definition,
            diagnostics: self.diagnostics.clone(),
            dependencies: self.dependencies(owner)?,
        })
    }
}
fn decode_definition(owner: &FrontLowerQueries, body: Reference) -> Option<DefinitionRef> {
    let source = Arc::new(owner.anchors.clone());
    Some(Arc::new(
        owner
            .records
            .open_tir(source.clone(), owner.store.get_content(body)?, body, source)?
            .materialize()?,
    ))
}
#[derive(Clone)]
struct Outcome {
    result: Result<Value, QueryError>,
    reads: Reads,
    reads_decode_bytes: u64,
    selections: staged::Selections,
    depth: depth::Depth,
    executed: bool,
    execution: Option<hirsel_validate::ExecutionReceipt>,
    execution_subject: ContentHash,
}
impl Outcome {
    fn archive(&self) -> Option<&reads::Archive> {
        match &self.result {
            Ok(Value::Lower(lower)) => lower.archived_metadata.as_deref(),
            Ok(Value::Constant(value)) => value.archived_reads.as_deref(),
            _ => None,
        }
    }
    fn read_len(&self) -> usize {
        if let Ok(Value::Lower(lower)) = &self.result
            && let Some(live) = lower.live.as_ref().and_then(WeakLowerProduct::upgrade)
        {
            return live.reads().len();
        }
        self.archive()
            .map_or_else(|| self.reads.len(), |archive| archive.len)
    }
    fn reads(&self, owner: &FrontLowerQueries) -> Option<Reads> {
        if let Ok(Value::Lower(lower)) = &self.result
            && let Some(live) = lower.live.as_ref().and_then(WeakLowerProduct::upgrade)
        {
            return Some(live.reads().clone());
        }
        match self.archive() {
            None => Some(self.reads.clone()),
            Some(archive) => archive.load(
                &owner.records,
                &owner.anchors,
                &*owner.store,
                owner.weight.scratch,
            ),
        }
    }
    fn archive_reads(&mut self, reference: Reference) {
        let read_bytes = self
            .reads
            .allocations()
            .map(|(_, bytes)| bytes)
            .sum::<usize>();
        let archive_bytes = size_of::<reads::Archive>() + 2 * size_of::<usize>();
        match &mut self.result {
            Ok(Value::Lower(lower)) if self.reads_decode_bytes != 0 => {
                // One descriptor owns both sequences; neither retained copy is
                // useful once their joint archive is smaller.
                if read_bytes.saturating_add(lower.dependencies.retained_bytes()) > archive_bytes {
                    lower.archived_metadata = Some(Box::new(reads::Archive {
                        reference,
                        decode_bytes: self.reads_decode_bytes,
                        len: self.reads.len(),
                        dependencies: lower.dependencies.len(),
                    }));
                    self.reads = Reads::empty();
                    lower.dependencies = Vec::new().into();
                }
            }
            Ok(Value::Constant(value)) if read_bytes > archive_bytes => {
                value.archived_reads = Some(Box::new(reads::Archive {
                    reference,
                    decode_bytes: value.working_bytes(),
                    len: self.reads.len(),
                    dependencies: 0,
                }));
                self.reads = Reads::empty();
            }
            _ => {}
        }
    }

    fn retained_bytes(&self) -> Option<u64> {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<Self>() + 2 * size_of::<usize>());
        for (_, bytes) in self.reads.allocations() {
            weight.add(bytes);
        }
        weight.add(self.depth.retained_bytes());
        weight.add(staged::selection_bytes(&self.selections).try_into().ok()?);
        match &self.result {
            Ok(Value::Lower(lower)) => {
                weight.add(size_of::<LowerProduct>() + 2 * size_of::<usize>());
                weight.add(
                    crate::memo::retained_lowering_output_bytes(&lower.diagnostics, &Vec::new())
                        .try_into()
                        .ok()?,
                );
                weight.add(lower.dependencies.retained_bytes());
                if lower.archived_metadata.is_some() {
                    weight.add(size_of::<reads::Archive>() + 2 * size_of::<usize>());
                }
                // AcceptedArtifact and every other inline product field are
                // included in the LowerProduct allocation above.
            }
            Ok(Value::Constant(value)) => {
                weight.add(size_of::<Constants>() + 2 * size_of::<usize>());
                weight.add(value.retained_bytes().try_into().ok()?);
                if value.archived_reads.is_some() {
                    weight.add(size_of::<reads::Archive>() + 2 * size_of::<usize>());
                }
            }
            Err(error) => weight.add(
                crate::memo::retained::retained_query_error_bytes(error)
                    .try_into()
                    .ok()?,
            ),
        }
        if let Some(receipt) = &self.execution {
            weight.add(receipt.retained_bytes()?.try_into().ok()?);
        }
        Some(weight.bytes())
    }
}
// Upper bound for products.rs's temporary required-constant map. Count every
// occurrence, including duplicates and references that will eventually refuse.
fn constant_selection_bytes(definition: &hirsel_tir::Definition) -> Result<u64, QueryError> {
    let mut weight = crate::NativeMetadataWeight::default();
    let mut rows = 1usize; // a constant definition may also require its own value
    weight.path(definition.path.path());
    weight.path(definition.path.path());
    for value in definition.bodies.iter().flat_map(|body| &body.rvalues) {
        if let hirsel_tir::Rvalue::ReadConstant {
            definition,
            arguments,
        } = value
        {
            rows = rows.checked_add(1).ok_or(QueryError::NotImplemented)?;
            weight.path(definition);
            weight.path(definition);
            weight.path(definition); // constant_instance_path's temporary clone
            weight.add(
                arguments
                    .len()
                    .checked_mul(3 * size_of::<u64>() + size_of::<String>() + 80)
                    .ok_or(QueryError::NotImplemented)?,
            );
            weight.add(definition.0.name.len().saturating_mul(2).saturating_add(32));
        }
    }
    weight.tree::<DefPath, (DefPath, Vec<u64>)>(rows);
    Ok(weight.bytes())
}

fn outcome_table_bytes(rows: usize) -> u64 {
    if rows == 0 {
        return 0;
    }
    let mut weight = crate::NativeMetadataWeight::default();
    weight.tree::<(u32, ContentHash), (Reference, Box<Outcome>, u64)>(rows);
    weight.bytes()
}

/// A held Lower product keeps immutable source/output admission alive. The
/// ordinary archive is data; native consumers independently validate it.
#[derive(Clone)]
pub struct FrontLowerOutput {
    pub output: QueryOutput<DefinitionRef>,
    pub reads: Reads,
    pub depth: DepthEvidence<Span>,
    pub execution: Option<hirsel_validate::ExecutionReceipt>,
    pub execution_subject: ContentHash,
    pub executed: bool,
    pub selections: Vec<(DefId, Vec<u64>)>,
    pub acceptance: Option<hirsel_validate::AcceptedArtifact>,
    pub implementation_revision: ContentHash,
    pub signature: ContentHash,
    pub verified: Option<hirsel_validate::VerifiedDefinitionOutput>,
    pub materialized: Vec<(hirsel_syntax::ExprId, Materialized)>,
    query_observations: Option<ProductArchive>,
    owner: Arc<LowerSource>,
}
struct LowerSource {
    anchors: crate::NativeRecordContext,
    records: crate::NativeRecords,
    store: Arc<dyn ResultStore>,
    _sources: SourceMap,
    _allowance: Arc<MemoryReservation>,
}
impl FrontLowerOutput {
    /// Move the typed ordinary result into an admitted live owner. Verifier
    /// capabilities are independent products and cannot be minted by this view.
    pub fn into_live(
        self,
        product: ProductRef,
        allowance: &mut MemoryReservation,
    ) -> Option<crate::runtime::LiveLowerProduct> {
        let source = ProductSource::prepare(
            self.owner.anchors.clone(),
            self.owner.records.clone(),
            self.owner.clone(),
        )?;
        crate::runtime::LiveLowerProduct::new(
            (product.address, product.action),
            self,
            source,
            allowance,
        )
    }

    pub fn store(&self) -> &Arc<dyn ResultStore> {
        &self.owner.store
    }
}
pub struct FrontLowerQueries {
    inputs: Inputs,
    staged: staged::Staged,
    paths: BTreeMap<(u32, ContentHash), DefId>,
    /// Implementation hashes of the opaque types a lowered body can read the
    /// private representation of: its LayoutBody inputs.
    layouts: BTreeMap<DefId, ContentHash>,
    front: Arc<FrontBodyQueries>,
    snapshot: Arc<SignatureSnapshot>,
    memo: Arc<hirsel_types::TypeMemo>,
    lowering: Option<Arc<dyn LoweringPhase>>,
    runtime: std::sync::OnceLock<Result<Arc<hirsel_interp::aggregate::RuntimeTypes>, QueryError>>,
    instances: Arc<hirsel_tir::InstanceBodies>,
    sources: SourceMap,
    limits: crate::ComptimeLimits,
    verify: Verify,
    anchors: crate::NativeRecordContext,
    records: crate::NativeRecords,
    store: Arc<dyn ResultStore>,
    outcomes: Mutex<BTreeMap<(u32, ContentHash), (Reference, Box<Outcome>, u64)>>,
    carry: Mutex<Option<Arc<LowerCarry>>>,
    carry_changes: Mutex<Option<Arc<carry::CarryChanges>>>,
    clean_memos: Mutex<BTreeSet<crate::memo::MemoKey>>,
    work: Mutex<BTreeMap<DefPath, FrontLowerWork>>,
    weight: MemoryWeight,
    max_reads: usize,
    errors: RuntimeErrors,
    retention: Arc<Mutex<u64>>,
    _allowance: Arc<MemoryReservation>,
    source: Arc<LowerSource>,
}
impl FrontLowerQueries {
    /// Compact potential dependency inventory; no Lower/Evaluate output or
    /// maximum continuation is reserved until that query is actually demanded.
    pub fn input_bytes(definitions: &[DefPath]) -> Option<u64> {
        let mut indexes = crate::NativeMetadataWeight::default();
        indexes.tree::<(u32, ContentHash), DefId>(definitions.len().checked_mul(2)?);
        indexes.add(size_of::<Self>());
        indexes.add(
            size_of::<LowerSource>() + size_of::<MemoryReservation>() + 4 * size_of::<usize>(),
        );
        indexes.add(size_of::<Mutex<u64>>() + 2 * size_of::<usize>());
        definitions.iter().try_fold(indexes.bytes(), |total, path| {
            let path_bytes = bincode::serialized_size(path).ok()?;
            total.checked_add(path_bytes.checked_add(
                (std::mem::size_of::<(DefPath, Input)>() + 6 * std::mem::size_of::<usize>()) as u64,
            )?)
        })
    }
    /// One completed descriptor plus its independently stored ordinary body,
    /// materialized value, reads/depth and opaque acceptance metadata.
    pub fn output_bytes(weight: MemoryWeight) -> Option<u64> {
        weight.retained.checked_mul(2)?.checked_add(
            (std::mem::size_of::<Outcome>() + std::mem::size_of::<Reference>() + 128) as u64,
        )
    }

    /// Input rows plus actual settled metadata/receipt debits. Historical
    /// provider archive traffic and live runtime futures are separate owners.
    pub fn retained_bytes(&self) -> u64 {
        self._allowance.bytes() - *self.retention.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Outcomes adopted through an unchanged subscription generation, without
    /// inspecting the carried memo's individual reads.
    pub fn clean_memos(&self) -> BTreeSet<crate::memo::MemoKey> {
        self.clean_memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    /// Immutable producer grant, including input rows and retained outcomes.
    pub fn admitted_bytes(&self) -> u64 {
        self._allowance.bytes()
    }

    /// Settled memo identities for work accounting. No read archive or body
    /// is decoded merely to count the producer work this graph demanded.
    pub fn memo_keys(&self) -> Vec<crate::memo::MemoKey> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let mut keys = Vec::new();
        for (path, input) in &self.inputs {
            for (kind, phase, address) in [
                (FRONT_LOWER_KIND, QueryKind::Lower, input.lower.0),
                (FRONT_EVALUATE_KIND, QueryKind::Evaluate, input.evaluate.0),
            ] {
                if outcomes.contains_key(&(kind, address)) {
                    keys.push(crate::memo::MemoKey::new(phase, DefId::intern(path)));
                }
            }
        }
        keys
    }

    pub fn memo_snapshot(&self) -> Option<Vec<(crate::memo::MemoKey, Arc<Reads>)>> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows = Vec::new();
        for (path, input) in &self.inputs {
            for (kind, phase, address) in [
                (FRONT_LOWER_KIND, QueryKind::Lower, input.lower.0),
                (FRONT_EVALUATE_KIND, QueryKind::Evaluate, input.evaluate.0),
            ] {
                if let Some((_, outcome, _)) = outcomes.get(&(kind, address)) {
                    rows.push((
                        crate::memo::MemoKey::new(phase, DefId::intern(path)),
                        Arc::new(outcome.reads(self)?),
                    ));
                }
            }
        }
        Some(rows)
    }
    /// Actual completed read evidence, with no producer or mutable-engine call.
    pub fn settled_reads(&self, path: &DefPath, phase: QueryKind) -> Option<Arc<Reads>> {
        let id = match phase {
            QueryKind::Lower => self.query(path)?,
            QueryKind::Evaluate => self.evaluate_query(path)?,
            _ => return None,
        };
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(id.kind, id.address.0))
            .and_then(|(_, outcome, _)| outcome.reads(self).map(Arc::new))
    }
    pub(crate) fn selected_read_len(
        &self,
        id: QueryId,
        selected: Result<ProductRef, Reference>,
    ) -> Option<usize> {
        Some(self.selected(id, selected)?.read_len())
    }
    /// Complete settled reads from the exact selected owner.
    pub(crate) fn selected_reads(
        &self,
        id: QueryId,
        selected: Result<ProductRef, Reference>,
    ) -> Option<Arc<Reads>> {
        self.selected(id, selected)?.reads(self).map(Arc::new)
    }
    pub fn work_snapshot(&self) -> Vec<(DefPath, FrontLowerWork)> {
        self.work
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(path, work)| (path.clone(), *work))
            .collect()
    }
    pub fn sources(&self) -> &SourceMap {
        &self.sources
    }
    pub fn runtime_types(&self) -> Result<Arc<hirsel_interp::aggregate::RuntimeTypes>, QueryError> {
        self.runtime
            .get_or_init(|| {
                let lowering = self.lowering.as_deref().ok_or(QueryError::NotImplemented)?;
                self.snapshot.comptime_runtime_types(lowering)
            })
            .clone()
    }
    pub fn signatures(&self) -> Arc<SignatureSnapshot> {
        self.snapshot.clone()
    }
    pub fn signature(&self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        self.snapshot
            .signature(DefId::intern(path))
            .ok_or(QueryError::NotImplemented)?
            .map(Arc::clone)
            .map_err(Clone::clone)
    }
    /// Captured physical decode working memory for an already settled original.
    /// Reads metadata only; no provider fetch, body decoder or query runs.
    pub fn settled_definition_decode_bytes(&self, path: &DefPath) -> Result<u64, QueryError> {
        self.settled_definition_decode_layout(path)?
            .total()
            .ok_or(QueryError::NotImplemented)
    }
    /// Captured layout only: no provider read, decode, body phase or clone.
    pub fn settled_definition_decode_layout(
        &self,
        path: &DefPath,
    ) -> Result<DefinitionDecodeLayout, QueryError> {
        let id = self.query(path).ok_or(QueryError::NotImplemented)?;
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (_, outcome, _) = outcomes
            .get(&(id.kind, id.address.0))
            .ok_or(QueryError::NotImplemented)?;
        match &outcome.result {
            Ok(Value::Lower(lower)) => Ok(lower.definition_decode_layout),
            Err(error) => Err(error.clone()),
            _ => Err(QueryError::NotImplemented),
        }
    }
    /// Prospective constant-selection maps captured while original TIR was live.
    /// This reads no archive and reconstructs no body.
    pub(crate) fn settled_constant_selection_bytes(
        &self,
        path: &DefPath,
    ) -> Result<u64, QueryError> {
        let id = self.query(path).ok_or(QueryError::NotImplemented)?;
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        match &outcomes
            .get(&(id.kind, id.address.0))
            .ok_or(QueryError::NotImplemented)?
            .1
            .result
        {
            Ok(Value::Lower(lower)) => Ok(lower.constant_selection_bytes),
            Err(error) => Err(error.clone()),
            _ => Err(QueryError::NotImplemented),
        }
    }
    /// Bound all settled constant clones without copying their payloads or paths.
    pub(crate) fn settled_constants_input_bytes(&self) -> Result<u64, QueryError> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let mut weight = crate::NativeMetadataWeight::default();
        weight.tree::<DefPath, Materialized>(outcomes.len());
        for path in self.inputs.keys().cloned().chain(self.staged.instances()) {
            let Some(id) = self.evaluate_query(&path) else {
                continue;
            };
            let Some((_, outcome, _)) = outcomes.get(&(id.kind, id.address.0)) else {
                continue;
            };
            if let Ok(Value::Constant(value)) = &outcome.result {
                weight.path(&path);
                weight.add(
                    value
                        .root_clone_bytes()
                        .checked_mul(2)
                        .and_then(|n| usize::try_from(n).ok())
                        .ok_or(QueryError::NotImplemented)?,
                );
                weight.add(
                    value
                        .decode_bytes()
                        .try_into()
                        .map_err(|_| QueryError::NotImplemented)?,
                );
                weight.add(size_of::<Materialized>() + 2 * size_of::<usize>());
            }
        }
        Ok(weight.bytes())
    }
    /// Decode under an explicitly retained caller working reservation. The
    /// precheck precedes provider I/O; callers keep this grant alive while any
    /// returned body clone is held. No Lower work or query is requested.
    pub fn settled_definition_with_allowance(
        self: &Arc<Self>,
        path: &DefPath,
        allowance: &MemoryReservation,
    ) -> Result<DefinitionRef, QueryError> {
        if self.settled_definition_decode_bytes(path)? > allowance.bytes() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "original body inspection".into(),
                message: "captured decode buffers exceed caller working admission".into(),
            }));
        }
        self.settled_definition(path)
    }
    pub fn settled_definition(
        self: &Arc<Self>,
        path: &DefPath,
    ) -> Result<DefinitionRef, QueryError> {
        let id = self.query(path).ok_or(QueryError::NotImplemented)?;
        let selected = self
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(id.kind, id.address.0))
            .cloned()
            .ok_or(QueryError::NotImplemented)?;
        match selected.1.result? {
            Value::Lower(lower) => lower
                .decode(self)
                .map(|output| output.value)
                .ok_or(QueryError::NotImplemented),
            _ => Err(QueryError::NotImplemented),
        }
    }
    pub(crate) fn settled_constant_clone_bytes(&self, path: &DefPath) -> Result<u64, QueryError> {
        let id = self
            .evaluate_query(path)
            .ok_or(QueryError::NotImplemented)?;
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (_, outcome, _) = outcomes
            .get(&(id.kind, id.address.0))
            .ok_or(QueryError::NotImplemented)?;
        match &outcome.result {
            Ok(Value::Constant(value)) => {
                let root = value
                    .root_clone_bytes()
                    .checked_add(
                        (std::mem::size_of::<Materialized>() + 2 * std::mem::size_of::<usize>())
                            as u64,
                    )
                    .and_then(|bytes| bytes.checked_mul(2))
                    .ok_or(QueryError::NotImplemented)?;
                root.checked_add(value.decode_bytes())
                    .ok_or(QueryError::NotImplemented)
            }
            Err(error) => Err(error.clone()),
            _ => Err(QueryError::NotImplemented),
        }
    }
    pub fn settled_constant(
        &self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<MaterializedRef, QueryError> {
        let selected = if arguments.is_empty() {
            path.clone()
        } else {
            hirsel_tir::constant_instance_path(path, arguments)
        };
        let id = self
            .evaluate_query(&selected)
            .ok_or(QueryError::NotImplemented)?;
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let selected = outcomes
            .get(&(id.kind, id.address.0))
            .ok_or(QueryError::NotImplemented)?;
        match &selected.1.result {
            Ok(Value::Constant(value)) => value
                .decoded(
                    &self.records,
                    &self.anchors,
                    &*self.store,
                    self.weight.scratch,
                )
                .map(|(value, _)| Arc::new(value))
                .ok_or(QueryError::NotImplemented),
            Err(error) => Err(error.clone()),
            _ => Err(QueryError::NotImplemented),
        }
    }
    pub fn settled_constants(&self) -> Vec<DefPath> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        self.inputs
            .keys()
            .cloned()
            .chain(self.staged.instances())
            .filter(|path| {
                self.evaluate_query(path).is_some_and(|id| {
                    outcomes
                        .get(&(id.kind, id.address.0))
                        .is_some_and(|(_, outcome, _)| {
                            matches!(outcome.result, Ok(Value::Constant(..)))
                        })
                })
            })
            .collect()
    }
    pub fn settled_instance(
        &self,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.instances
            .resolve(DefId::intern(path), self.snapshot.types(), arguments)
            .map(|selected| selected.map(|id| id.path().clone()))
    }

    pub(crate) fn instance_definitions(&self) -> impl Iterator<Item = &DefPath> {
        self.instances
            .instances
            .values()
            .flat_map(|instances| instances.values())
            .map(|instance| instance.definition.path())
    }

    /// Close CT-5 calls through generic helpers using this view's settled
    /// summaries and instance table. Unlike source registration, Run cannot
    /// create a missing instance or execute its generator.
    pub(crate) fn selected_instances(
        &self,
        roots: impl IntoIterator<Item = DefPath>,
        limit: u64,
    ) -> Result<BTreeSet<DefPath>, QueryError> {
        if self.instances.declarations.is_empty() {
            return Ok(BTreeSet::new());
        }
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "Run instance selection".into(),
                message: "closed instance selection exceeds admission or lacks a settled summary"
                    .into(),
            })
        };
        let mut pending: Vec<_> = roots
            .into_iter()
            .map(|p| (DefId::intern(&p), BTreeMap::new()))
            .collect();
        let mut seen = BTreeSet::new();
        let mut selected = BTreeSet::new();
        let mut work = crate::NativeMetadataWeight::default();
        work.flat_vec(&pending);
        while let Some((path, bindings)) = pending.pop() {
            let key: Vec<_> = bindings
                .iter()
                .map(
                    |(site, actuals): (&hirsel_types::SiteRef, &hirsel_types::Actuals)| {
                        (
                            *site,
                            hirsel_tir::InstanceBodies::key(
                                self.snapshot.types(),
                                &actuals.arguments,
                            ),
                        )
                    },
                )
                .collect();
            work.flat_vec(&key);
            if !seen.insert((path, key)) {
                continue;
            }
            work.add(
                size_of::<(
                    DefId,
                    Vec<(
                        hirsel_types::SiteRef,
                        Option<hirsel_types::GeneratedActualKey>,
                    )>,
                )>() + 16 * size_of::<usize>(),
            );
            if seen.len() > 65536 || work.bytes() > limit {
                return Err(refused());
            }
            if !self.inputs.contains_key(path.path()) || self.instances.declarations.contains(&path)
            {
                continue;
            }
            let product = self.front.checked_definition(path.path()).ok_or_else(|| {
                self.front
                    .completed_error(path.path())
                    .unwrap_or_else(refused)
            })?;
            let summary = &product.summary.summary;
            if bindings.is_empty() && summary.frames[0].generic {
                continue;
            }
            for call in summary
                .calls
                .iter()
                .filter(|call| call.target_frame.is_none())
            {
                if !self.inputs.contains_key(call.callee.path()) {
                    continue;
                }
                let actuals = hirsel_types::close_arguments(
                    self.snapshot.types(),
                    &self.memo,
                    &call.arguments,
                    &bindings,
                )
                .map_err(|_| refused())?;
                let closed =
                    hirsel_tir::InstanceBodies::key(self.snapshot.types(), &actuals.arguments)
                        .is_some();
                let next = if self.instances.declarations.contains(&call.callee) {
                    if !closed {
                        continue;
                    }
                    let instance = self
                        .instances
                        .resolve(call.callee, self.snapshot.types(), &actuals.arguments)?
                        .ok_or_else(refused)?;
                    if !selected.contains(instance.path()) {
                        work.tree::<DefPath, ()>(0);
                        work.path(instance.path());
                        if work.bytes() > limit {
                            return Err(refused());
                        }
                        selected.insert(instance.path().clone());
                    }
                    (instance, BTreeMap::new())
                } else if actuals.arguments.is_empty() {
                    (call.callee, BTreeMap::new())
                } else if closed {
                    (call.callee, BTreeMap::from([(call.binder, actuals)]))
                } else {
                    continue;
                };
                // Bound all visited and queued cursor allocations cumulatively;
                // released cursors never grant extra space to this walk.
                work.add(size_of_val(&next) + 16 * size_of::<usize>());
                work.tree::<hirsel_types::SiteRef, hirsel_types::Actuals>(next.1.len());
                for actuals in next.1.values() {
                    work.flat_vec(&actuals.arguments);
                }
                if work.bytes() > limit {
                    return Err(refused());
                }
                pending.push(next);
            }
        }
        Ok(selected)
    }

    pub fn work(&self, path: &DefPath) -> FrontLowerWork {
        self.work
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .copied()
            .unwrap_or_default()
    }
    fn count(&self, path: &DefPath, count: impl FnOnce(&mut FrontLowerWork)) {
        count(
            self.work
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(path.clone())
                .or_default(),
        );
    }
    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        self.id(path, FRONT_LOWER_KIND)
    }
    pub fn evaluate_query(&self, path: &DefPath) -> Option<QueryId> {
        self.id(path, FRONT_EVALUATE_KIND)
    }
    fn id(&self, path: &DefPath, kind: u32) -> Option<QueryId> {
        let Some(input) = self.inputs.get(path) else {
            return (kind == FRONT_EVALUATE_KIND)
                .then(|| self.staged.selected(path))
                .flatten();
        };
        let key = if kind == FRONT_LOWER_KIND {
            &input.lower
        } else {
            &input.evaluate
        };
        Some(QueryId {
            kind,
            address: hirsel_artifact_store::Address(key.0),
        })
    }
    fn path(&self, id: QueryId) -> Option<DefPath> {
        self.paths
            .get(&(id.kind, id.address.0))
            .map(|path| path.path().clone())
            .or_else(|| {
                self.staged
                    .selection(id)
                    .map(|selection| selection.owner.path().clone())
            })
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn LocalQueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontLowerOutput> {
        if let Some(live) = product.lower_value() {
            self.selected(
                QueryId {
                    kind: FRONT_LOWER_KIND,
                    address: product.address,
                },
                Ok(product.clone()),
            )?;
            return Some(live.observations().clone());
        }
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (reference, outcome, _) = outcomes.get(&(FRONT_LOWER_KIND, product.address.0))?;
        if *reference
            != product
                .archive(ArchiveAdmission::Retained)
                .ok()?
                .reference()
        {
            return None;
        }
        let Value::Lower(lower) = outcome.result.as_ref().ok()? else {
            return None;
        };
        let lower = lower.clone();
        let reads = outcome.reads(self)?;
        let depth = outcome.depth.evidence();
        let execution = outcome.execution.clone();
        let execution_subject = outcome.execution_subject;
        let executed = outcome.executed;
        let selections = outcome.selections.clone();
        drop(outcomes);
        let output = lower.decode(self)?;
        let materialized = self
            .records
            .decode(&self.anchors, &self.store.get_content(lower.materialized)?)?;
        let input = self.inputs.get(lower.definition.path())?;
        let signature = self
            .snapshot
            .signature(output.value.path)?
            .as_ref()
            .ok()?
            .fingerprint;
        Some(FrontLowerOutput {
            output,
            reads,
            depth,
            execution,
            execution_subject,
            executed,
            selections,
            acceptance: lower.acceptance.clone(),
            implementation_revision: input.implementation,
            signature,
            verified: lower.verified(self)?,
            materialized,
            query_observations: Some(product.archive(ArchiveAdmission::Retained).ok()?),
            owner: self.source.clone(),
        })
    }
    /// Native adaptation reads compact metadata only. Neither an unchanged
    /// Definition nor a native Value/Ready body is reconstructed here.
    pub fn native_product(&self, product: ProductRef) -> Option<crate::NativeOriginalProduct> {
        self.selected(
            QueryId {
                kind: FRONT_LOWER_KIND,
                address: product.address,
            },
            Ok(product.clone()),
        )?;
        let live = product.lower_value()?.clone();
        let observations = live.observations();
        Some(crate::NativeOriginalProduct {
            definition: live.output().value.path.path().clone(),
            implementation_revision: observations.implementation_revision,
            signature: observations.signature,
            verified: observations.verified.clone(),
            body: crate::NativeOriginalValue::Live(live),
        })
    }
    /// Complete settled query observations for Check. This retains the existing
    /// compact descriptor, without retaining or archiving the live TIR owner.
    pub(crate) fn observation_product(&self, product: ProductRef) -> Option<ProductRef> {
        let id = QueryId {
            kind: FRONT_LOWER_KIND,
            address: product.address,
        };
        self.selected(id, Ok(product.clone()))?;
        let archive = product
            .lower_value()?
            .observations()
            .query_observations
            .clone()?;
        Some(ProductRef::import_archive(
            product.address,
            product.action,
            archive,
        ))
    }
    pub fn failure(&self, reference: Reference) -> Option<QueryError> {
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|(r, _, _)| *r == reference)?
            .1
            .result
            .as_ref()
            .err()
            .cloned()
    }
    fn selected(&self, id: QueryId, selected: Result<ProductRef, Reference>) -> Option<Outcome> {
        self.outcomes.lock().unwrap_or_else(|e| e.into_inner())
            .get(&(id.kind, id.address.0))
            .filter(|(reference, outcome, _)| match &selected {
                Ok(product) => {
                    if let Some(live) = product.lower_value() {
                        product.address == live.address() && product.action == live.action()
                            && matches!(&outcome.result, Ok(Value::Lower(lower)) if lower.live.as_ref().and_then(WeakLowerProduct::upgrade).is_some_and(|stored| stored.same_owner(live)))
                    } else {
                        product.archive(ArchiveAdmission::Retained).is_ok_and(|archive| archive.reference() == *reference)
                    }
                }
                Err(failure) => failure == reference,
            })
            .map(|(_, outcome, _)| outcome.as_ref().clone())
    }
    async fn run(
        self: Arc<Self>,
        id: QueryId,
        path: DefPath,
        reads: QueryReads,
    ) -> Result<ProductRef, Reference> {
        let _read_scope = self.store.read_scope();
        let _publication = self.store.publication_scope();
        let input = &self.inputs[&path];
        let selection = self.staged.selection(id);
        let key = if let Some(selection) = &selection {
            &selection.key
        } else if id.kind == FRONT_LOWER_KIND {
            &input.lower
        } else {
            &input.evaluate
        };
        // A constant's checked Lower body does not replace its value product.
        // Restore Evaluate first even when the Lower descriptor itself hits;
        // receipt readers and other settled consumers cannot request it later.
        let can_restore = if id.kind == FRONT_LOWER_KIND
            && self
                .snapshot
                .signature(DefId::intern(&path))
                .is_some_and(|signature| {
                    signature.is_ok_and(|signature| {
                        matches!(signature.kind, hirsel_types::SignatureKind::Const { .. })
                    })
                }) {
            let evaluate = self.evaluate_query(&path).ok_or(self.errors.protocol)?;
            reads.read_consuming(evaluate).await.is_ok()
        } else {
            true
        };
        if can_restore {
            if let Some(product) = self.load_carry(id, &path, key, &reads).await {
                match product {
                    // The compact descriptor can outlive V's independently
                    // evictable content. A current V miss needs the ordinary
                    // producer, even while the source observations still match.
                    Ok(product)
                        if id.kind == FRONT_LOWER_KIND
                            && self.native_product(product.clone()).is_none() => {}
                    product => return product,
                }
            }
            if let Some(product) = self.load_stored(id, &path, key, &reads).await {
                return product;
            }
        }
        // The live future and working bodies use this task's runtime weight.
        // Only actual settled descriptor/receipt ownership is retained below;
        // provider archives and unused maximum task weights are not ownership.
        let mut source = Source {
            owner: self.clone(),
            reads,
            observed: Rc::new(RefCell::new(ReadsBuilder::new())),
            selections: Rc::new(RefCell::new(Vec::new())),
            refusal: Rc::new(Cell::new(None)),
            depth: Rc::new(RefCell::new(DepthEvidence::leaf(dependency_limit(
                self.limits.call_depth,
            )))),
            framed: id.kind == FRONT_EVALUATE_KIND,
            evidence: Rc::new(RefCell::new(Evidence::new(
                self.verify,
                self.weight.retained,
            ))),
        };
        // A root observes its own body before preparation, including when
        // that preparation fails. Nested callers keep the same root receipt.
        source.record(
            ReadKey::ExecutedBody(DefId::intern(&path)),
            memos::optional(Some(self.inputs[&path].implementation)),
        );
        let computed = if source.framed && source.depth.borrow().limit() == 0 {
            if self.verify == Verify::On {
                let mut observed = ReadsBuilder::new();
                FrozenProvider::new(&self.snapshot, path.0.package.clone(), &mut observed)
                    .validation_universe();
                source.merge(&observed.finish().map_err(|_| self.errors.protocol)?);
            }
            Err(source.overflow(self.inputs[&path].site))
        } else {
            self.compute(id.kind, &path, selection.as_ref(), &mut source)
                .await
        };
        let (mut result, verification, raw) = match computed {
            Ok(computed) => (Ok(computed.value), computed.verification, computed.raw),
            Err(error) => (Err(error), None, None),
        };
        // A missing dependency outcome is a scheduler/transport refusal, not
        // a typed body error. Preserve its exact identity through all clones
        // of the asynchronous source before constructing any descriptor.
        if let Some(reference) = source.refusal.get() {
            return Err(reference);
        }
        let mut depth = source.depth.borrow().clone();
        if source.framed {
            depth.first[0] = None;
        }
        let observed = std::mem::take(&mut *source.observed.borrow_mut())
            .finish()
            .map_err(|_| self.errors.protocol)?;
        let executed = source.evidence.borrow().executed;
        let execution = result
            .as_ref()
            .ok()
            .and_then(|_| source.evidence.borrow().finish(&observed));
        let selections = std::mem::take(&mut *source.selections.borrow_mut());
        let mut reads_decode_bytes = 0;
        let bytes = match &mut result {
            Ok(Value::Lower(lower)) => {
                let mut weight = crate::NativeMetadataWeight::default();
                weight.add(size_of::<reads::Descriptor>());
                weight.add(
                    crate::memo::retained_lowering_output_bytes(&lower.diagnostics, &Vec::new())
                        .try_into()
                        .map_err(|_| self.errors.resource)?,
                );
                weight.add(
                    lower
                        .dependencies
                        .len()
                        .checked_mul(size_of::<SignatureDependency>())
                        .ok_or(self.errors.resource)?,
                );
                for (_, bytes) in observed.allocations() {
                    weight.add(bytes);
                }
                weight.add(size_of_val(&*depth.first));
                weight.add(
                    staged::selection_bytes(&selections)
                        .try_into()
                        .map_err(|_| self.errors.resource)?,
                );
                let (bytes, working) = self
                    .records
                    .encode_with_decode_bytes(
                        &self.anchors,
                        &(
                            lower.definition,
                            lower.body,
                            &lower.diagnostics,
                            &lower.dependencies,
                            &lower.materialized,
                            &observed,
                            &depth,
                            executed,
                            &selections,
                        ),
                    )
                    .ok_or(self.errors.resource)?;
                reads_decode_bytes = weight
                    .bytes()
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(working))
                    .ok_or(self.errors.resource)?;
                Some(bytes)
            }
            Ok(Value::Constant(value)) => value.encode(
                &self.records,
                &self.anchors,
                &observed,
                &depth,
                executed,
                &selections,
            ),
            Err(error) => self
                .records
                .encode(&self.anchors, &(&path, error, &observed, &depth, executed)),
        }
        .ok_or(self.errors.resource)?;
        // The archive is an active encoding buffer. Settled descriptor/value
        // metadata is debited below from this producer's separate grant.
        if bytes.len() as u64 > self.weight.scratch {
            return Err(self.errors.resource);
        }
        // Descriptor domains are explicit: an S5 transport descriptor must
        // never overwrite a scalar Lower/Evaluate archive of another schema.
        let domain = if result.is_err() {
            FAILURE
        } else if id.kind == FRONT_LOWER_KIND {
            LOWER_DESCRIPTOR
        } else {
            CONSTANT_DESCRIPTOR
        };
        let execution_subject = persistent::evaluation_subject(key, &bytes);
        if let Some((definition, evidence)) = verification
            && let Ok(Value::Lower(lower)) = &mut result
            && let Some(cache) = self.store.verifier()
        {
            // Keep the checked body alive only until V binds this exact
            // descriptor, including its read/depth/selection/execution fields.
            let working = lower
                .definition_decode_layout
                .retained
                .checked_add(bytes.capacity() as u64)
                .and_then(|n| {
                    n.checked_add(
                        evidence
                            .reads()
                            .capacity()
                            .checked_mul(size_of::<hirsel_validate::VRead>())?
                            as u64,
                    )
                })
                .ok_or(self.errors.resource)?;
            if working > self.weight.scratch {
                return Err(self.errors.resource);
            }
            lower.verified = cache
                .publish_definition(
                    execution_subject,
                    &definition,
                    self.snapshot.types().clone(),
                    self.snapshot.source_verification_policy(),
                    self.snapshot.validation(&path.0.package),
                    &evidence,
                    &[],
                )
                .and_then(|output| {
                    let (authority, bytes) = cache.compact_definition_output(&output)?;
                    if bytes.len() as u64 > self.weight.scratch {
                        return None;
                    }
                    let metadata = self.store.put_content(LOWER_VERIFICATION, &bytes)?;
                    Some(StoredVerification {
                        authority,
                        metadata,
                    })
                });
        }
        let outcome = Outcome {
            result,
            reads: observed,
            reads_decode_bytes,
            selections,
            depth: depth.into(),
            executed,
            execution,
            execution_subject,
        };
        self.remember_carry(id, key, &bytes, &outcome);
        self.publish(
            id,
            key,
            outcome,
            Publication {
                domain,
                bytes: bytes.into(),
                persist: true,
                raw,
            },
            &source.reads,
        )
        .await
    }
    /// Settle one descriptor: store it, admit its outcome under this producer's
    /// retention and associate a successful product.
    async fn publish(
        &self,
        id: QueryId,
        key: &EntryKey,
        mut outcome: Outcome,
        publication: Publication,
        query_reads: &QueryReads,
    ) -> Result<ProductRef, Reference> {
        let Publication {
            domain,
            bytes,
            persist,
            raw,
        } = publication;
        let record = self
            .store
            .put_content(domain, &bytes)
            .ok_or(self.errors.resource)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.scheduled-action.v1",
            [
                key.0.0.as_slice(),
                key.1.0.as_slice(),
                crate::store::payload_reads_hash(&outcome.reads)
                    .0
                    .as_slice(),
                domain.kind.to_le_bytes().as_slice(),
                domain.schema.to_le_bytes().as_slice(),
            ],
        ));
        if let Some(receipt) = &outcome.execution
            && let Some(cache) = self.store.verifier()
        {
            cache.publish_execution(outcome.execution_subject, receipt);
        }
        let failed = outcome.result.is_err();
        if let Ok(Value::Constant(value)) = &mut outcome.result {
            value.pack(record);
        }
        if persist {
            self.remember_stored(id, key, record, &outcome);
        }
        let raw = match (raw, &outcome.result) {
            (Some(raw), _) => Some(raw),
            (None, Ok(Value::Lower(lower))) => Some((
                lower.decode(self).ok_or(self.errors.protocol)?,
                self.records
                    .decode(
                        &self.anchors,
                        &self
                            .store
                            .get_content(lower.materialized)
                            .ok_or(self.errors.protocol)?,
                    )
                    .ok_or(self.errors.protocol)?,
            )),
            (None, _) => None,
        };
        let mut original = if let Some((output, materialized)) = raw {
            Some({
                let lower = match outcome.result.as_ref().map_err(|_| self.errors.protocol)? {
                    Value::Lower(lower) => lower,
                    _ => return Err(self.errors.protocol),
                };
                let path = output.value.path;
                FrontLowerOutput {
                    output,
                    materialized,
                    reads: outcome.reads.clone(),
                    depth: outcome.depth.evidence(),
                    execution: outcome.execution.clone(),
                    execution_subject: outcome.execution_subject,
                    executed: outcome.executed,
                    selections: outcome.selections.clone(),
                    acceptance: lower.acceptance.clone(),
                    implementation_revision: self
                        .inputs
                        .get(path.path())
                        .ok_or(self.errors.protocol)?
                        .implementation,
                    signature: self
                        .snapshot
                        .signature(path)
                        .ok_or(self.errors.protocol)?
                        .as_ref()
                        .map_err(|_| self.errors.protocol)?
                        .fingerprint,
                    verified: lower.verified(self).unwrap_or(None),
                    query_observations: None,
                    owner: self.source.clone(),
                }
            })
        } else {
            None
        };
        outcome.archive_reads(record);
        let archive_bytes = if failed {
            0
        } else {
            ProductArchive::retained_required_bytes(record, &bytes)
                .map_err(|_| self.errors.resource)?
        };
        let live_bytes = if let Some(original) = &original {
            LiveLowerProduct::retained_required_bytes(original).ok_or(self.errors.resource)?
        } else {
            0
        };
        let required = outcome
            .retained_bytes()
            .and_then(|bytes| bytes.checked_add(archive_bytes))
            .and_then(|bytes| bytes.checked_add(live_bytes))
            .and_then(|bytes| bytes.checked_add(outcome_table_bytes(1)))
            .ok_or(self.errors.resource)?;
        while *self.retention.lock().unwrap_or_else(|e| e.into_inner()) < required {
            if !query_reads.evict_body().await {
                return Err(self.errors.resource);
            }
        }
        let archive = if failed {
            None
        } else {
            Some(
                ProductArchive::from_retained_bytes(
                    record,
                    bytes,
                    self._allowance.clone(),
                    self.retention.clone(),
                )
                .map_err(|_| self.errors.resource)?,
            )
        };
        if let Some(original) = &mut original {
            original.query_observations = archive.clone();
        }
        let live = original
            .map(|original| {
                LiveLowerProduct::from_retained(
                    (id.address, action),
                    original,
                    self.anchors.clone(),
                    self.records.clone(),
                    self.source.clone(),
                    self._allowance.clone(),
                    self.retention.clone(),
                )
                .ok_or(self.errors.resource)
            })
            .transpose()?;
        if let Ok(Value::Lower(lower)) = &mut outcome.result {
            lower.live = live.as_ref().map(LiveLowerProduct::downgrade);
        }
        let charge = outcome.retained_bytes().ok_or(self.errors.resource)?;
        let mut retention = self.retention.lock().unwrap_or_else(|e| e.into_inner());
        let mut outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let selected = (id.kind, id.address.0);
        let old = outcomes.get(&selected).map(|(_, _, charge)| *charge);
        let next_len = outcomes
            .len()
            .checked_add(usize::from(old.is_none()))
            .ok_or(self.errors.resource)?;
        let table_growth = outcome_table_bytes(next_len)
            .checked_sub(outcome_table_bytes(outcomes.len()))
            .ok_or(self.errors.resource)?;
        let remaining = retention
            .checked_add(old.unwrap_or(0))
            .and_then(|n| n.checked_sub(charge))
            .and_then(|n| n.checked_sub(table_growth))
            .ok_or(self.errors.resource)?;
        let replaced = outcomes.insert(selected, (record, Box::new(outcome), charge));
        *retention = remaining;
        drop(outcomes);
        drop(retention);
        drop(replaced);
        if failed {
            return Err(record);
        }
        self.store.associate_product(id.address, action, record);
        if let Some(live) = live {
            return Ok(ProductRef::lower(live));
        }
        Ok(ProductRef::import_archive(
            id.address,
            action,
            archive.ok_or(self.errors.protocol)?,
        ))
    }
    async fn compute(
        &self,
        kind: u32,
        path: &DefPath,
        selection: Option<&staged::Selection>,
        source: &mut Source,
    ) -> Result<Computed, QueryError> {
        let front = self.front.query(path).ok_or(QueryError::NotImplemented)?;
        let selected = source.reads.read_consuming(front).await;
        let body = match self.front.outcome(selected.clone()) {
            Some(body) => body,
            None if selected.is_ok() => {
                let materialize = self
                    .front
                    .body_query(path)
                    .ok_or(QueryError::NotImplemented)?;
                let product = source
                    .reads
                    .read_consuming(materialize)
                    .await
                    .map_err(|error| source.refused(error))?;
                self.front
                    .outcome(Ok(product.clone()))
                    .ok_or_else(|| source.refused(self.errors.protocol))?
            }
            None => return Err(source.refused(selected.err().unwrap_or(self.errors.protocol))),
        };
        source.merge(&body.outcome.reads);
        if self.verify == Verify::On {
            let mut observed = ReadsBuilder::new();
            FrozenProvider::new(&self.snapshot, path.0.package.clone(), &mut observed)
                .validation_universe();
            source.merge(
                &observed
                    .finish()
                    .map_err(|_| source.refused(self.errors.protocol))?,
            );
        }
        if let Some(registration) = self.front.registration(path) {
            source
                .evidence
                .borrow_mut()
                .dependency(true, registration.acceptance());
            source
                .depth
                .borrow_mut()
                .observe_transparent(registration.depth());
        }
        // As a materialization of the checked body, Lower also depends on the
        // private shapes that body read, beyond their representation digests.
        for (read, _) in body.outcome.reads.iter() {
            if let ReadKey::Representation(id) = read
                && let Some(hash) = self.layouts.get(id)
            {
                source.record(ReadKey::LayoutBody(*id), memos::optional(Some(*hash)));
            }
        }
        let analyzed = body.outcome.result?;
        source.execution_signatures(analyzed.dependencies.iter());
        let signature = source.signature(path)?;
        let mut observed = ReadsBuilder::new();
        let staged = crate::comptime::prepare_staged_types(
            self.snapshot.types(),
            &analyzed.value,
            &mut FrozenProvider::new(&self.snapshot, path.0.package.clone(), &mut observed),
        );
        source.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
        let staged = staged?;
        let mut values = Values {
            source: source.clone(),
            pending: None,
        };
        let mut preparation = ValuePreparation::new(&analyzed.value);
        let mut constants = if kind == FRONT_LOWER_KIND
            && matches!(signature.kind, hirsel_types::SignatureKind::Const { .. })
        {
            let (_, constants) = source.constant_value(path).await?;
            constants
        } else {
            poll_fn(|_| {
                preparation
                    .poll(
                        self.snapshot.types(),
                        &analyzed.value,
                        &staged,
                        &signature,
                        &mut values,
                    )
                    .map(|r| r.map(|v| v.to_vec()))
            })
            .await?
        };
        if let Some(selection) = selection {
            let declaration = &self.staged.declarations[&selection.declaration];
            source.record(
                ReadKey::Signature(selection.declaration),
                memos::signature_value(Some(declaration.signature.fingerprint)),
            );
            let expression = declaration.expression;
            let hirsel_syntax::ExprKind::Comptime(child) =
                analyzed.value.typed.syntax.tree.expressions[expression.0 as usize].kind
            else {
                return Err(QueryError::NotImplemented);
            };
            let ty = analyzed
                .value
                .typed
                .checked
                .expressions
                .iter()
                .find(|row| row.expression == expression)
                .ok_or(QueryError::NotImplemented)?
                .ty;
            let sizes = declaration
                .inputs
                .iter()
                .copied()
                .zip(selection.arguments.iter().copied())
                .collect();
            let value = evaluate(
                source.clone(),
                analyzed,
                staged,
                signature,
                expression,
                child,
                ty,
                constants.clone(),
                sizes,
            )
            .await?
            .result?;
            return Ok(Computed {
                value: Value::Constant(Box::new(Constants::new(value, constants))),
                verification: None,
                raw: None,
            });
        }
        if kind == FRONT_EVALUATE_KIND {
            let ItemKind::Const {
                value: expression, ..
            } = analyzed.value.typed.syntax.item.kind
            else {
                return Err(QueryError::NotImplemented);
            };
            let dictionary = crate::dictionaries::declaration_async(
                self.snapshot.types(),
                &self.memo,
                &analyzed.value,
                &signature,
                &constants,
                source,
            )
            .await?;
            let is_dictionary = dictionary.is_some();
            let value = if let Some(dictionary) = dictionary {
                dictionary
            } else if let Some((_, value)) = constants.iter().find(|(id, _)| *id == expression) {
                value.clone()
            } else {
                let ty = analyzed
                    .value
                    .typed
                    .checked
                    .expressions
                    .iter()
                    .find(|r| r.expression == expression)
                    .ok_or(QueryError::NotImplemented)?
                    .ty;
                evaluate(
                    source.clone(),
                    analyzed.clone(),
                    staged,
                    signature,
                    expression,
                    expression,
                    ty,
                    constants.clone(),
                    vec![],
                )
                .await?
                .result?
            };
            if is_dictionary && !constants.iter().any(|(id, _)| *id == expression) {
                constants.push((expression, value.clone()));
            }
            return Ok(Computed {
                value: Value::Constant(Box::new(Constants::new(value, constants))),
                verification: None,
                raw: None,
            });
        }
        let lowering = self.lowering.as_deref().ok_or(QueryError::NotImplemented)?;
        self.count(path, |work| work.lowerings += 1);
        let output = lowering.lower(
            self.snapshot.types(),
            &self.memo,
            &analyzed.value,
            &Comptime {
                materialized: constants.clone(),
                staged,
            },
            &signature,
        )?;
        let definition = Arc::new(output.value);
        for dependency in &output.dependencies {
            source.record(
                ReadKey::Signature(dependency.definition),
                dependency.fingerprint,
            );
        }
        let mut verification = None;
        let mut acceptance = None;
        if self.verify == Verify::On {
            let mut observed = ReadsBuilder::new();
            FrozenProvider::new(&self.snapshot, path.0.package.clone(), &mut observed)
                .validation_universe();
            source.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
            let (report, ran, accepted, evidence) = self
                .snapshot
                .validate_source_artifact(&definition, |anchor| self.anchors.record_owner(anchor));
            acceptance = accepted;
            source.evidence.borrow_mut().accepted(acceptance.clone());
            if ran {
                timings::count(Count::VRuns);
                self.count(path, |work| work.validation_runs += 1);
            }
            if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
                return Err(error);
            }
            if !report.violations.is_empty() {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "TIR validation".into(),
                    message: format!("{:?}", report.violations),
                }));
            }
            verification = evidence.map(|evidence| (definition.clone(), evidence));
        }
        if has_errors(&output.diagnostics) {
            return Err(QueryError::Diagnostics(output.diagnostics));
        }
        let (bytes, record_decode_bytes) = self
            .records
            .encode_tir_with_decode_bytes(&self.anchors, &definition)
            .ok_or(QueryError::NotImplemented)?;
        // Arc/slice decoding can temporarily overlap its construction buffers.
        // Count serialized shared rows independently; source sharing is not a
        // promise made by the ordinary decoder.
        let retained = crate::memo::retained::decoded_tir_bytes(&definition);
        let definition_decode_layout = DefinitionDecodeLayout {
            retained,
            transient: retained
                .checked_add(record_decode_bytes)
                .ok_or(QueryError::NotImplemented)?,
        };
        definition_decode_layout
            .total()
            .ok_or(QueryError::NotImplemented)?;
        // Ordinary body bytes belong to the selected provider. Encoding uses
        // the active scratch reservation, rather than the saved future frame.
        if bytes.len() as u64 > self.weight.scratch {
            return Err(QueryError::NotImplemented);
        }
        let body = self
            .store
            .put_content(crate::NATIVE_ORIGINAL_BODY, &bytes)
            .ok_or(QueryError::NotImplemented)?;
        drop(bytes);
        let materialized = self
            .records
            .encode(&self.anchors, &constants)
            .filter(|bytes| bytes.len() as u64 <= self.weight.scratch)
            .and_then(|bytes| self.store.put_content(LOWER_MATERIALIZED, &bytes))
            .ok_or(QueryError::NotImplemented)?;
        let mut dependencies = analyzed.dependencies.clone();
        dependencies.extend(output.dependencies);
        let constant_selection_bytes = constant_selection_bytes(&definition)?;
        let raw = Some((
            QueryOutput {
                value: definition.clone(),
                diagnostics: output.diagnostics.clone(),
                dependencies: dependencies.clone(),
            },
            constants,
        ));
        Ok(Computed {
            raw,
            value: Value::Lower(Box::new(LowerProduct {
                live: None,
                constant_selection_bytes,
                definition: definition.path,
                body,
                diagnostics: output.diagnostics,
                dependencies: dependencies.into(),
                archived_metadata: None,
                verified: None,
                acceptance,
                materialized,
                definition_decode_layout,
            })),
            verification,
        })
    }
}
struct Factory(Arc<FrontLowerQueries>);
// This producer's settled Outcome allocations and provider archives have
// independent owners. Only the saved future/mailbox/working state belongs to
// the runtime continuation, and S5 retires it after destruction at home.
struct LowerTask(LocalFutureTask);
impl LocalQueryTask for LowerTask {
    fn id(&self) -> QueryId {
        self.0.id()
    }
    fn weight(&self) -> MemoryWeight {
        self.0.weight()
    }
    fn continuation_bytes(&self) -> u64 {
        self.0.weight().retained
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        self.0.poll(cx)
    }
}
impl LocalQueryFactory for Factory {
    fn weight(&self, id: QueryId) -> Result<MemoryWeight, Reference> {
        if !matches!(id.kind, FRONT_LOWER_KIND | FRONT_EVALUATE_KIND) || self.0.path(id).is_none() {
            return Err(self.0.errors.protocol);
        }
        Ok(self.0.weight)
    }
    fn create(&self, id: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference> {
        let weight = self.weight(id)?;
        let path = self.0.path(id).ok_or(self.0.errors.protocol)?;
        let owner = self.0.clone();
        Ok(Box::new(LowerTask(LocalFutureTask::new(
            id,
            weight,
            self.0.max_reads,
            self.0.errors,
            move |reads| owner.run(id, path, reads),
        )?)))
    }
}
#[derive(Clone)]
struct Source {
    owner: Arc<FrontLowerQueries>,
    reads: QueryReads,
    observed: Rc<RefCell<ReadsBuilder>>,
    selections: Rc<RefCell<staged::Selections>>,
    refusal: Rc<Cell<Option<Reference>>>,
    depth: Rc<RefCell<DepthEvidence<Span>>>,
    framed: bool,
    evidence: Rc<RefCell<Evidence>>,
}
impl Source {
    fn select(&self, rows: &[(DefId, Vec<u64>)]) -> Result<(), QueryError> {
        let mut selections = self.selections.borrow_mut();
        let count = selections
            .len()
            .checked_add(rows.len())
            .ok_or_else(|| self.refused(self.owner.errors.resource))?;
        let arguments = selections
            .iter()
            .chain(rows)
            .try_fold(0u64, |sum, (_, values)| {
                sum.checked_add(size_of_val(values.as_slice()) as u64)
            })
            .ok_or_else(|| self.refused(self.owner.errors.resource))?;
        let bytes = (count as u64)
            .checked_mul(size_of::<(DefId, Vec<u64>)>() as u64)
            .and_then(|bytes| bytes.checked_add(arguments))
            .and_then(|bytes| bytes.checked_mul(2))
            .ok_or_else(|| self.refused(self.owner.errors.resource))?;
        if bytes > self.owner.weight.scratch {
            return Err(self.refused(self.owner.errors.resource));
        }
        selections.extend_from_slice(rows);
        Ok(())
    }
    fn refused(&self, reference: Reference) -> QueryError {
        if self.refusal.get().is_none() {
            self.refusal.set(Some(reference));
        }
        // The internal phase result only unwinds the shared computation. The
        // owner returns the original runtime reference before publication.
        QueryError::NotImplemented
    }
    fn overflow(&self, site: Span) -> QueryError {
        let limit = hirsel_tir::ToolingLimit {
            kind: hirsel_tir::ToolingLimitKind::CallDepth,
            limit: dependency_limit(self.owner.limits.call_depth) as u64,
            site: Some(site),
        };
        QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "compile-time tooling limit".into(),
            message: format!("{limit:?}"),
        })
    }
    fn merge(&self, reads: &Reads) {
        self.observed.borrow_mut().merge(reads);
    }
    fn record(&self, key: ReadKey, hash: ContentHash) {
        self.observed.borrow_mut().record(key, hash);
    }
    fn execution_signatures<'a>(
        &self,
        dependencies: impl IntoIterator<Item = &'a SignatureDependency>,
    ) {
        for dependency in dependencies {
            if let Some(signature) = self.owner.snapshot.signature(dependency.definition) {
                self.record(
                    ReadKey::ExecutedSignature(dependency.definition),
                    memos::signature_value(signature.ok().map(|s| s.fingerprint)),
                );
            }
        }
    }
    fn signature(&self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        let mut observed = ReadsBuilder::new();
        let result = SignatureProvider::signature(
            &mut FrozenProvider::new(&self.owner.snapshot, path.0.package.clone(), &mut observed),
            DefId::intern(path),
        );
        self.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
        result
    }
    async fn value(&self, path: &DefPath, kind: u32, site: Span) -> Result<Value, QueryError> {
        let id = self
            .owner
            .id(path, kind)
            .ok_or(QueryError::NotImplemented)?;
        self.selected_value(id, path, kind, site).await
    }
    async fn selected_value(
        &self,
        id: QueryId,
        path: &DefPath,
        kind: u32,
        site: Span,
    ) -> Result<Value, QueryError> {
        let selected = self.reads.read_consuming(id).await;
        let outcome = self
            .owner
            .selected(id, selected.clone())
            .ok_or_else(|| self.refused(selected.err().unwrap_or(self.owner.errors.protocol)))?;
        self.merge(
            &outcome
                .reads(&self.owner)
                .ok_or_else(|| self.refused(self.owner.errors.resource))?,
        );
        self.select(&outcome.selections)?;
        self.evidence
            .borrow_mut()
            .dependency(outcome.executed, outcome.execution.as_ref());
        if let Ok(Value::Lower(lower)) = &outcome.result {
            self.evidence
                .borrow_mut()
                .accepted(lower.acceptance.clone());
        }
        let n = u32::from(self.framed);
        let depth = outcome.depth.evidence();
        let overflow = if kind == FRONT_EVALUATE_KIND {
            self.depth.borrow_mut().observe_frame(&depth, site);
            depth.request(n, site).err()
        } else {
            self.depth.borrow_mut().observe_transparent(&depth);
            depth.first[n.min(depth.limit()) as usize]
        };
        if let Some(site) = overflow {
            return Err(self.overflow(site));
        }
        self.record(
            ReadKey::ExecutedBody(DefId::intern(path)),
            memos::optional(Some(self.owner.inputs[path].implementation)),
        );
        outcome.result
    }
    async fn constant_value(
        &self,
        path: &DefPath,
    ) -> Result<(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>), QueryError> {
        self.constant_at(
            path,
            self.owner
                .inputs
                .get(path)
                .ok_or(QueryError::NotImplemented)?
                .site,
        )
        .await
    }
    async fn constant_at(
        &self,
        path: &DefPath,
        site: Span,
    ) -> Result<(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>), QueryError> {
        match self.value(path, FRONT_EVALUATE_KIND, site).await? {
            Value::Constant(value) => value
                .decoded(
                    &self.owner.records,
                    &self.owner.anchors,
                    &*self.owner.store,
                    self.owner.weight.scratch,
                )
                .ok_or(QueryError::NotImplemented),
            _ => Err(QueryError::NotImplemented),
        }
    }
}
impl s1::AsyncDependencySource for Source {
    async fn definition(&mut self, path: &DefPath) -> s1::DependencyProduct<DefinitionRef> {
        let result = match self.value(path, FRONT_LOWER_KIND, Span::default()).await {
            Ok(Value::Lower(lower)) => match lower.decode(&self.owner) {
                Some(output) => {
                    self.execution_signatures(output.dependencies.iter());
                    Ok(output.value)
                }
                None => Err(QueryError::NotImplemented),
            },
            Err(e) => Err(e),
            _ => Err(QueryError::NotImplemented),
        };
        s1::DependencyProduct {
            result,
            reads: Reads::empty(),
        }
    }
    async fn constant(&mut self, path: &DefPath) -> s1::DependencyProduct<MaterializedRef> {
        let result = self.constant_value(path).await.map(|(v, _)| Arc::new(v));
        s1::DependencyProduct {
            result,
            reads: Reads::empty(),
        }
    }
    async fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> s1::DependencyProduct<MaterializedRef> {
        if arguments.is_empty() {
            return s1::AsyncDependencySource::constant(self, path).await;
        }
        let result = async {
            let id = self.owner.staged_query(path, arguments)?;
            self.select(&[(DefId::intern(path), arguments.to_vec())])?;
            let declaration = self
                .owner
                .staged
                .declarations
                .get(&DefId::intern(path))
                .ok_or(QueryError::NotImplemented)?;
            let owner = declaration.owner.clone();
            let site = self.owner.inputs[&owner].site;
            let Value::Constant(value) = self
                .selected_value(id, &owner, FRONT_EVALUATE_KIND, site)
                .await?
            else {
                return Err(QueryError::NotImplemented);
            };
            value
                .decoded(
                    &self.owner.records,
                    &self.owner.anchors,
                    &*self.owner.store,
                    self.owner.weight.scratch,
                )
                .map(|(value, _)| Arc::new(value))
                .ok_or(QueryError::NotImplemented)
        }
        .await;
        s1::DependencyProduct {
            result,
            reads: Reads::empty(),
        }
    }
    async fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> s1::DependencyProduct<Option<DefPath>> {
        let result = self
            .owner
            .instances
            .resolve(DefId::intern(path), self.owner.snapshot.types(), arguments)
            .map(|selected| selected.map(|id| id.path().clone()));
        s1::DependencyProduct {
            result,
            reads: Reads::empty(),
        }
    }
}
impl crate::dictionaries::DeclarationSource for Source {
    fn signature(&mut self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        Source::signature(self, path)
    }
    async fn evaluate(&mut self, input: EvaluationInput<'_>) -> Result<Materialized, QueryError> {
        evaluate(
            self.clone(),
            Arc::new(QueryOutput {
                value: input.analyzed.clone(),
                diagnostics: vec![],
                dependencies: vec![],
            }),
            input.staged.to_vec(),
            Arc::new(input.signature.clone()),
            input.expression,
            input.child,
            input.ty,
            input.constants.to_vec(),
            input.sizes.to_vec(),
        )
        .await?
        .result
    }
}
type Evaluation =
    Pin<Box<dyn Future<Output = Result<s1::DependencyProduct<Materialized>, QueryError>>>>;
struct Values {
    source: Source,
    pending: Option<Evaluation>,
}
impl PreparingSource for Values {
    fn signature(
        &mut self,
        path: &DefPath,
    ) -> Poll<Result<hirsel_types::SignatureRef, QueryError>> {
        Poll::Ready(self.source.signature(path))
    }
    fn constant(&mut self, path: &DefPath, site: Span) -> Poll<Result<Materialized, QueryError>> {
        if self.pending.is_none() {
            let source = self.source.clone();
            let path = path.clone();
            self.pending = Some(Box::pin(async move {
                Ok(s1::DependencyProduct {
                    result: source.constant_at(&path, site).await.map(|(v, _)| v),
                    reads: Reads::empty(),
                })
            }));
        }
        self.advance()
    }
    fn evaluate(&mut self, input: EvaluationInput<'_>) -> Poll<Result<Materialized, QueryError>> {
        if self.pending.is_none() {
            self.pending = Some(Box::pin(evaluate(
                self.source.clone(),
                Arc::new(QueryOutput {
                    value: input.analyzed.clone(),
                    diagnostics: vec![],
                    dependencies: vec![],
                }),
                input.staged.to_vec(),
                Arc::new(input.signature.clone()),
                input.expression,
                input.child,
                input.ty,
                input.constants.to_vec(),
                input.sizes.to_vec(),
            )));
        }
        self.advance()
    }
}
impl Values {
    fn advance(&mut self) -> Poll<Result<Materialized, QueryError>> {
        match self
            .pending
            .as_mut()
            .unwrap()
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.pending = None;
                Poll::Ready(result.and_then(|p| {
                    self.source.merge(&p.reads);
                    p.result
                }))
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn evaluate(
    mut source: Source,
    analyzed: Arc<QueryOutput<AnalyzedBody>>,
    staged: Vec<(hirsel_syntax::ExprId, hirsel_types::TypeId)>,
    signature: hirsel_types::SignatureRef,
    expression: hirsel_syntax::ExprId,
    child: hirsel_syntax::ExprId,
    ty: hirsel_types::TypeId,
    constants: Vec<(hirsel_syntax::ExprId, Materialized)>,
    sizes: Vec<(hirsel_types::SizeParameterRef, u64)>,
) -> Result<s1::DependencyProduct<Materialized>, QueryError> {
    let owner = source.owner.clone();
    let site = analyzed.value.typed.syntax.tree.expressions[expression.0 as usize]
        .origin
        .span;
    let lowering = owner
        .lowering
        .as_deref()
        .ok_or(QueryError::NotImplemented)?;
    owner.count(signature.definition.path(), |work| work.initializers += 1);
    source.evidence.borrow_mut().executed = true;
    let prepared = EvaluationInput {
        analyzed: &analyzed.value,
        staged: &staged,
        signature: &signature,
        expression,
        child,
        ty,
        constants: &constants,
        sizes: &sizes,
    }
    .lower(owner.snapshot.types(), &owner.memo, lowering)?;
    let admission =
        s1::ValidatedEvaluation::recorded(prepared, &owner.snapshot, owner.verify, site);
    source.merge(&admission.reads);
    let runtime = owner.runtime_types()?;
    let product = admission
        .result?
        .run_async(&owner.sources, owner.limits, &runtime, &mut source)
        .await;
    source.merge(&product.value.reads);
    source.evidence.borrow_mut().accepted(product.acceptance);
    let result = product.value.result.and_then(|(value, ty)| {
        let mut observed = ReadsBuilder::new();
        let result = crate::materialized::reify_value(
            owner.snapshot.types(),
            &owner.memo,
            &mut Materializer {
                provider: FrozenProvider::new(
                    &owner.snapshot,
                    signature.definition.path().0.package.clone(),
                    &mut observed,
                ),
                source: &source,
            },
            value,
            ty,
            site,
        );
        source.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
        result
    });
    Ok(s1::DependencyProduct {
        result,
        reads: Reads::empty(),
    })
}
struct Materializer<'a> {
    provider: FrozenProvider<'a>,
    source: &'a Source,
}
impl crate::materialized::MaterializationSource for Materializer<'_> {
    fn signature(&mut self, id: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        SignatureProvider::signature(&mut self.provider, id)
    }
    fn body_view(&mut self, id: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        let public = SignatureProvider::signature(&mut self.provider, id)?;
        let private = self.provider.body_view(id)?;
        if !Arc::ptr_eq(&public, &private)
            && let Some(input) = self.source.owner.inputs.get(id.path())
        {
            self.source.record(
                ReadKey::ExecutedBody(id),
                memos::optional(Some(input.implementation)),
            );
        }
        Ok(private)
    }
}
impl QueryEngine {
    /// Capture input identities and immutable phase handles. This does no body
    /// checking, lowering or interpretation, and never calls the S1 controller.
    pub fn front_lower_queries(
        &mut self,
        definitions: &[DefPath],
        front: Arc<FrontBodyQueries>,
        weight: MemoryWeight,
        max_reads: usize,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontLowerQueries>, QueryError> {
        let refusal = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "front Lower admission".into(),
                message: "immutable Lower inputs exceed admission or lack phase/source bindings"
                    .into(),
            })
        };
        let input_bytes = FrontLowerQueries::input_bytes(definitions).ok_or_else(refusal)?;
        if input_bytes > allowance.bytes() {
            return Err(refusal());
        }
        let retention = Arc::new(Mutex::new(allowance.bytes() - input_bytes));
        let store = self
            .result_store
            .clone()
            .filter(|s| s.enabled())
            .ok_or_else(refusal)?;
        let lowering = self.adapters.lowering.clone();
        let mut inputs = Vec::with_capacity(definitions.len());
        for path in definitions {
            if front.query(path).is_none() {
                return Err(refusal());
            }
            let syntax = self.snapshot(path)?;
            inputs.push((
                DefId::intern(path),
                Input {
                    lower: match front.registration(path) {
                        Some(accepted) => {
                            self.accepted_front_store_key(accepted, QueryKind::Lower)?
                        }
                        None if generation::instance_bodied(&syntax) => {
                            self.front_declaration_store_key(path, QueryKind::Lower)?
                        }
                        None => self.store_key(path, QueryKind::Lower).ok_or_else(refusal)?,
                    },
                    evaluate: match front.registration(path) {
                        Some(accepted) => {
                            self.accepted_front_store_key(accepted, QueryKind::Evaluate)?
                        }
                        None if generation::instance_bodied(&syntax) => {
                            self.front_declaration_store_key(path, QueryKind::Evaluate)?
                        }
                        None => self
                            .store_key(path, QueryKind::Evaluate)
                            .ok_or_else(refusal)?,
                    },
                    implementation: syntax.text_hash,
                    site: syntax.item.name_span,
                },
            ));
        }
        let inputs = Inputs::new(inputs);
        if inputs
            .retained_bytes()
            .is_none_or(|bytes| bytes > input_bytes)
        {
            return Err(refusal());
        }
        let snapshot = self.signature_inputs();
        let packages: std::collections::BTreeSet<_> =
            definitions.iter().map(|path| &path.0.package).collect();
        let opaque = self
            .declarations
            .opaque_paths()
            .cloned()
            .collect::<Vec<_>>();
        let layouts: BTreeMap<_, _> = opaque
            .into_iter()
            .filter(|path| packages.contains(&path.0.package))
            .filter_map(|path| Some((DefId::intern(&path), self.implementation_hash(&path)?)))
            .collect();
        let layout_bytes = (layouts.len() as u64)
            .checked_mul(
                (std::mem::size_of::<(DefId, ContentHash)>() + 6 * std::mem::size_of::<usize>())
                    as u64,
            )
            .ok_or_else(refusal)?;
        {
            let mut retention = retention.lock().unwrap_or_else(|e| e.into_inner());
            *retention = retention.checked_sub(layout_bytes).ok_or_else(refusal)?;
        }
        let staged = staged::Staged::new(&self.staged_constants);
        {
            let mut remaining = retention.lock().unwrap_or_else(|error| error.into_inner());
            *remaining = remaining
                .checked_sub(staged.retained_bytes())
                .ok_or_else(refusal)?;
        }
        let paths = inputs
            .iter()
            .flat_map(|(path, input)| {
                let path = DefId::intern(path);
                [
                    ((FRONT_LOWER_KIND, input.lower.0), path),
                    ((FRONT_EVALUATE_KIND, input.evaluate.0), path),
                ]
            })
            .collect();
        let allowance = Arc::new(allowance);
        let anchors = crate::NativeRecordContext::capture(self);
        let records = crate::NativeRecords::new(self.types.clone(), store.clone());
        let source = Arc::new(LowerSource {
            anchors: anchors.clone(),
            records: records.clone(),
            store: store.clone(),
            _sources: self.sources.clone(),
            _allowance: allowance.clone(),
        });
        Ok(Arc::new(FrontLowerQueries {
            source,
            inputs,
            staged,
            paths,
            layouts,
            front,
            snapshot,
            memo: self.memo.clone(),
            lowering,
            runtime: std::sync::OnceLock::new(),
            instances: Arc::new(self.instance_bodies(&definitions.iter().cloned().collect())),
            sources: self.sources.clone(),
            limits: self.comptime_limits,
            verify: self.verify,
            anchors,
            records,
            store,
            outcomes: Mutex::new(BTreeMap::new()),
            carry: Mutex::new(None),
            carry_changes: Mutex::new(None),
            clean_memos: Mutex::new(BTreeSet::new()),
            work: Mutex::new(BTreeMap::new()),
            weight,
            max_reads,
            errors,
            retention,
            _allowance: allowance,
        }))
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    #[test]
    fn retained_outcome_charges_capacity_and_compact_read_rows() {
        let mut bytes = Vec::with_capacity(64 << 10);
        bytes.push(1);
        let capacity = bytes.capacity();
        let substitutions = Vec::with_capacity(4);
        let substitutions_bytes =
            substitutions.capacity() * std::mem::size_of::<(hirsel_syntax::ExprId, Materialized)>();
        let outcome = Outcome {
            result: Ok(Value::Constant(Box::new(Constants::new(
                Materialized {
                    root: hirsel_tir::ValueNode {
                        ty: hirsel_types::TypeId::DANGLING,
                        kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Bytes(bytes)),
                    },
                },
                substitutions,
            )))),
            reads: Reads::from_vec(vec![(
                ReadKey::Signature(DefId::intern(&DefPath::core(&"long".repeat(4096)))),
                ContentHash::default(),
            )])
            .unwrap(),
            reads_decode_bytes: 0,
            selections: Vec::new(),
            depth: DepthEvidence::leaf(32).into(),
            executed: false,
            execution: None,
            execution_subject: ContentHash::default(),
        };
        let expected = size_of::<Outcome>()
            + 2 * size_of::<usize>()
            + size_of::<Constants>()
            + 2 * size_of::<usize>()
            + capacity
            + substitutions_bytes
            + std::mem::size_of::<(ReadKey, ContentHash)>()
                .next_multiple_of(std::mem::align_of::<usize>())
            + 2 * std::mem::size_of::<usize>()
            + outcome.depth.retained_bytes();
        assert_eq!(outcome.retained_bytes(), Some(expected as u64));
        assert_eq!(outcome_table_bytes(0), 0);
        assert!(outcome_table_bytes(1) >= std::mem::size_of::<(Reference, Outcome, u64)>() as u64);
        assert!(outcome_table_bytes(6) > outcome_table_bytes(1));
    }
}
