//! Install ordinary immutable source producers, then expose their settled roots.
//! The temporary S0 loader is dropped before the graph becomes visible. No
//! mutable engine, synchronous phase controller or second check cache survives.
use super::{
    CompileOperation, CompileOutput, CompileRequest, CompileResult, CompilerReadiness,
    RequestLimits, RequestWork,
    host_selector::{GraphAdmission, GraphBudget, GraphProducts, InstalledGraph, SnapshotPlan},
    inspection::{Counters, GraphMeter, InspectionLedger, KINDS, MeterOwner, SourceInspection},
};
use hirsel_engine::{
    CompileConfig, Compiler, FrontBodyQueries, FrontCheckPlan, FrontCheckQueries,
    FrontGrowthQueries, FrontInputFootprint, FrontLowerQueries, FrontRunQueries, LoweringPolicy,
    QueryKind, ToolError,
    loader_inputs::FrozenInputs,
    runtime::*,
    store::{Domain, RequestStore, ResultStore},
};
use hirsel_span::SourceMap;
use hirsel_syntax::DefPath;
use std::sync::{Arc, Mutex};

struct SourceProducts {
    entry: DefPath,
    inputs: FrozenInputs,
    lowering: LoweringPolicy,
    launch_entry: Option<DefPath>,
    foreign_unreplayable: bool,
    run: Option<Arc<FrontRunQueries>>,
    safe: Option<Arc<super::safe_check::SafeCheck>>,
    front: Arc<FrontBodyQueries>,
    lower: Arc<FrontLowerQueries>,
    check: Arc<FrontCheckQueries>,
    _growth: Arc<FrontGrowthQueries>,
    sources: SourceMap,
    store: Arc<RequestStore>,
    errors: RuntimeErrors,
    meter: Arc<GraphMeter>,
}

impl GraphProducts for SourceProducts {
    fn accepts(&self, operation: &CompileOperation) -> bool {
        match operation {
            CompileOperation::Check { lowering } => *lowering == self.lowering,
            CompileOperation::Inspect { .. } | CompileOperation::RecomputeCounts { .. } => true,
            CompileOperation::Tir { .. } => self.run.is_some(),
            CompileOperation::SafeCheck => self.safe.is_some(),
            _ => false,
        }
    }
    fn select(
        &self,
        operation: &CompileOperation,
    ) -> Result<(Vec<QueryId>, CompilerReadiness), ToolError> {
        let roots = match operation {
            CompileOperation::SafeCheck => {
                vec![self.safe.as_ref().ok_or_else(uninstalled)?.query()]
            }
            CompileOperation::Check { lowering } if *lowering == self.lowering => {
                vec![self.check.query()]
            }
            CompileOperation::Inspect { .. } | CompileOperation::RecomputeCounts { .. } => vec![],
            CompileOperation::Tir { definition } => {
                let entry = definition
                    .as_ref()
                    .or(self.launch_entry.as_ref())
                    .ok_or_else(|| ToolError::Runtime("launch manifest requires entry".into()))?;
                vec![
                    self.run
                        .as_ref()
                        .and_then(|run| run.query(entry))
                        .ok_or_else(|| {
                            ToolError::Runtime(
                                "selected TIR definition has no admitted run root".into(),
                            )
                        })?,
                ]
            }
            _ => return Err(uninstalled()),
        };
        let selected = roots.first().copied();
        let operation = operation.clone();
        let run = self.run.clone();
        let safe = self.safe.clone();
        let lower = self.lower.clone();
        let foreign_unreplayable = self.foreign_unreplayable;
        let check = self.check.clone();
        let expected = check.query();
        let entry = match &operation {
            CompileOperation::Tir { definition } => definition
                .as_ref()
                .or(self.launch_entry.as_ref())
                .cloned()
                .ok_or_else(|| ToolError::Runtime("launch manifest requires entry".into()))?,
            _ => self.entry.clone(),
        };
        let inputs = self.inputs.clone();
        let meter = self.meter.clone();
        let store = self.store.clone();
        let failure = self.failure_reader();
        Ok((
            roots,
            CompilerReadiness {
                materialize: Arc::new(move |products| {
                    if store.exhausted() {
                        return Err(ToolError::Runtime(
                            "compiler product transport allowance exhausted".into(),
                        ));
                    }
                    let work = meter.flush()?;
                    let result = match &operation {
                        CompileOperation::SafeCheck => {
                            let [product] = products else {
                                return Err(uninstalled());
                            };
                            if selected.is_none_or(|selected| selected.address != product.address) {
                                return Err(uninstalled());
                            }
                            let (checked, reach) =
                                safe.as_ref().ok_or_else(uninstalled)?.outcome(*product)?;
                            CompileResult::SafeCheck { checked, reach }
                        }
                        CompileOperation::Check { .. } => {
                            let [product] = products else {
                                return Err(ToolError::Runtime(
                                    "source check readiness requires exactly one root".into(),
                                ));
                            };
                            if product.address != expected.address {
                                return Err(ToolError::Runtime(
                                    "source check readiness received another root".into(),
                                ));
                            }
                            let completed = check.outcome(*product).ok_or_else(|| {
                                ToolError::Runtime("selected check product is unavailable".into())
                            })?;
                            CompileResult::Check(completed.checked)
                        }
                        CompileOperation::Tir { .. } => {
                            let [product] = products else {
                                return Err(ToolError::Runtime(
                                    "TIR readiness requires exactly one root".into(),
                                ));
                            };
                            if selected.is_none_or(|selected| selected.address != product.address) {
                                return Err(ToolError::Runtime(
                                    "TIR readiness received another root".into(),
                                ));
                            }
                            let facts = run
                                .as_ref()
                                .and_then(|run| run.outcome(*product))
                                .ok_or_else(|| {
                                    ToolError::Runtime(
                                        "selected TIR run facts are unavailable".into(),
                                    )
                                })?;
                            CompileResult::Tir(super::tir_products::materialize(
                                facts,
                                lower.clone(),
                                foreign_unreplayable,
                            )?)
                        }
                        CompileOperation::Inspect { definitions } if products.is_empty() => {
                            CompileResult::Inspect(meter.inspect(definitions)?)
                        }
                        CompileOperation::RecomputeCounts { queries } if products.is_empty() => {
                            CompileResult::RecomputeCounts(meter.counts(queries)?)
                        }
                        _ => return Err(uninstalled()),
                    };
                    Ok(Arc::new(CompileOutput::new(
                        entry.clone(),
                        inputs.clone(),
                        work,
                        result,
                    )))
                }),
                failure,
            },
        ))
    }
}
impl SourceProducts {
    fn failure_reader(
        &self,
    ) -> Arc<dyn Fn(hirsel_engine::store::Reference) -> ToolError + Send + Sync> {
        let check = self.check.clone();
        let lower = self.lower.clone();
        let front = self.front.clone();
        let sources = self.sources.clone();
        let errors = self.errors;
        let growth = self._growth.clone();
        let run = self.run.clone();
        Arc::new(move |reference| {
            let selected = check
                .failure(reference)
                .or_else(|| {
                    lower
                        .failure(reference)
                        .map(|error| (error, Some(QueryKind::Lower)))
                })
                .or_else(|| {
                    let outcome = front.outcome(reference)?.outcome;
                    outcome.result.err().map(|error| (error, outcome.phase))
                });
            let selected = selected.or_else(|| {
                growth
                    .failure(reference)
                    .map(|error| (error, Some(QueryKind::Check)))
            });
            let selected = selected.or_else(|| {
                run.as_ref()
                    .and_then(|run| run.failure(reference))
                    .map(|error| (error, None))
            });
            if let Some((error, phase)) = selected {
                return ToolError::Query {
                    error,
                    phase,
                    sources: sources.clone(),
                };
            }
            let message = if reference == errors.resource {
                "compiler query allowance exhausted"
            } else if reference == errors.cycle {
                "compiler query dependency cycle"
            } else if reference == errors.cancelled {
                "compiler query cancelled"
            } else if reference == errors.panicked {
                "compiler query producer panicked"
            } else {
                "compiler query protocol failure"
            };
            ToolError::Runtime(message.into())
        })
    }
}
fn uninstalled() -> ToolError {
    ToolError::Runtime("selected compiler operation has no installed producer".into())
}

/// Retained bounds come from the caller's existing snapshot partition. These
/// are hard producer allowances: constructors and completions refuse overflow.
/// Runtime scratch is separate, and shared across the admitted worker width.
struct Plan {
    snapshots: SnapshotPlan,
    body: MemoryWeight,
    root: MemoryWeight,
    run: MemoryWeight,
}
impl Plan {
    fn new(
        limits: RequestLimits,
        config: &CompileConfig,
        inputs: &FrozenInputs,
        definitions: &[DefPath],
        files: usize,
        needs_run: bool,
        footprint: FrontInputFootprint,
        inspection_bytes: u64,
    ) -> Result<Self, ToolError> {
        let source = u64::try_from(inputs.retained_bytes())
            .ok()
            .and_then(|bytes| bytes.checked_add(footprint.source_headers))
            .and_then(|bytes| bytes.checked_add(inspection_bytes))
            .ok_or_else(uninstalled)?;
        let metadata = MemoryWeight::default();
        let lower_inventory =
            FrontLowerQueries::input_bytes(definitions).ok_or_else(uninstalled)?;
        let growth_inventory =
            FrontGrowthQueries::input_bytes(definitions, metadata).ok_or_else(uninstalled)?;
        let check_inventory =
            FrontCheckQueries::input_bytes(definitions, files, metadata).ok_or_else(uninstalled)?;
        let run_inventory = if needs_run {
            FrontRunQueries::input_bytes(definitions).ok_or_else(uninstalled)?
        } else {
            0
        };
        let fixed = [
            source,
            footprint.front_inputs,
            lower_inventory,
            growth_inventory,
            check_inventory,
            run_inventory,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
        .ok_or_else(uninstalled)?;
        let slack = limits.snapshot_bytes.checked_sub(fixed).ok_or_else(|| {
            ToolError::Runtime(
                "compiler immutable input inventory exceeds snapshot admission".into(),
            )
        })?;
        let front_slack = slack / 4;
        let lower_slack = if needs_run { slack / 3 } else { slack / 2 };
        let growth_slack = slack / 8;
        let check_slack = if needs_run {
            slack / 8
        } else {
            slack - front_slack - lower_slack - growth_slack
        };
        let run_slack = slack - front_slack - lower_slack - growth_slack - check_slack;
        let front = footprint.front_inputs + front_slack;
        let lower = lower_inventory + lower_slack;
        let growth = growth_inventory + growth_slack;
        let check = check_inventory + check_slack;
        let run = run_inventory + run_slack;
        let stacks = (config.jobs.get() as u64)
            .checked_mul(hirsel_intern::jobs::COMPILER_STACK_BYTES as u64)
            .ok_or_else(uninstalled)?;
        let working = limits.runtime_bytes.checked_sub(stacks).ok_or_else(|| {
            ToolError::Runtime("compiler runtime cannot admit its worker stacks".into())
        })?;
        // Root continuations remain live while their P/Lower dependencies run.
        // A snapshot partition is not one task's executable memory allowance.
        let continuation_bound = working / (config.jobs.get() as u64 + 8);
        // Only demanded Lower outputs consume producer retention. Potential
        // addresses carry compact input slots, not maximum future/body grants.
        let output_metadata =
            FrontLowerQueries::output_bytes(MemoryWeight::default()).ok_or_else(uninstalled)?;
        let retained = lower_slack
            .checked_sub(output_metadata)
            .map(|bytes| (bytes / 2).min(continuation_bound))
            .filter(|bytes| *bytes != 0)
            .ok_or_else(|| {
                ToolError::Runtime("compiler Lower continuation exceeds admission".into())
            })?;
        let root_retained = growth_slack.min(check_slack).min(continuation_bound);
        if root_retained == 0 {
            return Err(ToolError::Runtime(
                "compiler root continuation exceeds admission".into(),
            ));
        }
        let scratch = working / (config.jobs.get() as u64 + 1);
        Ok(Self {
            snapshots: SnapshotPlan {
                source,
                front,
                lower,
                growth,
                check,
                run,
            },
            body: MemoryWeight {
                retained,
                scratch,
                stack: 0,
            },
            root: MemoryWeight {
                retained: root_retained,
                scratch,
                stack: 0,
            },
            run: MemoryWeight {
                retained: (run_slack / 4).min(retained),
                scratch,
                stack: 0,
            },
        })
    }
}

pub(super) fn install(
    compiler: &Compiler,
    config: &CompileConfig,
    limits: RequestLimits,
    budget: &GraphBudget,
    request: &CompileRequest,
    ledger: Arc<InspectionLedger>,
    scratch_parent: &std::path::Path,
) -> Result<Arc<InstalledGraph>, ToolError> {
    let (lowering, needs_run) = match &request.operation {
        CompileOperation::Check { lowering } => (*lowering, false),
        CompileOperation::Tir { .. } => (LoweringPolicy::Reachable, true),
        CompileOperation::SafeCheck => (config.lowering, true),
        _ => return Err(uninstalled()),
    };
    // The loader reads exactly the supplied immutable observations. Declaration
    // indexing/signature metadata is S0; no P body or Lower task runs here.
    let mut loading = budget.begin(limits)?;
    let disk_bytes = limits
        .runtime_bytes
        .checked_add(limits.snapshot_bytes)
        .and_then(|bytes| bytes.checked_add(limits.transport_bytes))
        .ok_or_else(uninstalled)?;
    let store = match RequestStore::with_scratch(
        config.store.clone(),
        loading.take_transport(),
        scratch_parent,
        disk_bytes,
    ) {
        Ok(store) => Arc::new(store),
        Err(error) => return Err(loading.failed(ToolError::Io(error))),
    };
    let mut selected_config = config.clone();
    selected_config.store = store.clone();
    let selected_compiler = compiler.clone().with_config(selected_config);
    let mut program =
        match selected_compiler.load_check_package_inputs(&request.path, &request.inputs, true) {
            Ok(program) => program,
            Err(error) => return Err(loading.failed(error)),
        };
    let launch_entry = program.launch_entry().cloned();
    let foreign_unreplayable = program.engine.foreign_unreplayable;
    let sources = program.engine.sources.clone();
    let required = program.engine.front_source_roots();
    let definitions = program
        .engine
        .indexed_definitions()
        .into_iter()
        .chain(required)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let footprint = match program.engine.front_input_footprint(&definitions) {
        Ok(footprint) => footprint,
        Err(error) => return Err(loading.failed(program.error(error))),
    };
    let plan = Plan::new(
        limits,
        config,
        &request.inputs,
        &definitions,
        program.files.len(),
        needs_run,
        footprint,
        GraphMeter::input_bytes(&definitions, program.engine.memo_snapshot().len())
            .ok_or_else(uninstalled)?,
    )?;
    let GraphAdmission {
        runtime,
        source,
        front,
        lower,
        growth,
        check,
        transport,
        mut run,
    } = loading.finish(plan.snapshots)?;
    let source = Arc::new(source);
    let query_error = |error| ToolError::Query {
        error,
        phase: None,
        sources: sources.clone().retain_owner(source.clone()),
    };
    drop(transport);
    let errors = runtime_errors(store.as_ref())?;
    let front = program
        .engine
        .front_body_queries(&definitions, plan.body, front, errors)
        .map_err(&query_error)?;
    let lower = program
        .engine
        .front_lower_queries(
            &definitions,
            front.clone(),
            plan.body,
            definitions
                .len()
                .checked_mul(3)
                .ok_or_else(uninstalled)?
                .min(4096),
            lower,
            errors,
        )
        .map_err(&query_error)?;
    let growth = program
        .engine
        .front_growth_queries(&program.entry, front.clone(), plan.root, growth, errors)
        .map_err(&query_error)?;
    let root_package = program.entry.0.package.clone();
    let check = program
        .engine
        .front_check_queries(
            FrontCheckPlan {
                files: program
                    .files
                    .iter()
                    .map(|(file, context)| {
                        (
                            *file,
                            context.clone(),
                            if context.package == root_package {
                                lowering
                            } else {
                                LoweringPolicy::Comptime
                            },
                        )
                    })
                    .collect(),
                front: front.clone(),
                lower: lower.clone(),
                growth: growth.clone(),
                unsafe_ffi: program.unsafe_ffi.clone(),
            },
            plan.root,
            check,
            errors,
        )
        .map_err(&query_error)?;
    let safe_allowance = if matches!(request.operation, CompileOperation::SafeCheck) {
        Some(
            run.split(super::safe_check::BYTES)
                .ok_or_else(uninstalled)?,
        )
    } else {
        None
    };
    let run = if needs_run {
        Some(
            program
                .engine
                .front_run_queries(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    plan.run,
                    run,
                    errors,
                )
                .map_err(&query_error)?,
        )
    } else {
        drop(run);
        None
    };
    let safe = safe_allowance
        .map(|allowance| {
            super::safe_check::SafeCheck::new(
                check.clone(),
                run.as_ref().unwrap().clone(),
                launch_entry.clone(),
                store.clone(),
                errors,
                allowance,
            )
        })
        .transpose()?;
    // The ordinary source namespace is immutable before the first request.
    // Preserve the original small-graph subscription/factory headroom while
    // admitting every actual P/Lower/Evaluate/Run address of larger packages.
    let mut inventory = std::collections::BTreeSet::new();
    for path in &definitions {
        for query in [
            front.query(path),
            lower.query(path),
            lower.evaluate_query(path),
            run.as_ref().and_then(|run| run.query(path)),
        ]
        .into_iter()
        .flatten()
        {
            inventory.insert((query.kind, query.address.0));
        }
    }
    for query in [
        Some(growth.query()),
        Some(check.query()),
        safe.as_ref().map(|safe| safe.query()),
    ]
    .into_iter()
    .flatten()
    {
        inventory.insert((query.kind, query.address.0));
    }
    let default_capacity =
        AdmittedRuntime::default_query_capacity(config.jobs, limits.runtime_bytes)
            .ok_or_else(uninstalled)?;
    let capacity = inventory.len().max(default_capacity);
    let mut runtime = AdmittedRuntime::with_query_capacity(config.jobs, runtime, errors, capacity)
        .map_err(ToolError::Io)?;
    runtime
        .register(hirsel_engine::FRONT_BODY_KIND, front.factory())
        .map_err(registration)?;
    runtime
        .register_local(hirsel_engine::FRONT_LOWER_KIND, lower.factory())
        .map_err(registration)?;
    runtime
        .register_local(hirsel_engine::FRONT_EVALUATE_KIND, lower.factory())
        .map_err(registration)?;
    runtime
        .register(hirsel_engine::FRONT_GROWTH_KIND, growth.factory())
        .map_err(registration)?;
    runtime
        .register(hirsel_engine::FRONT_CHECK_KIND, check.factory())
        .map_err(registration)?;
    if let Some(run) = &run {
        runtime
            .register(hirsel_engine::FRONT_RUN_KIND, run.factory())
            .map_err(registration)?;
    }
    if let Some(safe) = &safe {
        runtime
            .register(super::safe_check::KIND, safe.factory())
            .map_err(registration)?;
    }
    let (input_operations, input_bytes) = request.inputs.read_work();
    let initial = RequestWork {
        hold_bytes: config.hold_bytes,
        store_adoptions: program.engine.store_adoptions(),
        front: program.engine.work_stats(),
        input_operations,
        input_bytes,
        ..Default::default()
    };
    let counts = definitions
        .iter()
        .map(|path| {
            let phases = KINDS.map(|kind| program.engine.recompute_count(path, kind) as u64);
            (
                path.clone(),
                Counters {
                    phases,
                    validations: program.engine.body_validation_runs(path),
                    lower_calls: program.engine.s1_lower_work(path).adapter_calls,
                },
            )
        })
        .collect();
    let meter = GraphMeter::new(
        ledger,
        front.clone(),
        lower.clone(),
        SourceInspection {
            counts,
            memos: program.engine.memo_snapshot(),
            stats: program.engine.memo_stats().clone(),
            initial,
            owner: source,
        },
    );
    runtime
        .observe(Arc::new(MeterOwner(meter.clone())))
        .map_err(registration)?;
    let products = Arc::new(SourceProducts {
        entry: program.entry,
        inputs: request.inputs.clone(),
        lowering,
        launch_entry,
        foreign_unreplayable,
        run,
        safe,
        front,
        lower,
        check,
        _growth: growth,
        sources,
        store,
        errors,
        meter,
    });
    Ok(Arc::new(InstalledGraph {
        path: request.path.clone(),
        inputs: request.inputs.clone(),
        runtime: Arc::new(Mutex::new(runtime)),
        products,
    }))
}
fn registration(error: &'static str) -> ToolError {
    ToolError::Runtime(error.into())
}
fn runtime_errors(store: &dyn ResultStore) -> Result<RuntimeErrors, ToolError> {
    let record = |message: &[u8]| {
        store
            .put_content(
                Domain {
                    kind: 0x5200_0001,
                    schema: 1,
                },
                message,
            )
            .ok_or_else(|| {
                ToolError::Runtime("compiler runtime diagnostics exceed transport admission".into())
            })
    };
    Ok(RuntimeErrors {
        resource: record(b"compiler query allowance exhausted")?,
        cycle: record(b"compiler dependency cycle")?,
        cancelled: record(b"compiler query cancelled")?,
        protocol: record(b"compiler query protocol failure")?,
        panicked: record(b"compiler query producer panicked")?,
    })
}
