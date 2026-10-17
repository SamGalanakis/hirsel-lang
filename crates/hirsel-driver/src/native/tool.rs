//! `hirsel run --native`: the checked program as an optimized host executable.
use super::{
    NativeError, Observation, OptLevel, Options, Partition, Sites, StageName, TargetSpec,
    Toolchain, build, observe,
};
use hirsel_engine::{RunOutcome, ToolError};
use std::io::Write;
use std::path::Path;

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
    run_path_native_with_config(path, None, stdout, stderr)
}

pub fn run_path_native_with_config(
    path: &Path,
    record: Option<crate::LaunchConfig<'_>>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    let mut loaded = crate::load_program(path)?;
    loaded.check_for_execution()?;
    let admitted = if let Some(record) = record {
        let signature = crate::launch_signature(&mut loaded)?;
        let revision = loaded
            .engine
            .semantic_program_revision(&loaded.entry)
            .map_err(|e| loaded.error(e))?;
        let requested = record
            .read(&revision)
            .map_err(|e| ToolError::Runtime(e.to_string()))?;
        let mut host = hirsel_rt::host_launch::HostGrants::new(std::io::sink());
        Some(
            hirsel_engine::admit_run(
                &signature,
                &revision,
                hirsel_run_config::LaunchKind::Main,
                Some(&requested),
                &mut host,
                crate::admission_time()?,
            )
            .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?,
        )
    } else {
        None
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
    Ok(run(
        &validated,
        &entry,
        admitted.as_ref().map(|run| run.run()),
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
    run: Option<&hirsel_run_config::ValidatedRun>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<Observation, NativeError> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let program = build(validated, &Options::new(target.clone()))?;
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
        let launched = if let Some(run) = run {
            super::link::launch_live(
                &executable,
                &target,
                &toolchain,
                &scratch,
                run,
                stdout,
                stderr,
            )?
        } else {
            let launched = super::link::launch(&executable, &target, &toolchain, &scratch, false)?;
            stdout.write_all(&launched.stdout).map_err(io)?;
            launched
        };
        let observation = observe(&launched, &program, &Sites::new(validated.snapshot()))?;
        Ok(observation)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}
