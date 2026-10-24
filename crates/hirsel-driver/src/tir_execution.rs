//! Host TIR execution over one immutable compiler-request result.
//! The runtime provider reads only settled products; execution owns all live
//! host effects, admission, recording, cancellation and budgets.
use crate::launch_binding::LaunchSelection;
use crate::requests::TirExecution;
use crate::{HostLaunch, RunOutcome, ToolError, call_schemas, network};
use std::path::Path;

pub(crate) fn run_tir_on_host(
    input: &TirExecution,
    launch: HostLaunch<'_>,
    selection: LaunchSelection,
    max_steps: Option<u64>,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
    cancel: Option<network::ExternalCancel>,
    checked: bool,
) -> Result<(RunOutcome, Option<hirsel_run_schema::CalleeOutcome>), ToolError> {
    if let Some(extern_) = input.foreign.externs.first() {
        return Err(ToolError::Runtime(format!(
            "launch: the reference interpreter does not call C, and {extern_} is an \
             extern \"c\" function; build or run the program natively (PROG-4)"
        )));
    }
    if selection.requires_replayable(launch) {
        if let Some((user, extern_)) = input.foreign.unmodeled.first_key_value() {
            return Err(ToolError::Runtime(format!(
                "launch: {user} calls the extern {extern_} and is not a wrapper with a \
                 deterministic model, so this run cannot be recorded or replayed (PROG-4)"
            )));
        }
    }
    let host = hirsel_rt::host_launch::HostGrants::cooperative(hirsel_rt::launch::RawStdout);
    let tape = match launch {
        HostLaunch::Live { record, .. } | HostLaunch::Callee { record, .. } => {
            record.map(Path::to_path_buf)
        }
        HostLaunch::Replay(_) => None,
    };
    let (run, io, instance) = match crate::admit_bound_host_run(
        selection.bind(
            launch,
            &input.runtime.types,
            &input.signature,
            &input.revision,
            input.code.clone(),
        )?,
        launch,
        host,
    )? {
        HostRun::Live { admitted, io } => (
            admitted.run().clone(),
            network::HostIo::Live(io),
            admitted.instance(),
        ),
        // A replay parks where the recording parked (its `blocked` and `ready` events).
        HostRun::Replay { run, io } => {
            let instance = io.instance();
            (run, network::HostIo::Replay(io), instance)
        }
    };
    // A callee run's replay entered its callee entry (`bind_launch`).
    let signature = input.signature.clone();
    let call = run.record().call.clone();
    let tir_run = hirsel_tir::RunConfiguration {
        entry: input.entry.clone(),
        hash_seed: run.record().inputs.hash_seed,
        inputs: vec![],
    };
    let mut runtime = (*input.runtime).clone();
    runtime.checked = checked.then(|| (*input.checked_runtime).clone());
    runtime.trace_mode = hirsel_interp::TraceMode::Effects;
    // A replay re-applies the recorded budgets, so it stops where the run stopped.
    if let Some(budgets) = run.budgets() {
        runtime.budgets = hirsel_interp::RunBudgets {
            fuel: budgets.fuel,
            memory_bytes: budgets.memory_bytes,
        };
    }
    let mut operations = network::HostLibrary::new(&tir_run, &run, io)
        .map_err(|e| ToolError::Runtime(format!("host launch: {e:?}")))?;
    if observed.is_some() {
        operations.silence_replay_output();
    }
    let call_schemas = match &call {
        Some(_) => Some(call_schemas(&signature, &runtime)?),
        None => None,
    };
    if run.calls() || call.is_some() {
        let callees = input.callees.clone();
        // A live run's remote runners open their providers now, reading their TLS material.
        let remote = match &operations.io {
            network::HostIo::Live(_) => network::RemoteRunners::open(
                run.record()
                    .io
                    .as_ref()
                    .map_or(&[][..], |io| io.runners.as_slice()),
                instance.unwrap_or_default(),
                || network::remote_wire(&runtime, hirsel_remote::wire::DEFAULT_FRAME_BYTES),
            )
            .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?,
            network::HostIo::Replay(_) => None,
        };
        operations.set_runners(network::RunnerContext {
            instance,
            callees: std::sync::Arc::new(callees),
            tape,
            role: call_schemas.as_ref().map(|(schemas, _)| schemas.clone()),
            remote,
        });
    }
    if let Some(cancel) = cancel {
        operations
            .set_external_cancel(cancel)
            .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
    }
    // F0's free-mode test channel (`hirsel_free::session::TestHook`).
    let free = hirsel_free::session::TestHook::from_env().map_err(ToolError::Runtime)?;
    if let Some(hook) = &free {
        let session = hook.session(&run.identity()).map_err(ToolError::Runtime)?;
        operations.library.enable_free(session, hook.seed);
    }
    let args = match (&call, &call_schemas) {
        // A callee run enters with its place's Io and the argument its call carries.
        (Some(call), Some((_, argument))) => {
            let bytes = call
                .argument()
                .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
            let value = hirsel_interp::library::runners::decode(argument, &bytes, &runtime)
                .map_err(|e| ToolError::Runtime(format!("launch: the call's argument: {e:?}")))?;
            match call.form() {
                hirsel_run_schema::CallForm::Call => vec![operations.library.root_io(), value],
                hirsel_run_schema::CallForm::Compute => vec![value],
            }
        }
        _ => operations
            .library
            .entry_arguments(&runtime.types, &signature)
            .map_err(|e| ToolError::Runtime(format!("launch: {e:?}")))?,
    };
    let sources = input.sources.clone();
    let mut provider = input.provider();
    let execution = hirsel_interp::run_with_runtime(
        &mut provider,
        &sources,
        &tir_run,
        args,
        hirsel_interp::Limits {
            steps: max_steps,
            call_depth: 256,
        },
        &mut operations,
        &runtime,
    );
    if let Some(hook) = &free
        && let Some(session) = operations.library.take_free()
    {
        let outcome = match &execution {
            Ok(execution) => hirsel_interp::library::free_outcome_name(&execution.outcome),
            Err(_) => "stopped",
        };
        hook.finish(session, outcome).map_err(ToolError::Runtime)?;
    }
    let replay = matches!(operations.io, network::HostIo::Replay(_));
    if let Some(refusal) = operations.refused.take() {
        return Err(ToolError::Runtime(if replay {
            format!("replay refused: {refusal}")
        } else {
            format!("host Io: {refusal}")
        }));
    }
    let execution = execution.map_err(|e| match e.error {
        hirsel_interp::Error::Unavailable(reason) => {
            ToolError::Runtime(format!("interpretation unavailable: {reason}"))
        }
        hirsel_interp::Error::Query(error) => ToolError::Query {
            error,
            phase: Some(hirsel_engine::QueryKind::Interpret),
            sources: sources.clone(),
        },
        error => ToolError::Runtime(format!("interpret: {error:?}")),
    })?;
    // A callee run records its own outcome as it ended (RUN-11); replay checks it.
    let end = match (&call, &execution.outcome) {
        (Some(_), hirsel_interp::Outcome::Returned { value, .. }) => {
            Some(hirsel_interp::CalleeEnd::Returned(value.clone()))
        }
        (Some(_), hirsel_interp::Outcome::Failed(_)) => {
            Some(hirsel_interp::CalleeEnd::Failed(execution.failures.clone()))
        }
        _ => None,
    };
    let mut returned = None;
    if let Some(end) = end {
        let (mut stdout, mut trace) = (vec![], vec![]);
        let mut cx = hirsel_interp::OperationContext {
            configuration: &tir_run,
            sources: &sources,
            stdout: &mut stdout,
            trace: &mut trace,
            trace_mode: runtime.trace_mode,
            anchor: None,
        };
        returned = operations.callee_return(&end, &mut cx).map_err(|e| {
            ToolError::Runtime(match operations.refused.take() {
                Some(refusal) if replay => format!("replay refused: {refusal}"),
                _ => format!("the callee's outcome: {e:?}"),
            })
        })?;
    }
    let outcome = hirsel_engine::project_outcome(&execution, &sources)
        .map_err(|error| ToolError::Runtime(format!("{error:?}")))?;
    if let network::HostIo::Replay(io) = &mut operations.io
        && !operations.library.native_free()
        && !matches!(execution.outcome, hirsel_interp::Outcome::ToolingLimit(_))
    {
        io.finish_outcome(&outcome).map_err(|e| {
            ToolError::Runtime(format!(
                "replay refused: {e:?}; run stopped with {outcome:?}"
            ))
        })?;
    }
    if let Some(observed) = observed {
        *observed = execution.observable(&sources);
    }
    let outcome = operations
        .finish_free(&outcome)
        .map_err(|e| ToolError::Runtime(format!("Free replay: {e}")))?
        .unwrap_or(outcome);
    if let network::HostIo::Live(io) = &mut operations.io {
        io.finish_recording(&outcome)
            .map_err(|e| ToolError::Runtime(format!("the Io tape: {e}")))?;
    }
    Ok((outcome, returned))
}
