//! Host TIR execution over one immutable compiler-request result.
//! The runtime provider reads only settled products; execution owns all live
//! host effects, admission, recording, cancellation and budgets.
use crate::launch_binding::LaunchSelection;
use crate::requests::TirExecution;
use crate::{HostLaunch, HostRun, RunOutcome, ToolError, call_schemas, network};
use std::path::Path;

pub(crate) fn refuse_foreign_interpretation(input: &TirExecution) -> Result<(), ToolError> {
    if let Some(extern_) = input.foreign.externs.first() {
        return Err(ToolError::Runtime(format!(
            "launch: the reference interpreter does not call C, and {extern_} is an \
             extern \"c\" function; build or run the program natively (PROG-4)"
        )));
    }
    Ok(())
}

pub(crate) fn run_tir_on_host(
    input: &TirExecution,
    launch: HostLaunch<'_>,
    selection: LaunchSelection,
    max_steps: Option<u64>,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
    cancel: Option<network::ExternalCancel>,
    checked: bool,
) -> Result<(RunOutcome, Option<hirsel_run_schema::CalleeOutcome>), ToolError> {
    run_tir_on_host_with_grants(
        input, launch, selection, max_steps, observed, cancel, checked, None,
    )
}

pub(crate) fn run_tir_on_host_with_grants(
    input: &TirExecution,
    launch: HostLaunch<'_>,
    selection: LaunchSelection,
    max_steps: Option<u64>,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
    cancel: Option<network::ExternalCancel>,
    checked: bool,
    snapshot: Option<hirsel_rt::host_launch::HostGrants>,
) -> Result<(RunOutcome, Option<hirsel_run_schema::CalleeOutcome>), ToolError> {
    run_tir_on_host_traced(
        input, launch, selection, max_steps, observed, cancel, checked, snapshot, None,
    )
}

pub(crate) fn run_tir_on_host_traced(
    input: &TirExecution,
    launch: HostLaunch<'_>,
    selection: LaunchSelection,
    max_steps: Option<u64>,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
    cancel: Option<network::ExternalCancel>,
    checked: bool,
    snapshot: Option<hirsel_rt::host_launch::HostGrants>,
    ownership: Option<&mut crate::ownership_trace::Capture>,
) -> Result<(RunOutcome, Option<hirsel_run_schema::CalleeOutcome>), ToolError> {
    refuse_foreign_interpretation(input)?;
    let facts = HostFacts {
        entry: input.entry.clone(),
        signature: input.signature.clone(),
        sources: input.sources.clone(),
        runtime: input.runtime.clone(),
        checked_runtime: Some(input.checked_runtime.clone()),
        code: input.code.clone(),
        callees: input.callees.clone(),
        revision: input.revision.clone(),
        foreign: input.foreign.clone(),
    };
    run_provider_on_host(
        &facts,
        &mut input.provider(),
        launch,
        selection,
        max_steps,
        observed,
        cancel,
        checked,
        snapshot,
        ownership,
    )
}

struct HostFacts {
    entry: hirsel_syntax::DefPath,
    signature: hirsel_types::SignatureRef,
    sources: hirsel_span::SourceMap,
    runtime: std::sync::Arc<hirsel_interp::aggregate::RuntimeTypes>,
    checked_runtime: Option<std::sync::Arc<hirsel_interp::checked::CheckedRuntime>>,
    code: hirsel_run_schema::CodeReach,
    callees: std::collections::BTreeMap<String, hirsel_run_schema::CalleeFacts>,
    revision: hirsel_run_schema::ProgramRevision,
    foreign: hirsel_engine::ForeignUse,
}

/// Lower-level fixture callers explicitly supply their loaded compiler. Default
/// fresh/resident callers always use immutable request products above. Both
/// forms share the same host execution, admission, budgets and tape protocol.
pub(crate) fn run_loaded_on_host_with_checks(
    program: &mut hirsel_engine::LoadedProgram,
    launch: HostLaunch<'_>,
    max_steps: Option<u64>,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
    cancel: Option<network::ExternalCancel>,
    checked: bool,
    snapshot: Option<hirsel_rt::host_launch::HostGrants>,
) -> Result<(RunOutcome, Option<hirsel_run_schema::CalleeOutcome>), ToolError> {
    use hirsel_engine::DefinitionQueries;
    let original = program.entry.clone();
    let result = (|| {
        program.check_for_execution()?;
        let selection = LaunchSelection::read(launch)?;
        if let Some(entry) = selection.entry() {
            program.entry = entry.clone();
        }
        let entry = program.entry.clone();
        let signature = program
            .engine
            .item_signature(&entry)
            .map_err(|error| match error {
                hirsel_types::QueryError::NotImplemented => ToolError::Runtime(format!(
                    "launch: the program declares no entry `pub fn {}`",
                    entry.0.name
                )),
                error => program.error(error),
            })?
            .value;
        let revision = program
            .engine
            .semantic_program_revision(&entry)
            .map_err(|error| program.error(error))?;
        let code = match program.unsafe_reach()? {
            Some(reach) => hirsel_run_schema::CodeReach::Escapes(reach.spelled()),
            None => hirsel_run_schema::CodeReach::Safe,
        };
        program.refuse_foreign_interpretation()?;
        let foreign = program
            .engine
            .foreign_use(&entry)
            .map_err(|error| program.error(error))?;
        let runtime = program
            .engine
            .runtime_types()
            .map_err(|error| program.error(error))?;
        let checked_runtime = if checked {
            Some(std::sync::Arc::new(
                program
                    .engine
                    .checked_runtime_types()
                    .map_err(|error| program.error(error))?
                    .checked
                    .ok_or_else(|| {
                        ToolError::Runtime("checked execution has no checked runtime".into())
                    })?,
            ))
        } else {
            None
        };
        let callees = hirsel_engine::callee_facts(&mut program.engine, &entry)
            .map_err(|error| program.error(error))?;
        let allowed = if program.engine.closure_assertions {
            Some(
                program
                    .engine
                    .semantic_program_nodes([entry.clone()])
                    .map_err(|error| program.error(error))?
                    .into_iter()
                    .filter_map(|node| crate::def_path(&node.identity))
                    .collect(),
            )
        } else {
            None
        };
        let facts = HostFacts {
            entry,
            signature,
            sources: program.engine.sources.clone(),
            runtime: std::sync::Arc::new(runtime),
            checked_runtime,
            code,
            callees,
            revision,
            foreign,
        };
        let mut provider = LoadedProvider {
            engine: &mut program.engine,
            allowed,
            entry: facts.entry.clone(),
        };
        run_provider_on_host(
            &facts,
            &mut provider,
            launch,
            selection,
            max_steps,
            observed,
            cancel,
            checked,
            snapshot,
            None,
        )
    })();
    program.entry = original;
    result
}
struct LoadedProvider<'a> {
    engine: &'a mut hirsel_engine::QueryEngine,
    allowed: Option<std::collections::BTreeSet<hirsel_syntax::DefPath>>,
    entry: hirsel_syntax::DefPath,
}
impl hirsel_tir::TirProvider for LoadedProvider<'_> {
    fn definition(
        &mut self,
        path: &hirsel_syntax::DefPath,
    ) -> Result<hirsel_tir::DefinitionRef, hirsel_types::QueryError> {
        if self
            .allowed
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(path))
        {
            return Err(hirsel_types::QueryError::Operational(
                hirsel_types::OperationalFailure {
                    operation: "semantic closure".into(),
                    message: format!(
                        "interpretation entered {path}, outside the semantic closure of {}",
                        self.entry
                    ),
                },
            ));
        }
        hirsel_tir::TirProvider::definition(self.engine, path)
    }
    fn constant(
        &mut self,
        path: &hirsel_syntax::DefPath,
    ) -> Result<hirsel_tir::MaterializedRef, hirsel_types::QueryError> {
        hirsel_tir::TirProvider::constant(self.engine, path)
    }
    fn constant_instance(
        &mut self,
        path: &hirsel_syntax::DefPath,
        actuals: &[u64],
    ) -> Result<hirsel_tir::MaterializedRef, hirsel_types::QueryError> {
        hirsel_tir::TirProvider::constant_instance(self.engine, path, actuals)
    }
    fn constant_definitions(
        &mut self,
    ) -> Result<Vec<hirsel_syntax::DefPath>, hirsel_types::QueryError> {
        hirsel_tir::TirProvider::constant_definitions(self.engine)
    }
    fn instance_body(
        &mut self,
        path: &hirsel_syntax::DefPath,
        actuals: &[hirsel_types::GenericArgument],
    ) -> Result<Option<hirsel_syntax::DefPath>, hirsel_types::QueryError> {
        hirsel_tir::TirProvider::instance_body(self.engine, path, actuals)
    }
}

fn run_provider_on_host(
    input: &HostFacts,
    provider: &mut dyn hirsel_tir::TirProvider,
    launch: HostLaunch<'_>,
    selection: LaunchSelection,
    max_steps: Option<u64>,
    observed: Option<&mut Option<hirsel_interp::observation::Observable>>,
    cancel: Option<network::ExternalCancel>,
    checked: bool,
    snapshot: Option<hirsel_rt::host_launch::HostGrants>,
    ownership: Option<&mut crate::ownership_trace::Capture>,
) -> Result<(RunOutcome, Option<hirsel_run_schema::CalleeOutcome>), ToolError> {
    if selection.requires_replayable(launch) {
        if let Some((user, extern_)) = input.foreign.unmodeled.first_key_value() {
            return Err(ToolError::Runtime(format!(
                "launch: {user} calls the extern {extern_} and is not a wrapper with a \
                 deterministic model, so this run cannot be recorded or replayed (PROG-4)"
            )));
        }
    }
    let host = snapshot.unwrap_or_else(hirsel_rt::host_launch::HostGrants::process);
    let tape = match launch {
        HostLaunch::Live { record, .. } | HostLaunch::Callee { record, .. } => {
            record.map(Path::to_path_buf)
        }
        HostLaunch::Replay(_) => None,
    };
    let (run, io, instance, free) = match crate::admit_bound_host_run(
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
        HostRun::Live { admitted, mut io } => {
            let free = if admitted
                .run()
                .record()
                .io
                .as_ref()
                .and_then(|io| io.tasks.as_deref())
                == Some("tasks.free@1")
            {
                hirsel_rt::free_replay::Codec::record(&mut io, &admitted, tape.as_deref())
                    .map_err(|e| ToolError::Runtime(format!("Free recording: {e}")))?
            } else {
                None
            };
            (
                admitted.run().clone(),
                network::HostIo::Live(io),
                admitted.instance(),
                free,
            )
        }
        // A replay parks where the recording parked (its `blocked` and `ready` events).
        HostRun::Replay { run, io } => {
            let instance = io.instance();
            (run, network::HostIo::Replay(io), instance, None)
        }
    };
    // A callee replay selected its entry before requesting immutable TIR.
    let signature = input.signature.clone();
    let call = run.record().call.clone();
    let tir_run = hirsel_tir::RunConfiguration {
        entry: input.entry.clone(),
        hash_seed: run.record().inputs.hash_seed,
        inputs: vec![],
    };
    let mut runtime = (*input.runtime).clone();
    runtime.checked = if checked {
        Some(
            (**input.checked_runtime.as_ref().ok_or_else(|| {
                ToolError::Runtime("checked execution has no checked runtime".into())
            })?)
            .clone(),
        )
    } else {
        None
    };
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
    if let Some((codec, session, report)) = free {
        operations.enable_free_recording(codec, session, report, run.record().inputs.test_seed);
    }
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
    let limits = hirsel_interp::Limits {
        steps: max_steps,
        call_depth: 256,
        source: None,
    };
    let cpu_trace = std::env::var_os("HIRSEL_TIR_CPU_PHASES");
    let cpu_fuel = std::env::var_os("HIRSEL_TIR_CPU_FUEL");
    if cpu_trace.is_some() || cpu_fuel.is_some() {
        operations.library.enable_cpu_trace();
    }
    let execution = if let Some(capture) = ownership {
        capture.program = Some(run.program());
        hirsel_interp::ownership_trace::run_resources(
            provider,
            &sources,
            hirsel_interp::ownership_trace::ResourceRequest {
                configuration: &tir_run,
                program: run.program(),
                event_limit: capture.event_limit,
            },
            args,
            limits,
            &mut operations,
            &runtime,
        )
        .map(|result| {
            capture.trace = Some(result.trace);
            result.execution
        })
    } else {
        hirsel_interp::run_with_runtime(
            provider,
            &sources,
            &tir_run,
            args,
            limits,
            &mut operations,
            &runtime,
        )
    };
    if let Some(hook) = &free
        && let Some(session) = operations.library.take_free()
    {
        let outcome = match &execution {
            Ok(execution) => hirsel_interp::library::free_outcome_name(&execution.outcome),
            Err(_) => "stopped",
        };
        hook.finish(session, outcome).map_err(ToolError::Runtime)?;
    }
    if let Some(path) = cpu_trace {
        let trace = operations.library.cpu_trace().unwrap();
        let text = trace
            .iter()
            .map(|event| format!("{event:?}\n"))
            .collect::<String>();
        std::fs::write(path, text)
            .map_err(|error| ToolError::Runtime(format!("CPU phase trace: {error}")))?;
    }
    if let Some(path) = cpu_fuel {
        let trace = operations.library.cpu_trace().unwrap();
        let fuel = operations.library.cpu_fuel_trace().unwrap();
        let events: Vec<_> = trace.iter().zip(fuel).map(|(event, fuel)| serde_json::json!({
            "task":event.task, "phase":format!("{:?}", event.phase),
            "end":matches!(event.phase, hirsel_interp::library::CpuPhase::End(_)), "fuel":fuel,
        })).collect();
        std::fs::write(
            path,
            serde_json::to_vec(
                &serde_json::json!({"task":operations.library.task_path(),"events":events}),
            )
            .unwrap(),
        )
        .map_err(|error| ToolError::Runtime(format!("CPU fuel trace: {error}")))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::TirProducts;
    use hirsel_engine::DefinitionQueries;
    use hirsel_syntax::DefPath;
    use hirsel_tir::{DefinitionRef, MaterializedRef};
    use hirsel_types::{GenericArgument, QueryError};
    use std::sync::Arc;

    struct Settled(Option<DefinitionRef>);
    impl TirProducts for Settled {
        fn definition(&self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
            self.0
                .as_ref()
                .filter(|body| body.path.path() == path)
                .cloned()
                .ok_or(QueryError::NotImplemented)
        }
        fn constant(&self, _: &DefPath, _: &[u64]) -> Result<MaterializedRef, QueryError> {
            Err(QueryError::NotImplemented)
        }
        fn constant_definitions(&self) -> Result<Vec<DefPath>, QueryError> {
            Ok(vec![])
        }
        fn instance_body(
            &self,
            _: &DefPath,
            _: &[GenericArgument],
        ) -> Result<Option<DefPath>, QueryError> {
            Ok(None)
        }
    }

    fn fixture() -> TirExecution {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            crate::compiler_adapters(),
        );
        let file = engine
            .sources
            .add(
                "settled-execution.hir",
                "pub fn main()->Result[(),()]{Ok(())}".into(),
            )
            .unwrap();
        let context = hirsel_engine::default_context();
        hirsel_engine::check_snapshot(&mut engine, file, &context).unwrap();
        let entry = hirsel_engine::entry_path(&context);
        let definition = engine.lower_to_tir(&entry).unwrap().value;
        let checked_runtime = engine.checked_runtime_types().unwrap().checked.unwrap();
        TirExecution {
            entry: entry.clone(),
            definition: definition.clone(),
            signature: engine.item_signature(&entry).unwrap().value,
            sources: engine.sources.clone(),
            runtime: Arc::new(engine.runtime_types().unwrap()),
            checked_runtime: Arc::new(checked_runtime),
            code: hirsel_run_schema::CodeReach::Safe,
            callees: Default::default(),
            entries: Default::default(),
            revision: engine.semantic_program_revision(&entry).unwrap(),
            foreign_unreplayable: false,
            foreign: Default::default(),
            products: Arc::new(Settled(Some(definition))),
        }
    }

    #[test]
    fn immutable_tir_executes_normal_and_checked_after_engine_drop_with_identical_observations() {
        hirsel_engine::with_compiler_stack(|| {
            let input = fixture();
            let mut prior = None;
            for checked in [false, true] {
                let launch = HostLaunch::empty();
                let mut observed = None;
                let (outcome, callee) = run_tir_on_host(
                    &input,
                    launch,
                    LaunchSelection::read(launch).unwrap(),
                    Some(10000),
                    Some(&mut observed),
                    None,
                    checked,
                )
                .unwrap();
                assert_eq!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                );
                assert!(callee.is_none());
                let observed = observed.expect("completed execution has a semantic observation");
                if let Some(prior) = prior {
                    assert_eq!(prior, observed);
                }
                prior = Some(observed);
            }
        });
    }

    #[test]
    fn immutable_tir_refuses_foreign_use_and_missing_settled_bodies_without_compiler_fallback() {
        hirsel_engine::with_compiler_stack(|| {
            let mut input = fixture();
            let extern_ = DefPath::core("external");
            input.foreign.externs.insert(extern_.clone());
            let launch = HostLaunch::empty();
            let error = run_tir_on_host(
                &input,
                launch,
                LaunchSelection::read(launch).unwrap(),
                Some(10000),
                None,
                None,
                false,
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "launch: the reference interpreter does not call C, and {extern_} is an \
                 extern \"c\" function; build or run the program natively (PROG-4)"
                )
            );
            input.foreign.externs.clear();
            input.products = Arc::new(Settled(None));
            assert!(
                run_tir_on_host(
                    &input,
                    launch,
                    LaunchSelection::read(launch).unwrap(),
                    Some(10000),
                    None,
                    None,
                    false
                )
                .is_err()
            );
        });
    }
}
