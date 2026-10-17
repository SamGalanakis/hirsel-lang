//! The compiler: hirsel-engine's queries wired to the front phases it may not
//! depend on (signature, check, modes, lower) and to the native backends that
//! compile its validated native input. Callers import engine items from
//! hirsel-engine itself; this crate exports only the wiring.
use hirsel_engine::{
    CheckOutput, Compiler, DefinitionQueries, LoadedProgram, NotImplemented, PackageCache,
    PhaseAdapters, RunOutcome, SimulationConfig, ToolError,
};
pub mod native;
mod network;
mod pipeline;
pub use pipeline::{BodyAdapter, FrontPhases, LowerAdapter, ModeAdapter};
use std::{io::Write, path::Path};

/// The complete compiler over the front phases in [`FrontPhases`].
pub const COMPILER: Compiler = Compiler::new(FrontPhases::shared);

pub fn compiler_adapters() -> PhaseAdapters {
    COMPILER.adapters()
}
/// `.json` selects a launch manifest; `.hir` selects its exact `.hir.json`
/// sidecar if present, otherwise the explicit standalone@0 context.
pub fn load_program(path: &Path) -> Result<LoadedProgram, ToolError> {
    COMPILER.load_program(path)
}
/// Corpus transformations replace only the primary source, retaining its package graph.
pub fn load_program_source(
    path: &Path,
    replacement: Option<&str>,
) -> Result<LoadedProgram, ToolError> {
    COMPILER.load_program_source(path, replacement)
}
/// Load with explicit artifact storage, or `PackageCache::disabled()` for a fresh oracle.
pub fn load_program_source_with_cache(
    path: &Path,
    replacement: Option<&str>,
    cache: &PackageCache,
) -> Result<LoadedProgram, ToolError> {
    COMPILER.load_program_source_with_cache(path, replacement, cache)
}
/// A partial pipeline can produce rejection diagnostics, but never a clean acceptance.
pub fn check_path(path: &Path) -> Result<CheckOutput, ToolError> {
    COMPILER.check_path(path)
}
pub fn format_path(path: &Path) -> Result<String, ToolError> {
    COMPILER.format_path(path)
}
pub fn run_path(path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, ToolError> {
    COMPILER.run_path(path, stdout)
}
pub fn run_path_with_simulation(
    path: &Path,
    simulation: SimulationConfig,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<RunOutcome, ToolError> {
    let mut program = load_program(path)?;
    program.check_for_execution()?;
    // Tool launches classify an absent entry before requesting a rich observation.
    launch_signature(&mut program)?;
    let run = hirsel_tir::RunConfiguration {
        entry: program.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let observed = hirsel_engine::run_observed_snapshot(&mut program.engine, &run, simulation)?;
    if let Some(observation) = observed.observation {
        stdout
            .write_all(&observation.output)
            .map_err(ToolError::Io)?;
        if let Some(state) = observation.final_state {
            stderr.write_all(&state.stderr).map_err(ToolError::Io)?;
        }
    } else {
        stdout
            .write_all(&observed.tooling_output)
            .map_err(ToolError::Io)?;
    }
    observed.outcome.map_err(|error| match error {
        hirsel_engine::RuntimeError::Io(error) => ToolError::Io(error),
        hirsel_engine::RuntimeError::Query(error) => program.error(error),
        hirsel_engine::RuntimeError::Tool(message) => ToolError::Runtime(message),
    })
}
/// `hirsel export --tir <source|manifest.json>`: the proof-seam TIR export
///  of the checked program's entry closure.
pub fn export_tir_path(path: &Path) -> Result<String, ToolError> {
    hirsel_engine::with_compiler_stack(|| {
        let mut program = load_program(path)?;
        program.check_for_execution()?;
        let entry = program.entry.clone();
        program
            .engine
            .export_tir(&entry)
            .map_err(|error| program.error(error))
    })
}
/// Narrow conformance adapter returns diagnostics with revision-1 completion.
pub fn check_file(path: &Path) -> Result<CheckOutput, NotImplemented> {
    check_path(path).map_err(|_| NotImplemented)
}
pub fn run_file(path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, NotImplemented> {
    run_path(path, stdout).map_err(|_| NotImplemented)
}

pub(crate) fn launch_signature(
    program: &mut LoadedProgram,
) -> Result<hirsel_types::SignatureRef, ToolError> {
    program
        .engine
        .item_signature(&program.entry)
        .map(|q| q.value)
        .map_err(|e| match e {
            hirsel_types::QueryError::NotImplemented => ToolError::Runtime(format!(
                "launch: the program declares no entry `pub fn {}`",
                program.entry.0.name
            )),
            e => program.error(e),
        })
}
/// Export the semantic binding and denial defaults; callers explicitly select grants.
pub fn export_run_path(path: &Path) -> Result<Vec<u8>, ToolError> {
    let mut program = load_program(path)?;
    program.check_for_execution()?;
    let revision = program
        .engine
        .semantic_program_revision(&program.entry)
        .map_err(|e| program.error(e))?;
    Ok(
        hirsel_run_config::RunConfig::for_program(&revision, hirsel_run_config::LaunchKind::Main)
            .validated()
            .map_err(|e| ToolError::Runtime(e.to_string()))?
            .canonical()
            .to_vec(),
    )
}
/// Explicit production launch input. Templates bind to the code selected by this launch.
#[derive(Clone, Copy)]
pub enum LaunchConfig<'a> {
    Record(&'a Path),
    Grants(&'a Path),
}
impl LaunchConfig<'_> {
    fn read(
        self,
        revision: &hirsel_run_config::ProgramRevision,
    ) -> Result<hirsel_run_config::ValidatedRun, hirsel_run_config::RunRefusal> {
        match self {
            Self::Record(path) => hirsel_engine::read_run_record(path),
            Self::Grants(path) => hirsel_run_config::RunConfig::read_grants(
                path,
                revision.digest(),
                revision.entry().into(),
                hirsel_run_config::LaunchKind::Main,
            ),
        }
    }
}

/// Execute checked TIR with freshly admitted host grants and exact snapshots.
pub fn run_path_with_config(
    path: &Path,
    record: Option<LaunchConfig<'_>>,
    stdout: &mut dyn Write,
) -> Result<RunOutcome, ToolError> {
    use hirsel_interp::Outcome;
    if record.is_none() {
        return run_path_with_simulation(
            path,
            SimulationConfig::default(),
            stdout,
            &mut std::io::stderr().lock(),
        );
    }
    let mut program = load_program(path)?;
    program.check_for_execution()?;
    let signature = launch_signature(&mut program)?;
    let revision = program
        .engine
        .semantic_program_revision(&program.entry)
        .map_err(|e| program.error(e))?;
    let requested = record
        .map(|record| record.read(&revision))
        .transpose()
        .map_err(|e| ToolError::Runtime(e.to_string()))?;
    let mut host = hirsel_rt::host_launch::HostGrants::new(std::io::sink());
    let admitted = hirsel_engine::admit_run(
        &signature,
        &revision,
        hirsel_run_config::LaunchKind::Main,
        requested.as_ref(),
        &mut host,
        admission_time()?,
    )
    .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
    let config = admitted.run().record();
    let run = hirsel_tir::RunConfiguration {
        entry: program.entry.clone(),
        hash_seed: config.inputs.hash_seed,
        inputs: vec![],
    };
    let mut runtime = program
        .engine
        .runtime_types()
        .map_err(|e| program.error(e))?;
    runtime.trace_mode = hirsel_interp::TraceMode::Effects;
    let mut operations = network::HostLibrary::new(&run, admitted.run(), host.into_io(), stdout)
        .map_err(|e| ToolError::Runtime(format!("host launch: {e:?}")))?;
    let args = operations
        .library
        .entry_arguments(&signature)
        .map_err(|e| ToolError::Runtime(format!("launch: {e:?}")))?;
    let sources = program.engine.sources.clone();
    let execution = hirsel_interp::run_with_runtime(
        &mut program.engine,
        &sources,
        &run,
        args,
        hirsel_interp::Limits {
            steps: u64::MAX,
            call_depth: 256,
        },
        &mut operations,
        &runtime,
    )
    .map_err(|e| match e.error {
        hirsel_interp::Error::Unavailable(_) => ToolError::Query {
            error: hirsel_types::QueryError::NotImplemented,
            phase: Some(hirsel_engine::QueryKind::Interpret),
            sources: sources.clone(),
        },
        hirsel_interp::Error::Query(error) => ToolError::Query {
            error,
            phase: Some(hirsel_engine::QueryKind::Interpret),
            sources: sources.clone(),
        },
        error => ToolError::Runtime(format!("interpret: {error:?}")),
    })?;
    Ok(match execution.outcome {
        Outcome::Returned { exit_code, .. } => RunOutcome::Exit(exit_code),
        Outcome::Failed(f) => RunOutcome::Failure {
            rule: f.rule,
            line: f.line as usize,
        },
        Outcome::ToolingLimit(l) => RunOutcome::ToolingLimit(l),
        Outcome::UndefinedBehavior(u) => RunOutcome::UndefinedBehavior(u),
        Outcome::Trapped(t) => RunOutcome::Trapped(t),
        Outcome::Blocked(_) => RunOutcome::Blocked(vec![]),
    })
}
pub(crate) fn admission_time() -> Result<i64, ToolError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| ToolError::Runtime(e.to_string()))?;
    i64::try_from(now.as_nanos()).map_err(|e| ToolError::Runtime(e.to_string()))
}
