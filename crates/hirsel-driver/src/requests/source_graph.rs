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
    FrontGrowthQueries, FrontInputFootprint, FrontInstanceBootstrap,
    FrontInstanceBootstrapAdmission, FrontInstanceBootstrapError, FrontLowerQueries,
    FrontRegistrationBootstrapError, FrontRunQueries, LoadedProgram, LoweringPolicy, QueryError,
    QueryKind, ToolError,
    loader_inputs::FrozenInputs,
    runtime::*,
    store::{Domain, RequestStore, ResultStore},
};
use hirsel_span::SourceMap;
use hirsel_syntax::DefPath;
use std::sync::{Arc, Mutex};

/// The checked-product receipt of the completed Check root: its captured
/// headers, and the one grant its staged construction is admitted from.
struct CheckedReceipt {
    headers: Arc<hirsel_engine::FrontCheckedSignatures>,
    allowance: Mutex<Option<MemoryReservation>>,
    products: std::sync::OnceLock<Arc<hirsel_engine::CheckedProducts>>,
    /// The digest-only view, charged once to what the stages left.
    digest_view: std::sync::OnceLock<Arc<hirsel_engine::CheckedProducts>>,
}
impl CheckedReceipt {
    fn products(
        &self,
        check: &Arc<FrontCheckQueries>,
        product: ProductRef,
        entry: &DefPath,
    ) -> Result<Arc<hirsel_engine::CheckedProducts>, ToolError> {
        if let Some(products) = self.products.get() {
            return Ok(products.clone());
        }
        let refused = |stage: &str, error: hirsel_engine::QueryError| {
            ToolError::Runtime(format!("checked products {stage}: {error:?}"))
        };
        let mut pool = self
            .allowance
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .ok_or_else(|| uninstalled())?;
        let mut stage = |bytes: u64, what: &str| {
            pool.split(bytes).ok_or_else(|| {
                ToolError::Runtime(format!("checked product {what} exceeds its admission"))
            })
        };
        // Each stage is sized before it allocates. Scope, input, type capture
        // and construction are released with `built`; the receipt keeps only
        // its output text and read maps charged.
        let scope = check
            .completed_scope_bytes(product)
            .map_err(|error| refused("scope sizing", error))?;
        let scope = Arc::new(
            check
                .completed_scope(product, stage(scope, "scope")?)
                .map_err(|error| refused("scope capture", error))?,
        );
        let input = scope
            .checked_product_input_bytes(&self.headers, entry)
            .map_err(|error| refused("original input sizing", error))?;
        let input = std::rc::Rc::new(
            scope
                .checked_product_input_with_allowance(
                    self.headers.clone(),
                    entry,
                    stage(input, "input")?,
                )
                .map_err(|error| refused("original input capture", error))?,
        );
        let capture = input
            .type_capture_bytes()
            .map_err(|error| refused("type capture sizing", error))?;
        let prepared = input
            .capture_types_with_allowance(stage(capture, "type capture")?)
            .map_err(|error| refused("type capture", error))?;
        // Construction moves the uniquely owned originals and their grants
        // into the export rather than admitting a second payload copy.
        let build = prepared
            .owned_build_bytes()
            .map_err(|error| refused("construction sizing", error))?;
        let built = prepared
            .build_owned_with_allowance(stage(build, "construction")?)
            .map_err(|error| refused("construction", error))?;
        let output = stage(
            built
                .output_bytes()
                .map_err(|error| refused("output sizing", error))?,
            "output",
        )?;
        let receipt = stage(
            built
                .checked_products_bytes()
                .map_err(|error| refused("receipt sizing", error))?,
            "receipt",
        )?;
        let products = built
            .checked_products_with_allowances(output, receipt)
            .map_err(|error| refused("receipt publication", error))?;
        drop(built);
        *self
            .allowance
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(pool);
        Ok(self.products.get_or_init(|| products).clone())
    }
    fn digest_view(
        &self,
        check: &Arc<FrontCheckQueries>,
        product: ProductRef,
        entry: &DefPath,
    ) -> Result<Arc<hirsel_engine::CheckedProducts>, ToolError> {
        if let Some(view) = self.digest_view.get() {
            return Ok(view.clone());
        }
        let products = self.products(check, product, entry)?;
        let view = self
            .allowance
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .and_then(|pool| products.digest_view(pool))
            .ok_or_else(|| {
                ToolError::Runtime("checked product digest view exceeds its admission".into())
            })?;
        Ok(self.digest_view.get_or_init(|| view).clone())
    }
}

/// PS9 capture over the conditional Run view of an Artifacts request, with the
/// output bytes its canonical text may occupy.
struct ArtifactsExport {
    capture: Arc<hirsel_engine::FrontProofExport>,
    output: MemoryReservation,
}

struct SourceProducts {
    entry: DefPath,
    inputs: FrozenInputs,
    lowering: LoweringPolicy,
    launch_entry: Option<DefPath>,
    run: Option<Arc<FrontRunQueries>>,
    native: Option<Arc<super::native_selection::NativeSelection>>,
    ownership: Option<Arc<super::ownership_selection::OwnershipSelection>>,
    safe: Option<Arc<super::safe_check::SafeCheck>>,
    receipt: Option<Arc<CheckedReceipt>>,
    export: Option<Arc<ArtifactsExport>>,
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
            CompileOperation::CheckedProducts { lowering, .. } => {
                *lowering == self.lowering && self.receipt.is_some()
            }
            CompileOperation::Inspect { .. } | CompileOperation::RecomputeCounts { .. } => true,
            CompileOperation::Tir { .. } => self.run.is_some(),
            CompileOperation::SafeCheck => self.safe.is_some() && self.export.is_none(),
            CompileOperation::Artifacts { native: false } | CompileOperation::ExportTir => {
                self.export.is_some()
            }
            CompileOperation::NativeOwnership(request) => self
                .ownership
                .as_ref()
                .is_some_and(|selected| selected.accepts(request)),
            CompileOperation::NativeBuild(request) => self.native.as_ref().is_some_and(|native| {
                native.accepts(request, super::native_platform::NativePurpose::Build)
            }),
            CompileOperation::NativeRun(request) => self.native.as_ref().is_some_and(|native| {
                native.accepts(request, super::native_platform::NativePurpose::Run)
            }),
            _ => false,
        }
    }
    fn select(
        &self,
        operation: &CompileOperation,
    ) -> Result<(Vec<QueryId>, CompilerReadiness), ToolError> {
        let roots = match operation {
            CompileOperation::NativeOwnership(_) => {
                vec![
                    self.ownership
                        .as_ref()
                        .ok_or_else(|| uninstalled())?
                        .query(),
                ]
            }
            CompileOperation::NativeBuild(_) | CompileOperation::NativeRun(_) => {
                vec![self.native.as_ref().ok_or_else(|| uninstalled())?.query()]
            }
            CompileOperation::SafeCheck
            | CompileOperation::Artifacts { native: false }
            | CompileOperation::ExportTir => {
                vec![self.safe.as_ref().ok_or_else(|| uninstalled())?.query()]
            }
            CompileOperation::Check { lowering }
            | CompileOperation::CheckedProducts { lowering, .. }
                if *lowering == self.lowering =>
            {
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
        let native = self.native.clone();
        let ownership = self.ownership.clone();
        let safe = self.safe.clone();
        let export = self.export.clone();
        let lower = self.lower.clone();
        let check = self.check.clone();
        let receipt = self.receipt.clone();
        let expected = check.query();
        let entry = match &operation {
            CompileOperation::NativeOwnership(request) => request
                .definition
                .as_ref()
                .or(self.launch_entry.as_ref())
                .cloned()
                .ok_or_else(|| uninstalled())?,
            CompileOperation::NativeBuild(request) | CompileOperation::NativeRun(request) => {
                request
                    .definition
                    .as_ref()
                    .or(self.launch_entry.as_ref())
                    .cloned()
                    .ok_or_else(|| uninstalled())?
            }
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
                        CompileOperation::NativeOwnership(_) => {
                            let [product] = products else {
                                return Err(uninstalled());
                            };
                            if selected.is_none_or(|query| query.address != product.address) {
                                return Err(uninstalled());
                            }
                            CompileResult::Ownership(
                                ownership
                                    .as_ref()
                                    .ok_or_else(|| uninstalled())?
                                    .materialize(*product)?,
                            )
                        }
                        CompileOperation::NativeBuild(_) | CompileOperation::NativeRun(_) => {
                            let [product] = products else {
                                return Err(uninstalled());
                            };
                            if selected.is_none_or(|query| query.address != product.address) {
                                return Err(uninstalled());
                            }
                            if let CompileOperation::NativeBuild(request) = &operation
                                && matches!(request.output, super::NativeOutput::Objects { .. })
                            {
                                match native
                                    .as_ref()
                                    .ok_or_else(|| uninstalled())?
                                    .readiness
                                    .objects(*product)
                                {
                                    Ok(objects) => CompileResult::NativeObjects(objects),
                                    Err(error) => CompileResult::NativeError(error),
                                }
                            } else {
                                match native
                                    .as_ref()
                                    .ok_or_else(|| uninstalled())?
                                    .readiness
                                    .materialize(*product)
                                {
                                    Ok(execution) => CompileResult::Native(execution),
                                    Err(error) => CompileResult::NativeError(error),
                                }
                            }
                        }
                        CompileOperation::SafeCheck => {
                            let [product] = products else {
                                return Err(uninstalled());
                            };
                            if selected.is_none_or(|selected| selected.address != product.address) {
                                return Err(uninstalled());
                            }
                            let (checked, reach) = safe
                                .as_ref()
                                .ok_or_else(|| uninstalled())?
                                .outcome(*product)?;
                            CompileResult::SafeCheck { checked, reach }
                        }
                        CompileOperation::Artifacts { native: false }
                        | CompileOperation::ExportTir => {
                            let [product] = products else {
                                return Err(uninstalled());
                            };
                            if selected.is_none_or(|selected| selected.address != product.address) {
                                return Err(uninstalled());
                            }
                            let (checked, run) = safe
                                .as_ref()
                                .ok_or_else(|| uninstalled())?
                                .settled(*product)?;
                            // A rejected or incomplete check never demanded Run;
                            // completed_artifacts then publishes diagnostics only.
                            let tir = match run {
                                Some(run) => {
                                    let export = export.as_ref().ok_or_else(|| uninstalled())?;
                                    export
                                        .capture
                                        .input(run)
                                        .map_err(|error| format!("{error:?}"))
                                        .and_then(|input| {
                                            let limit = usize::try_from(export.output.bytes())
                                                .unwrap_or(usize::MAX);
                                            hirsel_proof_export::encode_with_output_limit(
                                                &hirsel_proof_export::build(&input),
                                                &input.locator,
                                                limit,
                                            )
                                            .map(String::into_bytes)
                                            .ok_or_else(|| {
                                                format!(
                                                    "proof export exceeds its {limit} admitted output bytes"
                                                )
                                            })
                                        })
                                }
                                None => Err("check was not accepted".into()),
                            };
                            if matches!(operation, CompileOperation::ExportTir) {
                                if checked.diagnostics.iter().any(|diagnostic| {
                                    diagnostic.severity == hirsel_diag::Severity::Error
                                }) {
                                    return Err(ToolError::Query {
                                        error: QueryError::Diagnostics(checked.diagnostics),
                                        phase: None,
                                        sources: checked.sources,
                                    });
                                }
                                if let hirsel_engine::CheckCompletion::Incomplete { phase } =
                                    checked.completion
                                {
                                    return Err(ToolError::Query {
                                        error: QueryError::NotImplemented,
                                        phase: Some(phase),
                                        sources: checked.sources,
                                    });
                                }
                                CompileResult::ExportTir(
                                    String::from_utf8(tir.map_err(ToolError::Runtime)?)
                                        .map_err(|error| ToolError::Runtime(error.to_string()))?,
                                )
                            } else {
                                CompileResult::Artifacts(super::host_selector::completed_artifacts(
                                    &checked, tir, None,
                                ))
                            }
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
                        CompileOperation::CheckedProducts { canonical, .. } => {
                            let [product] = products else {
                                return Err(uninstalled());
                            };
                            if product.address != expected.address {
                                return Err(uninstalled());
                            }
                            let completed = check.outcome(*product).ok_or_else(|| {
                                ToolError::Runtime("selected check product is unavailable".into())
                            })?;
                            let receipt = receipt.as_ref().ok_or_else(|| uninstalled())?;
                            // The digest commits to the full text either way.
                            let products = if *canonical {
                                receipt.products(&check, *product, &entry)?
                            } else {
                                receipt.digest_view(&check, *product, &entry)?
                            };
                            CompileResult::CheckedProducts {
                                checked: completed.checked,
                                products,
                            }
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
        let native = self.native.clone();
        let ownership = self.ownership.clone();
        Arc::new(move |reference| {
            if let Some(error) = ownership
                .as_ref()
                .and_then(|selected| selected.failure(reference))
            {
                return error;
            }
            if let Some(error) = native
                .as_ref()
                .and_then(|native| native.assembly.failure(reference))
            {
                return error;
            }
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
#[track_caller]
fn uninstalled() -> ToolError {
    let site = std::panic::Location::caller();
    ToolError::Runtime(format!(
        "selected compiler operation has no installed producer at {}:{}",
        site.file(),
        site.line()
    ))
}

/// Nested Lower/Evaluate continuations admitted under the active frames.
const BODY_CONTINUATIONS: u64 = 32;

/// Retained bounds come from the caller's existing snapshot partition. These
/// are hard producer allowances: constructors and completions refuse overflow.
/// Runtime scratch is separate, and shared across the admitted worker width.
struct Plan {
    snapshots: SnapshotPlan,
    query_capacity: usize,
    source_index_bytes: u64,
    body: MemoryWeight,
    root: MemoryWeight,
    run: MemoryWeight,
    native: Option<super::native_selection::Footprint>,
    native_root: MemoryWeight,
    ownership: Option<super::ownership_admission::Footprint>,
}
impl Plan {
    fn new(
        limits: RequestLimits,
        config: &CompileConfig,
        inputs: &FrozenInputs,
        definitions: &[DefPath],
        files: usize,
        needs_run: bool,
        needs_native: bool,
        needs_proofs: bool,
        needs_ownership: bool,
        needs_safe: bool,
        needs_receipt: bool,
        footprint: FrontInputFootprint,
        run_input_bytes: u64,
        native_catalogue_bytes: u64,
        inspection_bytes: u64,
        source_index_bytes: u64,
    ) -> Result<Self, ToolError> {
        let source = u64::try_from(inputs.retained_bytes())
            .ok()
            .and_then(|bytes| bytes.checked_add(footprint.source_headers))
            .and_then(|bytes| bytes.checked_add(inspection_bytes))
            .ok_or_else(|| uninstalled())?;
        let metadata = MemoryWeight::default();
        let lower_inventory =
            FrontLowerQueries::input_bytes(definitions).ok_or_else(|| uninstalled())?;
        let growth_inventory =
            FrontGrowthQueries::input_bytes(definitions, metadata).ok_or_else(|| uninstalled())?;
        let check_inventory = FrontCheckQueries::input_bytes(definitions, files, metadata)
            .ok_or_else(|| uninstalled())?;
        let run_inventory = if needs_run { run_input_bytes } else { 0 };
        let native_inventory = if needs_native {
            super::native_selection::Footprint::minimum(definitions).ok_or_else(|| uninstalled())?
        } else {
            0
        };
        let ownership_inventory = if needs_ownership {
            super::ownership_admission::Footprint::new(definitions, 0)
                .and_then(|footprint| {
                    footprint.with_catalogue_bytes(definitions, native_catalogue_bytes)
                })
                .and_then(|footprint| footprint.bytes())
                .ok_or_else(|| uninstalled())?
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
            native_inventory,
            ownership_inventory,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
        .ok_or_else(|| uninstalled())?;
        let slack = limits.snapshot_bytes.checked_sub(fixed).ok_or_else(|| {
            ToolError::Runtime(
                "compiler immutable input inventory exceeds snapshot admission".into(),
            )
        })?;
        // Source indexing is an optional reuse cache. Never let it reject an
        // otherwise admissible graph or consume the producers' main grants.
        let source_index_bytes = if source_index_bytes <= slack / 32 {
            source_index_bytes
        } else {
            0
        };
        let source = source
            .checked_add(source_index_bytes)
            .ok_or_else(|| uninstalled())?;
        let slack = slack - source_index_bytes;
        let native_slack = if needs_native {
            slack / 2
        } else if needs_ownership {
            slack / 8
        } else {
            0
        };
        let slack = slack - native_slack;
        let native = if needs_native {
            Some(
                super::native_selection::Footprint::new(definitions, native_slack, needs_proofs)
                    .ok_or_else(|| uninstalled())?,
            )
        } else {
            None
        };
        let ownership = if needs_ownership {
            Some(
                super::ownership_admission::Footprint::new(definitions, native_slack)
                    .and_then(|footprint| {
                        footprint.with_catalogue_bytes(definitions, native_catalogue_bytes)
                    })
                    .ok_or_else(|| uninstalled())?,
            )
        } else {
            None
        };
        // Every partition lives as long as the graph. A full-std receipt's
        // staged peak (~580MB) is 29 times the front's retained outcomes
        // and input rows (20MB), so a receipt leaves the front a sixteenth.
        let front_slack = if needs_ownership {
            slack * 83 / 96
        } else if needs_receipt {
            slack / 16
        } else {
            slack / 4
        };
        // A checked receipt is a root output after Check, like Run, and takes
        // the remainder with no Run inventory or rows. Its staged input and
        // construction hold every lowered original, while Lower itself keeps
        // only settled metadata (13MB with its rows for full std; a body's
        // grant is the smaller continuation bound), so Lower keeps a sixteenth.
        let lower_slack = if needs_ownership {
            slack / 12
        } else if needs_receipt {
            slack / 16
        } else if needs_run {
            slack / 3
        } else {
            slack / 2
        };
        // A receipt's root continuation is already bounded by Check's
        // thirty-second, and Growth's own output is its merged edges and reads
        // (2MB for full std), so Growth keeps that thirty-second too.
        let growth_slack = if needs_ownership {
            slack / 48
        } else if needs_receipt {
            slack / 32
        } else {
            slack / 8
        };
        // Check's grant only admits its fixed input rows; beyond them a
        // receipt needs just its captured headers from this partition.
        let check_slack = if needs_ownership {
            slack / 48
        } else if needs_receipt {
            slack / 32
        } else if needs_run {
            slack / 8
        } else {
            slack - front_slack - lower_slack - growth_slack
        };
        let run_slack = slack - front_slack - lower_slack - growth_slack - check_slack;
        let front = footprint.front_inputs + front_slack;
        let lower = lower_inventory + lower_slack;
        let growth = growth_inventory + growth_slack;
        let check = check_inventory + check_slack;
        let run = ownership
            .as_ref()
            .map(|ownership| ownership.bytes().ok_or_else(|| uninstalled()))
            .transpose()?
            .unwrap_or(0)
            + run_inventory
            + run_slack
            + native
                .as_ref()
                .map(|native| native.bytes().ok_or_else(|| uninstalled()))
                .transpose()?
                .unwrap_or(0);
        let stacks = (config.jobs.get() as u64)
            .checked_mul(hirsel_intern::jobs::COMPILER_STACK_BYTES as u64)
            .ok_or_else(|| uninstalled())?;
        // Each definition can name P, Lower, Evaluate and, when selected,
        // Run, and native selection adds its Run and definition rows. Growth/
        // Check, the selected SafeCheck root and the native bootstrap and
        // source roots have separate slots.
        // Reserve the same row bound here and in the runtime constructor so
        // task grants cannot spend the scheduler's bookkeeping allocation.
        let per_definition = 4
            + usize::from(needs_run)
            + 2 * usize::from(needs_native)
            + usize::from(needs_ownership);
        let roots = 2 + usize::from(needs_safe) + 2 * usize::from(needs_native || needs_ownership);
        let inventory = definitions
            .len()
            .checked_mul(per_definition)
            .and_then(|entries| entries.checked_add(roots))
            .ok_or_else(|| uninstalled())?;
        let default_capacity =
            AdmittedRuntime::default_query_capacity(config.jobs, limits.runtime_bytes)
                .ok_or_else(|| uninstalled())?;
        let query_capacity = inventory.max(default_capacity);
        let bookkeeping =
            AdmittedRuntime::query_capacity_bytes(query_capacity).ok_or_else(|| uninstalled())?;
        let working = limits
            .runtime_bytes
            .checked_sub(stacks)
            .and_then(|bytes| bytes.checked_sub(bookkeeping))
            .filter(|bytes| *bytes != 0)
            .ok_or_else(|| {
                ToolError::Runtime(
                    "compiler runtime cannot admit its worker stacks and query inventory".into(),
                )
            })?;
        // Every admitted worker may hold one active scratch frame. Suspended
        // continuations keep only their retained grant, so they share what
        // the active frames leave: half for the root continuations the request
        // holds (Growth, Check and any selected Run, SafeCheck or native source
        // root), half for a chain of nested Lower/Evaluate
        // bodies, such as a constant read by a constant read by a function.
        // A snapshot partition is not one task's executable memory allowance.
        let jobs = config.jobs.get() as u64;
        let scratch = working / (jobs + 1);
        let suspended = working - jobs * scratch;
        // A native request adds its source root to the root continuations and
        // its bootstrap controller to the nested bodies.
        let native_frames = u64::from(needs_native || needs_ownership);
        // Run accumulates the selected semantic closure, the largest retained
        // root graph, so it holds two of the root shares.
        let root_shares = 2 + 2 * u64::from(needs_run) + u64::from(needs_safe) + native_frames;
        let root_bound = suspended / (2 * root_shares);
        let body_bound = suspended / (2 * (BODY_CONTINUATIONS + native_frames));
        // Only demanded Lower outputs consume producer retention. Potential
        // addresses carry compact input slots, not maximum future/body grants.
        let output_metadata = FrontLowerQueries::output_bytes(MemoryWeight::default())
            .ok_or_else(|| uninstalled())?;
        let retained = lower_slack
            .checked_sub(output_metadata)
            .map(|bytes| (bytes / 2).min(body_bound))
            .filter(|bytes| *bytes != 0)
            .ok_or_else(|| {
                ToolError::Runtime("compiler Lower continuation exceeds admission".into())
            })?;
        // A receipt captures headers from Check's partition before installing
        // its root. Leave room for those headers instead of promising the
        // entire partition to the root continuation as well.
        let check_continuation = if needs_receipt {
            check_slack / 2
        } else {
            check_slack
        };
        let root_retained = growth_slack.min(check_continuation).min(root_bound);
        if root_retained == 0 {
            return Err(ToolError::Runtime(
                "compiler root continuation exceeds admission".into(),
            ));
        }
        Ok(Self {
            source_index_bytes,
            snapshots: SnapshotPlan {
                source,
                front,
                lower,
                growth,
                check,
                run,
            },
            query_capacity,
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
            native_root: MemoryWeight {
                retained: native
                    .as_ref()
                    .map_or(0, |native| native.source().min(root_bound)),
                scratch,
                stack: 0,
            },
            native,
            ownership,
            run: MemoryWeight {
                // Run accumulates the selected semantic closure, so it uses
                // its two root shares rather than one Lower body.
                retained: (run_slack / 4).min(2 * root_bound),
                scratch,
                stack: 0,
            },
        })
    }
}

/// One selector's carries across its source revisions' fresh graphs.
pub(super) struct Carry {
    pub front: Arc<hirsel_engine::FrontCarry>,
    pub lower: Arc<hirsel_engine::LowerCarry>,
}
pub(super) fn install(
    compiler: &Compiler,
    config: &CompileConfig,
    limits: RequestLimits,
    budget: &GraphBudget,
    may_wait: bool,
    request: &CompileRequest,
    ledger: Arc<InspectionLedger>,
    carry: Option<&Carry>,
    source_index: &Mutex<Option<hirsel_engine::SourceIndexState>>,
    scratch_parent: &std::path::Path,
    toolchain: Option<&Arc<crate::native::Toolchain>>,
    host_toolchain: bool,
) -> Result<Arc<InstalledGraph>, ToolError> {
    let (lowering, needs_run) = match &request.operation {
        CompileOperation::Check { lowering }
        | CompileOperation::CheckedProducts { lowering, .. } => (*lowering, false),
        CompileOperation::Tir { .. } | CompileOperation::NativeOwnership(_) => {
            (LoweringPolicy::Reachable, true)
        }
        CompileOperation::SafeCheck => (config.lowering, true),
        // The oracle export reads the check and TIR of every reachable body.
        CompileOperation::Artifacts { native: false } | CompileOperation::ExportTir => {
            (LoweringPolicy::Reachable, true)
        }
        CompileOperation::NativeBuild(_) | CompileOperation::NativeRun(_) => {
            (LoweringPolicy::Reachable, true)
        }
        _ => return Err(uninstalled()),
    };
    let artifacts = matches!(
        request.operation,
        CompileOperation::Artifacts { .. } | CompileOperation::ExportTir
    );
    // The loader reads exactly the supplied immutable observations. Declaration
    // indexing/signature metadata is S0; no P body or Lower task runs here.
    let mut loading = budget.begin(limits, may_wait)?;
    let disk_bytes = limits
        .runtime_bytes
        .checked_add(limits.snapshot_bytes)
        .and_then(|bytes| bytes.checked_add(limits.transport_bytes))
        .ok_or_else(|| uninstalled())?;
    let store = match RequestStore::with_scratch(
        config.store.clone(),
        loading.take_transport(),
        scratch_parent,
        disk_bytes,
    ) {
        Ok(store) => Arc::new(store),
        Err(error) => return Err(loading.failed(ToolError::Io(error))),
    };
    // Host requests select tools against this graph's admitted provider before
    // loading source inputs. Explicit selectors never acquire ambient tools.
    let selected_toolchain = match (&request.operation, toolchain, host_toolchain) {
        (
            CompileOperation::NativeBuild(native) | CompileOperation::NativeRun(native),
            None,
            true,
        ) => match crate::native::Toolchain::for_host_request(&native.target, store.clone()) {
            Ok(tools) => Some(Arc::new(tools)),
            Err(error) => return Err(loading.failed(ToolError::Runtime(error.to_string()))),
        },
        _ => toolchain.cloned(),
    };
    let mut selected_config = config.clone();
    selected_config.store = store.clone();
    let selected_compiler = compiler.clone().with_config(selected_config);
    let prior_index = source_index
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let loaded = match prior_index {
        Some(index) => selected_compiler.load_check_package_inputs_with_source_index(
            &request.path,
            &request.inputs,
            index,
        ),
        None => selected_compiler.load_check_package_inputs(&request.path, &request.inputs, true),
    };
    let mut program = match loaded {
        Ok(program) => program,
        Err(error) => return Err(loading.failed(error)),
    };
    if let CompileOperation::NativeBuild(native) | CompileOperation::NativeRun(native) =
        &request.operation
        && let Err(error) = super::native_sources::install(
            &mut program,
            &native.supplemental_sources,
            limits.snapshot_bytes,
        )
    {
        return Err(loading.failed(error));
    }
    let launch_entry = program.launch_entry().cloned();
    let inference = loading.lend_runtime();
    let inference = match program.engine.prepare_private_inference(inference) {
        Ok(inference) => inference,
        Err(error) => return Err(loading.failed(program.error(error))),
    };
    loading.return_runtime(inference);
    // Capturing the footprint and the Run and native inputs can still name a
    // specialization (GEN-6); select again until no new root appears.
    let mut definitions = selected_definitions(&mut program.engine);
    let (mut footprint, mut run_inputs, mut native_catalogue) = loop {
        let footprint = match program.engine.front_input_footprint(&definitions) {
            Ok(footprint) => footprint,
            Err(error) => return Err(loading.failed(program.error(error))),
        };
        let run_inputs = if needs_run {
            match program.engine.front_run_inputs(limits.snapshot_bytes) {
                Ok(inputs) => Some(inputs),
                Err(error) => return Err(loading.failed(program.error(error))),
            }
        } else {
            None
        };
        // Ordinary catalogue capture can name specializations. Generated
        // declarations need their accepted views from bootstrap first.
        let generated = match program.engine.front_generator_roots(&definitions) {
            Ok(generators) => !generators.is_empty(),
            Err(error) => return Err(loading.failed(program.error(error))),
        };
        let native_catalogue =
            if matches!(request.operation, CompileOperation::NativeOwnership(_)) && !generated {
                match program
                    .engine
                    .front_native_definition_inputs(&definitions, limits.snapshot_bytes)
                {
                    Ok(inputs) => Some(inputs),
                    Err(error) => return Err(loading.failed(program.error(error))),
                }
            } else {
                None
            };
        let selected = selected_definitions(&mut program.engine);
        if selected == definitions {
            break (footprint, run_inputs, native_catalogue);
        }
        definitions = selected;
    };
    // The root package is what a session edits; keep its rows over the
    // packages it imports when one request publishes more than the store holds.
    let root = definitions
        .iter()
        .filter(|path| path.0.package == program.entry.0.package)
        .cloned()
        .collect::<Vec<_>>();
    store.demand(&program.engine.front_store_keys(&root));
    let plan_within = |limits,
                       program: &LoadedProgram,
                       definitions: &[DefPath],
                       footprint,
                       run_input_bytes,
                       native_catalogue_bytes| {
        let inspection_bytes =
            GraphMeter::input_bytes(definitions, program.engine.memo_snapshot().len())
                .ok_or_else(|| uninstalled())?;
        let source_index_bytes = program.engine.source_index_bytes().unwrap_or(0);
        Plan::new(
            limits,
            config,
            &request.inputs,
            definitions,
            program.files.len(),
            needs_run,
            matches!(
                request.operation,
                CompileOperation::NativeBuild(_) | CompileOperation::NativeRun(_)
            ),
            matches!(&request.operation, CompileOperation::NativeBuild(native) | CompileOperation::NativeRun(native) if native.evidence.is_some()),
            matches!(request.operation, CompileOperation::NativeOwnership(_)),
            matches!(
                request.operation,
                CompileOperation::SafeCheck
                    | CompileOperation::Artifacts { .. }
                    | CompileOperation::ExportTir
            ),
            matches!(request.operation, CompileOperation::CheckedProducts { .. }),
            footprint,
            run_input_bytes,
            native_catalogue_bytes,
            inspection_bytes,
            source_index_bytes,
        )
    };
    let mut plan = plan_within(
        limits,
        &program,
        &definitions,
        footprint,
        run_inputs.as_ref().map_or(0, |inputs| inputs.input_bytes()),
        native_catalogue
            .as_ref()
            .map_or(0, |inputs| inputs.input_bytes()),
    )?;
    let mut final_limits = limits;
    // Written generators are accepted before the final front captures its
    // inputs. Their bootstrap borrows this graph's whole runtime and snapshot
    // charge, sized like the final front and Lower, and shuts down before the
    // charge it released is admitted again. Accepted views keep their own
    // retained bytes, which the final partitions no longer count.
    let generators = match program.engine.front_generator_roots(&definitions) {
        Ok(generators) => generators,
        Err(error) => return Err(loading.failed(program.error(error))),
    };
    let bootstrap = if generators.is_empty() {
        None
    } else {
        let metadata = FrontInstanceBootstrap::input_bytes(&definitions, &generators)
            .ok_or_else(|| uninstalled())?;
        let (runtime, snapshot) = loading.lend();
        let lent = (runtime.bytes(), snapshot.bytes());
        let snapshot = MemoryReservationPool::new(snapshot).map_err(|_| uninstalled())?;
        let split = |bytes| snapshot.take(bytes).ok_or_else(|| uninstalled());
        let front = split(plan.snapshots.front)?;
        let lower = split(plan.snapshots.lower)?;
        let rest = lent
            .1
            .checked_sub(plan.snapshots.front)
            .and_then(|bytes| bytes.checked_sub(plan.snapshots.lower))
            .and_then(|bytes| bytes.checked_sub(MemoryReservationPool::metadata_bytes()))
            .ok_or_else(|| uninstalled())?;
        let metadata = metadata.max(rest / 8);
        let reserved = split(metadata)?;
        let rest = rest.checked_sub(metadata).ok_or_else(|| uninstalled())?;
        let discovery = split(rest / 7)?;
        let text = split((rest - rest / 7) / 2)?;
        let syntax = split(rest - rest / 7 - (rest - rest / 7) / 2)?;
        let admission = FrontInstanceBootstrapAdmission {
            jobs: config.jobs,
            weight: plan.body,
            max_reads: lower_reads(&definitions)?,
            runtime,
            front,
            lower,
            text,
            syntax,
            discovery,
            metadata: reserved,
        };
        let errors = runtime_errors(store.as_ref())?;
        let bootstrap = match program.engine.bootstrap_front_instances(
            &definitions,
            &definitions,
            &generators,
            admission,
            errors,
        ) {
            Ok(bootstrap) => bootstrap,
            Err(error) => return Err(loading.failed(instance_bootstrap_error(&program, error))),
        };
        // Nested pools return every released subcharge to this original
        // partition. Generated syntax, accepted text, source owners and the
        // returned inventory keep exactly their live charges, including pool
        // metadata, so the final graph can reuse only the returned bytes.
        let snapshot_bytes = snapshot.available_bytes();
        drop(snapshot);
        if let Err(error) = budget.readmit(&mut loading, lent.0, snapshot_bytes) {
            return Err(loading.failed(error));
        }
        final_limits = RequestLimits {
            snapshot_bytes,
            ..limits
        };
        definitions = bootstrap.definitions().to_vec();
        // Bootstrap publishes ordinary rows before accepted generators. The
        // final inventory uses the host's canonical DefPath order.
        definitions.sort();
        Some(bootstrap)
    };
    let registrations = bootstrap.as_ref().map_or(&[][..], |b| b.registrations());
    // Registration changes the source namespace. Capture every final consumer
    // against the same accepted views, including Run and ownership catalogues.
    if bootstrap.is_some() {
        footprint = match program.engine.front_input_footprint(&definitions) {
            Ok(footprint) => footprint,
            Err(error) => return Err(loading.failed(program.error(error))),
        };
        if needs_run {
            run_inputs = Some(
                match program.engine.front_run_inputs(final_limits.snapshot_bytes) {
                    Ok(inputs) => inputs,
                    Err(error) => return Err(loading.failed(program.error(error))),
                },
            );
        }
        if matches!(request.operation, CompileOperation::NativeOwnership(_)) {
            native_catalogue = Some(
                match program
                    .engine
                    .front_native_definition_inputs_with_registrations(
                        &definitions,
                        registrations,
                        final_limits.snapshot_bytes,
                    ) {
                    Ok(inputs) => inputs,
                    Err(error) => return Err(loading.failed(program.error(error))),
                },
            );
        }
    }
    let current_definitions = selected_definitions(&mut program.engine);
    if current_definitions != definitions {
        let mismatch = definitions
            .iter()
            .zip(&current_definitions)
            .find(|(a, b)| a != b);
        return Err(loading.failed(ToolError::Runtime(format!(
            "compiler source inventory changed after bootstrap: prepared {}, current {}, first mismatch {mismatch:?}",
            definitions.len(), current_definitions.len()
        ))));
    }
    plan = plan_within(
        final_limits,
        &program,
        &definitions,
        footprint,
        run_inputs.as_ref().map_or(0, |inputs| inputs.input_bytes()),
        native_catalogue
            .as_ref()
            .map_or(0, |inputs| inputs.input_bytes()),
    )?;
    let sources = program.engine.sources.clone();
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
    let mut check = check;
    let mut source = source;
    let source_index_allowance = source
        .split(plan.source_index_bytes)
        .ok_or_else(|| uninstalled())?;
    let source = Arc::new(source);
    let query_error = |error| ToolError::Query {
        error,
        phase: None,
        sources: sources.clone().retain_owner(source.clone()),
    };
    drop(transport);
    let errors = runtime_errors(store.as_ref())?;
    let runtime = program
        .engine
        .prepare_private_inference_with_registrations(runtime, registrations)
        .map_err(&query_error)?;
    let front = program
        .engine
        .front_body_queries_with_registrations(
            &definitions,
            registrations,
            plan.body,
            front,
            errors,
        )
        .map_err(&query_error)?;
    // Before any task: this fresh graph adopts the selector's earlier accepted
    // front facts only where their text and reads still validate.
    if let Some(carry) = carry {
        front
            .install_carry(carry.front.clone())
            .map_err(|_| uninstalled())?;
    }
    let lower = program
        .engine
        .front_lower_queries(
            &definitions,
            front.clone(),
            plan.body,
            lower_reads(&definitions)?,
            lower,
            errors,
        )
        .map_err(&query_error)?;
    if let Some(carry) = carry {
        lower
            .install_carry(carry.lower.clone())
            .map_err(|_| uninstalled())?;
    }
    let growth = program
        .engine
        .front_growth_queries(&program.entry, front.clone(), plan.root, growth, errors)
        .map_err(&query_error)?;
    // The receipt's headers share the Check partition; their constructor
    // refuses metadata beyond this split.
    let receipt = if matches!(request.operation, CompileOperation::CheckedProducts { .. }) {
        let headers = program
            .engine
            .front_checked_signatures_bytes(&front)
            .map_err(&query_error)?;
        let headers = check.split(headers).ok_or_else(|| uninstalled())?;
        let headers = program
            .engine
            .front_checked_signatures(front.clone(), headers)
            .map_err(&query_error)?;
        // The receipt has no Run, so its stages draw from Run's partition.
        let allowance = run.split(run.bytes()).ok_or_else(|| uninstalled())?;
        Some(Arc::new(CheckedReceipt {
            headers,
            allowance: Mutex::new(Some(allowance)),
            products: std::sync::OnceLock::new(),
            digest_view: std::sync::OnceLock::new(),
        }))
    } else {
        None
    };
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
                lint_package: Some(root_package.clone()),
            },
            plan.root,
            check,
            errors,
        )
        .map_err(&query_error)?;
    let safe_allowance = if matches!(
        request.operation,
        CompileOperation::SafeCheck
            | CompileOperation::Artifacts { .. }
            | CompileOperation::ExportTir
    ) {
        Some(
            run.split(super::safe_check::BYTES)
                .ok_or_else(|| uninstalled())?,
        )
    } else {
        None
    };
    // PS9 headers, source metadata and the encoded text share the Run
    // partition; their constructors and the encoder refuse bytes beyond these
    // splits. Building the Export from its input is not yet admitted.
    let export_allowance = if artifacts {
        let headers = program
            .engine
            .front_checked_signatures_bytes(&front)
            .map_err(&query_error)?;
        let headers = run.split(headers).ok_or_else(|| uninstalled())?;
        let export = run.split(run.bytes() / 4).ok_or_else(|| uninstalled())?;
        let output = run.split(run.bytes() / 4).ok_or_else(|| uninstalled())?;
        Some((headers, export, output))
    } else {
        None
    };
    let native_allowance = plan
        .native
        .as_ref()
        .map(|native| {
            run.split(native.bytes().ok_or_else(|| uninstalled())?)
                .ok_or_else(|| uninstalled())
        })
        .transpose()?;
    let mut ownership_allowance = plan
        .ownership
        .as_ref()
        .map(|ownership| {
            run.split(ownership.bytes().ok_or_else(|| uninstalled())?)
                .ok_or_else(|| uninstalled())
        })
        .transpose()?;
    let mut run = if needs_run {
        Some(
            program
                .engine
                .front_run_queries_with_inputs(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    run_inputs.take().ok_or_else(|| uninstalled())?,
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
    // The export selects its own unshared Run view of the entry package; that
    // view replaces the ordinary Run for this graph.
    let mut export_products = None;
    if let Some((headers, allowance, output)) = export_allowance {
        let headers = program
            .engine
            .front_checked_signatures(front.clone(), headers)
            .map_err(&query_error)?;
        let entry = program.entry.clone();
        let export = program
            .engine
            .front_proof_export(
                &entry,
                run.take().ok_or_else(|| uninstalled())?,
                headers,
                allowance,
            )
            .map_err(&query_error)?;
        run = Some(export.run().clone());
        export_products = Some(Arc::new(ArtifactsExport {
            capture: export,
            output,
        }));
    }
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
    let native = if let Some(footprint) = &plan.native {
        let (native_request, purpose) = match &request.operation {
            CompileOperation::NativeBuild(request) => {
                (request, super::native_platform::NativePurpose::Build)
            }
            CompileOperation::NativeRun(request) => {
                (request, super::native_platform::NativePurpose::Run)
            }
            _ => return Err(uninstalled()),
        };
        let entry = native_request
            .definition
            .as_ref()
            .or(launch_entry.as_ref())
            .cloned()
            .ok_or_else(|| uninstalled())?;
        Some(
            super::native_selection::NativeSelection::new(
                &mut program,
                super::native_assembly::NativeSourceInputs {
                    definitions: definitions.clone(),
                    entry,
                    front: front.clone(),
                    lower: lower.clone(),
                    run: run.take().ok_or_else(|| uninstalled())?,
                    store: store.clone(),
                    registrations: bootstrap
                        .as_ref()
                        .map(|bootstrap| bootstrap.registrations().to_vec())
                        .unwrap_or_default(),
                },
                native_request.clone(),
                purpose,
                config.verify_store,
                selected_toolchain.ok_or_else(|| {
                    ToolError::Runtime("native request requires its selected toolchain".into())
                })?,
                footprint,
                &mut native_allowance.ok_or_else(|| uninstalled())?,
                plan.body,
                plan.native_root,
                errors,
                disk_bytes,
            )
            .map_err(|error| super::host_selector::retain_error(error, source.clone()))?,
        )
    } else {
        None
    };
    let ownership = if let CompileOperation::NativeOwnership(selected) = &request.operation {
        let entry = selected
            .definition
            .as_ref()
            .or(launch_entry.as_ref())
            .cloned()
            .ok_or_else(|| uninstalled())?;
        Some(
            super::ownership_selection::OwnershipSelection::new(
                &mut program,
                super::ownership_admission::SourceInputs {
                    definitions: definitions.clone(),
                    catalogue: native_catalogue.take().ok_or_else(|| uninstalled())?,
                    entry,
                    front: front.clone(),
                    lower: lower.clone(),
                    run: run.take().ok_or_else(|| uninstalled())?,
                    store: store.clone(),
                },
                selected,
                config.verify_store,
                plan.ownership.as_ref().ok_or_else(|| uninstalled())?,
                &mut ownership_allowance.take().ok_or_else(|| uninstalled())?,
                plan.body,
                errors,
            )
            .map_err(|error| super::host_selector::retain_error(error, source.clone()))?,
        )
    } else {
        None
    };
    // The ordinary source namespace is immutable before the first request.
    // Preserve the original small-graph subscription/factory headroom while
    // admitting every actual P/Lower/Evaluate/Run address of larger packages.
    let mut inventory = std::collections::BTreeSet::new();
    for path in &definitions {
        for query in [
            front.query(path),
            front.body_query(path),
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
    if let Some(native) = &native {
        for query in native.assembly.queries(&definitions) {
            inventory.insert((query.kind, query.address.0));
        }
    }
    if let Some(ownership) = &ownership {
        for query in ownership.queries(&definitions) {
            inventory.insert((query.kind, query.address.0));
        }
    }
    if inventory.len() > plan.query_capacity {
        return Err(ToolError::Runtime(
            "compiler query inventory exceeds its planned admission".into(),
        ));
    }
    let mut runtime =
        AdmittedRuntime::with_query_capacity(config.jobs, runtime, errors, plan.query_capacity)
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
    if let Some(native) = &native {
        for (kind, factory) in native.assembly.factories() {
            runtime.register(kind, factory).map_err(registration)?;
        }
    } else if let Some(ownership) = &ownership {
        for (kind, factory) in ownership.factories() {
            runtime.register(kind, factory).map_err(registration)?;
        }
    } else if let Some(run) = &run {
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
        memo_operations: program.engine.memo_stats().touched,
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
    let touched_memos = program.engine.take_touched_memos();
    let initial = RequestWork {
        touched_memos,
        ..initial
    };
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
    // Move only source/header state. Scalar bodies, S1 and every controller
    // are dropped before the immutable graph becomes visible.
    if plan.source_index_bytes != 0 {
        let state = program
            .engine
            .into_source_index(source_index_allowance)
            .ok();
        *source_index.lock().unwrap_or_else(|e| e.into_inner()) = state;
    }
    let products = Arc::new(SourceProducts {
        entry: program.entry,
        inputs: request.inputs.clone(),
        lowering,
        launch_entry,
        run,
        native,
        ownership,
        safe,
        receipt,
        export: export_products,
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
/// The read bound of one Lower body, shared by bootstrap and the final graph.
fn lower_reads(definitions: &[DefPath]) -> Result<usize, ToolError> {
    Ok(definitions
        .len()
        .checked_mul(3)
        .ok_or_else(|| uninstalled())?
        .min(4096))
}
fn instance_bootstrap_error(
    program: &LoadedProgram,
    error: FrontInstanceBootstrapError,
) -> ToolError {
    match error {
        FrontInstanceBootstrapError::Bootstrap(error) => bootstrap_error(program, error),
        FrontInstanceBootstrapError::Front {
            error,
            sources,
            owner,
            ..
        } => ToolError::Query {
            error: error.unwrap_or_else(|| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "instance bootstrap".into(),
                    message: "a front task failed without a diagnostic".into(),
                })
            }),
            phase: None,
            sources: sources.retain_owner(owner),
        },
        FrontInstanceBootstrapError::Discovery {
            error,
            sources,
            owner,
        } => ToolError::Query {
            error,
            phase: None,
            sources: sources.retain_owner(owner),
        },
    }
}
fn bootstrap_error(program: &LoadedProgram, error: FrontRegistrationBootstrapError) -> ToolError {
    match error {
        FrontRegistrationBootstrapError::Admission(error) => program.error(error),
        FrontRegistrationBootstrapError::Runtime {
            error,
            sources,
            owner,
            ..
        } => ToolError::Query {
            error: error.unwrap_or_else(|| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "generator bootstrap".into(),
                    message: "a generator task failed without a diagnostic".into(),
                })
            }),
            phase: None,
            sources: sources.retain_owner(owner),
        },
        FrontRegistrationBootstrapError::Acceptance(failure) => ToolError::Query {
            error: failure.error,
            phase: None,
            sources: failure.sources,
        },
    }
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

/// Every indexed definition and required source root, after registering the
/// closed `with` specializations they name (GEN-6), which become roots too.
fn selected_definitions(engine: &mut hirsel_engine::QueryEngine) -> Vec<DefPath> {
    let indexed = engine.indexed_definitions();
    engine.register_specializations(&indexed);
    let required = engine.front_source_roots();
    engine.register_specializations(&required);
    indexed
        .into_iter()
        .chain(engine.front_source_roots())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
