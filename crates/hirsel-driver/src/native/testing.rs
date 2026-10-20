//! Compile one package test executable and launch each test in a fresh process.
use super::{Backend, NativeError, OptLevel, Options, Partition, StageName, TargetSpec, Toolchain};
use crate::testing::{BenchResult, Selection, Test, TestResult};
use hirsel_engine::{LoadedProgram, NativeInput, ToolError};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub fn test_path(
    path: &Path,
    filters: &[String],
    seed: Option<u64>,
    backend: Backend,
    bless: bool,
    schedules: &crate::testing::Schedules,
    verify: hirsel_engine::Verify,
) -> Result<Vec<TestResult>, ToolError> {
    let mut loaded = crate::COMPILER
        .with_verify(verify)
        .load_test_package(path)?;
    let tests = crate::testing::discover(&mut loaded, filters, seed, Selection::Tests)?;
    if tests.is_empty() {
        return Ok(vec![]);
    }
    run(&mut loaded, &tests, backend, bless, schedules)
        .map_err(|e| ToolError::Runtime(e.to_string()))
}

/// Benches build at the ordinary executable optimization level and time each launch of
/// a loop entry from outside the process.
pub fn bench_path(
    path: &Path,
    filters: &[String],
    seed: Option<u64>,
    backend: Backend,
    target: Duration,
    verify: hirsel_engine::Verify,
) -> Result<Vec<BenchResult>, ToolError> {
    let mut loaded = crate::COMPILER
        .with_verify(verify)
        .load_test_package(path)?;
    let benches = crate::testing::discover(&mut loaded, filters, seed, Selection::Benches)?;
    if benches.is_empty() {
        return Ok(vec![]);
    }
    bench(&mut loaded, &benches, backend, target).map_err(|e| ToolError::Runtime(e.to_string()))
}

/// One executable (or its image in a runner) whose argument selects which entry of `tests`
/// it launches.
struct Executable {
    input: NativeInput,
    program: super::ReadyProgram,
    runnable: super::Runnable,
    toolchain: Toolchain,
    scratch: super::executable::Scratch,
}

impl Executable {
    fn build(
        loaded: &mut LoadedProgram,
        tests: &[Test],
        backend: Backend,
        optimization: OptLevel,
    ) -> Result<Self, NativeError> {
        let input = loaded
            .engine
            .export_native_input()
            .map_err(|e| NativeError::new(StageName::Manifest, format!("{e:?}")))?;
        let validated = loaded
            .engine
            .import_native_input(input.clone())
            .map_err(|e| NativeError::new(StageName::Manifest, format!("{e:?}")))?;
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut program = super::build(&validated, &Options::new(target.clone()))?;
        let scratch = super::executable::Scratch::new()?;
        let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(super::runtime_cache::default_directory);
        let tools = Toolchain::in_tree(&cache, &[&target])?;
        let objects = super::emit_backend_objects(
            &program,
            backend,
            optimization,
            Partition::Whole,
            false,
            None,
        )?;
        let runnable = super::link::link_tests(&mut program, tests, &objects, &tools, &scratch.0)?;
        Ok(Self {
            input,
            program,
            runnable,
            toolchain: tools,
            scratch,
        })
    }

    /// Launches entry `index` under `scheduler` (the default without one): its failure text
    /// (None when it passed), its stdout and the launch's wall time.
    fn launch(
        &self,
        index: usize,
        scheduler: Option<&hirsel_run_schema::Scheduler>,
    ) -> Result<(Option<String>, Vec<u8>, Duration), NativeError> {
        let sites = super::Sites::new(&self.input);
        let record = self.scratch.0.join("observation");
        let _ = std::fs::remove_file(&record);
        let start = Instant::now();
        let mut command = Command::new("tests");
        command
            .arg(index.to_string())
            .env_clear()
            .env("HIRSEL_RT_OBSERVATION", &record);
        if let Some(scheduler) = scheduler {
            command.env("HIRSEL_RT_SCHEDULER", scheduler.to_string());
        }
        let output = super::link::run_bounded(
            &self.runnable,
            &self.program.target,
            &self.toolchain,
            &self.scratch.0.join("tooling"),
            &command,
            None,
        )?;
        let elapsed = start.elapsed();
        let launched = super::Launched {
            audit_required: false,
            exit: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
            record: std::fs::read_to_string(&record).unwrap_or_default(),
        };
        let failure = match super::observe(&launched, &self.program, &sites) {
            Ok(observation) if observation.exit_code == Some(0) => None,
            Ok(observation) if observation.exit_code == Some(1) => Some("returned Error".into()),
            Ok(observation) => Some(match observation.failures.first() {
                Some(failure) => sites.report(failure, false)?,
                None => {
                    let waits = String::from_utf8_lossy(&launched.stderr);
                    format!("runtime failure: blocked{}", waits.trim_end())
                }
            }),
            Err(error) => Some(format!("runtime error: {error}")),
        };
        Ok((failure, launched.stdout, elapsed))
    }
}

fn run(
    loaded: &mut LoadedProgram,
    tests: &[Test],
    backend: Backend,
    bless: bool,
    schedules: &crate::testing::Schedules,
) -> Result<Vec<TestResult>, NativeError> {
    let executable = Executable::build(loaded, tests, backend, OptLevel::None)?;
    let mut results = Vec::new();
    for (index, test) in tests.iter().enumerate() {
        let mut error = None;
        let result = crate::testing::explore(test, schedules, |launch| {
            match executable.launch(index, launch.scheduler.as_ref()) {
                Ok((failure, stdout, _)) => launch.result(failure, &stdout, bless),
                Err(e) => {
                    error = Some(e);
                    launch.result(Some(String::new()), &[], false)
                }
            }
        });
        if let Some(error) = error {
            return Err(error);
        }
        results.push(result);
    }
    Ok(results)
}

fn bench(
    loaded: &mut LoadedProgram,
    benches: &[Test],
    backend: Backend,
    target: Duration,
) -> Result<Vec<BenchResult>, NativeError> {
    // Every loop entry of every bench is its own launchable test.
    let entries: Vec<Test> = benches
        .iter()
        .flat_map(|bench| {
            bench.loops.iter().map(|(_, entry)| Test {
                entry: entry.clone(),
                loops: vec![],
                ..bench.clone()
            })
        })
        .collect();
    let executable = Executable::build(loaded, &entries, backend, OptLevel::Default)?;
    let mut results = Vec::new();
    for (ordinal, bench) in benches.iter().enumerate() {
        let first = ordinal * bench.loops.len();
        let mut error = None;
        let measured = crate::testing::calibrate(target, |index| {
            match executable.launch(first + index, None) {
                Ok((None, _, elapsed)) => Ok(elapsed),
                Ok((Some(failure), _, _)) => Err(failure),
                Err(e) => {
                    error = Some(e);
                    Err(String::new())
                }
            }
        });
        if let Some(error) = error {
            return Err(error);
        }
        results.push(bench.bench_result(measured));
    }
    Ok(results)
}
