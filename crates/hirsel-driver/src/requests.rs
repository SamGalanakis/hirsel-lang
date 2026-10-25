//! Explicit immutable request selection shared by fresh and resident callers.
//! The engine installs factories; these descriptors select operations and never
//! accept caller-supplied tasks, caches, verifier receipts or mutable programs.
use crate::native::{Backend, BuildProfile, LaunchInputs, TargetSpec, Validation};
use hirsel_engine::{LoweringPolicy, loader_inputs::FrozenInputs};
use hirsel_syntax::DefPath;
use std::path::PathBuf;
mod host_selector;
mod inspection;
mod native_routing;
mod output;
mod safe_check;
mod selector;
mod source_graph;
mod tir_products;
pub use output::{
    CompileInspection, CompileOutput, CompileResult, CompilerReadiness, DefinitionWork,
    NativeExecution, NativeObservation, RequestWork, TirExecution, TirProducts,
};
pub use selector::{CompilerRequests, RequestLimits};

/// Already observed inputs, including missing files, manifests, aliases and
/// directory membership. Selection must use these bytes verbatim; observing
/// newer disk state is a separate request. Compiler policy is captured by the
/// request engine's Compiler, including explicit jobs, V and selected stores.
#[derive(Clone)]
pub struct CompileRequest {
    pub path: PathBuf,
    pub inputs: FrozenInputs,
    pub operation: CompileOperation,
}
#[derive(Clone)]
pub enum CompileOperation {
    /// Pure selected-graph metadata; this operation never demands a producer.
    Inspect {
        definitions: Vec<DefPath>,
    },
    /// Explicit canonical oracle exports from the same compiler graph.
    Artifacts {
        native: bool,
    },
    Check {
        lowering: LoweringPolicy,
    },
    SafeCheck,
    /// Explicit evidence collection; normal Check does not construct receipts.
    CheckedProducts {
        lowering: LoweringPolicy,
        canonical: bool,
    },
    /// Observe work counters without demanding or recomputing these definitions.
    RecomputeCounts {
        queries: Vec<(DefPath, hirsel_engine::QueryKind)>,
    },
    /// None selects the declared launch entry using the observed manifest.
    Tir {
        definition: Option<DefPath>,
    },
    NativeBuild(NativeRequest),
    NativeRun(NativeRequest),
}
/// Backend and launch policy, independent of destination paths or host output
/// writers. Build and run select the same compilation products; execution and
/// executable materialization follow their completed readiness descriptor.
#[derive(Clone)]
pub struct NativeRequest {
    /// Selected launch definition; None uses the observed package entry.
    pub definition: Option<DefPath>,
    pub backend: Backend,
    pub profile: BuildProfile,
    pub target: TargetSpec,
    pub validation: Validation,
    pub launch: LaunchInputs,
    /// The independently validated run bound against the selected source facts.
    pub run: Option<std::sync::Arc<hirsel_run_config::ValidatedRun>>,
    /// Build-time fuel instrumentation is part of native/emission identity.
    pub fuel: bool,
    /// Independently assessed source evidence; its selected discharge policy
    /// participates in native actions, never a launch-time fallback.
    pub evidence: Option<std::sync::Arc<crate::native::Evidence>>,
}
