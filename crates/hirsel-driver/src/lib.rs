//! The compiler: hirsel-engine's queries wired to the front phases it may not
//! depend on (signature, check, modes, lower) and to the native backends that
//! compile its validated native input. Callers import engine items from
//! hirsel-engine itself; this crate exports only the wiring.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_engine::{
    CheckOutput, Compiler, DefinitionQueries, LoadedProgram, NotImplemented, PackageCache,
    PhaseAdapters, RunOutcome, SimulationConfig, ToolError,
};
pub mod api_index;
pub mod native;
mod network;
mod pipeline;
pub mod testing;
pub use pipeline::{BodyAdapter, FrontPhases, LowerAdapter, ModeAdapter};
use std::{io::Write, path::Path};

/// The complete compiler over the front phases in [`FrontPhases`].
include!(concat!(env!("OUT_DIR"), "/compiler_identity.rs"));
pub const COMPILER: Compiler = Compiler::new(FrontPhases::shared, COMPILER_IDENTITY);

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
/// `check_path`, then the escape from capability checking an accepted program's
/// entry reaches (CAP-6).
pub fn check_safe_path(
    path: &Path,
) -> Result<
    (
        CheckOutput,
        Option<(hirsel_syntax::DefPath, Option<hirsel_engine::UnsafeReach>)>,
    ),
    ToolError,
> {
    COMPILER.check_safe_path(path)
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
    run_simulation(COMPILER, path, simulation, None, stdout, stderr)
}
/// Execute simulated inputs with an explicit step budget; None is unbounded.
/// Ordinary driver/conformance entry points retain their adapter defaults.
pub fn run_path_with_step_budget(
    path: &Path,
    simulation: SimulationConfig,
    max_steps: Option<u64>,
    verify: hirsel_engine::Verify,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<RunOutcome, ToolError> {
    run_simulation(
        COMPILER.with_verify(verify),
        path,
        simulation,
        Some(hirsel_interp::Limits {
            steps: max_steps,
            ..Default::default()
        }),
        stdout,
        stderr,
    )
}
fn run_simulation(
    compiler: Compiler,
    path: &Path,
    simulation: SimulationConfig,
    limits: Option<hirsel_interp::Limits>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<RunOutcome, ToolError> {
    let mut program = compiler.load_program(path)?;
    program.check_for_execution()?;
    program.refuse_foreign_interpretation()?;
    if let Some(limits) = limits {
        program.engine.adapters.interpreter = Some(Box::new(hirsel_engine::InterpreterAdapter {
            limits,
            ..Default::default()
        }));
    }
    // Tool launches classify an absent entry before requesting a rich observation.
    launch_signature(&mut program)?;
    let run = hirsel_tir::RunConfiguration {
        entry: program.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let observed = hirsel_engine::run_observed_snapshot(&mut program.engine, &run, simulation)?;
    let (output, program_stderr) = if let Some(observation) = observed.observation {
        (
            observation.output,
            observation
                .final_state
                .map(|s| s.stderr)
                .unwrap_or_default(),
        )
    } else {
        (observed.tooling_output, observed.tooling_stderr)
    };
    if let Some(report) = observed.exit_report {
        if report.stdout_len > output.len() || report.stderr_len > program_stderr.len() {
            return Err(ToolError::Runtime(
                "invalid exit-report stream position".into(),
            ));
        }
        stdout
            .write_all(&output[..report.stdout_len])
            .map_err(ToolError::Io)?;
        stderr
            .write_all(&program_stderr[..report.stderr_len])
            .map_err(ToolError::Io)?;
        stdout.flush().map_err(ToolError::Io)?;
        stderr
            .write_all(report.text.as_bytes())
            .and_then(|()| stderr.flush())
            .map_err(ToolError::Io)?;
        stdout
            .write_all(&output[report.stdout_len..])
            .map_err(ToolError::Io)?;
        stderr
            .write_all(&program_stderr[report.stderr_len..])
            .map_err(ToolError::Io)?;
    } else {
        stdout.write_all(&output).map_err(ToolError::Io)?;
        stderr.write_all(&program_stderr).map_err(ToolError::Io)?;
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
/// `hirsel hash <source|manifest.json>`: one `<hash> <definition path>` line
/// per source definition of the checked program's package, sorted by path
/// (LANG-6 definition hashes).
pub fn hash_path(path: &Path) -> Result<String, ToolError> {
    hirsel_engine::with_compiler_stack(|| {
        let mut program = load_program(path)?;
        let output = program
            .check(hirsel_engine::LoweringPolicy::Comptime)
            .map_err(|error| program.error(error))?;
        if output
            .diagnostics
            .iter()
            .any(|d| d.severity.as_str() == "error")
        {
            return Err(program.error(hirsel_engine::QueryError::Diagnostics(output.diagnostics)));
        }
        let package = program.entry.0.package.clone();
        let hashes = program
            .engine
            .package_definition_hashes(&package)
            .map_err(|error| program.error(error))?;
        let mut out = String::new();
        for (definition, hash) in hashes
            .hashes
            .iter()
            .filter(|(definition, _)| definition.0.package == package)
        {
            for byte in hash.0 {
                out.push_str(&format!("{byte:02x}"));
            }
            out.push_str(&format!(" {definition}\n"));
        }
        Ok(out)
    })
}
/// `hirsel expand <file>`: each generated body (CT-5) of the file's declarations, in
/// source order, exactly as its generator wrote it, so diagnostic lines match.
pub fn expand_path(path: &Path) -> Result<String, ToolError> {
    hirsel_engine::with_compiler_stack(|| {
        let mut program = load_program(path)?;
        let output = program
            .check(hirsel_engine::LoweringPolicy::Comptime)
            .map_err(|error| program.error(error))?;
        if output
            .diagnostics
            .iter()
            .any(|d| d.severity.as_str() == "error")
        {
            return Err(program.error(hirsel_engine::QueryError::Diagnostics(output.diagnostics)));
        }
        let mut generated: Vec<_> = program
            .engine
            .generated_sources()
            .filter(|(_, g)| g.site.file == program.primary)
            .collect();
        generated.sort_by_key(|(_, g)| g.site.start);
        let mut out = String::new();
        for (definition, g) in generated {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("// generated body of {definition}\n{}\n", g.text));
        }
        Ok(out)
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
    let signature = launch_signature(&mut program)?;
    let launch = hirsel_engine::entry_launch_kind(&signature).map_err(ToolError::Launch)?;
    let revision = program
        .engine
        .semantic_program_revision(&program.entry)
        .map_err(|e| program.error(e))?;
    Ok(hirsel_run_config::RunConfig::for_program(&revision, launch)
        .validated()
        .map_err(|e| ToolError::Runtime(e.to_string()))?
        .canonical()
        .to_vec())
}
/// Explicit production launch input. Templates bind to the code selected by this launch.
#[derive(Clone, Copy)]
pub enum LaunchConfig<'a> {
    Record(&'a std::ffi::OsStr),
    Grants {
        input: &'a std::ffi::OsStr,
        arguments: Option<&'a [std::ffi::OsString]>,
    },
}
impl LaunchConfig<'_> {
    fn read(
        self,
        revision: &hirsel_run_config::ProgramRevision,
        launch: hirsel_run_config::LaunchKind,
    ) -> Result<hirsel_run_config::ValidatedRun, hirsel_run_config::RunRefusal> {
        match self {
            Self::Record(input) => hirsel_run_config::RunConfig::record_input(input),
            Self::Grants { input, arguments } => {
                let run = hirsel_run_config::RunConfig::grants_input(
                    input,
                    revision.digest(),
                    revision.entry().into(),
                    launch,
                )?;
                match arguments {
                    None => Ok(run),
                    Some(arguments) => {
                        let mut record = run.record().clone();
                        record.args =
                            Some(hirsel_run_config::snapshot_args(arguments.iter().cloned())?);
                        record.validated()
                    }
                }
            }
        }
    }
}

/// A production host launch: freshly admitted grants, optionally streaming every Io
/// completion to a tape, or the replay of such a tape (DET-1). A replay binds the run
/// its tape recorded, needs no grants and performs no host effect.
#[derive(Clone, Copy)]
pub enum HostLaunch<'a> {
    Live {
        config: LaunchConfig<'a>,
        record: Option<&'a Path>,
        revocations: Option<&'a Path>,
    },
    Replay(&'a Path),
}
impl HostLaunch<'static> {
    /// The launch without a record: the empty-grant record with all seeds zero
    /// (determinism.md, Run configuration), admitted like any template.
    pub fn empty() -> Self {
        Self::Live {
            config: LaunchConfig::Grants {
                input: std::ffi::OsStr::new("{}"),
                arguments: None,
            },
            record: None,
            revocations: None,
        }
    }
}

/// A host launch bound to the checked program, before entry.
pub(crate) enum HostRun {
    Live {
        admitted: hirsel_run_config::AdmittedRun,
        io: hirsel_rt::io_real::RealIo,
    },
    Replay {
        run: hirsel_run_config::ValidatedRun,
        io: hirsel_rt::io_sim::SimIo,
    },
}
impl HostRun {
    pub(crate) fn run(&self) -> &hirsel_run_config::ValidatedRun {
        match self {
            Self::Live { admitted, .. } => admitted.run(),
            Self::Replay { run, .. } => run,
        }
    }
}

/// A host launch bound to the checked program, before host admission.
pub(crate) enum BoundHostRun {
    Live(hirsel_run_config::BoundRun),
    Replay {
        run: hirsel_run_config::ValidatedRun,
        io: hirsel_rt::io_sim::SimIo,
    },
}

/// Bind fresh grants, or a recording, to the selected semantic revision and entry, with no
/// host consulted. A recording bound to other code, entry or roots refuses here; so does a
/// record whose code policy the entry's reach breaks (CAP-6), naming the path.
pub(crate) fn bind_launch(
    program: &mut LoadedProgram,
    launch: HostLaunch<'_>,
) -> Result<BoundHostRun, ToolError> {
    if matches!(
        launch,
        HostLaunch::Replay(_)
            | HostLaunch::Live {
                record: Some(_),
                ..
            }
    ) {
        program.refuse_unreplayable()?;
    }
    let signature = launch_signature(program)?;
    let kind = hirsel_engine::entry_launch_kind(&signature).map_err(ToolError::Launch)?;
    let revision = program
        .engine
        .semantic_program_revision(&program.entry)
        .map_err(|e| program.error(e))?;
    let code = match program.unsafe_reach()? {
        None => hirsel_run_config::CodeReach::Safe,
        Some(reach) => hirsel_run_config::CodeReach::Escapes(reach.spelled()),
    };
    match launch {
        HostLaunch::Live { config, .. } => {
            let requested = config
                .read(&revision, kind)
                .map_err(|e| ToolError::Runtime(e.to_string()))?;
            let bound =
                hirsel_engine::bind_run(&signature, &revision, kind, Some(&requested), code)
                    .map_err(ToolError::Launch)?;
            Ok(BoundHostRun::Live(bound))
        }
        HostLaunch::Replay(tape) => {
            let refused = |e: hirsel_run_config::RunRefusal| {
                ToolError::Runtime(format!("replay refused: {e}"))
            };
            let (io, run) = hirsel_rt::host_launch::replay(tape).map_err(refused)?;
            let roots = hirsel_engine::entry_roots(&signature, kind).map_err(refused)?;
            let run = run
                .bind(&hirsel_run_config::ProgramFacts::of(
                    &revision, kind, roots, code,
                ))
                .map_err(refused)?
                .into_run();
            Ok(BoundHostRun::Replay { run, io })
        }
    }
}

/// Bind a launch as [`bind_launch`] does, then admit a live one once on `host`, whose Io
/// writes the granted standard output; a live launch opens its journal here.
pub(crate) fn bind_host_run(
    program: &mut LoadedProgram,
    launch: HostLaunch<'_>,
    mut host: hirsel_rt::host_launch::HostGrants,
) -> Result<HostRun, ToolError> {
    let (record, revocations) = match &launch {
        HostLaunch::Live {
            record,
            revocations,
            ..
        } => (*record, *revocations),
        HostLaunch::Replay(_) => (None, None),
    };
    match bind_launch(program, launch)? {
        BoundHostRun::Live(bound) => {
            if let Some(path) = revocations {
                host.read_revocations(path)
                    .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
            }
            let admitted = bound
                .admit(&mut host, admission_time()?)
                .map_err(ToolError::Launch)?;
            let mut io = host.into_io();
            hirsel_rt::host_launch::journal(&mut io, &admitted, record, &[])
                .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
            Ok(HostRun::Live { admitted, io })
        }
        BoundHostRun::Replay { run, io } => Ok(HostRun::Replay { run, io }),
    }
}

/// Execute checked TIR on the host: live with admitted grants and exact snapshots,
/// writing the process's standard streams, or replaying a recording without host effects.
pub fn run_path_on_host(
    path: &Path,
    launch: HostLaunch<'_>,
    max_steps: Option<u64>,
    verify: hirsel_engine::Verify,
) -> Result<RunOutcome, ToolError> {
    use hirsel_interp::Outcome;
    let mut program = COMPILER.with_verify(verify).load_program(path)?;
    program.check_for_execution()?;
    program.refuse_foreign_interpretation()?;
    let signature = launch_signature(&mut program)?;
    let host = hirsel_rt::host_launch::HostGrants::cooperative(hirsel_rt::launch::RawStdout);
    let (run, io) = match bind_host_run(&mut program, launch, host)? {
        HostRun::Live { admitted, io } => (admitted.run().clone(), network::HostIo::Live(io)),
        // A replay parks where the recording parked (its `blocked` and `ready` events).
        HostRun::Replay { run, io } => (run, network::HostIo::Replay(io)),
    };
    let tir_run = hirsel_tir::RunConfiguration {
        entry: program.entry.clone(),
        hash_seed: run.record().inputs.hash_seed,
        inputs: vec![],
    };
    let mut runtime = program
        .engine
        .runtime_types()
        .map_err(|e| program.error(e))?;
    runtime.trace_mode = hirsel_interp::TraceMode::Effects;
    // A replay re-applies the recorded budgets, so it stops where the run stopped.
    if let Some(budgets) = run.budgets() {
        runtime.budgets = hirsel_interp::RunBudgets {
            fuel: budgets.fuel,
            memory_bytes: budgets.memory_bytes,
        };
    }
    let mut operations = network::HostLibrary::new(&tir_run, &run, io)
        .map_err(|e| ToolError::Runtime(format!("host launch: {e:?}")))?;
    let args = operations
        .library
        .entry_arguments(&signature)
        .map_err(|e| ToolError::Runtime(format!("launch: {e:?}")))?;
    let sources = program.engine.sources.clone();
    let execution = hirsel_interp::run_with_runtime(
        &mut program.engine,
        &sources,
        &tir_run,
        args,
        hirsel_interp::Limits {
            steps: max_steps,
            call_depth: 256,
        },
        &mut operations,
        &runtime,
    );
    let replay = matches!(operations.io, network::HostIo::Replay(_));
    if let Some(refusal) = operations.refused.take() {
        return Err(ToolError::Runtime(if replay {
            format!("replay refused: {refusal}")
        } else {
            format!("host Io: {refusal}")
        }));
    }
    let execution = execution.map_err(|e| match e.error {
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
    if let network::HostIo::Replay(io) = &mut operations.io {
        io.finish()
            .map_err(|e| ToolError::Runtime(format!("replay refused: {e:?}")))?;
    }
    let outcome = hirsel_engine::project_outcome(&execution, &sources)
        .map_err(|error| ToolError::Runtime(format!("{error:?}")))?;
    if let network::HostIo::Live(io) = &mut operations.io {
        io.finish_recording(&outcome)
            .map_err(|e| ToolError::Runtime(format!("the Io tape: {e}")))?;
    }
    Ok(outcome)
}

pub(crate) fn admission_time() -> Result<i64, ToolError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| ToolError::Runtime(e.to_string()))?;
    i64::try_from(now.as_nanos()).map_err(|e| ToolError::Runtime(e.to_string()))
}
