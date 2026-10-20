//! `hirsel run --native`: the checked program, optimized, run as an image in the host's
//! runner (or, when it calls C, as an executable).
use super::{
    NativeError, OptLevel, Options, Partition, Sites, StageName, TargetSpec, Toolchain, build,
};
use hirsel_engine::{RunOutcome, ToolError};
use std::io::Write;
use std::path::Path;

/// Output destinations for a user launch. Inheriting the two OS streams preserves
/// the program's cross-stream write order; custom writers receive each stream.
pub enum Output<'a> {
    Inherit {
        json: bool,
    },
    Streams {
        stdout: &'a mut dyn Write,
        stderr: &'a mut dyn Write,
    },
}

/// Check `path` exactly as `run` does (the same first static diagnostics),
/// then build it for x86-64 Linux with every optimization, link it against
/// the in-tree runtime and launch it under the empty-grant record, as a launch
/// without a record. The executable's output is written to `stdout` and its
/// failure chain to `stderr`.
///
/// The outer error is a source or tool error of the checker; the inner one is
/// a native tool outcome (a stage does not support the program yet), which is
/// never a verdict about the source.
pub fn run_path_native(
    path: &Path,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    run_path_native_with_config(
        path,
        crate::HostLaunch::empty(),
        None,
        Output::Streams { stdout, stderr },
    )
}

/// `run_path_native` with a host launch and PS-12 evidence; the proof report goes to
/// `stderr` before the program runs. Record and replay pass to the executable as
/// `--hirsel-record` and `--hirsel-replay`; the driver binds the run first, so a
/// recording of other code refuses before building.
pub fn run_path_native_with_config(
    path: &Path,
    launch: crate::HostLaunch<'_>,
    evidence: Option<&super::Evidence>,
    output: Output<'_>,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    run_path_native_backend(
        path,
        launch,
        evidence,
        super::Backend::Llvm,
        output,
        hirsel_engine::Verify::On,
    )
}

/// `run_path_native_with_config` with `backend`: LLVM builds at O3, Cranelift
/// unoptimized, for a fast debug loop.
pub fn run_path_native_backend(
    path: &Path,
    launch: crate::HostLaunch<'_>,
    evidence: Option<&super::Evidence>,
    backend: super::Backend,
    output: Output<'_>,
    verify: hirsel_engine::Verify,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    let (mut host_stdout, mut host_stderr) = (std::io::stdout(), std::io::stderr());
    let (inherit, json, stdout, stderr): (bool, bool, &mut dyn Write, &mut dyn Write) = match output
    {
        Output::Inherit { json } => (true, json, &mut host_stdout, &mut host_stderr),
        Output::Streams { stdout, stderr } => (false, false, stdout, stderr),
    };
    let mut loaded = crate::COMPILER.with_verify(verify).load_program(path)?;
    loaded.check_for_execution()?;
    if matches!(
        launch,
        crate::HostLaunch::Replay(_)
            | crate::HostLaunch::Live {
                record: Some(_),
                ..
            }
    ) {
        loaded.refuse_unreplayable()?;
    }
    // The driver binds only: the executable admits the run, once (design.md).
    let host = match launch {
        crate::HostLaunch::Live {
            config,
            record,
            revocations,
        } => {
            let live = crate::HostLaunch::Live {
                config,
                record: None,
                revocations,
            };
            let crate::BoundHostRun::Live(bound) = crate::bind_launch(&mut loaded, live)? else {
                unreachable!("a live launch binds live")
            };
            // The executable records; the level and the destination must agree here first.
            hirsel_rt::host_launch::recording_level(bound.run(), record.is_some())
                .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
            (
                bound.into_run(),
                super::link::Tape::Live {
                    record,
                    revocations,
                },
            )
        }
        launch @ crate::HostLaunch::Replay(tape) => {
            let crate::BoundHostRun::Replay { run, .. } = crate::bind_launch(&mut loaded, launch)?
            else {
                unreachable!("a replay binds a recording")
            };
            (run, super::link::Tape::Replay(tape))
        }
        crate::HostLaunch::Callee { .. } => {
            return Err(ToolError::Runtime(
                "launch: a runner host runs its callees interpreted".into(),
            ));
        }
    };
    let entry = loaded.entry;
    let mut engine = loaded.engine;
    let query = |engine: &hirsel_engine::QueryEngine, error| ToolError::Query {
        error,
        phase: engine.unavailable,
        sources: engine.sources.clone(),
    };
    let input = match engine.export_native_input() {
        Ok(input) => input,
        Err(error) => return Err(query(&engine, error)),
    };
    let validated = match engine.import_native_input(input) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(Err(NativeError::new(
                StageName::Manifest,
                format!("the native snapshot: {error:?}"),
            )));
        }
    };
    let proofs = match evidence
        .map(|e| super::Proofs::assess(&mut engine, &entry, &validated, e))
        .transpose()
    {
        Ok(proofs) => proofs,
        Err(error) => return Ok(Err(error)),
    };
    let observation = match run(
        &validated,
        &entry,
        (&host.0, host.1),
        proofs.as_ref(),
        backend,
        inherit,
        stdout,
        stderr,
    ) {
        Ok(super::link::Observed::Ran(observation, waits)) => Ok((observation, waits)),
        // The executable admitted the run and refused it: the same refusal the interpreter's
        // admission reports.
        Ok(super::link::Observed::Refused(reason)) => {
            return Err(match hirsel_run_schema::RunRefusal::parse(&reason) {
                Some(refusal) => ToolError::Launch(refusal),
                None => ToolError::Runtime(format!("launch: {reason}")),
            });
        }
        Ok(super::link::Observed::Stopped(outcome)) => return Ok(Ok(outcome)),
        Err(error) => Err(error),
    };
    Ok(observation.and_then(|(observation, waits)| {
        let io = |e: std::io::Error| NativeError::new(StageName::Launch, e.to_string());

        let sites = Sites::new(validated.snapshot());
        for failure in &observation.failures {
            writeln!(stderr, "{}", sites.report(failure, json)?).map_err(io)?;
        }
        observation.outcome(&sites, waits)
    }))
}

fn run(
    validated: &hirsel_engine::ValidatedNativeInput,
    entry: &hirsel_engine::DefPath,
    (run, tape): (&hirsel_run_config::ValidatedRun, super::link::Tape<'_>),
    proofs: Option<&super::Proofs>,
    backend: super::Backend,
    inherit: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<super::link::Observed, NativeError> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut options = Options::new(target.clone());
    if let Some(proofs) = proofs {
        options.discharge = proofs.discharge.clone();
    }
    let mut program = build(validated, &options)?;
    // A fuel budget needs fuel checks; without one the executable pays nothing.
    program.fuel = run.budgets().is_some_and(|b| b.fuel.is_some());
    for line in proofs.map_or_else(Vec::new, |p| p.report(&program)) {
        writeln!(stderr, "hirsel: {line}")
            .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
    }
    let scratch = std::env::temp_dir().join(format!("hirsel-native-{}", std::process::id()));
    let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(super::runtime_cache::default_directory);
    let io = |e: std::io::Error| NativeError::new(StageName::Link, e.to_string());
    std::fs::create_dir_all(&scratch).map_err(io)?;
    let result = (|| {
        let toolchain = Toolchain::in_tree(&cache, &[&target])?;
        let opt = match backend {
            super::Backend::Llvm => OptLevel::Speed,
            super::Backend::Cranelift => OptLevel::None,
        };
        let objects = super::link::emit_backend_objects(
            &program,
            backend,
            opt,
            Partition::Whole,
            false,
            None,
        )?;
        let runnable =
            super::link::link_runnable(&program, entry, &objects, &toolchain, &scratch, Some(run))?;
        let launched = super::link::launch_live(
            &runnable,
            &target,
            &toolchain,
            &scratch,
            tape,
            super::link::LiveRun {
                run,
                inherit,
                stdout,
                stderr,
            },
        )?;
        let observation =
            super::link::observe_run(&launched, &program, &Sites::new(validated.snapshot()))?;
        Ok(observation)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}
