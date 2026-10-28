//! Host selection over admitted immutable producer graphs. This module owns
//! admission and output lifetimes, never a mutable program or a check cache.
use super::{
    CompileOperation, CompileOutput, CompileRequest, CompileResult, CompilerReadiness,
    RequestLimits,
};
use hirsel_engine::{
    CheckCompletion, CheckOutput, ToolError, loader_inputs::FrozenInputs, runtime::*,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// Bounds supplied by the actual source/producer footprint planner. These are
/// parts of the existing snapshot cap, not extra per-producer process caps.
#[derive(Clone, Copy)]
pub(super) struct SnapshotPlan {
    pub source: u64,
    pub front: u64,
    pub lower: u64,
    pub check: u64,
    pub growth: u64,
    pub run: u64,
}
pub(super) struct GraphAdmission {
    pub runtime: MemoryReservation,
    pub source: MemoryReservation,
    pub front: MemoryReservation,
    pub lower: MemoryReservation,
    pub check: MemoryReservation,
    pub growth: MemoryReservation,
    pub run: MemoryReservation,
    pub transport: MemoryReservation,
}
impl GraphAdmission {
    pub fn reserve(
        budget: &Arc<ProcessBudget>,
        limits: RequestLimits,
        plan: SnapshotPlan,
    ) -> Result<Self, ToolError> {
        let snapshot = plan
            .source
            .checked_add(plan.front)
            .and_then(|bytes| bytes.checked_add(plan.lower))
            .and_then(|bytes| bytes.checked_add(plan.check))
            .and_then(|bytes| bytes.checked_add(plan.growth))
            .and_then(|bytes| bytes.checked_add(plan.run));
        if snapshot.is_none_or(|bytes| bytes > limits.snapshot_bytes) {
            return Err(ToolError::Io(std::io::Error::other(
                "compiler snapshot footprint exceeds admission",
            )));
        }
        // A failed later reservation drops every earlier temporary. Nothing
        // becomes visible to request clients before this whole value exists.
        Ok(Self {
            runtime: budget
                .reserve(limits.runtime_bytes)
                .map_err(ToolError::Io)?,
            source: budget.reserve(plan.source).map_err(ToolError::Io)?,
            front: budget.reserve(plan.front).map_err(ToolError::Io)?,
            lower: budget.reserve(plan.lower).map_err(ToolError::Io)?,
            check: budget.reserve(plan.check).map_err(ToolError::Io)?,
            growth: budget.reserve(plan.growth).map_err(ToolError::Io)?,
            run: budget.reserve(plan.run).map_err(ToolError::Io)?,
            transport: budget
                .reserve(limits.transport_bytes)
                .map_err(ToolError::Io)?,
        })
    }
}

/// Charge the whole selected snapshot partition before S0 allocates a loader
/// view. Partitioning transfers that existing charge to immutable producers;
/// source maps returned on a loader failure keep their own charge as well.
pub(super) struct LoadingAdmission {
    runtime: MemoryReservation,
    snapshot: MemoryReservation,
    transport: MemoryReservation,
}
impl LoadingAdmission {
    fn reserve(budget: &Arc<ProcessBudget>, limits: RequestLimits) -> Result<Self, ToolError> {
        Ok(Self {
            runtime: budget
                .reserve(limits.runtime_bytes)
                .map_err(ToolError::Io)?,
            snapshot: budget
                .reserve(limits.snapshot_bytes)
                .map_err(ToolError::Io)?,
            transport: budget
                .reserve(limits.transport_bytes)
                .map_err(ToolError::Io)?,
        })
    }
    /// Transfer the already admitted transport before S0 uses the provider.
    /// The remaining zero-sized reservation keeps finish's ownership shape.
    pub fn take_transport(&mut self) -> MemoryReservation {
        self.transport.split(self.transport.bytes()).unwrap()
    }
    pub fn lend_runtime(&mut self) -> MemoryReservation {
        self.runtime.split(self.runtime.bytes()).unwrap()
    }
    pub fn return_runtime(&mut self, reservation: MemoryReservation) {
        self.runtime
            .join(reservation)
            .ok()
            .expect("same runtime partition");
    }
    /// Lend the whole runtime and snapshot charge to work that finishes and
    /// releases it before any final producer takes a partition.
    pub fn lend(&mut self) -> (MemoryReservation, MemoryReservation) {
        (
            self.runtime.split(self.runtime.bytes()).unwrap(),
            self.snapshot.split(self.snapshot.bytes()).unwrap(),
        )
    }
    pub fn finish(mut self, plan: SnapshotPlan) -> Result<GraphAdmission, ToolError> {
        let bytes = [
            plan.source,
            plan.front,
            plan.lower,
            plan.growth,
            plan.check,
            plan.run,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add);
        if bytes.is_none_or(|bytes| bytes > self.snapshot.bytes()) {
            return Err(ToolError::Runtime(
                "compiler snapshot footprint exceeds admission".into(),
            ));
        }
        // The checked total proves each split fits. No reservation is released
        // while ownership transfers, even when another host is admitting work.
        let source = self.snapshot.split(plan.source).unwrap();
        let front = self.snapshot.split(plan.front).unwrap();
        let lower = self.snapshot.split(plan.lower).unwrap();
        let growth = self.snapshot.split(plan.growth).unwrap();
        let check = self.snapshot.split(plan.check).unwrap();
        let run = self.snapshot.split(plan.run).unwrap();
        Ok(GraphAdmission {
            runtime: self.runtime,
            source,
            front,
            lower,
            growth,
            check,
            run,
            transport: self.transport,
        })
    }
    pub fn failed(self, error: ToolError) -> ToolError {
        retain_error(error, Arc::new(self.snapshot))
    }
}

/// Implemented only by installed compiler producer views. Public requests
/// carry no tasks, materializer callbacks or compiler/verifier capabilities.
pub(super) trait GraphProducts: Send + Sync {
    fn check_source_owner(&self) -> Option<Arc<dyn Send + Sync>> {
        None
    }
    fn accepts(&self, _operation: &CompileOperation) -> bool {
        true
    }
    fn select(
        &self,
        operation: &CompileOperation,
    ) -> Result<(Vec<QueryId>, CompilerReadiness), ToolError>;
}
pub(super) struct InstalledGraph {
    pub path: PathBuf,
    pub inputs: FrozenInputs,
    pub runtime: Arc<Mutex<AdmittedRuntime>>,
    pub products: Arc<dyn GraphProducts>,
}
struct OutputOwner {
    _products: Arc<dyn GraphProducts>,
}
type OutputLeases = Arc<Mutex<Vec<Weak<dyn Send + Sync>>>>;
struct OutputLease {
    _owner: Arc<dyn Send + Sync>,
}
fn lease_output(leases: &OutputLeases, owner: Arc<dyn Send + Sync>) -> Arc<dyn Send + Sync> {
    let owner: Arc<dyn Send + Sync> = Arc::new(OutputLease { _owner: owner });
    let mut leases = leases.lock().unwrap_or_else(|error| error.into_inner());
    leases.retain(|lease| lease.strong_count() != 0);
    leases.push(Arc::downgrade(&owner));
    owner
}
/// One hot-graph retention authority per process budget. Releasing an idle
/// graph drops its runtime/stacks outside the index lock. Live leases and
/// externally held source products keep their independent admitted owners.
pub(super) struct GraphBudget {
    budget: Arc<ProcessBudget>,
    hot: Mutex<Vec<Arc<InstalledGraph>>>,
}
type BudgetIndex = Vec<(Weak<ProcessBudget>, Weak<GraphBudget>)>;
static BUDGETS: OnceLock<Mutex<BudgetIndex>> = OnceLock::new();
impl GraphBudget {
    pub fn process_budget(&self) -> Arc<ProcessBudget> {
        self.budget.clone()
    }
    pub fn reserve_product(
        budget: &Arc<ProcessBudget>,
        bytes: u64,
    ) -> std::io::Result<MemoryReservation> {
        let owner = Self::shared(budget);
        match budget.reserve(bytes) {
            Ok(reservation) => Ok(reservation),
            Err(_) => {
                owner.release_idle();
                budget.reserve(bytes)
            }
        }
    }
    fn shared(budget: &Arc<ProcessBudget>) -> Arc<Self> {
        let mut budgets = BUDGETS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        budgets.retain(|(budget, owner)| budget.strong_count() != 0 && owner.strong_count() != 0);
        if let Some(owner) = budgets.iter().find_map(|(selected, owner)| {
            selected
                .upgrade()
                .filter(|selected| Arc::ptr_eq(selected, budget))
                .and_then(|_| owner.upgrade())
        }) {
            return owner;
        }
        let owner = Arc::new(Self {
            budget: budget.clone(),
            hot: Mutex::new(vec![]),
        });
        budgets.push((Arc::downgrade(budget), Arc::downgrade(&owner)));
        owner
    }
    /// A request whose selection holds no live graph waits, holding nothing,
    /// for other requests' releases instead of refusing; one that holds a graph
    /// could wait on itself, so it refuses as soon as idle graphs cannot make
    /// room. A request that cannot fit even alone, or a host whose charge stops
    /// moving for `ADMISSION_STALL`, is refused rather than left waiting.
    pub fn begin(
        &self,
        limits: RequestLimits,
        may_wait: bool,
    ) -> Result<LoadingAdmission, ToolError> {
        let fits_alone = limits
            .runtime_bytes
            .checked_add(limits.snapshot_bytes)
            .and_then(|bytes| bytes.checked_add(limits.transport_bytes))
            .is_some_and(|bytes| bytes <= self.budget.limit());
        let mut seen = self.budget.reserved();
        let mut moved = std::time::Instant::now();
        loop {
            let error = match LoadingAdmission::reserve(&self.budget, limits) {
                Ok(admission) => return Ok(admission),
                Err(error) => error,
            };
            self.release_idle();
            if let Ok(admission) = LoadingAdmission::reserve(&self.budget, limits) {
                return Ok(admission);
            }
            let reserved = self.budget.reserved();
            if reserved != seen {
                seen = reserved;
                moved = std::time::Instant::now();
            }
            if !may_wait || !fits_alone || reserved == 0 || moved.elapsed() >= ADMISSION_STALL {
                return Err(error);
            }
            self.budget
                .wait_for_release(std::time::Duration::from_secs(1));
        }
    }
    pub fn admit(
        &self,
        limits: RequestLimits,
        plan: SnapshotPlan,
    ) -> Result<GraphAdmission, ToolError> {
        let snapshot = plan
            .source
            .checked_add(plan.front)
            .and_then(|bytes| bytes.checked_add(plan.lower))
            .and_then(|bytes| bytes.checked_add(plan.check))
            .and_then(|bytes| bytes.checked_add(plan.growth))
            .and_then(|bytes| bytes.checked_add(plan.run));
        if snapshot.is_none_or(|bytes| bytes > limits.snapshot_bytes) {
            return GraphAdmission::reserve(&self.budget, limits, plan);
        }
        match GraphAdmission::reserve(&self.budget, limits, plan) {
            Ok(admission) => Ok(admission),
            Err(_) => {
                self.release_idle();
                GraphAdmission::reserve(&self.budget, limits, plan)
            }
        }
    }
    fn keep(&self, graph: Arc<InstalledGraph>) {
        let mut hot = self.hot.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = hot
            .iter()
            .position(|previous| Arc::ptr_eq(previous, &graph))
        {
            hot.remove(previous);
        }
        hot.push(graph);
    }
    /// Charge a loading admission again after lent work released its part.
    pub fn readmit(
        &self,
        loading: &mut LoadingAdmission,
        runtime: u64,
        snapshot: u64,
    ) -> Result<(), ToolError> {
        let reserve = || -> std::io::Result<_> {
            Ok((
                self.budget.reserve(runtime)?,
                self.budget.reserve(snapshot)?,
            ))
        };
        let (runtime, snapshot) = match reserve() {
            Ok(reserved) => reserved,
            Err(_) => {
                self.release_idle();
                reserve().map_err(ToolError::Io)?
            }
        };
        // Both come from this budget, as the loading charge does.
        let _ = loading.runtime.join(runtime);
        let _ = loading.snapshot.join(snapshot);
        Ok(())
    }
    fn release_idle(&self) {
        let discarded = {
            let mut hot = self.hot.lock().unwrap_or_else(|error| error.into_inner());
            let mut discarded = vec![];
            let mut index = 0;
            while index < hot.len() {
                if Arc::strong_count(&hot[index]) == 1
                    && Arc::strong_count(&hot[index].runtime) == 1
                {
                    discarded.push(hot.remove(index));
                } else {
                    index += 1;
                }
            }
            discarded
        };
        drop(discarded);
    }
}
/// How long a waiting admission tolerates a process charge that neither grows
/// nor shrinks before it refuses.
const ADMISSION_STALL: std::time::Duration = std::time::Duration::from_secs(600);
pub(super) struct GraphSelections {
    retention: Arc<GraphBudget>,
    live: Vec<Weak<InstalledGraph>>,
    outputs: OutputLeases,
}
impl Drop for GraphSelections {
    fn drop(&mut self) {
        // Its hot graphs may now be idle; a waiting admission can release them.
        self.retention.budget.notify_idle();
    }
}
impl GraphSelections {
    pub fn new(budget: &Arc<ProcessBudget>) -> Self {
        Self {
            retention: GraphBudget::shared(budget),
            live: vec![],
            outputs: Arc::new(Mutex::new(vec![])),
        }
    }
    pub fn select(
        &mut self,
        request: &CompileRequest,
        install: impl FnOnce(&GraphBudget, bool) -> Result<Arc<InstalledGraph>, ToolError>,
    ) -> Result<
        hirsel_engine::requests::SelectedRequest<AdmittedRuntime, CompilerReadiness>,
        ToolError,
    > {
        self.live.retain(|graph| graph.strong_count() != 0);
        let selected = self
            .live
            .iter()
            .rev()
            .filter_map(Weak::upgrade)
            .find(|graph| {
                graph.path == request.path
                    && graph.inputs == request.inputs
                    && graph.products.accepts(&request.operation)
            });
        let graph = match selected {
            Some(graph) => graph,
            None => {
                // Graphs only the hot index holds are idle, not this
                // selection's own outputs.
                let no_active_graphs = self.live.iter().all(|graph| graph.strong_count() <= 1);
                let may_wait = no_active_graphs && {
                    let mut outputs = self
                        .outputs
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    outputs.retain(|output| output.strong_count() != 0);
                    outputs.is_empty()
                };
                let graph = install(&self.retention, may_wait)?;
                if graph.path != request.path || graph.inputs != request.inputs {
                    return Err(ToolError::Runtime(
                        "installed compiler graph differs from selected inputs".into(),
                    ));
                }
                self.live.push(Arc::downgrade(&graph));
                graph
            }
        };
        let (roots, mut output) = graph.products.select(&request.operation).map_err(|error| {
            retain_error(
                error,
                lease_output(
                    &self.outputs,
                    Arc::new(OutputOwner {
                        _products: graph.products.clone(),
                    }),
                ),
            )
        })?;
        let read_scope = output.read_scope.take();
        let ready_graph = graph.clone();
        let failed_graph = graph.clone();
        let ready_outputs = self.outputs.clone();
        let failed_outputs = self.outputs.clone();
        self.retention.keep(graph.clone());
        Ok(hirsel_engine::requests::SelectedRequest {
            runtime: graph.runtime.clone(),
            roots,
            output: CompilerReadiness {
                read_scope,
                materialize: Arc::new(move |products| {
                    let completed = (output.materialize)(products)?;
                    let owner: Arc<dyn Send + Sync> =
                        if matches!(completed.result, CompileResult::Check(_)) {
                            ready_graph.products.check_source_owner()
                        } else {
                            None
                        }
                        .unwrap_or_else(|| {
                            Arc::new(OutputOwner {
                                _products: ready_graph.products.clone(),
                            })
                        });
                    Ok(Arc::new(
                        CompileOutput::new(
                            completed.entry.clone(),
                            completed.inputs.clone(),
                            completed.work.clone(),
                            completed.result.clone(),
                        )
                        .retain_owner(lease_output(&ready_outputs, owner)),
                    ))
                }),
                failure: Arc::new(move |reference| {
                    let owner: Arc<dyn Send + Sync> = Arc::new(OutputOwner {
                        _products: failed_graph.products.clone(),
                    });
                    retain_error(
                        (output.failure)(reference),
                        lease_output(&failed_outputs, owner),
                    )
                }),
            },
        })
    }
}
pub(super) fn retain_error(error: ToolError, owner: Arc<dyn Send + Sync>) -> ToolError {
    match error {
        ToolError::Query {
            error,
            phase,
            sources,
        } => ToolError::Query {
            error,
            phase,
            sources: sources.retain_owner(owner),
        },
        other => other,
    }
}

/// Already completed export outputs. Native receipt/object bytes come from
/// their ordinary selected producers; formatting opens no compiler bodies.
pub(super) struct NativeArtifacts {
    pub receipts: Vec<u8>,
    pub objects: Vec<(super::super::native::Backend, Result<Vec<Vec<u8>>, String>)>,
}
pub(super) fn completed_artifacts(
    checked: &CheckOutput,
    tir: Result<Vec<u8>, String>,
    native: Option<NativeArtifacts>,
) -> BTreeMap<String, Vec<u8>> {
    use std::fmt::Write;
    let mut artifacts = BTreeMap::new();
    let mut diagnostics = String::new();
    for diagnostic in &checked.diagnostics {
        writeln!(diagnostics, "{}", diagnostic.to_json(&checked.sources)).unwrap();
    }
    if let CheckCompletion::Incomplete { phase } = checked.completion {
        writeln!(diagnostics, "incomplete: {}", phase.phase_name()).unwrap();
    }
    artifacts.insert("diagnostics".into(), diagnostics.into_bytes());
    if checked.completion != CheckCompletion::Complete
        || checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == hirsel_diag::Severity::Error)
    {
        return artifacts;
    }
    artifacts.insert(
        "tir".into(),
        tir.unwrap_or_else(|error| format!("export: {error}").into_bytes()),
    );
    if let Some(native) = native {
        artifacts.insert("receipts".into(), native.receipts);
        for (backend, objects) in native.objects {
            match objects {
                Ok(objects) => {
                    for (index, bytes) in objects.into_iter().enumerate() {
                        artifacts.insert(format!("objects/{}/{index:04}.o", backend.name()), bytes);
                    }
                }
                Err(error) => {
                    artifacts.insert(
                        format!("objects/{}/error", backend.name()),
                        error.into_bytes(),
                    );
                }
            }
        }
    }
    artifacts
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loading_transport_transfer_keeps_one_shared_charge_before_s0() {
        use hirsel_engine::store::{Domain, NoneStore, RequestStore, ResultStore};
        let budget = ProcessBudget::new(8320);
        let limits = RequestLimits {
            runtime_bytes: 64,
            snapshot_bytes: 64,
            transport_bytes: 8192,
        };
        let mut loading = LoadingAdmission::reserve(&budget, limits).unwrap();
        let store = Arc::new(RequestStore::new(
            Arc::new(NoneStore),
            loading.take_transport(),
        ));
        assert_eq!(budget.reserved(), 8320);
        assert!(budget.reserve(1).is_err());
        let record = store
            .put_content(Domain { kind: 1, schema: 1 }, b"source input")
            .unwrap();
        let admission = loading
            .finish(SnapshotPlan {
                source: 16,
                front: 16,
                lower: 16,
                check: 16,
                growth: 0,
                run: 0,
            })
            .unwrap();
        assert_eq!(admission.transport.bytes(), 0);
        assert_eq!(budget.reserved(), 8320);
        let held = store.get_content(record).unwrap();
        drop(admission);
        drop(store);
        assert_eq!(
            budget.reserved(),
            8192,
            "a held transport view keeps its original admission"
        );
        assert_eq!(&*held, b"source input");
        drop(held);
        assert_eq!(budget.reserved(), 0);
    }
    #[test]
    fn loading_admission_transfers_charge_and_holds_failed_diagnostic_sources() {
        let limits = RequestLimits {
            runtime_bytes: 40,
            snapshot_bytes: 40,
            transport_bytes: 20,
        };
        let budget = ProcessBudget::new(100);
        let loading = LoadingAdmission::reserve(&budget, limits).unwrap();
        assert_eq!(budget.reserved(), 100);
        assert!(budget.reserve(1).is_err());
        let graph = loading
            .finish(SnapshotPlan {
                source: 5,
                front: 5,
                lower: 5,
                growth: 5,
                check: 5,
                run: 0,
            })
            .unwrap();
        assert_eq!(budget.reserved(), 85);
        drop(graph);
        assert_eq!(budget.reserved(), 0);
        for plan in [
            SnapshotPlan {
                source: u64::MAX,
                front: 1,
                lower: 0,
                growth: 0,
                check: 0,
                run: 0,
            },
            SnapshotPlan {
                source: 41,
                front: 0,
                lower: 0,
                growth: 0,
                check: 0,
                run: 0,
            },
        ] {
            assert!(
                LoadingAdmission::reserve(&budget, limits)
                    .unwrap()
                    .finish(plan)
                    .is_err()
            );
            assert_eq!(budget.reserved(), 0);
        }
        let mut sources = hirsel_span::SourceMap::default();
        let file = sources.add("failed.hir", "bad source".into()).unwrap();
        let error = LoadingAdmission::reserve(&budget, limits)
            .unwrap()
            .failed(ToolError::Query {
                error: hirsel_engine::QueryError::NotImplemented,
                phase: None,
                sources,
            });
        assert_eq!(budget.reserved(), 40);
        let ToolError::Query { sources, .. } = error else {
            unreachable!()
        };
        let held = sources.file(file).unwrap().clone();
        drop(sources);
        assert_eq!(budget.reserved(), 40);
        assert_eq!(held.text(), "bad source");
        drop(held);
        assert_eq!(budget.reserved(), 0);
    }
    #[test]
    fn partial_graph_admission_rolls_back_and_never_multiplies_snapshot_cap() {
        let limits = RequestLimits {
            runtime_bytes: 40,
            snapshot_bytes: 40,
            transport_bytes: 20,
        };
        let plan = SnapshotPlan {
            source: 10,
            front: 10,
            lower: 10,
            check: 10,
            growth: 0,
            run: 0,
        };
        let budget = ProcessBudget::new(99);
        assert!(GraphAdmission::reserve(&budget, limits, plan).is_err());
        assert_eq!(budget.reserved(), 0);
        let budget = ProcessBudget::new(100);
        let admitted = GraphAdmission::reserve(&budget, limits, plan).unwrap();
        assert_eq!(budget.reserved(), 100);
        assert!(GraphAdmission::reserve(&budget, limits, plan).is_err());
        assert_eq!(budget.reserved(), 100);
        drop(admitted);
        assert_eq!(budget.reserved(), 0);
        for plan in [
            SnapshotPlan { check: 11, ..plan },
            SnapshotPlan {
                source: u64::MAX,
                ..plan
            },
        ] {
            assert!(GraphAdmission::reserve(&budget, limits, plan).is_err());
            assert_eq!(budget.reserved(), 0);
        }
    }
    #[test]
    fn shared_graph_budget_evicts_idle_views_but_refuses_to_overcommit_live_leases() {
        struct Empty {
            source: Arc<MemoryReservation>,
            _producer: MemoryReservation,
        }
        impl GraphProducts for Empty {
            fn check_source_owner(&self) -> Option<Arc<dyn Send + Sync>> {
                Some(self.source.clone())
            }
            fn select(
                &self,
                _: &CompileOperation,
            ) -> Result<(Vec<QueryId>, CompilerReadiness), ToolError> {
                let (_, inputs) = hirsel_engine::requests::observe_inputs(
                    hirsel_engine::loader_inputs::InputLimits {
                        bytes: 0,
                        operations: 0,
                    },
                    1,
                    |_| Ok::<_, std::io::Error>(()),
                )
                .unwrap();
                Ok((
                    vec![],
                    CompilerReadiness {
                        read_scope: None,
                        materialize: Arc::new(move |_| {
                            Ok(Arc::new(CompileOutput::new(
                                hirsel_engine::entry_path(&hirsel_engine::default_context()),
                                inputs.clone(),
                                Default::default(),
                                CompileResult::Check(CheckOutput::default()),
                            )))
                        }),
                        failure: Arc::new(|_| ToolError::Runtime("fixture runtime error".into())),
                    },
                ))
            }
        }
        let (_, inputs) = hirsel_engine::requests::observe_inputs(
            hirsel_engine::loader_inputs::InputLimits {
                bytes: 0,
                operations: 0,
            },
            1,
            |_| Ok::<_, std::io::Error>(()),
        )
        .unwrap();
        let request = CompileRequest {
            path: "first.hir".into(),
            inputs,
            operation: CompileOperation::Check {
                lowering: hirsel_engine::LoweringPolicy::Comptime,
            },
        };
        let bytes = hirsel_intern::jobs::COMPILER_STACK_BYTES as u64 + 4096;
        let budget = ProcessBudget::new(bytes + 3);
        let mut first = GraphSelections::new(&budget);
        let mut second = GraphSelections::new(&budget);
        assert!(Arc::ptr_eq(&first.retention, &second.retention));
        let create = |owner: &GraphBudget, request: &CompileRequest| {
            let admission = owner.admit(
                RequestLimits {
                    runtime_bytes: bytes,
                    snapshot_bytes: 2,
                    transport_bytes: 0,
                },
                SnapshotPlan {
                    source: 1,
                    front: 1,
                    lower: 0,
                    check: 0,
                    growth: 0,
                    run: 0,
                },
            )?;
            let error = hirsel_engine::store::Domain {
                kind: 0x4600_00fe,
                schema: 1,
            }
            .reference(b"fixture error")
            .unwrap();
            let runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::ONE,
                admission.runtime,
                RuntimeErrors {
                    resource: error,
                    cycle: error,
                    cancelled: error,
                    protocol: error,
                    panicked: error,
                },
            )
            .map_err(ToolError::Io)?;
            Ok(Arc::new(InstalledGraph {
                path: request.path.clone(),
                inputs: request.inputs.clone(),
                runtime: Arc::new(Mutex::new(runtime)),
                products: Arc::new(Empty {
                    source: Arc::new(admission.source),
                    _producer: admission.front,
                }),
            }))
        };
        let active = first
            .select(&request, |owner, _| create(owner, &request))
            .unwrap();
        let equal = first
            .select(&request, |_, _| {
                panic!("equal immutable inputs must reuse the installed graph")
            })
            .unwrap();
        assert!(Arc::ptr_eq(&active.runtime, &equal.runtime));
        assert!(
            second
                .select(&request, |owner, _| create(owner, &request))
                .is_err()
        );
        assert_eq!(budget.reserved(), bytes + 2);
        let completed = (active.output.materialize)(&[]).unwrap();
        let CompileResult::Check(checked) = &completed.result else {
            unreachable!()
        };
        let detached_sources = checked.sources.clone();
        drop(completed);
        drop(equal);
        drop(active);
        let replacement = second
            .select(&request, |owner, _| create(owner, &request))
            .unwrap();
        assert_eq!(budget.reserved(), bytes + 3);
        drop(replacement);
        drop(first);
        drop(second);
        assert_eq!(
            budget.reserved(),
            1,
            "detached source retains its original grant"
        );
        drop(detached_sources);
        assert_eq!(budget.reserved(), 0);

        // The source-only output survives after the idle runtime is reclaimed.
        // A later request by this same selector must refuse admission instead
        // of waiting for the output its own caller is holding.
        let budget = ProcessBudget::new(bytes + 2);
        let mut selector = GraphSelections::new(&budget);
        let active = selector
            .select(&request, |owner, _| create(owner, &request))
            .unwrap();
        let held = (active.output.materialize)(&[]).unwrap();
        drop(active);
        let mut changed = request.clone();
        changed.path = "after-held-output.hir".into();
        std::thread::scope(|scope| {
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let selector = &mut selector;
            let changed = &changed;
            let create = &create;
            let worker = scope.spawn(move || {
                let result = selector
                    .select(changed, |owner, may_wait| {
                        let loading = owner.begin(
                            RequestLimits {
                                runtime_bytes: bytes,
                                snapshot_bytes: 2,
                                transport_bytes: 0,
                            },
                            may_wait,
                        )?;
                        drop(loading);
                        create(owner, changed)
                    })
                    .map(|_| ());
                tx.send(result).unwrap();
            });
            let result = rx.recv_timeout(std::time::Duration::from_secs(2));
            drop(held);
            if result.is_err() {
                // Release the held source before joining even on the old
                // blocking implementation; this regression cannot hang CI.
                let _ = rx.recv_timeout(std::time::Duration::from_secs(5));
            }
            worker.join().unwrap();
            assert!(
                result
                    .expect("own output caused admission to wait")
                    .is_err()
            );
        });
        drop(selector);
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn completed_artifact_export_preserves_bytes_and_incomplete_check_boundary() {
        let checked = CheckOutput::default();
        let artifacts = completed_artifacts(
            &checked,
            Ok(b"canonical TIR".to_vec()),
            Some(NativeArtifacts {
                receipts: b"canonical receipts\n".to_vec(),
                objects: vec![
                    (crate::native::Backend::Llvm, Ok(vec![vec![1, 2], vec![3]])),
                    (
                        crate::native::Backend::Cranelift,
                        Err("unsupported operation".into()),
                    ),
                ],
            }),
        );
        assert_eq!(artifacts["tir"], b"canonical TIR");
        assert_eq!(artifacts["receipts"], b"canonical receipts\n");
        assert_eq!(artifacts["objects/llvm/0000.o"], [1, 2]);
        assert_eq!(artifacts["objects/llvm/0001.o"], [3]);
        assert_eq!(
            artifacts["objects/cranelift/error"],
            b"unsupported operation"
        );
        let incomplete = CheckOutput {
            completion: CheckCompletion::Incomplete {
                phase: hirsel_engine::QueryKind::Lower,
            },
            ..Default::default()
        };
        let artifacts = completed_artifacts(&incomplete, Ok(b"must not leak".to_vec()), None);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts["diagnostics"], b"incomplete: TIR lowering\n");
    }
}
