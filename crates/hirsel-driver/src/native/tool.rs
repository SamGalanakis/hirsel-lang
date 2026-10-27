//! `hirsel run --native`: the checked program, optimized, run as an image in the host's
//! runner (or, when it calls C, as an executable).
use super::NativeError;
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

/// `run_path_native_with_config` with `backend`: LLVM builds at O2, Cranelift
/// unoptimized, for a fast debug loop.
pub fn run_path_native_backend(
    path: &Path,
    launch: crate::HostLaunch<'_>,
    evidence: Option<&super::Evidence>,
    backend: super::Backend,
    output: Output<'_>,
    verify: hirsel_engine::Verify,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    run_path_native_backend_with_validation(
        path,
        launch,
        evidence,
        backend,
        output,
        verify,
        super::Validation::Ready,
    )
}

/// Run with an explicit LIR validation policy. Harnesses use `EveryStage`.
pub fn run_path_native_backend_with_validation(
    path: &Path,
    launch: crate::HostLaunch<'_>,
    evidence: Option<&super::Evidence>,
    backend: super::Backend,
    output: Output<'_>,
    verify: hirsel_engine::Verify,
    validation: super::Validation,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    run_path_native_backend_with_trace_events(
        path, launch, evidence, backend, output, verify, validation, None,
    )
}

/// Native launch with a host-owned canonical Ordered trace destination. The
/// runtime validates qualification before entry and writes only complete evidence.
pub fn run_path_native_backend_with_trace_events(
    path: &Path,
    launch: crate::HostLaunch<'_>,
    evidence: Option<&super::Evidence>,
    backend: super::Backend,
    output: Output<'_>,
    verify: hirsel_engine::Verify,
    validation: super::Validation,
    trace_events: Option<&Path>,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    if matches!(launch, crate::HostLaunch::Callee { .. }) {
        return Err(ToolError::Runtime(
            "launch: a runner host runs its callees interpreted".into(),
        ));
    }
    let selection = crate::launch_binding::LaunchSelection::read(launch)?;
    let definition = selection.entry().cloned();
    crate::session::with_session(path, verify, |session| {
        // Source launch facts and native roots select exactly the same frozen
        // observations. Do not poll disk between binding and native selection.
        let selected_facts = session.request(crate::requests::CompileOperation::Tir {
            definition: definition.clone(),
        })?;
        let crate::requests::CompileResult::Tir(facts) = &selected_facts.result else {
            return Err(ToolError::Runtime(
                "native launch facts returned another operation".into(),
            ));
        };
        if selection.requires_replayable(launch) {
            if let Some((user, extern_)) = facts.foreign.unmodeled.first_key_value() {
                return Err(ToolError::Runtime(format!(
                    "launch: {user} calls the extern {extern_} and is not a wrapper with a \
                     deterministic model, so this run cannot be recorded or replayed (PROG-4)"
                )));
            }
        }
        let bound = selection.bind(
            launch,
            &facts.runtime.types,
            &facts.signature,
            &facts.revision,
            facts.code.clone(),
        )?;
        let revision = facts.revision.digest();
        // Binding owns the compact launch record. Release the completed TIR
        // closure before the native request admits its own source graph.
        drop(selected_facts);
        let run = match &bound {
            crate::BoundHostRun::Live(bound) => bound.run(),
            crate::BoundHostRun::Replay { run, .. } => run,
        };
        let selected = session.request_applied(crate::requests::CompileOperation::NativeRun(
            crate::requests::NativeRequest {
                definition,
                backend,
                profile: super::BuildProfile::Default,
                target: super::TargetSpec::x86_64_unknown_linux_gnu(),
                validation,
                launch: run.record().inputs.clone(),
                run: Some(std::sync::Arc::new(run.clone())),
                fuel: run.budgets().is_some_and(|budgets| budgets.fuel.is_some()),
                evidence: evidence.cloned().map(std::sync::Arc::new),
                test_entries: vec![],
                supplemental_sources: vec![],
                output: crate::requests::NativeOutput::Executable,
                optimization: None,
            },
        ))?;
        match &selected.result {
            crate::requests::CompileResult::Native(input) => {
                // The launch was bound before compilation against these exact
                // observations; the compiler cannot substitute another revision.
                if input.revision.digest() != revision {
                    return Err(ToolError::Runtime(
                        "native launch revision changed during selection".into(),
                    ));
                }
                super::request_output::run_bound_output(input, launch, bound, output, trace_events)
            }
            crate::requests::CompileResult::NativeError(error) => Ok(Err(error.clone())),
            _ => Err(ToolError::Runtime(
                "native run request returned another operation".into(),
            )),
        }
    })
}

#[cfg(test)]
#[path = "concurrent_tests.rs"]
mod concurrent_tests;

#[cfg(test)]
pub(super) fn before_request_link(path: &Path) {
    concurrent_tests::before_link(path);
}
