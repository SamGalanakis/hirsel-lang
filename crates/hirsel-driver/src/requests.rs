//! Explicit immutable request selection shared by fresh and resident callers.
//! The engine installs factories; these descriptors select operations and never
//! accept caller-supplied tasks, caches, verifier receipts or mutable programs.
use crate::native::{Backend, BuildProfile, LaunchInputs, TargetSpec, Validation};
use hirsel_engine::{LoweringPolicy, loader_inputs::FrozenInputs};
use hirsel_syntax::DefPath;
use std::path::PathBuf;
mod host_selector;
mod inspection;
mod native_artifacts;
mod native_assembly;
mod native_bootstrap;
mod native_link;
mod native_lower;
mod native_platform;
mod native_products;
mod native_routing;
mod native_selection;
mod native_sources;
mod native_store;
mod output;
mod ownership_admission;
mod ownership_bootstrap;
mod ownership_selection;
mod proof_export;
mod resident_store;
mod safe_check;
mod selector;
mod source_graph;
mod tir_products;
pub use output::{
    CompileInspection, CompileOutput, CompileResult, CompilerReadiness, DefinitionWork,
    NativeExecution, NativeObjectOutput, NativeObservation, RequestWork, TirBatchExecution,
    TirExecution, TirProducts,
};
pub use ownership_selection::{OwnershipExecution, OwnershipObjects};
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
    /// Populate complete source semantic, audit, Lower and constant products.
    Precompute,
    SafeCheck,
    /// Generated bodies in primary-source order after the accepted Check.
    Expand,
    /// LANG-6 hashes of a package and its complete source closure.
    Hashes {
        package: Option<hirsel_syntax::PackageId>,
    },
    /// Canonical TIR text from the accepted immutable Run closure.
    ExportTir,
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
    /// One checked source closure serving these independently bound entries.
    TirBatch {
        definitions: Vec<DefPath>,
    },
    NativeBuild(NativeRequest),
    NativeRun(NativeRequest),
    /// Explicit source-selected Value-to-Ready ownership diagnostics.
    NativeOwnership(OwnershipRequest),
    /// Checked C declaration exports before foreign object inputs exist.
    DataAbi,
    /// Checked source inventory and original Lower products, before native import.
    NativeSources {
        roots: Vec<DefPath>,
    },
}
/// Backend and launch policy, independent of destination paths or host output
/// writers. Build and run select the same compilation products; execution and
/// executable materialization follow their completed readiness descriptor.
#[derive(Clone)]
pub struct NativeRequest {
    /// Selected launch definition; None uses the observed package entry.
    pub definition: Option<DefPath>,
    /// Ordered test dispatcher entries and their seeds. Empty for an ordinary
    /// executable; binding occurs before native metadata admission.
    pub test_entries: Vec<(DefPath, u64)>,
    /// Additional source produced by the package test or surface compiler.
    /// These ordinary inputs are parsed and checked in the owning package.
    pub supplemental_sources: Vec<NativeSourceText>,
    pub output: NativeOutput,
    pub optimization: Option<crate::native::OptLevel>,
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

/// Immutable source input, never a body, task, or validation capability.
#[derive(Clone, serde::Serialize)]
pub struct NativeSourceText {
    pub name: String,
    pub context: hirsel_syntax::FileContext,
    pub text: std::sync::Arc<str>,
}

/// Diagnostic compiler policy; execution consumes only its checked products.
#[derive(Clone, Debug)]
pub struct OwnershipRequest {
    pub definition: Option<DefPath>,
    pub target: TargetSpec,
    pub validation: Validation,
    pub launch: LaunchInputs,
    pub fuel: bool,
}

/// The requested build artifact, independent of runtime launchability.
#[derive(Clone, serde::Serialize)]
pub enum NativeOutput {
    Executable,
    Objects { roots: Vec<DefPath> },
}
