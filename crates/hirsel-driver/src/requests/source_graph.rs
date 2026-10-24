//! Install ordinary immutable source producers, then expose their settled roots.
//! The temporary S0 loader is dropped before the graph becomes visible. No
//! mutable engine, synchronous phase controller or second check cache survives.
use super::{
    CompileOperation, CompileOutput, CompileRequest, CompileResult, CompilerReadiness,
    RequestLimits, RequestWork,
    host_selector::{GraphAdmission, GraphBudget, GraphProducts, InstalledGraph, SnapshotPlan},
};
use hirsel_engine::{
    CompileConfig, Compiler, FrontBodyQueries, FrontCheckPlan, FrontCheckQueries,
    FrontGrowthQueries, FrontLowerQueries, LoweringPolicy, QueryKind, ToolError,
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
    front: Arc<FrontBodyQueries>,
    lower: Arc<FrontLowerQueries>,
    check: Arc<FrontCheckQueries>,
    _growth: Arc<FrontGrowthQueries>,
    sources: SourceMap,
    store: Arc<RequestStore>,
    errors: RuntimeErrors,
    definitions: Arc<Vec<DefPath>>,
    work: Arc<Mutex<(Option<RequestWork>, u64)>>,
    hold_bytes: u64,
    _source: MemoryReservation,
}

impl GraphProducts for SourceProducts {
    fn accepts(&self, operation: &CompileOperation) -> bool {
        match operation {
            CompileOperation::Check { lowering } => *lowering == self.lowering,
            _ => false,
        }
    }
    fn select(
        &self,
        operation: &CompileOperation,
    ) -> Result<(Vec<QueryId>, CompilerReadiness), ToolError> {
        let CompileOperation::Check { lowering } = operation else {
            return Err(uninstalled());
        };
        if *lowering != self.lowering {
            return Err(uninstalled());
        }
        let check = self.check.clone();
        let expected = check.query();
        let entry = self.entry.clone();
        let inputs = self.inputs.clone();
        let work = self.work.clone();
        let definitions = self.definitions.clone();
        let front = self.front.clone();
        let lower = self.lower.clone();
        let hold_bytes = self.hold_bytes;
        let store = self.store.clone();
        let failure = self.failure_reader();
        Ok((
            vec![expected],
            CompilerReadiness {
                materialize: Arc::new(move |products| {
                    if store.exhausted() {
                        return Err(ToolError::Runtime(
                            "compiler product transport allowance exhausted".into(),
                        ));
                    }
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
                    let recomputations = definitions.iter().fold(0u64, |sum, definition| {
                        let lower = lower.work(definition);
                        [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes]
                            .into_iter()
                            .fold(sum, |sum, kind| {
                                sum.saturating_add(front.recompute_count(definition, kind))
                            })
                            .saturating_add(lower.lowerings)
                            .saturating_add(lower.initializers)
                    });
                    let mut prior = work.lock().unwrap_or_else(|error| error.into_inner());
                    let mut work = prior.0.take().unwrap_or_default();
                    work.hold_bytes = hold_bytes;
                    work.recomputations = recomputations.saturating_sub(prior.1);
                    prior.1 = recomputations;
                    drop(prior);
                    Ok(Arc::new(CompileOutput::new(
                        entry.clone(),
                        inputs.clone(),
                        work,
                        CompileResult::Check(completed.checked),
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
}
impl Plan {
    fn new(
        limits: RequestLimits,
        config: &CompileConfig,
        inputs: &FrozenInputs,
        definitions: usize,
    ) -> Result<Self, ToolError> {
        let count = u64::try_from(definitions).map_err(|_| uninstalled())?;
        let source = (inputs.retained_bytes() as u64)
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(count.checked_mul(8192)?))
            .ok_or_else(uninstalled)?;
        let available = limits.snapshot_bytes.checked_sub(source).ok_or_else(|| {
            ToolError::Runtime("compiler source inventory exceeds snapshot admission".into())
        })?;
        let front = available / 4;
        let lower = available / 2;
        let growth = available / 8;
        let check = available - front - lower - growth;
        let retained = front
            .checked_div(count.max(1))
            .and_then(|bytes| bytes.checked_sub(8192))
            .filter(|bytes| *bytes != 0)
            .ok_or_else(|| {
                ToolError::Runtime("compiler body inventory exceeds admission".into())
            })?;
        let root_retained = growth
            .checked_sub(count.checked_mul(8192).ok_or_else(uninstalled)?)
            .filter(|bytes| *bytes != 0)
            .ok_or_else(|| {
                ToolError::Runtime("compiler root inventory exceeds admission".into())
            })?;
        let stacks = (config.jobs.get() as u64)
            .checked_mul(hirsel_intern::jobs::COMPILER_STACK_BYTES as u64)
            .ok_or_else(uninstalled)?;
        let scratch = limits.runtime_bytes.saturating_sub(stacks) / (config.jobs.get() as u64 + 1);
        Ok(Self {
            snapshots: SnapshotPlan {
                source,
                front,
                lower,
                growth,
                check,
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
        })
    }
}

pub(super) fn install(
    compiler: &Compiler,
    config: &CompileConfig,
    limits: RequestLimits,
    budget: &GraphBudget,
    request: &CompileRequest,
) -> Result<Arc<InstalledGraph>, ToolError> {
    let CompileOperation::Check { lowering } = request.operation else {
        return Err(uninstalled());
    };
    // The loader reads exactly the supplied immutable observations. Declaration
    // indexing/signature metadata is S0; no P body or Lower task runs here.
    let mut program = compiler.load_check_package_inputs(&request.path, &request.inputs, true)?;
    let sources = program.engine.sources.clone();
    let query_error = |error| ToolError::Query {
        error,
        phase: None,
        sources: sources.clone(),
    };
    let definitions = program.engine.indexed_definitions();
    let plan = Plan::new(limits, config, &request.inputs, definitions.len())?;
    let GraphAdmission {
        runtime,
        source,
        front,
        lower,
        growth,
        check,
        transport,
    } = budget.admit(limits, plan.snapshots)?;
    let store = Arc::new(RequestStore::new(config.store.clone(), transport));
    program.engine = program.engine.with_result_store(store.clone());
    let errors = runtime_errors(store.as_ref())?;
    let front = program
        .engine
        .front_body_queries(&definitions, plan.body, front, errors)
        .map_err(&query_error)?;
    let lower = program
        .engine
        .front_lower_queries(&definitions, front.clone(), plan.body, 4096, lower, errors)
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
    let mut runtime = AdmittedRuntime::new(config.jobs, runtime, errors).map_err(ToolError::Io)?;
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
    let (input_operations, input_bytes) = request.inputs.read_work();
    let initial = RequestWork {
        hold_bytes: config.hold_bytes,
        front: program.engine.work_stats(),
        input_operations,
        input_bytes,
        ..Default::default()
    };
    let products = Arc::new(SourceProducts {
        entry: program.entry,
        inputs: request.inputs.clone(),
        lowering,
        front,
        lower,
        check,
        _growth: growth,
        sources,
        store,
        errors,
        definitions: Arc::new(definitions),
        work: Arc::new(Mutex::new((Some(initial), 0))),
        hold_bytes: config.hold_bytes,
        _source: source,
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
