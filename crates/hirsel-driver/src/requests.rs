//! Explicit immutable request selection shared by fresh and resident callers.
//! The engine installs factories; these descriptors select operations and never
//! accept caller-supplied tasks, caches, verifier receipts or mutable programs.
use crate::native::{Backend, BuildProfile, LaunchInputs, TargetSpec, Validation};
use hirsel_engine::{LoweringPolicy, loader_inputs::FrozenInputs};
use hirsel_syntax::DefPath;
use std::path::PathBuf;

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
    Check {
        lowering: LoweringPolicy,
    },
    SafeCheck,
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
    pub backend: Backend,
    pub profile: BuildProfile,
    pub target: TargetSpec,
    pub validation: Validation,
    pub launch: LaunchInputs,
}
