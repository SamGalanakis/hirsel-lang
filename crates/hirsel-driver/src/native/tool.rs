//! `hirsel run --native`: the checked program as an optimized host executable.
use super::{
    NativeError, Observation, OptLevel, Options, Partition, Sites, StageName, TargetSpec,
    Toolchain, build, launch, link, observe,
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
    let mut loaded = crate::load_program(path)?;
    loaded.check_for_execution()?;
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
    Ok(run(&validated, &entry).and_then(|(observation, launched)| {
        let io = |e: std::io::Error| NativeError::new(StageName::Launch, e.to_string());
        stdout.write_all(&launched).map_err(io)?;
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
) -> Result<(Observation, Vec<u8>), NativeError> {
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
        let executable = link(
            &program,
            entry,
            OptLevel::Speed,
            Partition::Whole,
            &toolchain,
            &scratch,
        )?;
        let launched = launch(&executable, &target, &toolchain, &scratch, false)?;
        let observation = observe(&launched, &program, &Sites::new(validated.snapshot()))?;
        Ok((observation, launched.stdout))
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}
