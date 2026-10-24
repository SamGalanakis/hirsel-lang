//! Resource-only host execution diagnostics. This staged producer retains the
//! source outcome separately from trace refusal; it does not certify transports
//! that the reference observer cannot yet see.
use crate::{HostLaunch, RunOutcome, ToolError, session};
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
    let (outcome, _) = session::with_program(path, verify, |program| {
        crate::run_loaded_on_host_traced(
            program,
            launch,
            max_steps,
            None,
            None,
            checked,
            None,
            Some(&mut capture),
        )
    })?;
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
