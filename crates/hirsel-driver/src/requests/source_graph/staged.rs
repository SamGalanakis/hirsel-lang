//! Staged headers are admitted source products before any final Lower or V
//! universe is captured. Their P work runs on the same request executor.
use super::*;
use crate::requests::host_selector::LoadingAdmission;
use hirsel_engine::{
    AcceptedFrontRegistration, FRONT_BODY_KIND, FRONT_STAGE_KIND, FrontStageQueries,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn install(
    program: &mut LoadedProgram,
    definitions: &[DefPath],
    registrations: &[Arc<AcceptedFrontRegistration>],
    jobs: hirsel_intern::jobs::Jobs,
    weight: MemoryWeight,
    front_bytes: u64,
    loading: &mut LoadingAdmission,
    budget: &GraphBudget,
    limits: RequestLimits,
    store: &dyn ResultStore,
) -> Result<(bool, RequestLimits), ToolError> {
    let roots = program
        .engine
        .front_staged_roots(definitions)
        .map_err(|error| program.error(error))?;
    if roots.is_empty() {
        return Ok((false, limits));
    }
    let errors = runtime_errors(store)?;
    let (runtime, snapshot) = loading.lend();
    let snapshot = MemoryReservationPool::new(snapshot).map_err(|_| uninstalled())?;
    let runtime_bytes = runtime.bytes();
    let front = program
        .engine
        .front_body_queries_with_registrations(
            definitions,
            registrations,
            weight,
            snapshot.take(front_bytes).ok_or_else(|| uninstalled())?,
            errors,
        )
        .map_err(|error| program.error(error))?;
    // The producer and the installed source copy have independent grants;
    // unused producer bytes return before the final source graph is admitted.
    let stage = FrontStageQueries::new(
        &roots,
        front.clone(),
        weight,
        snapshot
            .take(snapshot.available_bytes() / 2)
            .ok_or_else(|| uninstalled())?,
        errors,
    )
    .map_err(|error| program.error(error))?;
    let mut executor = AdmittedRuntime::new(jobs, runtime, errors)
        .map_err(|error| ToolError::Runtime(error.to_string()))?;
    executor
        .register(FRONT_BODY_KIND, front.factory())
        .map_err(ToolError::Runtime)?;
    executor
        .register(FRONT_STAGE_KIND, stage.factory())
        .map_err(ToolError::Runtime)?;
    for root in roots {
        let query = stage.query(&root).ok_or_else(|| uninstalled())?;
        let request = executor.request(&[query]);
        let products = executor.wait(request).map_err(|reference| {
            stage
                .failure(reference)
                .map(|error| program.error(error))
                .unwrap_or_else(|| {
                    ToolError::Runtime(format!(
                        "staged declaration preparation refused: {reference:?}"
                    ))
                })
        })?;
        let [product] = products.as_slice() else {
            return Err(uninstalled());
        };
        let output = stage.outcome(*product).ok_or_else(|| uninstalled())?;
        let installed = output
            .installation_bytes()
            .map_err(|error| program.error(error))?;
        program
            .engine
            .accept_front_staged(
                &output,
                snapshot.take(installed).ok_or_else(|| uninstalled())?,
            )
            .map_err(|error| program.error(error))?;
    }
    let runtime = executor
        .shutdown_with_reservation()
        .ok_or_else(|| uninstalled())?;
    // Reclaim the producers' whole partitions, leaving only accepted source
    // copies retained by program.sources and every later immutable clone.
    drop(executor);
    drop(stage);
    drop(front);
    let returned = snapshot.available_bytes();
    drop(snapshot);
    drop(runtime);
    budget.readmit(loading, runtime_bytes, returned)?;
    Ok((
        true,
        RequestLimits {
            snapshot_bytes: returned,
            ..limits
        },
    ))
}
