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
    pub(crate) fn options(self, target: TargetSpec) -> Options {
        let mut options = Options::new(target);
        options.scoped_specialization = matches!(self, Self::Release | Self::ReleaseLto);
        options
    }
    fn opt(self) -> OptLevel {
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
        hirsel_intern::memory::checkpoint("front complete");
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
            let mut options = profile.options(target.clone());
            options.validation = validation;
            let proofs = evidence
                .map(|e| super::Proofs::assess(engine, &loaded.entry, &validated, e))
                .transpose()?;
            if let Some(proofs) = &proofs {
                options.discharge = proofs.discharge.clone();
            }
            if profile != BuildProfile::ReleaseLto {
                options.objects = super::units::selected(
                    &target,
                    backend,
                    profile.opt(),
                    sources.is_some(),
                    fuel,
                )?;
            }
            let mut program = super::build(&validated, &options)?;
            let facts = super::callee_metadata(engine, &loaded.entry)?;
            program.bind_callees(facts)?;
            hirsel_intern::memory::checkpoint("native workers complete");
            // Fuel checks let the executable accept a fuel budget (`budgets.limits@2`).
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
            hirsel_intern::memory::checkpoint("link complete");
            let timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Link);
            std::fs::copy(&linked.path, output).map_err(io)?;
            drop(timing);
            Ok(proofs.map_or_else(Vec::new, |p| p.report(&program)))
        })())
    })
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
