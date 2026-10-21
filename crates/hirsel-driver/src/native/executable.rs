//! Standalone executable compilation; launch authority is supplied by its caller at runtime.
use super::{NativeError, OptLevel, Options, Partition, StageName, TargetSpec, Toolchain};
use hirsel_engine::ToolError;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The user-facing executable profiles; runtime archives always use production settings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BuildProfile {
    #[default]
    Default,
    Release,
    /// Explicit whole-module LLVM optimization (`--lto`).
    ReleaseLto,
    Debug,
}
impl BuildProfile {
    fn opt(self) -> OptLevel {
        match self {
            Self::Default | Self::Release | Self::ReleaseLto => OptLevel::Default,
            Self::Debug => OptLevel::None,
        }
    }
}

pub(crate) struct Scratch(pub(crate) PathBuf);
impl Scratch {
    pub(crate) fn new() -> Result<Self, NativeError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "hirsel-build-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io(error)),
            }
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn io(error: std::io::Error) -> NativeError {
    NativeError::new(StageName::Link, error.to_string())
}

/// Check a source or manifest, compile it and write a directly executable artifact.
/// Source/checker errors remain separate from native tool outcomes, as for native run.
/// With `evidence`, accepted proofs remove their checks (PS-13) and the result
/// is the proof report; without, it is empty.
pub fn build_path_native(
    source: &Path,
    output: &Path,
    target: TargetSpec,
    profile: BuildProfile,
    evidence: Option<&super::Evidence>,
) -> Result<Result<Vec<String>, NativeError>, ToolError> {
    build_path_native_backend(
        source,
        output,
        target,
        profile,
        super::Backend::Llvm,
        evidence,
        false,
        hirsel_engine::Verify::On,
    )
}

/// `build_path_native` with `backend`, which lowers the same Ready LIR.
/// Release builds require LLVM.
pub fn build_path_native_backend(
    source: &Path,
    output: &Path,
    target: TargetSpec,
    profile: BuildProfile,
    backend: super::Backend,
    evidence: Option<&super::Evidence>,
    fuel: bool,
    verify: hirsel_engine::Verify,
) -> Result<Result<Vec<String>, NativeError>, ToolError> {
    build_path_native_backend_with_validation(
        source,
        output,
        target,
        profile,
        backend,
        evidence,
        fuel,
        verify,
        super::Validation::Ready,
    )
}

/// Build with an explicit LIR validation policy. Harnesses use `EveryStage`.
pub fn build_path_native_backend_with_validation(
    source: &Path,
    output: &Path,
    target: TargetSpec,
    profile: BuildProfile,
    backend: super::Backend,
    evidence: Option<&super::Evidence>,
    fuel: bool,
    verify: hirsel_engine::Verify,
    validation: super::Validation,
) -> Result<Result<Vec<String>, NativeError>, ToolError> {
    if matches!(profile, BuildProfile::Release | BuildProfile::ReleaseLto)
        && backend == super::Backend::Cranelift
    {
        return Ok(Err(NativeError::new(
            StageName::Emission,
            "release builds use LLVM; select LLVM or a non-release profile",
        )));
    }
    crate::session::with_program(source, verify, |loaded| {
        loaded.check_for_execution()?;
        let engine = &mut loaded.engine;
        let input = engine
            .export_native_input()
            .map_err(|error| ToolError::Query {
                error,
                phase: engine.unavailable,
                sources: engine.sources.clone(),
            })?;
        let sources = (profile == BuildProfile::Debug).then(|| super::profile_sources(&input));
        let validated = match engine.import_native_input(input) {
            Ok(input) => input,
            Err(error) => {
                return Ok(Err(NativeError::new(
                    StageName::Manifest,
                    format!("the native snapshot: {error:?}"),
                )));
            }
        };
        Ok((|| {
            let mut options = Options::new(target.clone());
            options.validation = validation;
            let proofs = evidence
                .map(|e| super::Proofs::assess(engine, &loaded.entry, &validated, e))
                .transpose()?;
            if let Some(proofs) = &proofs {
                options.discharge = proofs.discharge.clone();
            }
            if profile != BuildProfile::ReleaseLto {
                options.objects = super::units::selected(&target, backend, profile.opt(), sources.is_some(), fuel)?;
            }
            let mut program = super::build(&validated, &options)?;
            // Fuel checks let the executable accept a fuel budget (`budgets.limits@1`).
            program.fuel = fuel;
            let scratch = Scratch::new()?;
            let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
                .map(PathBuf::from)
                .unwrap_or_else(super::runtime_cache::default_directory);
            let mut tools = Toolchain::in_tree(&cache, &[&target])?;
            tools.debug_info = profile == BuildProfile::Debug;
            let objects = super::emit_backend_objects(
                &program,
                backend,
                profile.opt(),
                Partition::Whole,
                false,
                sources.as_ref(),
            )?;
            let linked = super::link::link_standalone(
                &program,
                &loaded.entry,
                &objects,
                &tools,
                &scratch.0,
            )?;
            let timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Link);
            std::fs::copy(&linked.path, output).map_err(io)?;
            // The debug file keeps its build-id name beside the executable, where
            // its `.gnu_debuglink` leads a debugger.
            if let Some(debug) = &linked.debug {
                let beside = output.with_file_name(debug.file_name().unwrap());
                std::fs::copy(debug, beside).map_err(io)?;
            }
            drop(timing);
            Ok(proofs.map_or_else(Vec::new, |p| p.report(&program)))
        })())
    })
}
