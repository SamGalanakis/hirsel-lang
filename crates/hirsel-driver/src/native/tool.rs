//! `hirsel run --native`: the checked program as an optimized host executable.
use super::{
    NativeError, Observation, OptLevel, Options, Partition, Sites, StageName, TargetSpec,
    Toolchain, build, observe,
};
use hirsel_engine::{RunOutcome, ToolError};
use std::io::Write;
use std::path::Path;

/// Output destinations for a user launch. Inheriting the two OS streams preserves
/// the program's cross-stream write order; custom writers receive each stream.
pub enum Output<'a> {
    Inherit,
    Streams {
        stdout: &'a mut dyn Write,
        stderr: &'a mut dyn Write,
    },
}

/// Check `path` exactly as `run` does (the same first static diagnostics),
/// then build it for x86-64 Linux with every optimization, link it against
/// the in-tree runtime and launch it. The executable's output is written to
/// `stdout` and its failure chain to `stderr`.
///
/// The outer error is a source or tool error of the checker; the inner one is
/// a native tool outcome (a stage does not support the program yet), which is
/// never a verdict about the source.
pub fn run_path_native(
    path: &Path,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    run_path_native_with_config(path, None, None, Output::Streams { stdout, stderr })
}

/// `run_path_native` with a host launch and PS-12 evidence; the proof report goes to
/// `stderr` before the program runs. Record and replay pass to the executable as
/// `--hirsel-record` and `--hirsel-replay`; the driver binds the run first, so a
/// recording of other code refuses before building.
pub fn run_path_native_with_config(
    path: &Path,
    launch: Option<crate::HostLaunch<'_>>,
    evidence: Option<&super::Evidence>,
    output: Output<'_>,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    let (mut host_stdout, mut host_stderr) = (std::io::stdout(), std::io::stderr());
    let (inherit, stdout, stderr): (bool, &mut dyn Write, &mut dyn Write) = match output {
        Output::Inherit => (true, &mut host_stdout, &mut host_stderr),
        Output::Streams { stdout, stderr } => (false, stdout, stderr),
    };
    let mut loaded = crate::load_program(path)?;
    loaded.check_for_execution()?;
    if matches!(
        launch,
        Some(
            crate::HostLaunch::Replay(_)
                | crate::HostLaunch::Live {
                    record: Some(_),
                    ..
                }
        )
    ) {
        loaded.refuse_unreplayable()?;
    }
    let host = hirsel_rt::host_launch::HostGrants::new(std::io::sink());
    let host = match launch {
        Some(crate::HostLaunch::Live {
            config,
            record,
            revocations,
        }) => {
            let live = crate::HostLaunch::Live {
                config,
                record: None,
                revocations,
            };
            let bound = crate::bind_host_run(&mut loaded, live, host)?;
            Some((
                bound.run().clone(),
                super::link::Tape::Live {
                    record,
                    revocations,
                },
            ))
        }
        Some(launch @ crate::HostLaunch::Replay(tape)) => {
            let bound = crate::bind_host_run(&mut loaded, launch, host)?;
            Some((bound.run().clone(), super::link::Tape::Replay(tape)))
        }
        None => None,
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
    Ok(run(
        &validated,
        &entry,
        host.as_ref().map(|(run, tape)| (run, *tape)),
        proofs.as_ref(),
        inherit,
        stdout,
        stderr,
    )
    .and_then(|observation| {
        let io = |e: std::io::Error| NativeError::new(StageName::Launch, e.to_string());

        for failure in &observation.failures {
            writeln!(
                stderr,
                "runtime failure[{}] at line {}",
                failure.rule, failure.line
            )
            .map_err(io)?;
        }
        Ok(
            match (observation.exit_code, observation.failures.first()) {
                (Some(code), _) => RunOutcome::Exit(code),
                (None, Some(failure)) => RunOutcome::Failure {
                    rule: hirsel_diag::RuleId::new(&failure.rule)
                        .map_err(|_| NativeError::new(StageName::Launch, "a failure of no rule"))?,
                    line: failure.line as usize,
                },
                (None, None) => {
                    return Err(NativeError::new(
                        StageName::Launch,
                        "neither an exit code nor a failure",
                    ));
                }
            },
        )
    }))
}

fn run(
    validated: &hirsel_engine::ValidatedNativeInput,
    entry: &hirsel_engine::DefPath,
    host: Option<(&hirsel_run_config::ValidatedRun, super::link::Tape<'_>)>,
    proofs: Option<&super::Proofs>,
    inherit: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<Observation, NativeError> {
    let run = host.map(|(run, _)| run);
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut options = Options::new(target.clone());
    if let Some(proofs) = proofs {
        options.discharge = proofs.discharge.clone();
    }
    let program = build(validated, &options)?;
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
        let objects =
            super::link::emit_objects(&program, OptLevel::Speed, Partition::Whole, false, None)?;
        let executable = super::link::link_objects_with_run(
            &program, entry, &objects, &toolchain, &scratch, run,
        )?;
        let launched = if host.is_some() || inherit {
            let tape = host
                .map(|(_, tape)| tape)
                .unwrap_or(super::link::Tape::Live {
                    record: None,
                    revocations: None,
                });
            super::link::launch_live(
                &executable,
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
            )?
        } else {
            let launched = super::link::launch(&executable, &target, &toolchain, &scratch, false)?;
            stdout
                .write_all(&launched.stdout)
                .and_then(|()| stdout.flush())
                .map_err(io)?;
            stderr.write_all(&launched.stderr).map_err(io)?;
            launched
        };
        let observation = observe(&launched, &program, &Sites::new(validated.snapshot()))?;
        Ok(observation)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}
