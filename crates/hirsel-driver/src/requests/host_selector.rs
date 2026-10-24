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
    sync::{Arc, Mutex, Weak},
};

/// Bounds supplied by the actual source/producer footprint planner. These are
/// parts of the existing snapshot cap, not extra per-producer process caps.
#[derive(Clone, Copy)]
pub(super) struct SnapshotPlan {
    pub source: u64,
    pub front: u64,
    pub lower: u64,
    pub check: u64,
}
pub(super) struct GraphAdmission {
    pub runtime: MemoryReservation,
    pub source: MemoryReservation,
    pub front: MemoryReservation,
    pub lower: MemoryReservation,
    pub check: MemoryReservation,
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
            .and_then(|bytes| bytes.checked_add(plan.check));
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
            transport: budget
                .reserve(limits.transport_bytes)
                .map_err(ToolError::Io)?,
        })
    }
}

/// Implemented only by installed compiler producer views. Public requests
/// carry no tasks, materializer callbacks or compiler/verifier capabilities.
pub(super) trait GraphProducts: Send + Sync {
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
#[derive(Default)]
pub(super) struct GraphSelections {
    // A current immutable graph is hot state in the one selector authority.
    // Old graphs survive only through actual live requests, not revision history.
    current: Option<Arc<InstalledGraph>>,
    live: Vec<Weak<InstalledGraph>>,
}
impl GraphSelections {
    pub fn select(
        &mut self,
        request: &CompileRequest,
        install: impl FnOnce() -> Result<Arc<InstalledGraph>, ToolError>,
    ) -> Result<
        hirsel_engine::requests::SelectedRequest<AdmittedRuntime, CompilerReadiness>,
        ToolError,
    > {
        self.live.retain(|graph| graph.strong_count() != 0);
        let selected = self
            .live
            .iter()
            .filter_map(Weak::upgrade)
            .find(|graph| graph.path == request.path && graph.inputs == request.inputs);
        let graph = match selected {
            Some(graph) => graph,
            None => {
                // Release an idle previous view before reserving its replacement.
                // A queued lease's readiness still retains its exact old graph.
                self.current = None;
                let graph = install()?;
                if graph.path != request.path || graph.inputs != request.inputs {
                    return Err(ToolError::Runtime(
                        "installed compiler graph differs from selected inputs".into(),
                    ));
                }
                self.live.push(Arc::downgrade(&graph));
                graph
            }
        };
        let (roots, output) = graph.products.select(&request.operation)?;
        let ready_graph = graph.clone();
        let failed_graph = graph.clone();
        self.current = Some(graph.clone());
        Ok(hirsel_engine::requests::SelectedRequest {
            runtime: graph.runtime.clone(),
            roots,
            output: CompilerReadiness {
                materialize: Arc::new(move |products| {
                    let completed = (output.materialize)(products)?;
                    let owner: Arc<dyn Send + Sync> = Arc::new(OutputOwner {
                        _products: ready_graph.products.clone(),
                    });
                    Ok(Arc::new(
                        CompileOutput::new(
                            completed.entry.clone(),
                            completed.inputs.clone(),
                            completed.work.clone(),
                            completed.result.clone(),
                        )
                        .retain_owner(owner),
                    ))
                }),
                failure: Arc::new(move |reference| {
                    let owner: Arc<dyn Send + Sync> = Arc::new(OutputOwner {
                        _products: failed_graph.products.clone(),
                    });
                    retain_error((output.failure)(reference), owner)
                }),
            },
        })
    }
}
fn retain_error(error: ToolError, owner: Arc<dyn Send + Sync>) -> ToolError {
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
