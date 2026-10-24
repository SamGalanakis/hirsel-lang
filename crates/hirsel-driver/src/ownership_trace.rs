//! Resource-only host execution diagnostics. This staged producer retains the
//! source outcome separately from trace refusal; it does not certify transports
//! that the reference observer cannot yet see.
use crate::{HostLaunch, RunOutcome, ToolError};
use hirsel_interp::ownership_trace::TraceError;
use hirsel_run_schema::ownership_trace::Trace;
use std::path::Path;

pub struct ResourceRun {
    pub outcome: RunOutcome,
    /// Semantic program identity bound by the admitted host run.
    pub program: [u8; 32],
    pub trace: Result<Trace, TraceError>,
}

pub(crate) struct Capture {
    pub event_limit: usize,
    pub program: Option<[u8; 32]>,
    pub trace: Option<Result<Trace, TraceError>>,
}

/// Run the actual admitted source under reference execution or checked H.
/// Unsupported resource transitions return a trace error alongside the original
/// source outcome. They cannot turn the language run into a success or failure.
pub fn run_resources_on_host(
    path: &Path,
    launch: HostLaunch<'_>,
    max_steps: Option<u64>,
    verify: hirsel_engine::Verify,
    checked: bool,
    event_limit: usize,
) -> Result<ResourceRun, ToolError> {
    let mut capture = Capture {
        event_limit,
        program: None,
        trace: None,
    };
    let selection = crate::launch_binding::LaunchSelection::read(launch)?;
    let input = crate::request_tir(path, verify, selection.entry().cloned())?;
    let (outcome, _) = crate::tir_execution::run_tir_on_host_traced(
        &input, launch, selection, max_steps, None, None, checked, None,
        Some(&mut capture),
    )?;
    Ok(ResourceRun {
        outcome,
        program: capture
            .program
            .ok_or_else(|| ToolError::Runtime("resource trace has no admitted program".into()))?,
        trace: capture.trace.ok_or_else(|| {
            ToolError::Runtime("resource trace execution did not complete".into())
        })?,
    })
}

pub struct ResourceSimulation {
    pub execution: hirsel_engine::ObservedExecution,
    pub program: [u8; 32],
    pub trace: Result<Trace, TraceError>,
}

/// Execute one discovered source test under the ordinary simulator. Resource
/// diagnostics use that generated entry's checked semantic closure and never
/// become part of its semantic observation or its capability tape.
pub fn execute_test_resources(
    program: &mut hirsel_engine::LoadedProgram,
    test: &crate::testing::Test,
    checked: bool,
    hash_seed: u64,
    limits: hirsel_interp::Limits,
    event_limit: usize,
) -> Result<ResourceSimulation, ToolError> {
    let binding = program
        .engine
        .native_semantic_program([test.entry.clone()])
        .and_then(|semantic| semantic.semantic_program_revision(&test.entry))
        .map_err(|error| program.error(error))?
        .digest();
    let previous =
        program
            .engine
            .adapters
            .interpreter
            .replace(Box::new(hirsel_engine::InterpreterAdapter {
                limits,
                resource_binding: Some(hirsel_interp::ownership_trace::ResourceBinding {
                    program: binding,
                    event_limit,
                }),
                ..Default::default()
            }));
    let execution = crate::testing::execute(program, test, checked, hash_seed);
    let trace = program
        .engine
        .adapters
        .interpreter
        .as_mut()
        .and_then(|adapter| adapter.take_resource_trace())
        .unwrap_or_else(|| {
            Err(TraceError::Unsupported(
                "simulation did not complete ownership observations".into(),
            ))
        });
    program.engine.adapters.interpreter = previous;
    Ok(ResourceSimulation {
        execution: execution?,
        program: binding,
        trace,
    })
}

pub struct TestResources {
    pub result: crate::testing::TestResult,
    pub program: [u8; 32],
    pub trace: Result<Trace, TraceError>,
}

/// `hirsel test --trace-ownership`: each selected test runs once under the
/// ordinary simulator and is judged exactly as an untraced run judges it.
pub fn test_path_resources(
    path: &Path,
    filters: &[String],
    override_seed: Option<u64>,
    max_steps: Option<u64>,
    verify: hirsel_engine::Verify,
    event_limit: usize,
) -> Result<Vec<TestResources>, ToolError> {
    use crate::testing::{Selection, discover, outcome_failure};
    let mut program = crate::COMPILER
        .with_verify(verify)
        .load_test_package(path)?;
    let tests = discover(&mut program, filters, override_seed, Selection::Tests)?;
    let limits = hirsel_interp::Limits {
        steps: max_steps,
        ..Default::default()
    };
    let mut results = vec![];
    for test in &tests {
        let traced = execute_test_resources(&mut program, test, false, 0, limits, event_limit)?;
        let stdout = match &traced.execution.observation {
            Some(observation) => observation.output.clone(),
            None => traced.execution.tooling_output.clone(),
        };
        let failure = match &traced.execution.outcome {
            Ok(outcome) => outcome_failure(outcome, &program.engine.sources),
            Err(error) => Some(format!("runtime error: {error:?}")),
        };
        results.push(TestResources {
            result: test.result(failure, &stdout, false),
            program: traced.program,
            trace: traced.trace,
        });
    }
    Ok(results)
}
