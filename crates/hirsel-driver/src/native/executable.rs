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
    Debug,
}
impl BuildProfile {
    fn opt(self) -> OptLevel {
        match self {
            Self::Default => OptLevel::Default,
            Self::Release => OptLevel::Speed,
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
    )
}

/// `build_path_native` with `backend`, which lowers the same Ready LIR; Cranelift reads
/// the profile's optimization level as none or speed.
pub fn build_path_native_backend(
    source: &Path,
    output: &Path,
    target: TargetSpec,
    profile: BuildProfile,
    backend: super::Backend,
    evidence: Option<&super::Evidence>,
) -> Result<Result<Vec<String>, NativeError>, ToolError> {
    let mut loaded = crate::load_program(source)?;
    loaded.check_for_execution()?;
    let mut engine = loaded.engine;
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
        let proofs = evidence
            .map(|e| super::Proofs::assess(&mut engine, &loaded.entry, &validated, e))
            .transpose()?;
        if let Some(proofs) = &proofs {
            options.discharge = proofs.discharge.clone();
        }
        let program = super::build(&validated, &options)?;
        let scratch = Scratch::new()?;
        let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(super::runtime_cache::default_directory);
        let tools = Toolchain::in_tree(&cache, &[&target])?;
        let objects = super::emit_backend_objects(
            &program,
            backend,
            profile.opt(),
            Partition::Whole,
            false,
            sources.as_ref(),
        )?;
        let executable =
            super::link::link_standalone(&program, &loaded.entry, &objects, &tools, &scratch.0)?;
        std::fs::copy(executable, output).map_err(io)?;
        Ok(proofs.map_or_else(Vec::new, |p| p.report(&program)))
    })())
}
