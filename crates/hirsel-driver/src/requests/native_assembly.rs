//! Capture native source factories at S0, then install one complete dispatch
//! namespace before the first request. Source bodies, discovery, C and emission
//! remain producer work; this constructor retains no mutable compiler engine.
use super::{
    NativeRequest,
    native_bootstrap::{NativeAdmission, NativeBootstrap, NativePlan},
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
}
/// These partitions transfer the caller's existing snapshot reservation.
/// This module cannot reserve another process cap or enlarge a task frame.
pub(super) struct NativeAssemblyPlan {
    pub native: NativePlan,
    pub tasks: NativeAdmission,
    pub catalogue: MemoryReservation,
    pub source: MemoryReservation,
    pub routing: MemoryReservation,
    pub bootstrap: MemoryReservation,
    pub weight: MemoryWeight,
    pub source_weight: MemoryWeight,
    pub errors: RuntimeErrors,
}

const KINDS: &[u32] = &[
    INSTANCE_KIND,
    instance_tasks::INSTANCE_INPUT_KIND,
    instance_tasks::DISCOVERY_KIND,
    ARENA_CANDIDATES_KIND,
    ARENA_CHOSEN_KIND,
    arena_tasks::ARENA_GRAPH_KIND,
    compilation_tasks::COMPILATION_KIND,
    READY_KIND,
    ROOT_KIND,
    object_queries::CODEGEN_KIND,
    object_queries::LINK_KIND,
    link_tasks::LINK_INPUT_KIND,
    link_tasks::LINK_MANIFEST_KIND,
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
        let catalogue = program
            .engine
            .front_native_definition_queries(
                &inputs.definitions,
                inputs.front,
                inputs.lower.clone(),
                plan.source_weight,
                plan.catalogue,
                plan.errors,
            )
            .map_err(source_error)?;
        let source = program
            .engine
            .front_native_queries(
                FrontNativePlan {
                    entry: inputs.entry,
                    roots: vec![],
                    run: inputs.run,
                    definitions: catalogue,
                },
                plan.source_weight,
                plan.source,
                plan.errors,
            )
            .map_err(|error| program.error(error))?;
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
    pub fn query(&self) -> QueryId {
        self.bootstrap.query()
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
        platform: Arc<FrozenPlatform>,
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
