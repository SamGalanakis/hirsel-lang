//! Isolated serial checked patches. Host/snapshot separation follows RA
//! 3131751e crates/ide/src/lib.rs:192-202,238-252; head CAS follows TS-Go
//! 89d5d5b internal/project/session.go:1393-1425 with generations and receipts.
mod edits;
pub mod protocol;
pub mod v2;
pub use edits::{ByteEdit, EditError};
use hirsel_engine::loader_inputs::{
    CaptureInputs, FrozenInputs, InputLimit, InputLimits, InputUnavailable, LoaderInputs,
};
use hirsel_engine::{
    CheckCompletion, CheckOutput, CheckedProducts, CompileConfig, Compiler, LoweringPolicy,
    QueryError, ToolError, Verify,
};
use hirsel_syntax::{ContentHash, PackageId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

static WORKSPACES: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseToken {
    pub workspace: u64,
    pub generation: u64,
    pub inputs: ContentHash,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CandidateId {
    pub client: u64,
    pub nonce: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SourceKey(pub PathBuf);
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEdit {
    pub file: SourceKey,
    pub expected: ContentHash,
    pub edits: Vec<ByteEdit>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Priority {
    Selected,
    Background,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRequest {
    pub id: CandidateId,
    pub base: BaseToken,
    pub edits: Vec<FileEdit>,
    pub priority: Priority,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckPolicy {
    pub verify: Verify,
    pub lowering: LoweringPolicy,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedScope {
    pub packages: Vec<PackageId>,
    pub policy: CheckPolicy,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IncompleteReason {
    Cancelled,
    ResourceLimit,
    InputUnavailable,
    UnsupportedEdit,
    MissingPhase,
    ToolFailure,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Completion {
    Complete,
    Incomplete(IncompleteReason),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncompleteProgress {
    /// Packages whose complete check has not been certified. This serial
    /// implementation publishes an aggregate check, so the list conservatively
    /// includes the selected closure; some phases may already have completed.
    pub uncompleted_packages: Vec<PackageId>,
    /// Last known affected phase; None if work stopped before a phase ran.
    pub phase: Option<hirsel_engine::QueryKind>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Pass,
    Reject,
    Undecided,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DependencyDelta {
    Unknown {
        reason: String,
    },
    Known {
        parent: CheckedIdentity,
        child: CheckedIdentity,
        /// Sorted by exact canonical (owner, key) bytes. Equal values are omitted.
        reads: Arc<[ReadChange]>,
    },
}
/// Both ends are complete checks, including complete language rejections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedIdentity {
    pub inputs: ContentHash,
    pub compiler_environment: ContentHash,
    pub scope: CheckedScope,
    pub products: ContentHash,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadChange {
    /// Existing encode_def_path bytes, including package and private owner.
    pub owner: Vec<u8>,
    /// Exact ReadKey::encode bytes, including negative and private-view keys.
    pub key: Vec<u8>,
    /// None -> Some is added; Some -> None removed; unequal Some -> Some changed.
    pub before: Option<ContentHash>,
    pub after: Option<ContentHash>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeadFreshness {
    CurrentAt(u64),
    Stale(BaseToken),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiskFreshness {
    Unobserved,
    MatchesAt(u64),
    Changed(Vec<PathBuf>),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureClass {
    LoadIo,
    LoadManifest,
    LoadQuery,
    CheckQuery,
    Unavailable,
    ResourceLimit,
}
#[derive(Clone, Debug)]
pub struct CheckReceipt {
    pub id: CandidateId,
    pub base: BaseToken,
    pub candidate_inputs: ContentHash,
    pub compiler_environment: ContentHash,
    pub scope: CheckedScope,
    pub completion: Completion,
    pub incomplete: Option<IncompleteProgress>,
    pub verdict: Verdict,
    pub output: Arc<CheckOutput>,
    pub loader_error: Option<String>,
    pub failure_class: Option<FailureClass>,
    pub checked_products: Option<Arc<CheckedProducts>>,
    pub dependencies: DependencyDelta,
    pub head: HeadFreshness,
    pub disk: DiskFreshness,
    _selection: Option<Arc<v2::Selection>>,
    _namespace: Option<v2::NamespaceLease>,
    _input_charge: Option<Arc<Charge>>,
    _charge: Arc<Charge>,
    _observation_charge: Option<Arc<Charge>>,
    _dependency_charge: Option<Arc<Charge>>,
}
impl CheckReceipt {
    /// V2 selected closure facts, derived by the ordinary graph loader.
    pub fn selection_v2(&self) -> Option<&serde_json::Value> {
        self._selection.as_ref().map(|s| &s.facts)
    }
}
#[derive(Clone, Debug)]
pub enum CandidateResult {
    Pending,
    Ready(Arc<CheckReceipt>),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelAck {
    Cancelled,
    CancelRequested,
    AlreadyComplete,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestError {
    UnknownBase,
    UnknownLease,
    RequestIdConflict,
    ExpiredRequestId,
    UnsupportedEdit,
    StaleBytes,
    DuplicateTarget,
    InvalidEdit(EditError),
    ResourceLimit,
    PolicyMismatch,
}
#[derive(Debug)]
pub enum CaptureError {
    Load(ToolError),
    InputsChanged,
    ResourceLimit,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Promotion {
    Promoted(BaseToken),
    Conflict { actual: BaseToken },
    Ineligible,
    OperationIdConflict,
    ResourceLimit,
}
#[derive(Clone, Debug)]
pub struct Limits {
    pub bytes: usize,
    pub input_bytes: usize,
    pub input_operations: usize,
    pub edit_bytes: usize,
    pub edit_files: usize,
    pub byte_edits: usize,
    pub leases: usize,
    pub requests: usize,
    pub promotions: usize,
    pub result_bytes: usize,
    pub capture_attempts: usize,
    /// Retain the complete PS-9 text for differential tooling. Ordinary
    /// receipts carry its digest and complete read edges, not a second copy
    /// of every registry/dependency signature. Counts against result_bytes.
    pub retain_canonical_export: bool,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: 64 << 20,
            input_bytes: 16 << 20,
            input_operations: 4096,
            edit_bytes: 1 << 20,
            edit_files: 128,
            byte_edits: 4096,
            leases: 64,
            requests: 1024,
            promotions: 1024,
            result_bytes: 8 << 20,
            capture_attempts: 3,
            retain_canonical_export: false,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Counters {
    pub frozen_loads: u64,
    pub store_adoptions: u64,
    pub recomputations: u64,
    pub retained_sources: usize,
    pub completed_results: u64,
    pub dropped_engines: u64,
    pub peak_type_bytes: u64,
    pub retained_bytes: usize,
    pub capture_nanos: u64,
    pub load_nanos: u64,
    pub check_nanos: u64,
    pub receipt_nanos: u64,
}
#[derive(Debug)]
struct Budget {
    used: AtomicUsize,
    maximum: usize,
}
#[derive(Debug)]
struct Charge {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
impl Budget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> Option<Arc<Charge>> {
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|n| *n <= self.maximum)
            })
            .ok()?;
        Some(Arc::new(Charge {
            budget: self.clone(),
            bytes,
        }))
    }
}
struct CheckedEvidence {
    _metadata_charge: Arc<Charge>,
    identity: CheckedIdentity,
    products: Arc<CheckedProducts>,
    // Keep the original charge even after the receipt/work/base handle drops.
    _charge: Arc<Charge>,
}
struct Inputs {
    checked: Mutex<Option<Arc<CheckedEvidence>>>,
    token: BaseToken,
    frozen: FrozenInputs,
    namespace: Option<v2::NamespaceLease>,
    selection: Option<Arc<v2::Selection>>,
    _selection_charge: Option<Arc<Charge>>,
    sources: Arc<BTreeMap<SourceKey, Source>>,
    scope: CheckedScope,
    entry: PathBuf,
    _charge: Arc<Charge>,
    // Retain ancestor allocation owners and their single charges. This is
    // bounded lineage retention, not a constant-space mutable overlay.
    _parent: Option<Arc<Inputs>>,
}
#[derive(Clone)]
struct Source {
    physical: PathBuf,
    text: SourceText,
    editable: bool,
}
#[derive(Clone)]
enum SourceText {
    Captured(hirsel_span::SourceFile),
    Edited(Arc<str>),
}
impl std::ops::Deref for SourceText {
    type Target = str;
    fn deref(&self) -> &str {
        match self {
            Self::Captured(file) => file.text(),
            Self::Edited(text) => text,
        }
    }
}
impl AsRef<str> for SourceText {
    fn as_ref(&self) -> &str {
        self
    }
}
impl From<String> for SourceText {
    fn from(text: String) -> Self {
        Self::Edited(text.into())
    }
}
#[derive(Clone)]
pub struct BaseLease {
    inputs: Arc<Inputs>,
    owner: Weak<Workspace>,
}
impl BaseLease {
    pub fn entry(&self) -> &Path {
        &self.inputs.entry
    }
    pub fn source_path(&self, key: &SourceKey) -> Option<&Path> {
        self.inputs.sources.get(key).map(|s| s.physical.as_path())
    }
    pub fn token(&self) -> &BaseToken {
        &self.inputs.token
    }
    pub fn sources(&self) -> impl Iterator<Item = (&SourceKey, &str, ContentHash)> {
        self.inputs
            .sources
            .iter()
            .map(|(k, s)| (k, s.text.as_ref(), ContentHash::of_bytes(s.text.as_bytes())))
    }
}
struct Work {
    inputs: Arc<Inputs>,
    state: Mutex<WorkState>,
    waiters: Mutex<Vec<Weak<Waiter>>>,
}
enum WorkState {
    Queued,
    Running,
    Complete(Arc<Snapshot>),
    Failed(Arc<Snapshot>),
    Stopping,
}
struct Snapshot {
    selection: Option<Arc<v2::Selection>>,
    completion: Completion,
    phase: Option<hirsel_engine::QueryKind>,
    verdict: Verdict,
    output: Arc<CheckOutput>,
    loader_error: Option<String>,
    failure_class: Option<FailureClass>,
    products: Option<Arc<CheckedProducts>>,
    _charge: Arc<Charge>,
}
struct Waiter {
    id: CandidateId,
    parent: BaseToken,
    parent_inputs: Arc<Inputs>,
    work: Arc<Work>,
    state: Mutex<WaiterState>,
    owner: Weak<Workspace>,
    _charge: Arc<Charge>,
}
enum WaiterState {
    Pending,
    Cancelled,
    Complete(Arc<CheckReceipt>),
}
impl Drop for Waiter {
    fn drop(&mut self) {
        if let Some(w) = self.owner.upgrade() {
            w.leases.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
#[derive(Clone)]
pub struct CandidateLease {
    waiter: Arc<Waiter>,
}
struct RequestRecord {
    request: CandidateRequest,
    waiter: Weak<Waiter>,
    _charge: Arc<Charge>,
}
struct PromotionRecord {
    lease: CandidateId,
    expected: BaseToken,
    result: Promotion,
    _charge: Arc<Charge>,
}
struct State {
    head: Option<Arc<Inputs>>,
    requests: BTreeMap<CandidateId, RequestRecord>,
    requests_v2: BTreeMap<CandidateId, v2::RequestRecord>,
    works: BTreeMap<ContentHash, Weak<Work>>,
    promotions: BTreeMap<CandidateId, PromotionRecord>,
    generation: u64,
    counters: Counters,
}
struct Workspace {
    id: u64,
    compiler: Compiler,
    config: CompileConfig,
    namespace_budget: v2::LeaseBudget,
    environment: ContentHash,
    limits: Limits,
    budget: Arc<Budget>,
    state: Mutex<State>,
    head_update: Mutex<()>,
    active: AtomicUsize,
    leases: AtomicUsize,
    #[cfg(test)]
    before_compute: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    before_check: Mutex<Option<fn(&mut hirsel_engine::LoadedProgram)>>,
    #[cfg(test)]
    before_capture_recheck: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
#[derive(Clone)]
pub struct CandidateWorkspace {
    inner: Arc<Workspace>,
}
impl CandidateWorkspace {
    /// Explicit compiler policy and CT limits over immutable observed source inputs.
    pub fn new(
        config: CompileConfig,
        comptime: hirsel_engine::ComptimeLimits,
        limits: Limits,
    ) -> Result<Self, RequestError> {
        if config.verify != Verify::On || config.lowering != LoweringPolicy::Comptime {
            return Err(RequestError::PolicyMismatch);
        }
        let parts = vec![
            crate::COMPILER_IDENTITY.to_vec(),
            hirsel_tir::CHARGING_VERSION.to_le_bytes().to_vec(),
            comptime.budget.to_le_bytes().to_vec(),
            (comptime.call_depth as u64).to_le_bytes().to_vec(),
        ];
        let environment = ContentHash::of_parts(
            "hirsel.candidate-environment.v3",
            parts.iter().map(Vec::as_slice),
        );
        let compiler = crate::COMPILER
            .clone()
            .with_config(config.clone())
            .with_comptime_limits(comptime)
            .with_owned_engines();
        let budget = Arc::new(Budget {
            used: AtomicUsize::new(0),
            maximum: limits.bytes,
        });
        Ok(Self {
            inner: Arc::new(Workspace {
                id: WORKSPACES.fetch_add(1, Ordering::Relaxed),
                compiler,
                config,
                namespace_budget: v2::LeaseBudget::new(limits.bytes),
                environment,
                limits,
                budget,
                state: Mutex::new(State {
                    head: None,
                    requests: BTreeMap::new(),
                    requests_v2: BTreeMap::new(),
                    works: BTreeMap::new(),
                    promotions: BTreeMap::new(),
                    generation: 0,
                    counters: Counters::default(),
                }),
                head_update: Mutex::new(()),
                active: AtomicUsize::new(0),
                leases: AtomicUsize::new(0),
                #[cfg(test)]
                before_compute: Mutex::new(None),
                #[cfg(test)]
                before_check: Mutex::new(None),
                #[cfg(test)]
                before_capture_recheck: Mutex::new(None),
            }),
        })
    }
    /// Capture publishes only after all actual loader observations recheck.
    /// Old leases continue to own their snapshots across capture/promotion.
    pub fn capture(&self, entry: &Path) -> Result<BaseLease, CaptureError> {
        let started = std::time::Instant::now();
        let w = &self.inner;
        let _update = w.head_update.lock().unwrap();
        let entry = if entry.is_absolute() {
            entry.to_owned()
        } else {
            std::env::current_dir()
                .map_err(|e| CaptureError::Load(e.into()))?
                .join(entry)
        };
        for _ in 0..w.limits.capture_attempts {
            let capture = CaptureInputs::new(InputLimits {
                bytes: w.limits.input_bytes,
                operations: w.limits.input_operations,
            });
            // Record root alias even for a standalone source; diagnostics keep
            // its original spelling while edit identity uses its physical file.
            let alias = capture.canonicalize(&entry);
            let loaded = alias.map(|_| ()).map_err(ToolError::from).and_then(|_| {
                hirsel_engine::with_compiler_stack(|| {
                    w.compiler.load_program_inputs(&entry, &capture, false)
                })
            });
            let frozen = capture.freeze();
            #[cfg(test)]
            {
                let hook = w.before_capture_recheck.lock().unwrap().clone();
                if let Some(hook) = hook {
                    hook();
                }
            }
            if !frozen.changed_on_disk().is_empty() {
                continue;
            }
            let program = loaded.map_err(|e| {
                if is_limit(&e) {
                    CaptureError::ResourceLimit
                } else {
                    CaptureError::Load(e)
                }
            })?;
            let scope = scope(&program);
            let mut sources = BTreeMap::new();
            for (file, context) in &program.files {
                let source = program.engine.sources.file(*file).unwrap();
                let physical = frozen
                    .known_path(&source.path)
                    .unwrap_or_else(|| source.path.clone());
                sources.insert(
                    SourceKey(source.logical_path().to_owned()),
                    Source {
                        physical,
                        text: SourceText::Captured(source.clone()),
                        editable: true,
                    },
                );
            }
            drop(program);
            let charge = w
                .budget
                .reserve(
                    frozen.retained_bytes().saturating_add(
                        sources
                            .values()
                            .map(|s| {
                                s.text.len()
                                    + (s.text.bytes().filter(|b| *b == b'\n').count() + 1) * 4
                                    + s.physical.as_os_str().len() * 3
                                    + 512
                            })
                            .sum::<usize>(),
                    ),
                )
                .ok_or(CaptureError::ResourceLimit)?;
            let digest = input_digest(&frozen, w.environment);
            let mut state = w.state.lock().unwrap();
            let generation = state
                .generation
                .checked_add(1)
                .ok_or(CaptureError::ResourceLimit)?;
            let inputs = Arc::new(Inputs {
                checked: Mutex::new(None),
                token: BaseToken {
                    workspace: w.id,
                    generation,
                    inputs: digest,
                },
                frozen,
                namespace: None,
                selection: None,
                _selection_charge: None,
                sources: Arc::new(sources),
                scope,
                entry: entry.clone(),
                _charge: charge,
                _parent: None,
            });
            state.generation = generation;
            state.head = Some(inputs.clone());
            state.counters.capture_nanos += nanos(started);
            return Ok(BaseLease {
                inputs,
                owner: Arc::downgrade(w),
            });
        }
        Err(CaptureError::InputsChanged)
    }
    pub fn head(&self) -> Option<BaseToken> {
        self.inner
            .state
            .lock()
            .unwrap()
            .head
            .as_ref()
            .map(|h| h.token.clone())
    }
    /// IDs/tombstones live for this workspace's bounded lifetime. Once their
    /// allowance fills, new IDs refuse admission; expired IDs never rerun.
    pub fn submit(
        &self,
        base: &BaseLease,
        request: CandidateRequest,
    ) -> Result<CandidateLease, RequestError> {
        let w = &self.inner;
        if base.inputs.namespace.is_some() {
            return Err(RequestError::UnknownBase);
        }
        let mut request = request;
        request.priority = Priority::Background;
        for file in &mut request.edits {
            if !base.inputs.sources.contains_key(&file.file) {
                let path = base
                    .inputs
                    .frozen
                    .known_path(&file.file.0)
                    .ok_or(RequestError::UnsupportedEdit)?;
                file.file = base
                    .inputs
                    .sources
                    .iter()
                    .find(|(_, source)| source.physical == path)
                    .map(|(key, _)| key.clone())
                    .ok_or(RequestError::UnsupportedEdit)?;
            }
            file.edits.sort_by_key(|edit| (edit.start, edit.end));
        }
        request.edits.sort_by(|a, b| a.file.cmp(&b.file));
        if !base.owner.ptr_eq(&Arc::downgrade(w)) || base.inputs.token != request.base {
            return Err(RequestError::UnknownBase);
        }
        let mut state = w.state.lock().unwrap();
        if let Some(old) = state.requests.get(&request.id) {
            if old.request != request {
                return Err(RequestError::RequestIdConflict);
            }
            return old
                .waiter
                .upgrade()
                .map(|waiter| CandidateLease { waiter })
                .ok_or(RequestError::ExpiredRequestId);
        }
        if state.requests.len() >= w.limits.requests
            || w.leases.load(Ordering::Acquire) >= w.limits.leases
        {
            return Err(RequestError::ResourceLimit);
        }
        if request.edits.len() > w.limits.edit_files
            || request.edits.iter().map(|f| f.edits.len()).sum::<usize>() > w.limits.byte_edits
        {
            return Err(RequestError::ResourceLimit);
        }
        let edit_bytes = request
            .edits
            .iter()
            .try_fold(0usize, |n, f| {
                f.edits
                    .iter()
                    .try_fold(n, |n, e| n.checked_add(e.replacement.len()))
            })
            .filter(|n| *n <= w.limits.edit_bytes)
            .ok_or(RequestError::ResourceLimit)?;
        let mut replacements = BTreeMap::new();
        let mut sources = (*base.inputs.sources).clone();
        for file in &request.edits {
            let key = if sources.contains_key(&file.file) {
                file.file.clone()
            } else {
                let physical = base
                    .inputs
                    .frozen
                    .known_path(&file.file.0)
                    .ok_or(RequestError::UnsupportedEdit)?;
                sources
                    .iter()
                    .find(|(_, s)| s.physical == physical)
                    .map(|(key, _)| key.clone())
                    .ok_or(RequestError::UnsupportedEdit)?
            };
            let source = sources.get_mut(&key).unwrap();
            if !source.editable {
                return Err(RequestError::UnsupportedEdit);
            }
            if replacements.contains_key(&source.physical) {
                return Err(RequestError::DuplicateTarget);
            }
            if ContentHash::of_bytes(source.text.as_bytes()) != file.expected {
                return Err(RequestError::StaleBytes);
            }
            let text = edits::apply_bytes(&source.text, &file.edits, w.limits.input_bytes)
                .map_err(RequestError::InvalidEdit)?;
            replacements.insert(source.physical.clone(), Arc::<[u8]>::from(text.as_bytes()));
            source.text = text.into();
        }
        let frozen = base
            .inputs
            .frozen
            .replace_sources(&replacements, w.limits.input_bytes)
            .map_err(|_| RequestError::ResourceLimit)?;
        let digest = input_digest(&frozen, w.environment);
        state.works.retain(|_, v| v.strong_count() > 0);
        let work = state
            .works
            .get(&digest)
            .and_then(Weak::upgrade)
            .filter(|work| {
                work.inputs.frozen == frozen
                    && !matches!(*work.state.lock().unwrap(), WorkState::Stopping)
            });
        let work = match work {
            Some(work) => work,
            None => {
                let charge = w
                    .budget
                    .reserve(
                        frozen
                            .private_bytes_over(&base.inputs.frozen)
                            .saturating_add(
                                sources
                                    .iter()
                                    .map(|(key, s)| {
                                        key.0.as_os_str().len()
                                            + s.physical.as_os_str().len()
                                            + 256
                                            + if replacements.contains_key(&s.physical) {
                                                s.text.len()
                                            } else {
                                                0
                                            }
                                    })
                                    .sum::<usize>(),
                            ),
                    )
                    .ok_or(RequestError::ResourceLimit)?;
                let inputs = Arc::new(Inputs {
                    checked: Mutex::new(None),
                    token: BaseToken {
                        workspace: w.id,
                        generation: request.base.generation,
                        inputs: digest,
                    },
                    frozen,
                    namespace: None,
                    selection: None,
                    _selection_charge: None,
                    sources: Arc::new(sources),
                    scope: base.inputs.scope.clone(),
                    entry: base.inputs.entry.clone(),
                    _charge: charge,
                    _parent: Some(base.inputs.clone()),
                });
                let work = Arc::new(Work {
                    inputs,
                    state: Mutex::new(WorkState::Queued),
                    waiters: Mutex::new(vec![]),
                });
                state.works.insert(digest, Arc::downgrade(&work));
                work
            }
        };
        let request_bytes = edit_bytes
            .saturating_add(
                request
                    .edits
                    .iter()
                    .map(|file| {
                        file.file.0.as_os_str().len()
                            + std::mem::size_of::<FileEdit>()
                            + file.edits.len() * std::mem::size_of::<ByteEdit>()
                    })
                    .sum::<usize>(),
            )
            .saturating_add(512);
        let charge = w
            .budget
            .reserve(request_bytes)
            .ok_or(RequestError::ResourceLimit)?;
        let waiter = Arc::new(Waiter {
            id: request.id,
            parent: request.base.clone(),
            parent_inputs: base.inputs.clone(),
            work,
            state: Mutex::new(WaiterState::Pending),
            owner: Arc::downgrade(w),
            _charge: w
                .budget
                .reserve(
                    1024 + base
                        .inputs
                        .scope
                        .packages
                        .iter()
                        .map(|p| 2 * (p.name.len() + p.version.len() + 128))
                        .sum::<usize>(),
                )
                .ok_or(RequestError::ResourceLimit)?,
        });
        waiter
            .work
            .waiters
            .lock()
            .unwrap()
            .push(Arc::downgrade(&waiter));
        w.leases.fetch_add(1, Ordering::AcqRel);
        state.requests.insert(
            request.id,
            RequestRecord {
                request,
                waiter: Arc::downgrade(&waiter),
                _charge: charge,
            },
        );
        Ok(CandidateLease { waiter })
    }
    /// Observe only; a protocol reader must not drive compiler work while it
    /// owns control-message admission. A separate serial worker calls result.
    pub fn poll(&self, lease: &CandidateLease) -> Result<CandidateResult, RequestError> {
        let w = &self.inner;
        let waiter = &lease.waiter;
        if !waiter.owner.ptr_eq(&Arc::downgrade(w)) {
            return Err(RequestError::UnknownLease);
        }
        {
            let state = waiter.state.lock().unwrap();
            if let WaiterState::Complete(receipt) = &*state {
                let receipt = receipt.clone();
                drop(state);
                return Ok(CandidateResult::Ready(self.observe(&receipt)?));
            }
            if matches!(*state, WaiterState::Cancelled) {
                drop(state);
                return Ok(CandidateResult::Ready(
                    self.control_receipt(waiter, IncompleteReason::Cancelled)?,
                ));
            }
        }
        let completed = {
            let work = waiter.work.state.lock().unwrap();
            if let WorkState::Complete(snapshot) | WorkState::Failed(snapshot) = &*work {
                Some(snapshot.clone())
            } else {
                None
            }
        };
        if let Some(snapshot) = completed {
            return self.publish(waiter, snapshot);
        }
        Ok(CandidateResult::Pending)
    }
    /// Drive at most one computation; competing calls return Pending.
    pub fn result(&self, lease: &CandidateLease) -> Result<CandidateResult, RequestError> {
        if let ready @ CandidateResult::Ready(_) = self.poll(lease)? {
            return Ok(ready);
        }
        let w = &self.inner;
        let waiter = &lease.waiter;
        if w.active
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(CandidateResult::Pending);
        }
        struct Active<'a>(&'a AtomicUsize);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.store(0, Ordering::Release);
            }
        }
        let _active = Active(&w.active);
        {
            let mut work = waiter.work.state.lock().unwrap();
            if !matches!(*work, WorkState::Queued) {
                return Ok(CandidateResult::Pending);
            }
            *work = WorkState::Running;
        }
        let snapshot =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.compute(&waiter.work)))
                .unwrap_or_else(|_| self.control_snapshot(IncompleteReason::ToolFailure));
        let snapshot = Arc::new(match snapshot {
            Ok(snapshot) => snapshot,
            Err(_) => self.control_snapshot(IncompleteReason::ResourceLimit)?,
        });
        // Snapshot publication is terminal for each waiter under its own lock.
        let waiters: Vec<_> = waiter
            .work
            .waiters
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for other in waiters {
            let _ = self.publish(&other, snapshot.clone());
        }
        {
            let mut state = waiter.work.state.lock().unwrap();
            *state = if snapshot.completion == Completion::Complete
                && !matches!(*state, WorkState::Stopping)
            {
                WorkState::Complete(snapshot)
            } else if waiter.work.inputs.namespace.is_some() {
                // Keep terminal evidence for a waiter whose metadata publication
                // met backpressure. New requests must compute afresh.
                WorkState::Failed(snapshot)
            } else {
                WorkState::Stopping
            };
        }
        self.result(lease)
    }
    pub fn cancel(&self, lease: &CandidateLease) -> Result<CancelAck, RequestError> {
        let w = &self.inner;
        let waiter = &lease.waiter;
        if !waiter.owner.ptr_eq(&Arc::downgrade(w)) {
            return Err(RequestError::UnknownLease);
        }
        let mut state = waiter.state.lock().unwrap();
        if matches!(*state, WaiterState::Complete(_)) {
            return Ok(CancelAck::AlreadyComplete);
        }
        *state = WaiterState::Cancelled;
        drop(state);
        let live = waiter
            .work
            .waiters
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
            .any(|waiter| matches!(*waiter.state.lock().unwrap(), WaiterState::Pending));
        let mut work = waiter.work.state.lock().unwrap();
        if !live && matches!(*work, WorkState::Running) {
            *work = WorkState::Stopping;
            Ok(CancelAck::CancelRequested)
        } else {
            if !live {
                *work = WorkState::Stopping;
            }
            Ok(CancelAck::Cancelled)
        }
    }
    pub fn promote(
        &self,
        operation: CandidateId,
        lease: &CandidateLease,
        expected: &BaseToken,
    ) -> Result<Promotion, RequestError> {
        let w = &self.inner;
        if !lease.waiter.owner.ptr_eq(&Arc::downgrade(w)) {
            return Err(RequestError::UnknownLease);
        }
        let _update = w.head_update.lock().unwrap();
        let mut state = w.state.lock().unwrap();
        if let Some(old) = state.promotions.get(&operation) {
            return Ok(
                if old.lease == lease.waiter.id && old.expected == *expected {
                    old.result.clone()
                } else {
                    Promotion::OperationIdConflict
                },
            );
        }
        if state.promotions.len() >= w.limits.promotions {
            return Ok(Promotion::ResourceLimit);
        }
        let Some(promotion_charge) = w.budget.reserve(512) else {
            return Ok(Promotion::ResourceLimit);
        };
        let actual = state
            .head
            .as_ref()
            .ok_or(RequestError::UnknownBase)?
            .token
            .clone();
        let receipt = lease.waiter.state.lock().unwrap();
        let result = if actual != *expected || lease.waiter.parent != *expected {
            Promotion::Conflict { actual }
        } else if let WaiterState::Complete(receipt) = &*receipt {
            if receipt.completion != Completion::Complete
                || receipt.verdict != Verdict::Pass
                || receipt.scope.policy != policy()
            {
                Promotion::Ineligible
            } else if let Some(generation) = state.generation.checked_add(1) {
                let candidate = &lease.waiter.work.inputs;
                let token = BaseToken {
                    workspace: w.id,
                    generation,
                    inputs: candidate.token.inputs,
                };
                state.head = Some(Arc::new(Inputs {
                    checked: Mutex::new(self.checked_evidence(receipt)),
                    token: token.clone(),
                    frozen: candidate.frozen.clone(),
                    namespace: candidate.namespace.clone(),
                    selection: receipt._selection.clone(),
                    _selection_charge: receipt._selection.as_ref().map(|_| receipt._charge.clone()),
                    sources: candidate.sources.clone(),
                    scope: receipt.scope.clone(),
                    entry: candidate.entry.clone(),
                    _charge: candidate._charge.clone(),
                    _parent: candidate._parent.clone(),
                }));
                state.generation = generation;
                Promotion::Promoted(token)
            } else {
                Promotion::ResourceLimit
            }
        } else {
            Promotion::Ineligible
        };
        state.promotions.insert(
            operation,
            PromotionRecord {
                lease: lease.waiter.id,
                expected: expected.clone(),
                result: result.clone(),
                _charge: promotion_charge,
            },
        );
        Ok(result)
    }
    pub fn counters(&self) -> Counters {
        let state = self.inner.state.lock().unwrap();
        let mut counters = state.counters.clone();
        counters.retained_bytes = self.inner.budget.used.load(Ordering::Acquire);
        counters.retained_sources = state.head.as_ref().map_or(0, |h| h.sources.len())
            + state
                .works
                .values()
                .filter_map(Weak::upgrade)
                .map(|w| w.inputs.sources.len())
                .sum::<usize>();
        counters
    }
    pub fn head_lease(&self) -> Option<BaseLease> {
        self.inner
            .state
            .lock()
            .unwrap()
            .head
            .as_ref()
            .map(|inputs| BaseLease {
                inputs: inputs.clone(),
                owner: Arc::downgrade(&self.inner),
            })
    }
    /// Explicit disk observation; result() never invokes it or applies bytes.
    pub fn observe_disk(&self, lease: &CandidateLease) -> Result<DiskFreshness, RequestError> {
        if !lease.waiter.owner.ptr_eq(&Arc::downgrade(&self.inner)) {
            return Err(RequestError::UnknownLease);
        }
        let changed = lease.waiter.work.inputs.frozen.changed_on_disk();
        Ok(if changed.is_empty() {
            DiskFreshness::MatchesAt(self.inner.state.lock().unwrap().generation)
        } else {
            DiskFreshness::Changed(
                changed
                    .iter()
                    .map(|op| match op {
                        hirsel_engine::loader_inputs::Operation::Read(p)
                        | hirsel_engine::loader_inputs::Operation::Canonicalize(p)
                        | hirsel_engine::loader_inputs::Operation::Metadata(p)
                        | hirsel_engine::loader_inputs::Operation::Directory(p) => p.clone(),
                    })
                    .collect(),
            )
        })
    }
    fn observe(&self, receipt: &Arc<CheckReceipt>) -> Result<Arc<CheckReceipt>, RequestError> {
        let charge = self
            .inner
            .budget
            .reserve(receipt_metadata_bytes(receipt))
            .ok_or(RequestError::ResourceLimit)?;
        let mut observed = (**receipt).clone();
        if let Some(head) = self.head() {
            observed.head = if head == receipt.base {
                HeadFreshness::CurrentAt(head.generation)
            } else {
                HeadFreshness::Stale(head)
            }
        }
        observed._observation_charge = Some(charge);
        Ok(Arc::new(observed))
    }
    fn publish(
        &self,
        waiter: &Arc<Waiter>,
        snapshot: Arc<Snapshot>,
    ) -> Result<CandidateResult, RequestError> {
        let parent = waiter.parent_inputs.checked.lock().unwrap().clone();
        let child_identity = snapshot.products.as_ref().map(|products| CheckedIdentity {
            inputs: waiter.work.inputs.token.inputs,
            compiler_environment: if waiter.work.inputs.namespace.is_some() {
                v2::environment(&self.inner)
            } else {
                self.inner.environment
            },
            scope: snapshot
                .selection
                .as_ref()
                .map_or_else(|| waiter.work.inputs.scope.clone(), |s| s.scope.clone()),
            products: products.digest,
        });
        let (dependencies, dependency_charge) = match (&parent, &child_identity, &snapshot.products)
        {
            (Some(parent), Some(child), Some(products))
                if snapshot.completion == Completion::Complete =>
            {
                self.read_delta(parent, child, products, snapshot._charge.bytes)?
            }
            _ => (
                DependencyDelta::Unknown {
                    reason: if snapshot.completion == Completion::Complete {
                        "complete checked parent evidence unavailable"
                    } else {
                        "incomplete computation"
                    }
                    .into(),
                },
                None,
            ),
        };
        let mut receipt = CheckReceipt {
            id: waiter.id,
            base: waiter.parent.clone(),
            candidate_inputs: waiter.work.inputs.token.inputs,
            compiler_environment: if waiter.work.inputs.namespace.is_some() {
                v2::environment(&self.inner)
            } else {
                self.inner.environment
            },
            scope: snapshot
                .selection
                .as_ref()
                .map_or_else(|| waiter.work.inputs.scope.clone(), |s| s.scope.clone()),
            completion: snapshot.completion.clone(),
            incomplete: (snapshot.completion != Completion::Complete).then(|| IncompleteProgress {
                uncompleted_packages: snapshot.selection.as_ref().map_or_else(
                    || waiter.work.inputs.scope.packages.clone(),
                    |s| s.scope.packages.clone(),
                ),
                phase: snapshot.phase,
            }),
            verdict: snapshot.verdict,
            output: snapshot.output.clone(),
            loader_error: snapshot.loader_error.clone(),
            failure_class: snapshot.failure_class,
            checked_products: snapshot.products.clone(),
            dependencies,
            _selection: snapshot.selection.clone(),
            _namespace: waiter.work.inputs.namespace.clone(),
            _input_charge: waiter
                .work
                .inputs
                .namespace
                .as_ref()
                .map(|_| waiter.work.inputs._charge.clone()),
            _dependency_charge: dependency_charge,
            head: HeadFreshness::CurrentAt(waiter.parent.generation),
            disk: DiskFreshness::Unobserved,
            _charge: snapshot._charge.clone(),
            _observation_charge: None,
        };
        if waiter.work.inputs.namespace.is_some() {
            receipt._observation_charge = Some(
                self.inner
                    .budget
                    .reserve(receipt_metadata_bytes(&receipt))
                    .ok_or(RequestError::ResourceLimit)?,
            );
        }
        let receipt = Arc::new(receipt);
        let mut state = waiter.state.lock().unwrap();
        match &*state {
            WaiterState::Complete(receipt) => {
                let receipt = receipt.clone();
                drop(state);
                Ok(CandidateResult::Ready(self.observe(&receipt)?))
            }
            WaiterState::Cancelled => {
                drop(state);
                Ok(CandidateResult::Ready(
                    self.control_receipt(waiter, IncompleteReason::Cancelled)?,
                ))
            }
            WaiterState::Pending => {
                *state = WaiterState::Complete(receipt.clone());
                drop(state);
                // Explicit unchanged-base requests certify this exact capture without
                // an implicit second compilation. Existing receipts remain immutable.
                if receipt.completion == Completion::Complete
                    && waiter.work.inputs.token.inputs == waiter.parent.inputs
                    && v2::same_inputs(&waiter.work.inputs, &waiter.parent_inputs)
                {
                    let mut checked = waiter.parent_inputs.checked.lock().unwrap();
                    if checked.is_none() {
                        *checked = self.checked_evidence(&receipt);
                    }
                }
                Ok(CandidateResult::Ready(self.observe(&receipt)?))
            }
        }
    }
    fn checked_evidence(&self, receipt: &CheckReceipt) -> Option<Arc<CheckedEvidence>> {
        let products = receipt.checked_products.as_ref()?;
        let bytes = 256
            + receipt
                .scope
                .packages
                .iter()
                .map(|p| p.name.len() + p.version.len() + 128)
                .sum::<usize>();
        // Optional precision must not change successful promotion rules under
        // pressure. If its metadata cannot be retained, that base stays Unknown.
        let metadata_charge = self.inner.budget.reserve(bytes)?;
        Some(Arc::new(CheckedEvidence {
            _metadata_charge: metadata_charge,
            identity: checked_identity(receipt),
            products: products.clone(),
            _charge: receipt._charge.clone(),
        }))
    }
    fn read_delta(
        &self,
        parent: &CheckedEvidence,
        child: &CheckedIdentity,
        products: &CheckedProducts,
        product_bytes: usize,
    ) -> Result<(DependencyDelta, Option<Arc<Charge>>), RequestError> {
        let before = parent.products.canonical_reads();
        let after = products.canonical_reads();
        // Count retained bytes before copying keys. Temporary canonical indexes
        // have the same bounded source as the already charged product read sets.
        let mut count = 0;
        let mut bytes = 512 + identity_bytes(&parent.identity) + identity_bytes(child);
        visit_read_changes(&before, &after, |owner, key, _, _| {
            count += 1;
            bytes =
                bytes.saturating_add(owner.len() + key.len() + std::mem::size_of::<ReadChange>());
        });
        if product_bytes.saturating_add(bytes) > self.inner.limits.result_bytes {
            return Err(RequestError::ResourceLimit);
        }
        let charge = self
            .inner
            .budget
            .reserve(bytes)
            .ok_or(RequestError::ResourceLimit)?;
        let mut reads = Vec::with_capacity(count);
        visit_read_changes(&before, &after, |owner, key, before, after| {
            reads.push(ReadChange {
                owner: owner.to_vec(),
                key: key.to_vec(),
                before,
                after,
            });
        });
        Ok((
            DependencyDelta::Known {
                parent: parent.identity.clone(),
                child: child.clone(),
                reads: reads.into(),
            },
            Some(charge),
        ))
    }
    fn control_receipt(
        &self,
        waiter: &Arc<Waiter>,
        reason: IncompleteReason,
    ) -> Result<Arc<CheckReceipt>, RequestError> {
        Ok(Arc::new(CheckReceipt {
            id: waiter.id,
            base: waiter.parent.clone(),
            candidate_inputs: waiter.work.inputs.token.inputs,
            compiler_environment: if waiter.work.inputs.namespace.is_some() {
                v2::environment(&self.inner)
            } else {
                self.inner.environment
            },
            scope: waiter.work.inputs.scope.clone(),
            completion: Completion::Incomplete(reason),
            incomplete: Some(IncompleteProgress {
                uncompleted_packages: waiter.work.inputs.scope.packages.clone(),
                phase: None,
            }),
            verdict: Verdict::Undecided,
            output: Arc::new(CheckOutput::default()),
            loader_error: None,
            failure_class: None,
            checked_products: None,
            dependencies: DependencyDelta::Unknown {
                reason: "incomplete computation".into(),
            },
            head: HeadFreshness::CurrentAt(waiter.parent.generation),
            disk: DiskFreshness::Unobserved,
            _selection: None,
            _namespace: waiter.work.inputs.namespace.clone(),
            _input_charge: waiter
                .work
                .inputs
                .namespace
                .as_ref()
                .map(|_| waiter.work.inputs._charge.clone()),
            _charge: waiter._charge.clone(),
            _dependency_charge: None,
            _observation_charge: Some(
                self.inner
                    .budget
                    .reserve(
                        1024 + waiter
                            .work
                            .inputs
                            .scope
                            .packages
                            .iter()
                            .map(|p| 2 * (p.name.len() + p.version.len() + 128))
                            .sum::<usize>(),
                    )
                    .ok_or(RequestError::ResourceLimit)?,
            ),
        }))
    }
    fn control_snapshot(&self, reason: IncompleteReason) -> Result<Snapshot, RequestError> {
        Ok(Snapshot {
            selection: None,
            completion: Completion::Incomplete(reason),
            phase: None,
            verdict: Verdict::Undecided,
            output: Arc::new(CheckOutput::default()),
            loader_error: None,
            failure_class: None,
            products: None,
            _charge: self.inner.budget.reserve(0).unwrap(),
        })
    }
    fn compute(&self, work: &Work) -> Result<Snapshot, RequestError> {
        let inputs = &work.inputs;
        let w = &self.inner;
        #[cfg(test)]
        {
            let hook = w.before_compute.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook();
            }
        }
        if matches!(*work.state.lock().unwrap(), WorkState::Stopping) {
            return self.control_snapshot(IncompleteReason::Cancelled);
        }
        let Some(charge) = w.budget.reserve(w.limits.result_bytes) else {
            return self.control_snapshot(IncompleteReason::ResourceLimit);
        };
        let computed = hirsel_engine::with_compiler_stack(|| -> Result<Snapshot, ToolError> {
            w.state.lock().unwrap().counters.frozen_loads += 1;
            let load_started = std::time::Instant::now();
            let mut program = match if let Some(namespace) = &inputs.namespace {
                namespace.load(&w.compiler, w.config.clone(), false)
            } else {
                w.compiler
                    .load_program_inputs(&inputs.entry, &inputs.frozen, true)
            } {
                Ok(program) => program,
                Err(error) => return Ok(snapshot_error(error, charge.clone(), true)),
            };
            w.state.lock().unwrap().counters.load_nanos += nanos(load_started);
            #[cfg(test)]
            if let Some(hook) = *w.before_check.lock().unwrap() {
                hook(&mut program);
            }
            let selection = inputs
                .namespace
                .as_ref()
                .map(|_| Arc::new(v2::Selection::of(&program)));
            if inputs.namespace.is_some() {
                if let Err(error) = program.parse_sources() {
                    let mut snapshot = snapshot_error(error, charge.clone(), false);
                    snapshot.selection = selection;
                    return Ok(snapshot);
                }
            }
            let check_started = std::time::Instant::now();
            let result = program.check(LoweringPolicy::Comptime);
            w.state.lock().unwrap().counters.check_nanos += nanos(check_started);
            let receipt_started = std::time::Instant::now();
            let limit = program.engine.last_comptime_limit.is_some();
            let snapshot = match result {
                Ok(output) => {
                    let complete = output.completion == CheckCompletion::Complete && !limit;
                    let rejecting = output
                        .diagnostics
                        .iter()
                        .any(|d| d.severity == hirsel_diag::Severity::Error);
                    let products = if complete {
                        match program.engine.checked_products(&program.entry) {
                            Ok(mut products) => {
                                if !w.limits.retain_canonical_export {
                                    products.canonical = None;
                                }
                                Some(Arc::new(products))
                            }
                            Err(QueryError::NotImplemented) if rejecting => None,
                            Err(QueryError::Diagnostics(diagnostics)) if rejecting => {
                                let diagnostics = hirsel_engine::select_diagnostics(
                                    diagnostics,
                                    Some(&output.sources),
                                );
                                if diagnostics != output.diagnostics {
                                    return Err(program.error(QueryError::Diagnostics(diagnostics)));
                                }
                                None
                            }
                            Err(error) => return Err(program.error(error)),
                        }
                    } else {
                        None
                    };
                    Ok(Snapshot {
                        selection: selection.clone(),
                        phase: if complete {
                            None
                        } else {
                            match output.completion {
                                CheckCompletion::Incomplete { phase } => Some(phase),
                                CheckCompletion::Complete => {
                                    program.engine.unavailable.or(program.engine.last_phase)
                                }
                            }
                        },
                        completion: if complete {
                            Completion::Complete
                        } else if !limit {
                            Completion::Incomplete(IncompleteReason::MissingPhase)
                        } else {
                            Completion::Incomplete(IncompleteReason::ResourceLimit)
                        },
                        verdict: if !complete {
                            Verdict::Undecided
                        } else if rejecting {
                            Verdict::Reject
                        } else {
                            Verdict::Pass
                        },
                        output: Arc::new(output),
                        loader_error: None,
                        failure_class: None,
                        products,
                        _charge: charge.clone(),
                    })
                }
                Err(error) => Err(program.error(error)),
            };
            let mut state = w.state.lock().unwrap();
            state.counters.receipt_nanos += nanos(receipt_started);
            state.counters.recomputations +=
                program.engine.memo_stats().recomputed.values().sum::<u64>();
            state.counters.store_adoptions += program.engine.store_adoptions();
            state.counters.peak_type_bytes = state
                .counters
                .peak_type_bytes
                .max(program.engine.types().stats().row_bytes);
            state.counters.dropped_engines += 1;
            drop(state);
            let mut snapshot =
                snapshot.unwrap_or_else(|error| snapshot_error(error, charge.clone(), false));
            snapshot.selection = selection;
            if limit {
                snapshot.completion = Completion::Incomplete(IncompleteReason::ResourceLimit);
                snapshot.verdict = Verdict::Undecided;
                snapshot.phase = snapshot.phase.or(program.engine.last_phase);
            }
            drop(program);
            Ok(snapshot)
        });
        let mut snapshot: Snapshot = match computed {
            Ok(snapshot) => snapshot,
            Err(error) => snapshot_error(error, charge.clone(), false),
        };
        drop(charge);
        let bytes = snapshot
            .selection
            .as_ref()
            .map_or(0, |s| s.retained_bytes())
            + if inputs.namespace.is_some() {
                snapshot
                    .loader_error
                    .as_ref()
                    .map_or(0, |text| text.capacity())
                    + v2::ffi_bytes(&snapshot.output.unsafe_ffi)
            } else {
                0
            }
            + source_map_bytes(&snapshot.output.sources)
            + 2048
            + snapshot
                .output
                .diagnostics
                .iter()
                .map(|d| d.to_json(&snapshot.output.sources).to_string().len())
                .sum::<usize>()
            + snapshot.products.as_ref().map_or(0, |p| {
                p.canonical.as_ref().map_or(0, |text| text.len())
                    + p.reads.len() * 256
                    + p.reads
                        .values()
                        .flat_map(|r| r.keys())
                        .map(|k| k.len() + 32)
                        .sum::<usize>()
            });
        if bytes > w.limits.result_bytes {
            return self.control_snapshot(IncompleteReason::ResourceLimit);
        }
        // Charge actual retained data after the bounded construction reserve.
        // No other owner exists until publication. This frees unused allowance
        // so eight small distinct results do not each retain an 8MiB reserve.
        if let Some(charge) = Arc::get_mut(&mut snapshot._charge) {
            let released = charge.bytes.saturating_sub(bytes);
            charge.budget.used.fetch_sub(released, Ordering::AcqRel);
            charge.bytes -= released;
        }
        if snapshot.completion == Completion::Complete {
            w.state.lock().unwrap().counters.completed_results += 1;
        }
        Ok(snapshot)
    }
}
fn checked_identity(receipt: &CheckReceipt) -> CheckedIdentity {
    CheckedIdentity {
        inputs: receipt.candidate_inputs,
        compiler_environment: receipt.compiler_environment,
        scope: receipt.scope.clone(),
        products: receipt.checked_products.as_ref().unwrap().digest,
    }
}
fn identity_bytes(identity: &CheckedIdentity) -> usize {
    256 + identity
        .scope
        .packages
        .iter()
        .map(|p| p.name.len() + p.version.len() + 128)
        .sum::<usize>()
}
// Rust std a7c37f9f library/alloc/src/collections/btree/merge_iter.rs:47-84:
// ordered merge, equal keys paired once. Unlike set difference, compare values
// too and retain both ends. Hirsel keys must be canonical bytes, not intern IDs.
fn visit_read_changes(
    before: &BTreeMap<(Vec<u8>, Vec<u8>), ContentHash>,
    after: &BTreeMap<(Vec<u8>, Vec<u8>), ContentHash>,
    mut visit: impl FnMut(&[u8], &[u8], Option<ContentHash>, Option<ContentHash>),
) {
    let mut a = before.iter().peekable();
    let mut b = after.iter().peekable();
    loop {
        let order = match (a.peek(), b.peek()) {
            (Some((ka, _)), Some((kb, _))) => ka.cmp(kb),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => break,
        };
        let (key, old, new) = match order {
            std::cmp::Ordering::Less => {
                let (key, value) = a.next().unwrap();
                (key, Some(*value), None)
            }
            std::cmp::Ordering::Greater => {
                let (key, value) = b.next().unwrap();
                (key, None, Some(*value))
            }
            std::cmp::Ordering::Equal => {
                let (key, old) = a.next().unwrap();
                let (_, new) = b.next().unwrap();
                (key, Some(*old), Some(*new))
            }
        };
        if old != new {
            visit(&key.0, &key.1, old, new);
        }
    }
}
fn source_map_bytes(sources: &hirsel_span::SourceMap) -> usize {
    let files: BTreeSet<_> = sources.anchors().map(|(_, file, _)| file).collect();
    sources.anchors().count() * 64
        + files
            .into_iter()
            .filter_map(|file| sources.file(file))
            .map(|file| {
                file.text().len()
                    + (file.text().bytes().filter(|b| *b == b'\n').count() + 1) * 4
                    + file.path.as_os_str().len()
                    + file.logical_path().as_os_str().len()
                    + 256
            })
            .sum::<usize>()
}
fn receipt_metadata_bytes(receipt: &CheckReceipt) -> usize {
    1024 + match &receipt.dependencies {
        DependencyDelta::Known { parent, child, .. } => {
            identity_bytes(parent) + identity_bytes(child)
        }
        DependencyDelta::Unknown { reason } => reason.len(),
    } + receipt.loader_error.as_ref().map_or(0, String::len)
        + receipt
            .scope
            .packages
            .iter()
            .map(|p| p.name.len() + p.version.len() + 128)
            .sum::<usize>()
        + receipt.incomplete.as_ref().map_or(0, |progress| {
            progress
                .uncompleted_packages
                .iter()
                .map(|p| p.name.len() + p.version.len() + 128)
                .sum::<usize>()
        })
}
fn nanos(started: std::time::Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}
fn snapshot_error(error: ToolError, charge: Arc<Charge>, loader: bool) -> Snapshot {
    let phase = match &error {
        ToolError::Query { phase, .. } => *phase,
        _ => None,
    };
    let failure_class = match &error {
        ToolError::Io(e) if e.get_ref().is_some_and(|e| e.is::<InputUnavailable>()) => {
            FailureClass::Unavailable
        }
        ToolError::Io(_) => FailureClass::LoadIo,
        ToolError::Runtime(_) => FailureClass::LoadManifest,
        ToolError::Query { .. } if loader => FailureClass::LoadQuery,
        _ => FailureClass::CheckQuery,
    };
    let text = format!("{}: {error}", if loader { "load" } else { "check" });
    let unavailable =
        matches!(&error,ToolError::Io(e) if e.get_ref().is_some_and(|e|e.is::<InputUnavailable>()));
    let limited = is_limit(&error);
    let mut output = CheckOutput::default();
    if let ToolError::Query { sources, .. } = &error {
        output.sources = sources.clone();
    }
    let (completion, verdict) = match error {
        ToolError::Query {
            error: QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } => {
            output.sources = sources;
            output.diagnostics = diagnostics;
            (Completion::Complete, Verdict::Reject)
        }
        ToolError::Query {
            error: QueryError::NotImplemented,
            ..
        } => (
            Completion::Incomplete(IncompleteReason::MissingPhase),
            Verdict::Undecided,
        ),
        _ => (
            Completion::Incomplete(if unavailable {
                IncompleteReason::InputUnavailable
            } else if limited {
                IncompleteReason::ResourceLimit
            } else {
                IncompleteReason::ToolFailure
            }),
            Verdict::Undecided,
        ),
    };
    Snapshot {
        selection: None,
        completion,
        phase,
        verdict,
        output: Arc::new(output),
        loader_error: Some(text),
        failure_class: Some(failure_class),
        products: None,
        _charge: charge,
    }
}
fn is_limit(error: &ToolError) -> bool {
    matches!(error,ToolError::Io(e) if e.get_ref().is_some_and(|e|e.is::<InputLimit>()))
}
fn input_digest(inputs: &FrozenInputs, environment: ContentHash) -> ContentHash {
    let mut parts = inputs.identity_parts();
    parts.push(environment.0.to_vec());
    ContentHash::of_parts(
        "hirsel.candidate-inputs.v1",
        parts.iter().map(Vec::as_slice),
    )
}
fn policy() -> CheckPolicy {
    CheckPolicy {
        verify: Verify::On,
        lowering: LoweringPolicy::Comptime,
    }
}
fn scope(program: &hirsel_engine::LoadedProgram) -> CheckedScope {
    CheckedScope {
        packages: program
            .files
            .iter()
            .map(|(_, context)| context.package.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        policy: policy(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hirsel-candidate-control-{}-{}",
                std::process::id(),
                WORKSPACES.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("main.hir"), "pub fn main()->u64{1}\n").unwrap();
            // Explicit small package: standalone source entries otherwise select std.
            std::fs::write(path.join("main.hir.json"), r#"{"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn workspace() -> CandidateWorkspace {
        CandidateWorkspace::new(
            CompileConfig::for_memory_cap(None),
            hirsel_engine::ComptimeLimits::default(),
            Limits::default(),
        )
        .unwrap()
    }
    fn submit(w: &CandidateWorkspace, base: &BaseLease, id: u64) -> CandidateLease {
        let (file, _, _) = base.sources().next().unwrap();
        let _ = file;
        w.submit(
            base,
            CandidateRequest {
                id: CandidateId {
                    client: 1,
                    nonce: id,
                },
                base: base.token().clone(),
                edits: vec![],
                priority: Priority::Selected,
            },
        )
        .unwrap()
    }
    #[test]
    fn candidate_delta_ordered_merge_ports_std_difference_vectors() {
        // Rust a7c37f9f btree/set/tests.rs:123-135, both directions;
        // extend equal keys with changed values and a distinct owner.
        for (a, b) in [
            (vec![], vec![]),
            (vec![1, 12], vec![]),
            (vec![], vec![1, 2, 3, 9]),
            (vec![1, 3, 5, 9, 11], vec![3, 9]),
            (vec![1, 3, 5, 9, 11], vec![3, 6, 9]),
            (vec![1, 3, 5, 9, 11], vec![0, 1]),
            (vec![1, 3, 5, 9, 11], vec![11, 12]),
        ] {
            let before: BTreeMap<_, _> = a
                .into_iter()
                .map(|n| ((vec![0], vec![n]), ContentHash::of_bytes(&[n])))
                .collect();
            let after: BTreeMap<_, _> = b
                .into_iter()
                .map(|n| ((vec![0], vec![n]), ContentHash::of_bytes(&[n + 1])))
                .collect();
            for (a, b) in [(&before, &after), (&after, &before)] {
                let mut actual = vec![];
                visit_read_changes(a, b, |owner, key, before, after| {
                    actual.push(((owner.to_vec(), key.to_vec()), before, after))
                });
                let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
                let expected: Vec<_> = keys
                    .into_iter()
                    .filter_map(|k| {
                        let (old, new) = (a.get(k).copied(), b.get(k).copied());
                        (old != new).then(|| (k.clone(), old, new))
                    })
                    .collect();
                assert_eq!(actual, expected);
            }
        }
    }
    #[test]
    fn candidate_delta_budget_refusal_retries_without_check_or_unaccounted_parent() {
        let f = Fixture::new();
        let w = workspace();
        let base = w.capture(&f.0.join("main.hir")).unwrap();
        let seed = submit(&w, &base, 1);
        let CandidateResult::Ready(first) = w.result(&seed).unwrap() else {
            panic!()
        };
        assert!(matches!(
            first.dependencies,
            DependencyDelta::Unknown { .. }
        ));
        let next = submit(&w, &base, 2);
        let before = w.counters();
        let pressure = w
            .inner
            .budget
            .reserve(w.inner.limits.bytes - before.retained_bytes)
            .unwrap();
        assert_eq!(w.result(&next).unwrap_err(), RequestError::ResourceLimit);
        assert_eq!(
            w.promote(
                CandidateId {
                    client: 9,
                    nonce: 1
                },
                &next,
                base.token()
            )
            .unwrap(),
            Promotion::ResourceLimit
        );
        drop(pressure);
        let CandidateResult::Ready(receipt) = w.result(&next).unwrap() else {
            panic!()
        };
        let DependencyDelta::Known { reads, .. } = &receipt.dependencies else {
            panic!()
        };
        assert!(reads.is_empty());
        assert_eq!(w.counters().frozen_loads, before.frozen_loads);
        let parent = base.inputs.checked.lock().unwrap().clone().unwrap();
        assert!(matches!(
            w.read_delta(
                &parent,
                &parent.identity,
                &parent.products,
                w.inner.limits.result_bytes
            ),
            Err(RequestError::ResourceLimit)
        ));
        let evidence_bytes = parent._charge.bytes;
        drop(parent);
        drop(base);
        drop(first);
        drop(seed);
        drop(next);
        drop(receipt);
        // The selected head owns complete evidence and its original charge.
        assert!(w.counters().retained_bytes >= evidence_bytes);
        assert!(w.counters().retained_bytes <= w.inner.limits.bytes);
    }
    #[test]
    fn candidate_capture_racing_manifest_never_publishes_mixed_inputs() {
        let f = Fixture::new();
        let manifest = f.0.join("hirsel.json");
        let text = r#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#;
        std::fs::write(&manifest, text).unwrap();
        let w = workspace();
        let original = w.capture(&manifest).unwrap();
        let target = manifest.clone();
        let changes = Arc::new(AtomicUsize::new(0));
        let count = changes.clone();
        *w.inner.before_capture_recheck.lock().unwrap() = Some(Arc::new(move || {
            let spaces = count.fetch_add(1, Ordering::AcqRel) + 1;
            std::fs::write(&target, format!("{text}{}", " ".repeat(spaces))).unwrap();
        }));
        assert!(matches!(
            w.capture(&manifest),
            Err(CaptureError::InputsChanged)
        ));
        assert_eq!(
            changes.load(Ordering::Acquire),
            w.inner.limits.capture_attempts
        );
        assert_eq!(w.head().as_ref(), Some(original.token()));
        *w.inner.before_capture_recheck.lock().unwrap() = None;
        assert!(w.capture(&manifest).unwrap().token().generation > original.token().generation);
    }
    #[test]
    fn candidate_missing_phase_preserves_progress_and_never_promotes() {
        let f = Fixture::new();
        let w = workspace();
        let base = w.capture(&f.0.join("main.hir")).unwrap();
        let seed = submit(&w, &base, 3);
        let CandidateResult::Ready(seed_receipt) = w.result(&seed).unwrap() else {
            panic!()
        };
        assert_eq!(seed_receipt.completion, Completion::Complete);
        drop(seed_receipt);
        drop(seed);
        assert!(base.inputs.checked.lock().unwrap().is_some());
        *w.inner.before_check.lock().unwrap() = Some(|program| {
            program.engine.adapters.modes = None;
        });
        let candidate = submit(&w, &base, 1);
        let CandidateResult::Ready(receipt) = w.result(&candidate).unwrap() else {
            panic!("missing incomplete receipt");
        };
        assert_eq!(
            receipt.completion,
            Completion::Incomplete(IncompleteReason::MissingPhase)
        );
        assert_eq!(receipt.verdict, Verdict::Undecided);
        assert_eq!(
            receipt.incomplete,
            Some(IncompleteProgress {
                uncompleted_packages: receipt.scope.packages.clone(),
                phase: Some(hirsel_engine::QueryKind::Modes),
            })
        );
        assert!(receipt.checked_products.is_none());
        assert!(matches!(
            receipt.dependencies,
            DependencyDelta::Unknown { .. }
        ));
        assert!(receipt.output.sources.anchors().next().is_some());
        assert!(matches!(
            w.promote(
                CandidateId {
                    client: 7,
                    nonce: 1
                },
                &candidate,
                base.token()
            )
            .unwrap(),
            Promotion::Ineligible
        ));
        *w.inner.before_check.lock().unwrap() = None;
        let retry = submit(&w, &base, 2);
        let CandidateResult::Ready(retry) = w.result(&retry).unwrap() else {
            panic!("missing recomputed receipt");
        };
        assert_eq!(retry.completion, Completion::Complete);
        assert_eq!(retry.verdict, Verdict::Pass);
        assert!(retry.incomplete.is_none());
        assert_eq!(
            w.counters().frozen_loads,
            3,
            "missing-phase outcome was cached"
        );
    }
    #[test]
    fn candidate_standalone_capture_freezes_toolchain_and_absent_sidecar() {
        let f = Fixture::new();
        let entry = f.0.join("main.hir");
        let sidecar = f.0.join("main.hir.json");
        std::fs::remove_file(&sidecar).unwrap();
        let w = workspace();
        let base = w.capture(&entry).unwrap();
        assert!(
            base.inputs
                .scope
                .packages
                .iter()
                .any(|p| p.package_is("std", "1"))
        );
        let manifest = hirsel_engine::toolchain_std_manifest().unwrap();
        assert_eq!(base.inputs.frozen.known_path(&manifest), Some(manifest));
        // A later sidecar cannot redirect an already captured standalone graph.
        std::fs::write(&sidecar, "{}").unwrap();
        assert!(!base.inputs.frozen.changed_on_disk().is_empty());
        hirsel_engine::with_compiler_stack(|| {
            let frozen = w
                .inner
                .compiler
                .load_program_inputs(&entry, &base.inputs.frozen, false)
                .unwrap();
            assert_eq!(scope(&frozen), base.inputs.scope);
            for (file, _) in &frozen.files {
                let source = frozen.engine.sources.file(*file).unwrap();
                assert!(
                    base.inputs
                        .sources
                        .values()
                        .any(|captured| captured.physical == source.path
                            && captured.text.as_ref() == source.text())
                );
            }
            assert!(w.inner.compiler.load_program_source(&entry, None).is_err());
        });
    }
    #[test]
    fn candidate_capture_and_promotion_have_one_serial_head_order() {
        let f = Fixture::new();
        let w = workspace();
        let base = w.capture(&f.0.join("main.hir")).unwrap();
        let candidate = submit(&w, &base, 1);
        assert!(matches!(
            w.result(&candidate).unwrap(),
            CandidateResult::Ready(_)
        ));
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (begin, end) = (arrived.clone(), release.clone());
        *w.inner.before_capture_recheck.lock().unwrap() = Some(Arc::new(move || {
            begin.wait();
            end.wait();
        }));
        let (runner, entry) = (w.clone(), f.0.join("main.hir"));
        let capture = std::thread::spawn(move || runner.capture(&entry).unwrap());
        arrived.wait(); // capture holds head_update until it publishes
        let (runner, expected) = (w.clone(), base.token().clone());
        let promote = std::thread::spawn(move || {
            runner
                .promote(
                    CandidateId {
                        client: 9,
                        nonce: 1,
                    },
                    &candidate,
                    &expected,
                )
                .unwrap()
        });
        release.wait();
        let captured = capture.join().unwrap();
        assert_eq!(
            promote.join().unwrap(),
            Promotion::Conflict {
                actual: captured.token().clone()
            }
        );
        assert_eq!(w.head().as_ref(), Some(captured.token()));
    }
    #[test]
    fn cancellation_at_publication_boundary_does_not_cancel_another_waiter() {
        let f = Fixture::new();
        let w = workspace();
        let base = w.capture(&f.0.join("main.hir")).unwrap();
        let a = submit(&w, &base, 1);
        let b = submit(&w, &base, 2);
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (begin, end) = (arrived.clone(), release.clone());
        *w.inner.before_compute.lock().unwrap() = Some(Arc::new(move || {
            begin.wait();
            end.wait();
        }));
        let (runner, lease) = (w.clone(), a.clone());
        let worker = std::thread::spawn(move || runner.result(&lease).unwrap());
        arrived.wait();
        assert_eq!(w.cancel(&b).unwrap(), CancelAck::Cancelled);
        release.wait();
        let CandidateResult::Ready(passed) = worker.join().unwrap() else {
            panic!("missing result")
        };
        assert_eq!(passed.verdict, Verdict::Pass);
        let CandidateResult::Ready(cancelled) = w.result(&b).unwrap() else {
            panic!("missing cancellation")
        };
        assert_eq!(
            cancelled.completion,
            Completion::Incomplete(IncompleteReason::Cancelled)
        );
        assert_eq!(w.cancel(&a).unwrap(), CancelAck::AlreadyComplete);
    }
    #[test]
    fn stopping_job_refuses_new_waiters_and_control_panic_is_incomplete() {
        let f = Fixture::new();
        let w = workspace();
        let base = w.capture(&f.0.join("main.hir")).unwrap();
        let a = submit(&w, &base, 1);
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let once = Arc::new(AtomicUsize::new(0));
        let (begin, end) = (arrived.clone(), release.clone());
        *w.inner.before_compute.lock().unwrap() = Some(Arc::new(move || {
            if once.fetch_add(1, Ordering::AcqRel) == 0 {
                begin.wait();
                end.wait();
            }
        }));
        let (runner, lease) = (w.clone(), a.clone());
        let worker = std::thread::spawn(move || runner.result(&lease).unwrap());
        arrived.wait();
        assert_eq!(w.cancel(&a).unwrap(), CancelAck::CancelRequested);
        let b = submit(&w, &base, 2);
        assert!(!Arc::ptr_eq(&a.waiter.work, &b.waiter.work));
        release.wait();
        worker.join().unwrap();
        let CandidateResult::Ready(passed) = w.result(&b).unwrap() else {
            panic!("missing fresh work")
        };
        assert_eq!(passed.verdict, Verdict::Pass);
        *w.inner.before_compute.lock().unwrap() =
            Some(Arc::new(|| panic!("controlled candidate panic")));
        // Use a new input state so the completed previous job is not reused.
        let (file, text, digest) = base.sources().next().unwrap();
        let c = w
            .submit(
                &base,
                CandidateRequest {
                    id: CandidateId {
                        client: 1,
                        nonce: 3,
                    },
                    base: base.token().clone(),
                    edits: vec![FileEdit {
                        file: file.clone(),
                        expected: digest,
                        edits: vec![ByteEdit {
                            start: 0,
                            end: text.len() as u32,
                            replacement: "pub fn main()->u64{2}\n".into(),
                        }],
                    }],
                    priority: Priority::Selected,
                },
            )
            .unwrap();
        let CandidateResult::Ready(failed) = w.result(&c).unwrap() else {
            panic!("missing panic result")
        };
        assert_eq!(
            failed.completion,
            Completion::Incomplete(IncompleteReason::ToolFailure)
        );
        assert_eq!(failed.verdict, Verdict::Undecided);
    }
}
