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
    if profile == BuildProfile::Release && backend == super::Backend::Cranelift {
        return Ok(Err(NativeError::new(
            StageName::Emission,
            "release builds use LLVM; select LLVM or a non-release profile",
        )));
    }
    // An unchanged program's build copies its cached executable (`program_cache`).
    let cache = evidence
        .is_none()
        .then(super::program_cache::ProgramCache::open)
        .flatten();
    let program = cache.as_ref().and_then(|_| {
        let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::S0);
        crate::COMPILER
            .with_verify(verify)
            .program_identity(source)
            .ok()
            .flatten()
    });
    let toolchain = || {
        let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(super::runtime_cache::default_directory);
        let mut tools = Toolchain::in_tree(&cache, &[&target])?;
        tools.debug_info = profile == BuildProfile::Debug;
        Ok::<_, NativeError>(tools)
    };
    let entry = |tools: &Toolchain| -> Option<String> {
        let identity = super::program_cache::toolchain_identity(tools, &target.triple).ok()?;
        let key = super::program_cache::key(
            "hirsel.program-cache.build.v1",
            [
                program.as_ref()?.0.as_slice(),
                backend.name().as_bytes(),
                target.triple.as_bytes(),
                format!("{profile:?}").as_bytes(),
                &[u8::from(fuel)],
                &identity.0,
            ],
        );
        Some(format!("b-{key}"))
    };
    if let Some(cache) = &cache
        && program.is_some()
    {
        let copied = toolchain()
            .ok()
            .and_then(|tools| entry(&tools))
            .and_then(|entry| copy_cached(cache, &entry, output));
        hirsel_intern::timings::count(if copied.is_some() {
            hirsel_intern::timings::Count::ProgramHits
        } else {
            hirsel_intern::timings::Count::ProgramMisses
        });
        if copied.is_some() {
            return Ok(Ok(Vec::new()));
        }
    }
    let mut loaded = crate::COMPILER.with_verify(verify).load_program(source)?;
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
        let mut program = super::build(&validated, &options)?;
        // Fuel checks let the executable accept a fuel budget (`budgets.limits@2`).
        program.fuel = fuel;
        let scratch = Scratch::new()?;
        let tools = toolchain()?;
        let objects = super::emit_backend_objects(
            &program,
            backend,
            profile.opt(),
            Partition::Whole,
            false,
            sources.as_ref(),
        )?;
        let linked =
            super::link::link_standalone(&program, &loaded.entry, &objects, &tools, &scratch.0)?;
        let timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Link);
        std::fs::copy(&linked.path, output).map_err(io)?;
        // The debug file keeps its build-id name beside the executable, where
        // its `.gnu_debuglink` leads a debugger.
        if let Some(debug) = &linked.debug {
            let beside = output.with_file_name(debug.file_name().unwrap());
            std::fs::copy(debug, beside).map_err(io)?;
        }
        if let (Some(cache), Some(entry)) = (&cache, entry(&tools))
            && proofs.is_none()
        {
            store_cached(cache, &entry, &linked.path, linked.debug.as_deref());
        }
        drop(timing);
        Ok(proofs.map_or_else(Vec::new, |p| p.report(&program)))
    })())
}

/// Store a build's executable and debug file as `entry`: its debug file and the
/// debug file's name first, the executable last, so a present executable means a
/// complete entry.
fn store_cached(
    cache: &super::program_cache::ProgramCache,
    entry: &str,
    executable: &Path,
    debug: Option<&Path>,
) {
    let name = debug
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned());
    if let Some(debug) = debug {
        cache.put_file(&format!("{entry}.debug"), debug);
    }
    let record = serde_json::json!({"schema": 1, "debug": name});
    cache.put(
        &format!("{entry}.json"),
        &serde_json::to_vec(&record).unwrap(),
    );
    cache.put_file(&format!("{entry}.exe"), executable);
}

/// Copy the cached build `entry` to `output` (and its debug file beside it), or
/// `None` when the cache does not hold all of it.
fn copy_cached(
    cache: &super::program_cache::ProgramCache,
    entry: &str,
    output: &Path,
) -> Option<()> {
    let executable = cache.get(&format!("{entry}.exe"))?;
    let record: serde_json::Value =
        serde_json::from_slice(&cache.read(&format!("{entry}.json"))?).ok()?;
    if record["schema"] != 1 {
        return None;
    }
    let debug = match &record["debug"] {
        serde_json::Value::Null => None,
        name => {
            let name = name.as_str()?;
            if name.contains('/') {
                return None;
            }
            Some((cache.get(&format!("{entry}.debug"))?, name.to_owned()))
        }
    };
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Link);
    std::fs::copy(&executable, output).ok()?;
    if let Some((debug, name)) = debug {
        std::fs::copy(debug, output.with_file_name(name)).ok()?;
    }
    Some(())
}
