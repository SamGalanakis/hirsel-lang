//! Standalone executable compilation; launch authority is supplied by its caller at runtime.
use super::{NativeError, OptLevel, Options, StageName, TargetSpec};
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
    pub(crate) fn options(self, target: TargetSpec) -> Options {
        let mut options = Options::new(target);
        options.scoped_specialization = matches!(self, Self::Release | Self::ReleaseLto);
        options
    }
    pub(crate) fn opt(self) -> OptLevel {
        match self {
            Self::Default | Self::Release | Self::ReleaseLto => OptLevel::Default,
            Self::Debug => OptLevel::None,
        }
    }
}

/// One build owns its directory from exclusive creation until drop.
pub(crate) struct Scratch(pub(crate) PathBuf, bool);
impl Scratch {
    pub(crate) fn new() -> Result<Self, NativeError> {
        Self::create_in(
            &std::env::temp_dir(),
            std::env::var_os("HIRSEL_KEEP_NATIVE_SCRATCH").is_some(),
        )
    }

    fn create_in(parent: &Path, keep: bool) -> Result<Self, NativeError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self::create_with_counter(parent, keep, &NEXT)
    }

    fn create_with_counter(
        parent: &Path,
        keep: bool,
        next: &AtomicU64,
    ) -> Result<Self, NativeError> {
        use std::os::unix::fs::DirBuilderExt;
        loop {
            let path = parent.join(format!(
                "hirsel-build-{}-{}",
                std::process::id(),
                next.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    if keep {
                        eprintln!("hirsel: retaining native scratch at {}", path.display());
                    }
                    return Ok(Self(path, keep));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io(error)),
            }
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.1 {
            let _ = std::fs::remove_dir_all(&self.0);
        }
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

/// `hirsel export --c-header` (or `--native-abi`, the JSON descriptor): the program's
/// published native data ABI for `target` (MEM-7, PROG-4).
pub fn export_data_abi(
    source: &Path,
    target: TargetSpec,
    header: bool,
) -> Result<Result<String, NativeError>, ToolError> {
    let compiler = crate::COMPILER;
    let mut requests =
        crate::requests::CompilerRequests::for_host(compiler.clone(), compiler.configuration());
    let output = requests.execute(source, crate::requests::CompileOperation::DataAbi)?;
    let crate::requests::CompileResult::NativeSources(facts) = &output.result else {
        return Err(ToolError::Runtime(
            "data ABI request returned another operation".into(),
        ));
    };
    let declarations =
        match super::queries::declarations::NativeDeclarations::from_front(facts.clone()) {
            Ok(declarations) => declarations,
            Err(error) => {
                return Ok(Err(NativeError::new(
                    StageName::Manifest,
                    format!("native source declarations: {error:?}"),
                )));
            }
        };
    Ok(super::c_abi::data_abi(
        &declarations.program.types,
        &declarations.program.signatures,
        declarations
            .sources
            .iter()
            .map(|(path, source)| (path, source.representation.as_ref())),
        &target,
    )
    .map(|abi| {
        if header {
            abi.header()
        } else {
            abi.descriptor()
        }
    }))
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
    let selected = crate::session::request(
        source,
        verify,
        crate::requests::CompileOperation::NativeBuild(crate::requests::NativeRequest {
            backend,
            profile,
            target,
            validation,
            launch: super::LaunchInputs::default(),
            run: None,
            definition: None,
            test_entries: vec![],
            supplemental_sources: vec![],
            output: crate::requests::NativeOutput::Executable,
            optimization: None,
            fuel,
            evidence: evidence.cloned().map(std::sync::Arc::new),
        }),
    )?;
    match &selected.result {
        crate::requests::CompileResult::Native(input) => {
            Ok(super::request_output::materialize_output(input, output))
        }
        crate::requests::CompileResult::NativeError(error) => Ok(Err(error.clone())),
        _ => Err(ToolError::Runtime(
            "native build request returned another operation".into(),
        )),
    }
}

#[cfg(test)]
mod scratch_tests {
    use super::*;

    #[test]
    fn exclusive_creation_skips_existing_names_and_drop_cleans_up() {
        let parent = Scratch::new().unwrap();
        let next_id = AtomicU64::new(0);
        let first = Scratch::create_with_counter(&parent.0, true, &next_id).unwrap();
        let occupied = first.0.clone();
        std::fs::write(occupied.join("sentinel"), b"another build").unwrap();
        drop(first);
        next_id.store(0, Ordering::Relaxed);
        let next = Scratch::create_with_counter(&parent.0, false, &next_id).unwrap();
        let path = next.0.clone();
        assert_ne!(occupied, path);
        drop(next);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(occupied.join("sentinel")).unwrap(),
            b"another build"
        );
    }
}

#[cfg(test)]
mod scoped_consumer_9_tests {
    use super::*;
    use hirsel_engine::DefinitionQueries;
    #[test]
    fn profile_switch_keeps_debug_heap_and_release_zero_environment() {
        hirsel_engine::with_compiler_stack(|| {
            let source = "fn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\npub fn main()->Result[(),u64]{let n=3u64;Error(invoke(fn()->u64 uses {}{n}))}";
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                crate::compiler_adapters(),
            );
            let file = engine
                .sources
                .add("scoped-profile.hir", source.into())
                .unwrap();
            let checked =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                    .unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let raw = engine.export_native_input().unwrap();
            let input = engine.import_native_input(raw).unwrap();
            let entry = hirsel_engine::entry_path(&hirsel_engine::default_context());
            let sites = super::super::Sites::new(input.snapshot());
            for profile in [
                BuildProfile::Debug,
                BuildProfile::Default,
                BuildProfile::Release,
                BuildProfile::ReleaseLto,
            ] {
                let options = profile.options(TargetSpec::x86_64_unknown_linux_gnu());
                let release = matches!(profile, BuildProfile::Release | BuildProfile::ReleaseLto);
                let ready = super::super::build(&input, &options).unwrap();
                let run = super::super::run_heap(&ready, &entry, &sites).unwrap();
                assert!(run.observation.failures.is_empty());
                assert_eq!(run.observation.exit_code, Some(1));
                assert_eq!(run.counters.allocations, u64::from(!release), "{profile:?}");
                assert_eq!(run.counters.frees, u64::from(!release), "{profile:?}");
                assert_eq!(run.counters.releases, u64::from(!release), "{profile:?}");
            }
        });
    }
}
