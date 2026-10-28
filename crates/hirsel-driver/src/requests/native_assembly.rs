//! Capture native source factories at S0, then install one complete dispatch
//! namespace before the first request. Source bodies, discovery, C and emission
//! remain producer work; this constructor retains no mutable compiler engine.
use super::{
    NativeRequest,
    native_bootstrap::{NativeAdmission, NativeBootstrap, NativePlan, NativeProofs},
    native_link::FrozenPlatform,
    native_products::NativeBuildReadiness,
    native_routing::NativeRouting,
};
use crate::native::{self, object_queries, queries::*};
use hirsel_engine::{
    FrontBodyQueries, FrontLowerQueries, FrontNativePlan, FrontNativeQueries, FrontRunQueries,
    LoadedProgram, ToolError,
    runtime::{MemoryReservation, MemoryWeight, QueryFactory, QueryId, RuntimeErrors},
    store::{Reference, ResultStore},
};
use hirsel_syntax::DefPath;
use std::sync::Arc;

/// All source handles come from one pinned loader view. Native source capture
/// checks their signature/context ownership before any factory becomes visible.
pub(super) struct NativeSourceInputs {
    pub definitions: Vec<DefPath>,
    pub entry: DefPath,
    pub front: Arc<FrontBodyQueries>,
    pub lower: Arc<FrontLowerQueries>,
    pub run: Arc<FrontRunQueries>,
    pub store: Arc<dyn ResultStore>,
    /// Accepted views of the selected generated declarations.
    pub registrations: Vec<Arc<hirsel_engine::AcceptedFrontRegistration>>,
}
/// Selected evidence and its partitions of the native snapshot allowance.
pub(super) struct NativeProofPlan {
    pub evidence: Arc<native::Evidence>,
    pub headers: MemoryReservation,
    pub export: MemoryReservation,
}
/// These partitions transfer the caller's existing snapshot reservation.
/// This module cannot reserve another process cap or enlarge a task frame.
pub(super) struct NativeAssemblyPlan {
    pub native: NativePlan,
    pub output: super::NativeOutput,
    pub proofs: Option<NativeProofPlan>,
    pub tasks: NativeAdmission,
    pub catalogue: MemoryReservation,
    pub source: MemoryReservation,
    pub routing: MemoryReservation,
    pub bootstrap: MemoryReservation,
    pub weight: MemoryWeight,
    pub source_weight: MemoryWeight,
    /// The native source root suspends beside Growth/Check/Run as a planned
    /// root continuation while it accumulates selected catalogue records.
    pub root_weight: MemoryWeight,
    pub errors: RuntimeErrors,
}

pub(super) const KINDS: &[u32] = &[
    INSTANCE_KIND,
    crate::native::queries::instance::CALLABLE_VALUE_KIND,
    instance_tasks::CALLABLE_GRAPH_KIND,
    instance_tasks::INSTANCE_INPUT_KIND,
    instance_tasks::DISCOVERY_KIND,
    ARENA_CANDIDATES_KIND,
    ARENA_CHOSEN_KIND,
    arena_tasks::ARENA_GRAPH_KIND,
    compilation_tasks::COMPILATION_KIND,
    crate::native::queries::seal_demand::SEAL_DEMAND_KIND,
    READY_KIND,
    ROOT_KIND,
    object_queries::CODEGEN_KIND,
    object_queries::LINK_KIND,
    link_tasks::LINK_INPUT_KIND,
    link_tasks::LINK_MANIFEST_KIND,
    crate::native::queries::execution::EXECUTION_KIND,
    crate::native::queries::execution::EXECUTION_OBJECTS_KIND,
];

pub(super) struct NativeAssembly {
    source: Arc<FrontNativeQueries>,
    bootstrap: Arc<NativeBootstrap>,
    routing: Arc<NativeRouting>,
    sources: hirsel_span::SourceMap,
}
impl NativeAssembly {
    pub fn new(
        program: &mut LoadedProgram,
        inputs: NativeSourceInputs,
        plan: NativeAssemblyPlan,
    ) -> Result<Arc<Self>, ToolError> {
        let sources = program.engine.sources.clone();
        let source_error = |error| ToolError::Query {
            error,
            phase: None,
            sources: sources.clone(),
        };
        let entry = inputs.entry.clone();
        let request_output = plan.output.clone();
        let catalogue = program
            .engine
            .front_native_definition_inputs_with_registrations(
                &inputs.definitions,
                &inputs.registrations,
                plan.catalogue.bytes(),
            )
            .and_then(|catalogue| {
                program.engine.front_native_definition_queries_with_inputs(
                    inputs.front.clone(),
                    inputs.lower.clone(),
                    catalogue,
                    plan.source_weight,
                    plan.catalogue,
                    plan.errors,
                )
            })
            .map_err(source_error)?;
        let source = program
            .engine
            .front_native_queries(
                FrontNativePlan {
                    entry: inputs.entry,
                    roots: if let super::NativeOutput::Objects { roots } = &request_output {
                        roots.clone()
                    } else {
                        plan.native
                            .test_entries
                            .iter()
                            .map(|(path, _)| path.clone())
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .collect()
                    },
                    run: inputs.run,
                    definitions: catalogue,
                },
                plan.root_weight,
                plan.source,
                plan.errors,
            )
            .map_err(|error| program.error(error))?;
        // PS9 attaches to this exact native Run before any request, so the
        // assessment reads the same selected roots that native compiles.
        let proofs = plan
            .proofs
            .map(|proofs| {
                let headers = program
                    .engine
                    .front_checked_signatures(inputs.front, proofs.headers)?;
                let export = program.engine.front_proof_export_from_native(
                    &entry,
                    &source,
                    headers,
                    proofs.export,
                )?;
                Ok(NativeProofs {
                    evidence: proofs.evidence,
                    export,
                })
            })
            .transpose()
            .map_err(source_error)?;
        let routing = NativeRouting::new(KINDS, plan.errors.protocol, plan.routing)
            .map_err(|message| ToolError::Runtime(message.into()))?;
        let bootstrap = NativeBootstrap::new(
            source.clone(),
            inputs.lower,
            inputs.definitions,
            routing.clone(),
            inputs.store,
            plan.native,
            plan.tasks,
            proofs,
            plan.weight,
            plan.bootstrap,
            plan.errors,
        )
        .map_err(ToolError::Runtime)?;
        Ok(Arc::new(Self {
            source,
            bootstrap,
            routing,
            sources,
        }))
    }
    pub fn source(&self) -> &Arc<FrontNativeQueries> {
        &self.source
    }
    pub fn bootstrap(&self) -> &Arc<NativeBootstrap> {
        &self.bootstrap
    }
    pub fn query(&self) -> QueryId {
        self.bootstrap.query()
    }
    /// Exact IDs after native source selection has rebound Run and catalogue
    /// namespaces. Counting them performs no query or source-body read.
    pub fn queries(&self, definitions: &[DefPath]) -> Vec<QueryId> {
        let mut queries = vec![self.query(), self.source.query()];
        queries.extend(self.source.run_query_inventory());
        for path in definitions {
            queries.extend(self.source.definition_query(path));
        }
        queries
    }
    /// Return the source-specific Run factory, not a separately captured Run
    /// namespace. The host installs this set exactly once before requests.
    pub fn factories(&self) -> Vec<(u32, Arc<dyn QueryFactory>)> {
        let mut factories = self.routing.factories();
        factories.extend([
            (hirsel_engine::FRONT_RUN_KIND, self.source.run_factory()),
            (
                hirsel_engine::FRONT_NATIVE_DEFINITION_KIND,
                self.source.definition_factory(),
            ),
            (hirsel_engine::FRONT_NATIVE_ROOT_KIND, self.source.factory()),
            (super::native_bootstrap::KIND, self.bootstrap.factory()),
        ]);
        factories
    }
    pub fn readiness(
        &self,
        request: NativeRequest,
        tools: Arc<native::Toolchain>,
        platform: Option<Arc<FrozenPlatform>>,
    ) -> Result<NativeBuildReadiness, native::NativeError> {
        NativeBuildReadiness::new(self.bootstrap.clone(), request, tools, platform)
    }
    pub fn failure(&self, reference: Reference) -> Option<ToolError> {
        if let Some(error) = self.source.failure(reference) {
            return Some(ToolError::Query {
                error,
                phase: None,
                sources: self.sources.clone(),
            });
        }
        self.bootstrap.failure(reference).map(ToolError::Runtime)
    }
}
