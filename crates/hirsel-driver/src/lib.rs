//! The compiler: hirsel-engine's queries wired to the front phases it may not
//! depend on (signature, check, modes, lower) and to the native backends that
//! compile its validated native input. Callers import engine items from
//! hirsel-engine itself; this crate exports only the wiring.

#[cfg(test)]
hirsel_guard::install!();

#[cfg(test)]
fn fixture_product(
    store: &dyn hirsel_engine::store::ResultStore,
    address: hirsel_engine::store::Address,
    action: hirsel_engine::store::Action,
    reference: hirsel_engine::store::Reference,
) -> hirsel_engine::runtime::ProductRef {
    let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
    let mut allowance = budget.reserve(64 << 20).unwrap();
    let archive =
        hirsel_engine::runtime::ProductArchive::open(store, reference, &mut allowance).unwrap();
    hirsel_engine::runtime::ProductRef::import_archive(address, action, archive)
}

#[cfg(test)]
fn fixture_product_bytes(
    address: hirsel_engine::store::Address,
    action: hirsel_engine::store::Action,
    domain: hirsel_engine::store::Domain,
    bytes: &[u8],
) -> hirsel_engine::runtime::ProductRef {
    use hirsel_engine::store::ResultStore;
    let store = hirsel_engine::store::MemStore::default();
    let reference = store.put_content(domain, bytes).unwrap();
    fixture_product(&store, address, action, reference)
}

use hirsel_engine::{
    CheckOutput, Compiler, LoadedProgram, NotImplemented, PhaseAdapters, RunOutcome,
    SimulationConfig, ToolError,
};
pub mod api_index;
pub mod candidate;
mod compiler_requests;
pub use compiler_requests::CompilerRequestsExt;
pub mod dependency_card;
mod launch_binding;
pub mod native;
mod network;
pub mod observation_view;
pub mod ownership_trace;
mod pipeline;
#[cfg(test)]
mod prepared_host_152;
pub mod requests;
pub mod resident;
pub mod serve;
pub mod session;
pub mod testing;
mod tir_execution;
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
/// A partial pipeline can produce rejection diagnostics, but never a clean acceptance.
pub fn check_path(path: &Path) -> Result<CheckOutput, ToolError> {
    session::with_session(path, hirsel_engine::Verify::On, session::Session::check)
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
    session::with_session(
        path,
        hirsel_engine::Verify::On,
        session::Session::check_safe,
    )
}

pub fn format_path(path: &Path) -> Result<String, ToolError> {
    COMPILER.format_path(path)
}
/// Prepare all direct source outputs without writing. Callers may compare or
/// write changed files only after this succeeds for the entire input.
pub fn prepare_format_path(path: &Path) -> Result<Vec<hirsel_engine::FormattedSource>, ToolError> {
    COMPILER.prepare_format_path(path)
}
pub fn run_path(path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, ToolError> {
    run_simulation(
        COMPILER,
        path,
        SimulationConfig::default(),
        None,
        stdout,
        &mut std::io::sink(),
    )
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
    let input = request_tir(path, compiler.verify(), None)?;
    let run = hirsel_tir::RunConfiguration {
        entry: input.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let observed =
        testing::observe_tir(&input, &run, simulation, limits.unwrap_or_default(), false)?;
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
        hirsel_engine::RuntimeError::Query(error) => ToolError::Query {
            error,
            phase: Some(hirsel_engine::QueryKind::Interpret),
            sources: input.sources.clone(),
        },
        hirsel_engine::RuntimeError::Tool(message) => ToolError::Runtime(message),
    })
}

/// `hirsel export --tir <source|manifest.json>`: the proof-seam TIR export
///  of the checked program's entry closure.
pub fn export_tir_path(path: &Path) -> Result<String, ToolError> {
    let output = session::request(
        path,
        hirsel_engine::Verify::On,
        requests::CompileOperation::ExportTir,
    )?;
    match &output.result {
        requests::CompileResult::ExportTir(text) => Ok(text.clone()),
        _ => Err(ToolError::Runtime(
            "TIR export request returned another operation".into(),
        )),
    }
}
/// `hirsel hash <source|manifest.json>`: one `<hash> <definition path>` line
/// per source definition of the checked program's package, sorted by path
/// (LANG-6 definition hashes).
pub fn hash_path(path: &Path) -> Result<String, ToolError> {
    let output = session::request(
        path,
        hirsel_engine::Verify::On,
        requests::CompileOperation::Hashes { package: None },
    )?;
    let requests::CompileResult::Hashes(hashes) = &output.result else {
        return Err(ToolError::Runtime(
            "hash request returned another operation".into(),
        ));
    };
    let mut text = String::new();
    for (definition, hash) in hashes
        .iter()
        .filter(|(path, _)| path.0.package == output.entry.0.package)
    {
        use std::fmt::Write;
        for byte in hash.0 {
            write!(text, "{byte:02x}").unwrap();
        }
        writeln!(text, " {definition}").unwrap();
    }
    Ok(text)
}
/// `hirsel expand <file>`: each generated body (CT-5) of the file's declarations, in
/// source order, exactly as its generator wrote it, so diagnostic lines match.
pub fn expand_path(path: &Path) -> Result<String, ToolError> {
    let output = session::request(
        path,
        hirsel_engine::Verify::On,
        requests::CompileOperation::Expand,
    )?;
    match &output.result {
        requests::CompileResult::Expansion(text) => Ok(text.to_string()),
        _ => Err(ToolError::Runtime(
            "expansion request returned another operation".into(),
        )),
    }
}
/// Narrow conformance adapter returns diagnostics with revision-1 completion.
pub fn check_file(path: &Path) -> Result<CheckOutput, NotImplemented> {
    check_path(path).map_err(|_| NotImplemented)
}
pub fn run_file(path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, NotImplemented> {
    run_path(path, stdout).map_err(|_| NotImplemented)
}

/// A minimal inline template for the entry's requested roots, for launch hints.
/// Io selects stdout only; no directory or other projection is implied.
pub fn entry_grants_hint(path: &Path) -> Result<String, ToolError> {
    let input = request_tir(path, hirsel_engine::Verify::On, None)?;
    let launch = hirsel_engine::entry_launch_kind(&input.runtime.types, &input.signature)
        .map_err(ToolError::Launch)?;
    let roots = hirsel_engine::entry_roots(&input.runtime.types, &input.signature, launch)
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
    let input = request_tir(path, hirsel_engine::Verify::On, None)?;
    let launch = hirsel_engine::entry_launch_kind(&input.runtime.types, &input.signature)
        .map_err(ToolError::Launch)?;
    let revision = &input.revision;
    Ok(hirsel_run_config::RunConfig::for_program(revision, launch)
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
/// A host launch bound to the checked program, before host admission.
pub(crate) enum BoundHostRun {
    Live(hirsel_run_config::BoundRun),
    Replay {
        run: hirsel_run_config::ValidatedRun,
        io: hirsel_rt::io_sim::SimIo,
    },
}

/// Parse an exact canonical logical definition path from a run record.
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

/// Admit the already bound live launch once on `host`; its Io and journal
/// open only after the immutable source facts and explicit grants have bound.
pub(crate) fn admit_bound_host_run(
    bound: BoundHostRun,
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
    match bound {
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
            if admitted
                .run()
                .record()
                .io
                .as_ref()
                .and_then(|io| io.tasks.as_deref())
                != Some("tasks.free@1")
            {
                hirsel_rt::host_launch::journal(&mut io, &admitted, record, &[])
                    .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
            }
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

/// Render canonical failure sites against the current source snapshot. Stored
/// outcomes keep logical paths; human and JSON diagnostics use physical paths.
pub fn report_runtime_failures(
    path: &Path,
    outcome: &RunOutcome,
    json: bool,
    stderr: &mut dyn Write,
) -> Result<(), ToolError> {
    let RunOutcome::Failed { primary, causes } = outcome else {
        return Ok(());
    };
    let input = request_tir(path, hirsel_engine::Verify::On, None)?;
    let sources = &input.sources;
    {
        let physical = |site: &hirsel_run_schema::Site| {
            let mut site = site.clone();
            if let Some(file) = sources.anchors().find_map(|(_, file, _)| {
                let file = sources.file(file)?;
                (file.logical_path().to_string_lossy() == site.file).then_some(file)
            }) {
                site.file = file.path.to_string_lossy().into_owned();
            }
            site
        };
        for failure in std::iter::once(primary).chain(causes) {
            let site = physical(&failure.site);
            let operation = failure.operation.as_ref().map(physical);
            writeln!(
                stderr,
                "{}",
                site.report(&failure.rule, json, operation.as_ref())
            )
            .map_err(ToolError::Io)?;
        }
        Ok(())
    }
}

/// Replay-only semantic enrichment. Binding and exact request consumption happen before
/// an observation is returned; this path never constructs or admits live host providers.
pub fn replay_observations(
    path: &Path,
    tape: &Path,
    max_steps: Option<u64>,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    let mut observed = None;
    let outcome = run_path_on_host_observed(
        path,
        HostLaunch::Replay(tape),
        max_steps,
        hirsel_engine::Verify::On,
        Some(&mut observed),
    )?;
    replay_observation(observed, &outcome)
}

/// Replay the same bound host tape with the interpreter's checked memory model.
/// Tooling stops remain non-comparable and cannot produce an observation.
pub fn replay_observations_checked(
    path: &Path,
    tape: &Path,
    max_steps: Option<u64>,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    let mut observed = None;
    let (outcome, _) = run_request_on_host(
        path,
        hirsel_engine::Verify::On,
        HostLaunch::Replay(tape),
        max_steps,
        Some(&mut observed),
        None,
        true,
    )?;
    replay_observation(observed, &outcome)
}

/// Record a reference or checked-H fixture through the native host tape protocol.
/// The supplied run is bound to this program before the snapshot is admitted.
pub fn record_fixture_observations(
    path: &Path,
    run: &Path,
    fixture: &native::Fixture,
    tape: &Path,
    checked: bool,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    let selection = launch_binding::LaunchSelection::read(HostLaunch::Live {
        config: LaunchConfig::Record(run.as_os_str()),
        record: Some(tape),
        revocations: None,
    })?;
    let input = request_tir(path, hirsel_engine::Verify::On, selection.entry().cloned())?;
    record_prepared_fixture_observations(&input, run, fixture, tape, checked)
}

/// Record a fixture from settled immutable source products with fresh host state.
pub fn record_prepared_fixture_observations(
    input: &requests::TirExecution,
    run: &Path,
    fixture: &native::Fixture,
    tape: &Path,
    checked: bool,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    let launch = HostLaunch::Live {
        config: LaunchConfig::Record(run.as_os_str()),
        record: Some(tape),
        revocations: None,
    };
    let selection = launch_binding::LaunchSelection::read(launch)?;
    let mut observed = None;
    let host = hirsel_rt::host_launch::HostGrants::snapshot(
        fixture.files.clone(),
        fixture.stdin.clone(),
        fixture.monotonic.unwrap_or(0),
    );
    let (outcome, _) = tir_execution::run_tir_on_host_with_grants(
        input,
        launch,
        selection,
        fixture.steps,
        Some(&mut observed),
        None,
        checked,
        Some(host),
    )?;
    replay_observation(observed, &outcome)
}

/// Replay a bound tape from settled products, creating fresh replay state per call.
pub fn replay_prepared_observations(
    input: &requests::TirExecution,
    tape: &Path,
    max_steps: Option<u64>,
    checked: bool,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    let launch = HostLaunch::Replay(tape);
    let selection = launch_binding::LaunchSelection::read(launch)?;
    let mut observed = None;
    let (outcome, _) = tir_execution::run_tir_on_host(
        input,
        launch,
        selection,
        max_steps,
        Some(&mut observed),
        None,
        checked,
    )?;
    replay_observation(observed, &outcome)
}

/// Record a fresh fixture execution while reusing this program's checked query graph.
/// Provider state, grants, admission and the recording are new for every call.
pub fn record_loaded_fixture_observations(
    program: &mut LoadedProgram,
    run: &Path,
    fixture: &native::Fixture,
    tape: &Path,
    checked: bool,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    let mut observed = None;
    let host = hirsel_rt::host_launch::HostGrants::snapshot(
        fixture.files.clone(),
        fixture.stdin.clone(),
        fixture.monotonic.unwrap_or(0),
    );
    let outcome = tir_execution::run_loaded_on_host_with_checks(
        program,
        HostLaunch::Live {
            config: LaunchConfig::Record(run.as_os_str()),
            record: Some(tape),
            revocations: None,
        },
        fixture.steps,
        Some(&mut observed),
        None,
        checked,
        Some(host),
    );
    let (outcome, _) = outcome?;
    replay_observation(observed, &outcome)
}

/// Replay a bound tape with fresh replay state, retaining immutable compiler queries.
pub fn replay_loaded_observations(
    program: &mut LoadedProgram,
    tape: &Path,
    max_steps: Option<u64>,
    checked: bool,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    let mut observed = None;
    let (outcome, _) = tir_execution::run_loaded_on_host_with_checks(
        program,
        HostLaunch::Replay(tape),
        max_steps,
        Some(&mut observed),
        None,
        checked,
        None,
    )?;
    replay_observation(observed, &outcome)
}

fn replay_observation(
    observed: Option<hirsel_interp::observation::Observable>,
    outcome: &RunOutcome,
) -> Result<hirsel_interp::observation::Observable, ToolError> {
    observed.ok_or_else(|| {
        ToolError::Runtime(format!(
            "replay stopped without a semantic observation: {outcome:?}"
        ))
    })
}

fn run_path_on_host_observed(
    path: &Path,
    launch: HostLaunch<'_>,
    max_steps: Option<u64>,
    verify: hirsel_engine::Verify,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
) -> Result<RunOutcome, ToolError> {
    run_request_on_host(path, verify, launch, max_steps, observed, None, false)
        .map(|(outcome, _)| outcome)
}

/// Prepare the selected entry through the ordinary immutable compiler graph.
/// The returned products can serve repeated simulations with fresh runtime state.
/// `None` selects the manifest entry; an explicit definition binds that entry.
pub fn request_tir(
    path: &Path,
    verify: hirsel_engine::Verify,
    definition: Option<hirsel_syntax::DefPath>,
) -> Result<std::sync::Arc<requests::TirExecution>, ToolError> {
    let output = session::request(path, verify, requests::CompileOperation::Tir { definition })?;
    match &output.result {
        requests::CompileResult::Tir(input) => Ok(input.clone()),
        _ => Err(ToolError::Runtime(
            "TIR request returned another operation".into(),
        )),
    }
}

pub(crate) fn run_request_on_host(
    path: &Path,
    verify: hirsel_engine::Verify,
    launch: HostLaunch<'_>,
    max_steps: Option<u64>,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
    cancel: Option<network::ExternalCancel>,
    checked: bool,
) -> Result<(RunOutcome, Option<hirsel_run_schema::CalleeOutcome>), ToolError> {
    let selection = launch_binding::LaunchSelection::read(launch)?;
    let input = request_tir(path, verify, selection.entry().cloned())?;
    tir_execution::run_tir_on_host(
        &input, launch, selection, max_steps, observed, cancel, checked,
    )
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
        let bytes = std::fs::read(path).map_err(|e| vec![format!("{}: {e}", path.display())])?;
        let recording = if bytes.starts_with(hirsel_free::tape::MAGIC) {
            hirsel_rt::free_replay::linked_recording(&bytes)
                .map_err(|e| vec![format!("{}: {e}", path.display())])?
        } else {
            hirsel_rt::host_launch::read_recording(path)
                .map_err(|e| vec![format!("{}: {e}", path.display())])?
        };
        tapes.push((path.display().to_string(), recording));
    }
    hirsel_rt::tape_links::check(&tapes)
}
