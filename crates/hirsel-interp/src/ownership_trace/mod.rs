//! Opt-in resource observation for the reference executor. Ordinary execution
//! monomorphizes the same evaluator with `NoTrace`; its observer calls disappear.
//! This initial adapter refuses unsupported transports instead of certifying a
//! partial lineage. It is not yet the CLI's complete owned-value trace producer.
use crate::frame::Frame;
use crate::{Execution, ExecutionError, Limits, Operations, Value};
use hirsel_span::{SourceMap, Span};
use hirsel_tir::{InstructionKind, PlaceId, RunConfiguration, Terminator, TirProvider};

mod closure_support;
mod reference;

pub(crate) trait Observer {
    const ENABLED: bool;
    #[inline(always)]
    fn run(&mut self, _index: usize) {}
    #[inline(always)]
    fn sync(&mut self, _task: u64, _frames: &[Frame], _sources: &SourceMap) {}
    #[inline(always)]
    fn task_started(&mut self, _parent: u64, _child: u64, _path: Option<Vec<u32>>, _frame: &Frame) {
    }
    #[inline(always)]
    fn before_step(
        &mut self,
        _task: u64,
        _frame: &Frame,
        _instruction: Option<&InstructionKind>,
        _terminator: &Terminator,
        _site: Span,
        _sources: &SourceMap,
    ) {
    }
    #[inline(always)]
    fn after_step(&mut self, _task: u64, _frame: &Frame, _success: bool) {}
    #[inline(always)]
    fn call_entered(&mut self, _task: u64, _caller: &Frame, _callee: &Frame) {}
    #[inline(always)]
    fn callback_entered(&mut self, _task: u64, _caller: &Frame, _callee: &Frame) {}
    #[inline(always)]
    fn builtin_entered(
        &mut self,
        _task: u64,
        _frame: &Frame,
        _arguments: &[Value],
        _result: &crate::OperationResult,
        _channel: Option<u64>,
    ) {
    }
    #[inline(always)]
    fn builtin_completed(&mut self, _task: u64, _frame: &Frame, _result: &crate::OperationResult) {}
    #[inline(always)]
    fn before_handle_drop(&mut self, _frame: &Frame, _kind: &str, _id: u64, _channel: Option<u64>) {
    }
    #[inline(always)]
    fn handle_released(
        &mut self,
        _task: u64,
        _frame: &Frame,
        _values: &[Value],
        _anchor: &crate::observation::Anchor,
        _sources: &SourceMap,
    ) {
    }
    #[inline(always)]
    fn callback_released(
        &mut self,
        _frame: &Frame,
        _values: &[Value],
        _abandoned: bool,
        _anchor: &crate::observation::Anchor,
        _sources: &SourceMap,
    ) {
    }
    #[inline(always)]
    fn builtin_returned(&mut self, _task: u64, _frame: &Frame, _value: &Value) {}
    #[inline(always)]
    fn return_discarded(&mut self, _task: u64, _frame: &Frame, _value: &Value) {}
    #[inline(always)]
    fn rest_entered(&mut self, _task: u64, _frame: &Frame) {}
    #[inline(always)]
    fn drop_place(
        &mut self,
        _task: u64,
        _frame: &Frame,
        _place: PlaceId,
        _value: &Value,
        _site: Span,
        _sources: &SourceMap,
    ) {
    }
    #[inline(always)]
    fn released_without_finalizer(
        &mut self,
        _task: u64,
        _frame: &Frame,
        _value: &Value,
        _site: Span,
        _sources: &SourceMap,
    ) {
    }
    #[inline(always)]
    fn finish_frame(
        &mut self,
        _task: u64,
        _frame: &Frame,
        _ending: &crate::resources::Ending,
        _sources: &SourceMap,
    ) {
    }
    #[inline(always)]
    fn forgotten(&mut self, _task: u64, _frame: &Frame, _site: Span, _sources: &SourceMap) {}
}
pub(crate) struct NoTrace;
impl Observer for NoTrace {
    const ENABLED: bool = false;
}

#[derive(Debug)]
pub enum TraceError {
    Unsupported(String),
    Consistency(hirsel_run_schema::ownership_trace::Diagnostic),
}
impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(message) => write!(f, "ownership trace unavailable: {message}"),
            Self::Consistency(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for TraceError {}

/// Semantic binding and diagnostic bound supplied by an admitted consumer.
#[derive(Clone, Copy, Debug)]
pub struct ResourceBinding {
    pub program: [u8; 32],
    pub event_limit: usize,
}

pub struct ResourceRequest<'a> {
    pub configuration: &'a RunConfiguration,
    pub program: [u8; 32],
    pub event_limit: usize,
}

pub struct ResourceExecution {
    pub execution: Execution,
    pub trace: Result<hirsel_run_schema::ownership_trace::Trace, TraceError>,
}

/// Observe supported resource transitions in an actual reference execution.
/// Tracing errors are separate from the language outcome; unsupported paths
/// cannot produce a complete trace. General owned-value/transport support is
/// deliberately not claimed by this staged resource-only API.
pub fn run_resources(
    provider: &mut (impl TirProvider + ?Sized),
    sources: &SourceMap,
    request: ResourceRequest<'_>,
    arguments: Vec<Value>,
    limits: Limits,
    operations: &mut impl Operations,
    runtime: &crate::aggregate::RuntimeTypes,
) -> Result<ResourceExecution, ExecutionError> {
    run_selected::<false>(
        provider, sources, request, arguments, limits, operations, runtime,
    )
}

/// Observe Resources and owned String/Bytes leaves through their actual moves
/// and releases. Copies and unobserved library producers refuse the trace.
/// The source outcome and the ordinary execution limits remain independent.
pub fn run_owned(
    provider: &mut (impl TirProvider + ?Sized),
    sources: &SourceMap,
    request: ResourceRequest<'_>,
    arguments: Vec<Value>,
    limits: Limits,
    operations: &mut impl Operations,
    runtime: &crate::aggregate::RuntimeTypes,
) -> Result<ResourceExecution, ExecutionError> {
    run_selected::<true>(
        provider, sources, request, arguments, limits, operations, runtime,
    )
}

fn run_selected<const OWNED: bool>(
    provider: &mut (impl TirProvider + ?Sized),
    sources: &SourceMap,
    request: ResourceRequest<'_>,
    arguments: Vec<Value>,
    limits: Limits,
    operations: &mut impl Operations,
    runtime: &crate::aggregate::RuntimeTypes,
) -> Result<ResourceExecution, ExecutionError> {
    let mut observer =
        reference::Resources::<OWNED>::new(runtime.forget.clone(), request.event_limit);
    let execution = crate::eval::run_observed(
        provider,
        sources,
        request.configuration,
        arguments,
        limits,
        operations,
        runtime,
        &mut observer,
    )?;
    let complete = matches!(
        execution.outcome,
        crate::Outcome::Returned { .. } | crate::Outcome::Failed(_)
    );
    let trace = observer.finish(request.program, complete);
    Ok(ResourceExecution { execution, trace })
}
