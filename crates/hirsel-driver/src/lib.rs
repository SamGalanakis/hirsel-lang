//! The compiler: hirsel-engine's queries wired to the front phases it may not
//! depend on (signature, check, modes, lower) and to the native backends that
//! compile its validated native input. Callers import engine items from
//! hirsel-engine itself; this crate exports only the wiring.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_engine::{
    CheckOutput, Compiler, DefinitionQueries, LoadedProgram, NotImplemented, PhaseAdapters,
    RunOutcome, SimulationConfig, StdImages, ToolError,
};
pub mod api_index;
pub mod candidate;
pub mod dependency_card;
pub mod native;
mod network;
pub mod observation_view;
mod pipeline;
pub mod serve;
pub mod session;
pub mod testing;
mod trial;
pub use pipeline::{BodyAdapter, FrontPhases, LowerAdapter, ModeAdapter};
use std::{io::Write, path::Path};
pub use trial::{TrialCall, trial_tapes};

/// The complete compiler over the front phases in [`FrontPhases`].
include!(concat!(env!("OUT_DIR"), "/compiler_identity.rs"));
pub const COMPILER: Compiler = Compiler::new(FrontPhases::shared, COMPILER_IDENTITY);

pub fn compiler_adapters() -> PhaseAdapters {
    COMPILER.adapters()
}
/// `.json` selects a launch manifest; `.hir` selects its exact `.hir.json`
/// sidecar if present, otherwise standalone@0 with the toolchain std@1.
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
/// Load adopting exactly `images` (`StdImages::none()` checks every package from source).
pub fn load_program_source_with_images(
    path: &Path,
    replacement: Option<&str>,
    images: &StdImages,
) -> Result<LoadedProgram, ToolError> {
    COMPILER.load_program_source_with_images(path, replacement, images)
}
/// A partial pipeline can produce rejection diagnostics, but never a clean acceptance.
pub fn check_path(path: &Path) -> Result<CheckOutput, ToolError> {
    session::with_session(path, hirsel_engine::Verify::On, session::Session::check)
        .unwrap_or_else(|| COMPILER.check_path(path))
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
    if !session::resident() {
        return COMPILER.check_safe_path(path);
    }
    session::with_program(path, hirsel_engine::Verify::On, |program| {
        let output = program
            .check(hirsel_engine::LoweringPolicy::Comptime)
            .map_err(|error| program.error(error))?;
        if output
            .diagnostics
            .iter()
            .any(|d| d.severity == hirsel_diag::Severity::Error)
            || output.completion != hirsel_engine::CheckCompletion::Complete
        {
            return Ok((output, None));
        }
        let reach = program.unsafe_reach()?;
        Ok((output, Some((program.entry.clone(), reach))))
    })
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
            .filter(|(_, g)| program.engine.sources.resolve(g.site).file == program.primary)
            .collect();
        generated.sort_by_key(|(_, g)| program.engine.sources.resolve(g.site).start);
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
/// A minimal inline template for the entry's requested roots, for launch hints.
/// Io selects stdout only; no directory or other projection is implied.
pub fn entry_grants_hint(path: &Path) -> Result<String, ToolError> {
    let mut program = load_program(path)?;
    program.check_for_execution()?;
    let signature = launch_signature(&mut program)?;
    let launch = hirsel_engine::entry_launch_kind(program.engine.types(), &signature)
        .map_err(ToolError::Launch)?;
    let roots = hirsel_engine::entry_roots(program.engine.types(), &signature, launch)
        .map_err(ToolError::Launch)?;
    let mut grants = serde_json::Map::new();
    for root in roots {
        use hirsel_run_config::Root;
        let (name, value) = match root {
            Root::Io => (
                "io",
                serde_json::json!({"stdout":"stdout.bytes@1","dirs":[]}),
            ),
            Root::Args => ("args", serde_json::json!([])),
            Root::Env => ("env", serde_json::json!({})),
            Root::Unsafe => ("unsafe", serde_json::json!(true)),
            Root::Test => continue,
        };
        grants.insert(name.into(), value);
    }
    Ok(serde_json::Value::Object(grants).to_string())
}

/// Export the semantic binding and denial defaults; callers explicitly select grants.
pub fn export_run_path(path: &Path) -> Result<Vec<u8>, ToolError> {
    let mut program = load_program(path)?;
    program.check_for_execution()?;
    let signature = launch_signature(&mut program)?;
    let launch = hirsel_engine::entry_launch_kind(program.engine.types(), &signature)
        .map_err(ToolError::Launch)?;
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
    /// CLI placement override, applied to the validated record or grants template.
    Cores {
        input: &'a std::ffi::OsStr,
        grants: bool,
        arguments: Option<&'a [std::ffi::OsString]>,
        cores: u64,
    },
    Record(&'a std::ffi::OsStr),
    Grants {
        input: &'a std::ffi::OsStr,
        arguments: Option<&'a [std::ffi::OsString]>,
    },
}
impl LaunchConfig<'_> {
    fn read(
        self,
        revision: &hirsel_run_schema::ProgramRevision,
        launch: hirsel_run_schema::LaunchKind,
    ) -> Result<hirsel_run_config::ValidatedRun, hirsel_run_schema::RunRefusal> {
        match self {
            Self::Cores {
                input,
                grants,
                arguments,
                cores,
            } => {
                let config = if grants {
                    Self::Grants { input, arguments }
                } else {
                    Self::Record(input)
                };
                config.read(revision, launch)?.with_cores(cores)
            }
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
                            Some(hirsel_run_schema::snapshot_args(arguments.iter().cloned())?);
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
    /// A callee run a runner host admits for an arrived call (RUN-10): its validated `call`
    /// record, and the tape it records to.
    Callee {
        run: &'a hirsel_run_config::ValidatedRun,
        record: Option<&'a Path>,
    },
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
    let refused =
        |e: hirsel_run_schema::RunRefusal| ToolError::Runtime(format!("replay refused: {e}"));
    // A callee run's tape replays its callee entry (RUN-10, RUN-11).
    let replay = match launch {
        HostLaunch::Replay(tape) => {
            let (io, run) = hirsel_rt::host_launch::replay(tape).map_err(refused)?;
            if run.record().launch == hirsel_run_schema::LaunchKind::Call {
                program.entry = def_path(&run.record().entry).ok_or_else(|| {
                    ToolError::Runtime("replay refused: the callee entry's path".into())
                })?;
            }
            Some((io, run))
        }
        HostLaunch::Callee { run, .. } => {
            program.entry = def_path(&run.record().entry)
                .ok_or_else(|| ToolError::Runtime("launch: the callee entry's path".into()))?;
            None
        }
        HostLaunch::Live { .. } => None,
    };
    if replay.is_some()
        || matches!(
            launch,
            HostLaunch::Live {
                record: Some(_),
                ..
            } | HostLaunch::Callee {
                record: Some(_),
                ..
            }
        )
    {
        program.refuse_unreplayable()?;
    }
    let signature = launch_signature(program)?;
    let kind = match (&replay, launch) {
        (Some((_, run)), _) if run.record().launch == hirsel_run_schema::LaunchKind::Call => {
            hirsel_run_schema::LaunchKind::Call
        }
        (_, HostLaunch::Callee { .. }) => hirsel_run_schema::LaunchKind::Call,
        _ => hirsel_engine::entry_launch_kind(program.engine.types(), &signature)
            .map_err(ToolError::Launch)?,
    };
    let revision = program
        .engine
        .semantic_program_revision(&program.entry)
        .map_err(|e| program.error(e))?;
    let code = match program.unsafe_reach()? {
        None => hirsel_run_schema::CodeReach::Safe,
        Some(reach) => hirsel_run_schema::CodeReach::Escapes(reach.spelled()),
    };
    match (launch, replay) {
        (HostLaunch::Live { config, .. }, _) => {
            let requested = config
                .read(&revision, kind)
                .map_err(|e| ToolError::Runtime(e.to_string()))?;
            let bound = hirsel_engine::bind_run(
                program.engine.types(),
                &signature,
                &revision,
                kind,
                Some(&requested),
                code,
            )
            .map_err(ToolError::Launch)?;
            Ok(BoundHostRun::Live(bound))
        }
        (HostLaunch::Callee { run, .. }, _) => {
            let bound = hirsel_engine::bind_run(
                program.engine.types(),
                &signature,
                &revision,
                kind,
                Some(run),
                code,
            )
            .map_err(ToolError::Launch)?;
            Ok(BoundHostRun::Live(bound))
        }
        (HostLaunch::Replay(_), Some((io, run))) => {
            let roots = hirsel_engine::entry_roots(program.engine.types(), &signature, kind)
                .map_err(refused)?;
            let run = run
                .bind(&hirsel_run_config::ProgramFacts::of(
                    &revision, kind, roots, code,
                ))
                .map_err(refused)?
                .into_run();
            Ok(BoundHostRun::Replay { run, io })
        }
        (HostLaunch::Replay(_), None) => unreachable!("a replay reads its tape first"),
    }
}

/// A definition's canonical spelling (`package@version/module/name`) as a path.
pub(crate) fn def_path(spelled: &str) -> Option<hirsel_syntax::DefPath> {
    let (package, rest) = spelled.split_once('@')?;
    let mut parts = rest.split('/');
    let version = parts.next()?;
    let mut modules: Vec<String> = parts.map(str::to_owned).collect();
    let name = modules.pop()?;
    let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
        package: hirsel_syntax::PackageId {
            name: package.into(),
            version: version.into(),
        },
        modules,
        name,
    });
    (path.to_string() == spelled).then_some(path)
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
        HostLaunch::Callee { record, .. } => (*record, None),
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
            // The interpreter reaches every operation, so it takes the host network and
            // process providers at once, as a compiled program's entries hand them over.
            hirsel_rt::io::Adapter::net(&mut io, hirsel_rt::net_host::provider);
            hirsel_rt::io::Adapter::process(&mut io, hirsel_rt::process::host);
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
    run_path_on_host_observed(path, launch, max_steps, verify, None)
}

/// Replay-only semantic enrichment. Binding and exact request consumption happen before
/// an observation is returned; this path never constructs or admits live host providers.
pub fn replay_observations(
    path: &Path,
    tape: &Path,
    max_steps: Option<u64>,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    let mut observed = None;
    run_path_on_host_observed(
        path,
        HostLaunch::Replay(tape),
        max_steps,
        hirsel_engine::Verify::On,
        Some(&mut observed),
    )?;
    observed.ok_or_else(|| ToolError::Runtime("replay produced no semantic observation".into()))
}

fn run_path_on_host_observed(
    path: &Path,
    launch: HostLaunch<'_>,
    max_steps: Option<u64>,
    verify: hirsel_engine::Verify,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
) -> Result<RunOutcome, ToolError> {
    session::with_program(path, verify, |program| {
        run_loaded_on_host(program, launch, max_steps, observed, None).map(|(outcome, _)| outcome)
    })
}

/// Execute a loaded, checked program on the host as [`run_path_on_host`] does. A callee
/// launch also returns its own outcome's bytes (RUN-11), and `cancel` lets another thread
/// cancel the run's root scope (RUN-9).
pub(crate) fn run_loaded_on_host(
    program: &mut LoadedProgram,
    launch: HostLaunch<'_>,
    max_steps: Option<u64>,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
    cancel: Option<network::ExternalCancel>,
) -> Result<(RunOutcome, Option<hirsel_run_schema::CalleeOutcome>), ToolError> {
    program.check_for_execution()?;
    program.refuse_foreign_interpretation()?;
    let host = hirsel_rt::host_launch::HostGrants::cooperative(hirsel_rt::launch::RawStdout);
    let tape = match launch {
        HostLaunch::Live { record, .. } | HostLaunch::Callee { record, .. } => {
            record.map(Path::to_path_buf)
        }
        HostLaunch::Replay(_) => None,
    };
    let (run, io, instance) = match bind_host_run(program, launch, host)? {
        HostRun::Live { admitted, io } => (
            admitted.run().clone(),
            network::HostIo::Live(io),
            admitted.instance(),
        ),
        // A replay parks where the recording parked (its `blocked` and `ready` events).
        HostRun::Replay { run, io } => {
            let instance = io.instance();
            (run, network::HostIo::Replay(io), instance)
        }
    };
    // A callee run's replay entered its callee entry (`bind_launch`).
    let signature = launch_signature(program)?;
    let call = run.record().call.clone();
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
    if observed.is_some() {
        operations.silence_replay_output();
    }
    let call_schemas = match &call {
        Some(_) => Some(call_schemas(&signature, &runtime)?),
        None => None,
    };
    if run.calls() || call.is_some() {
        let callees = hirsel_engine::callee_facts(&mut program.engine, &program.entry)
            .map_err(|e| program.error(e))?;
        // A live run's remote runners open their providers now, reading their TLS material.
        let remote = match &operations.io {
            network::HostIo::Live(_) => network::RemoteRunners::open(
                run.record()
                    .io
                    .as_ref()
                    .map_or(&[][..], |io| io.runners.as_slice()),
                instance.unwrap_or_default(),
                || network::remote_wire(&runtime, hirsel_remote::wire::DEFAULT_FRAME_BYTES),
            )
            .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?,
            network::HostIo::Replay(_) => None,
        };
        operations.set_runners(network::RunnerContext {
            instance,
            callees: std::sync::Arc::new(callees),
            tape,
            role: call_schemas.as_ref().map(|(schemas, _)| schemas.clone()),
            remote,
        });
    }
    if let Some(cancel) = cancel {
        operations
            .set_external_cancel(cancel)
            .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
    }
    // F0's free-mode test channel (`hirsel_free::session::TestHook`).
    let free = hirsel_free::session::TestHook::from_env().map_err(ToolError::Runtime)?;
    if let Some(hook) = &free {
        let session = hook.session(&run.identity()).map_err(ToolError::Runtime)?;
        operations.library.enable_free(session, hook.seed);
    }
    let args = match (&call, &call_schemas) {
        // A callee run enters with its place's Io and the argument its call carries.
        (Some(call), Some((_, argument))) => {
            let bytes = call
                .argument()
                .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
            let value = hirsel_interp::library::runners::decode(argument, &bytes, &runtime)
                .map_err(|e| ToolError::Runtime(format!("launch: the call's argument: {e:?}")))?;
            match call.form() {
                hirsel_run_schema::CallForm::Call => vec![operations.library.root_io(), value],
                hirsel_run_schema::CallForm::Compute => vec![value],
            }
        }
        _ => operations
            .library
            .entry_arguments(&runtime.types, &signature)
            .map_err(|e| ToolError::Runtime(format!("launch: {e:?}")))?,
    };
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
    if let Some(hook) = &free
        && let Some(session) = operations.library.take_free()
    {
        let outcome = match &execution {
            Ok(execution) => hirsel_interp::library::free_outcome_name(&execution.outcome),
            Err(_) => "stopped",
        };
        hook.finish(session, outcome).map_err(ToolError::Runtime)?;
    }
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
    // A callee run records its own outcome as it ended (RUN-11); replay checks it.
    let end = match (&call, &execution.outcome) {
        (Some(_), hirsel_interp::Outcome::Returned { value, .. }) => {
            Some(hirsel_interp::CalleeEnd::Returned(value.clone()))
        }
        (Some(_), hirsel_interp::Outcome::Failed(_)) => {
            Some(hirsel_interp::CalleeEnd::Failed(execution.failures.clone()))
        }
        _ => None,
    };
    let mut returned = None;
    if let Some(end) = end {
        let (mut stdout, mut trace) = (vec![], vec![]);
        let mut cx = hirsel_interp::OperationContext {
            configuration: &tir_run,
            sources: &sources,
            stdout: &mut stdout,
            trace: &mut trace,
            trace_mode: runtime.trace_mode,
            anchor: None,
        };
        returned = operations.callee_return(&end, &mut cx).map_err(|e| {
            ToolError::Runtime(match operations.refused.take() {
                Some(refusal) if replay => format!("replay refused: {refusal}"),
                _ => format!("the callee's outcome: {e:?}"),
            })
        })?;
    }
    if let network::HostIo::Replay(io) = &mut operations.io {
        io.finish()
            .map_err(|e| ToolError::Runtime(format!("replay refused: {e:?}")))?;
    }
    if let Some(observed) = observed {
        *observed = execution.observable(&sources);
    }
    let outcome = hirsel_engine::project_outcome(&execution, &sources)
        .map_err(|error| ToolError::Runtime(format!("{error:?}")))?;
    if let network::HostIo::Live(io) = &mut operations.io {
        io.finish_recording(&outcome)
            .map_err(|e| ToolError::Runtime(format!("the Io tape: {e}")))?;
    }
    Ok((outcome, returned))
}

/// A callee entry's call schemas and its argument's schema, from its checked signature.
pub(crate) fn call_schemas(
    signature: &hirsel_types::Signature,
    runtime: &hirsel_interp::aggregate::RuntimeTypes,
) -> Result<
    (
        hirsel_interp::CallSchemas,
        hirsel_types::serialization::SerializableSchema,
    ),
    ToolError,
> {
    let invalid = || ToolError::Runtime("launch: a callee entry's signature".into());
    let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
        return Err(invalid());
    };
    let schema = |ty| {
        hirsel_types::serialization::serializable_schema(
            &runtime.types,
            ty,
            &hirsel_types::GenericDeclarations::default(),
            &mut |d: hirsel_syntax::DefId| Ok(runtime.serialization.get(d.path()).cloned()),
        )
        .ok()
        .flatten()
        .ok_or_else(invalid)
    };
    let Some(hirsel_types::TypeKind::Named { arguments, .. }) =
        runtime.types.get(function.result).map(|t| &t.kind)
    else {
        return Err(invalid());
    };
    let [
        hirsel_types::GenericArgument::Type(ok),
        hirsel_types::GenericArgument::Type(error),
    ] = arguments.as_ref()
    else {
        return Err(invalid());
    };
    let argument = function.parameters.last().ok_or_else(invalid)?.ty;
    Ok((
        hirsel_interp::CallSchemas {
            result: schema(*ok)?,
            error: schema(*error)?,
            failure: hirsel_interp::library::runners::failure_schema(runtime)
                .map_err(|_| invalid())?,
        },
        schema(argument)?,
    ))
}

/// End this process by `kind`'s default action, as a standalone launcher ends a `Signaled`
/// run, and return the status a parent sees if it is not ended (`RunOutcome::exit_code`).
pub fn end_by_signal(kind: hirsel_run_schema::SignalKind) -> u8 {
    let _ = std::io::Write::flush(&mut std::io::stdout());
    // SAFETY: resetting a disposition and raising a signal on this process.
    unsafe {
        libc::signal(kind.number(), libc::SIG_DFL);
        libc::raise(kind.number());
    }
    hirsel_run_schema::RunOutcome::Signaled(kind).exit_code() as u8
}
pub(crate) fn admission_time() -> Result<i64, ToolError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| ToolError::Runtime(e.to_string()))?;
    i64::try_from(now.as_nanos()).map_err(|e| ToolError::Runtime(e.to_string()))
}

/// `hirsel tapes check`: the joined check over linked tapes (RUN-11).
pub fn check_tapes(
    paths: &[std::path::PathBuf],
) -> Result<hirsel_rt::tape_links::Links, Vec<String>> {
    let mut tapes = vec![];
    for path in paths {
        let recording = hirsel_rt::host_launch::read_recording(path)
            .map_err(|e| vec![format!("{}: {e}", path.display())])?;
        tapes.push((path.display().to_string(), recording));
    }
    hirsel_rt::tape_links::check(&tapes)
}
