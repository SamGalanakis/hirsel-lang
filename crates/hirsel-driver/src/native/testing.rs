//! Compile one package test executable and launch each test in a fresh process.
use super::{Backend, NativeError, OptLevel, StageName, TargetSpec};
use crate::requests::{
    CompileOperation, CompileRequest, CompileResult, CompilerRequests, NativeExecution,
    NativeRequest,
};
use crate::testing::{BenchResult, Selection, Test, TestResult};
use hirsel_engine::runtime::QueryRuntime;
use hirsel_engine::{
    ToolError,
    requests::{OutputReadiness, RequestEngine},
};
use std::sync::Arc;
use std::{
    path::Path,
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
    let (discovery, mut selector) =
        crate::testing::discover_with_selector(path, filters, seed, Selection::Tests, verify)?;
    let tests = &discovery.tests;
    if tests.is_empty() {
        crate::testing::check_empty_selection(path, &discovery, &mut selector)?;
        return Ok(vec![]);
    }
    run(path, &discovery, tests, selector, backend, bless, schedules)
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
    let (discovery, mut selector) =
        crate::testing::discover_with_selector(path, filters, seed, Selection::Benches, verify)?;
    let benches = &discovery.tests;
    if benches.is_empty() {
        crate::testing::check_empty_selection(path, &discovery, &mut selector)?;
        return Ok(vec![]);
    }
    bench(path, &discovery, benches, selector, backend, target)
        .map_err(|e| ToolError::Runtime(e.to_string()))
}

/// One executable (or its image in a runner) whose argument selects which entry of `tests`
/// it launches.
struct Executable {
    execution: Arc<NativeExecution>,
    runnable: super::Runnable,
    scratch: super::executable::Scratch,
}

impl Executable {
    fn build(
        path: &Path,
        discovery: &crate::testing::TestDiscovery,
        tests: &[Test],
        mut selector: CompilerRequests,
        backend: Backend,
        optimization: OptLevel,
    ) -> Result<Self, NativeError> {
        let failed = |error: &dyn std::fmt::Display| {
            NativeError::new(StageName::Manifest, error.to_string())
        };
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let request = CompileRequest {
            path: path.to_owned(),
            inputs: discovery.inputs.clone(),
            operation: CompileOperation::NativeBuild(NativeRequest {
                definition: Some(
                    tests
                        .first()
                        .ok_or_else(|| failed(&"test selection is empty"))?
                        .entry
                        .clone(),
                ),
                test_entries: tests
                    .iter()
                    .map(|test| (test.entry.clone(), test.seed))
                    .collect(),
                supplemental_sources: discovery.sources.clone(),
                backend,
                profile: super::BuildProfile::Default,
                optimization: Some(optimization),
                output: crate::requests::NativeOutput::Executable,
                target,
                validation: super::Validation::EveryStage,
                launch: Default::default(),
                run: None,
                fuel: false,
                evidence: None,
            }),
        };
        let mut selected = selector.select(&request).map_err(|error| failed(&error))?;
        let complete = {
            let mut runtime = selected
                .runtime
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let request = runtime.request(&selected.roots);
            runtime.wait(request)
        };
        let complete = match complete {
            Ok(complete) => complete,
            Err(reference) => {
                return Err(failed(
                    &selected
                        .output
                        .failed(reference)
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| "native test compilation refused".into()),
                ));
            }
        };
        let output = selected
            .output
            .ready(&complete)
            .map_err(|error| failed(&error))?;
        let execution = match &output.result {
            CompileResult::Native(execution) => execution.clone(),
            CompileResult::NativeError(error) => return Err(error.clone()),
            _ => return Err(failed(&"test request completed another operation")),
        };
        let runnable = execution
            .platform
            .executable(execution.executable.clone())
            .map_err(|error| NativeError::new(StageName::Launch, error))?;
        Ok(Self {
            execution,
            runnable,
            scratch: super::executable::Scratch::new()?,
        })
    }

    /// Launches entry `index` under `scheduler` (the default without one): its failure text
    /// (None when it passed), its stdout and the launch's wall time.
    fn launch(
        &self,
        index: usize,
        scheduler: Option<&hirsel_run_schema::Scheduler>,
    ) -> Result<(Option<String>, Vec<u8>, Duration), NativeError> {
        let sites = super::Sites::from_anchors(
            &self.execution.sources,
            self.execution
                .observation
                .anchors
                .iter()
                .map(|(path, anchors)| (path, anchors.clone()))
                .collect(),
        );
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
            &self.execution.request.target,
            &self.execution.toolchain,
            &self.scratch.0.join("tooling"),
            &command,
            None,
        )?;
        let elapsed = start.elapsed();
        let launched = super::Launched {
            audit_required: false,
            exit: output.status.code(),
            signal: std::os::unix::process::ExitStatusExt::signal(&output.status),
            stdout: output.stdout,
            stderr: output.stderr,
            record: std::fs::read_to_string(&record).unwrap_or_default(),
        };
        let observed = super::link::observe_request(
            &launched,
            &self.execution.observation,
            &self.execution.sources,
        )
        .and_then(|observed| match observed {
            super::link::Observed::Ran(observation, _) => Ok(observation),
            super::link::Observed::Refused(reason) => Err(NativeError::new(
                StageName::Launch,
                format!("launch refused: {reason}"),
            )),
            super::link::Observed::Stopped(outcome) => Err(NativeError::new(
                StageName::Launch,
                format!("the run stopped: {outcome:?}"),
            )),
        });
        let failure = match observed {
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
    path: &Path,
    discovery: &crate::testing::TestDiscovery,
    tests: &[Test],
    selector: CompilerRequests,
    backend: Backend,
    bless: bool,
    schedules: &crate::testing::Schedules,
) -> Result<Vec<TestResult>, NativeError> {
    let executable = Executable::build(path, discovery, tests, selector, backend, OptLevel::None)?;
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
    path: &Path,
    discovery: &crate::testing::TestDiscovery,
    benches: &[Test],
    selector: CompilerRequests,
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
    let executable = Executable::build(
        path,
        discovery,
        &entries,
        selector,
        backend,
        OptLevel::Default,
    )?;
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
