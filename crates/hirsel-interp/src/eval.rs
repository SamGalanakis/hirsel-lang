//! A single iterative scheduling loop: Hirsel calls never recurse on the host stack.
use crate::Failure;
use crate::checked::Shadow;
use crate::frame::{Continuation, Frame};
use crate::scalar::{self, ScalarError};
use crate::*;
use hirsel_diag::RuleId;
use hirsel_span::{SourceMap, Span};
use hirsel_tir::*;
use hirsel_types::Destination;

pub fn run(
    provider: &mut (impl TirProvider + ?Sized),
    sources: &SourceMap,
    configuration: &RunConfiguration,
    arguments: Vec<Value>,
    limits: Limits,
) -> Result<Execution, ExecutionError> {
    run_with_operations(
        provider,
        sources,
        configuration,
        arguments,
        limits,
        &mut NoOperations,
    )
}
pub fn run_with_operations(
    provider: &mut (impl TirProvider + ?Sized),
    sources: &SourceMap,
    configuration: &RunConfiguration,
    arguments: Vec<Value>,
    limits: Limits,
    operations: &mut impl Operations,
) -> Result<Execution, ExecutionError> {
    run_with_runtime(
        provider,
        sources,
        configuration,
        arguments,
        limits,
        operations,
        &crate::aggregate::RuntimeTypes::default(),
    )
}
pub fn run_with_runtime(
    provider: &mut (impl TirProvider + ?Sized),
    sources: &SourceMap,
    configuration: &RunConfiguration,
    arguments: Vec<Value>,
    limits: Limits,
    operations: &mut impl Operations,
    runtime: &crate::aggregate::RuntimeTypes,
) -> Result<Execution, ExecutionError> {
    let constants = crate::constants::Constants::initialize(provider, runtime)?;
    let definition = provider
        .definition(&configuration.entry)
        .map_err(Error::Query)?;
    let mut frame = Frame::new(definition, arguments)?;
    let checker = runtime.checked.clone().map(crate::checked::state);
    if let Some(state) = &checker {
        frame.check = Some(enter_checked(state, &frame, vec![], None));
    }
    frame.depth_failure = limits.call_depth == 0;
    let frames = vec![frame];
    let mut execution = Execution {
        failures: vec![],
        outcome: Outcome::Returned {
            value: Value::Unit,
            exit_code: 0,
        },
        stdout: vec![],
        trace: if runtime.trace_mode == TraceMode::Full {
            vec![TraceEvent::Enter {
                path: configuration.entry.clone(),
                depth: 1,
            }]
        } else {
            vec![]
        },
        steps: 0,
    };
    let mut entry_consumed = false;
    let result = execute_frames(
        provider,
        sources,
        configuration,
        limits,
        operations,
        runtime,
        frames,
        constants,
        &mut execution,
        &mut entry_consumed,
    );
    let result = match (result, &checker) {
        (Err(Error::Blocked(waits)), _) => {
            execution.outcome = Outcome::Blocked(waits);
            Ok(())
        }
        (Err(Error::Trapped(trap)), Some(_)) => {
            execution.outcome = Outcome::Trapped(*trap);
            Ok(())
        }
        (Ok(()), Some(state)) => {
            execution.outcome =
                audit_resources(&state.borrow(), &execution.outcome, entry_consumed)
                    .map_or(execution.outcome, |trap| Outcome::Trapped(*trap));
            Ok(())
        }
        (result, _) => result,
    };
    if let Outcome::Trapped(trap) = &mut execution.outcome {
        trap.line = sources.location(trap.site).map_or(0, |l| l.line);
    }
    result.map_err(|error| ExecutionError {
        error,
        stdout: std::mem::take(&mut execution.stdout),
    })?;
    Ok(execution)
}
/// DROP-2 exactly once: every constructed resource was released or forgotten,
/// or is still owned by the returned value. Tooling limits stop early.
fn audit_resources(
    state: &crate::checked::State,
    outcome: &Outcome,
    entry_consumed: bool,
) -> Option<Box<crate::checked::Trap>> {
    let returned = match outcome {
        Outcome::Returned { value, .. } => {
            if entry_consumed {
                0
            } else {
                crate::checked::resources_in(value)
            }
        }
        Outcome::Failed(_) => 0,
        _ => return None,
    };
    let accounted = state.released + state.forgotten + returned;
    if accounted == state.constructed {
        return None;
    }
    let Error::Trapped(trap) = state.trap(
        crate::checked::TrapRule::FinalizerCount,
        format!(
            "{} constructed, {} released, {} forgotten, {returned} returned",
            state.constructed, state.released, state.forgotten
        ),
    ) else {
        unreachable!()
    };
    Some(trap)
}

fn execute_frames(
    provider: &mut (impl TirProvider + ?Sized),
    sources: &SourceMap,
    configuration: &RunConfiguration,
    limits: Limits,
    operations: &mut impl Operations,
    runtime: &crate::aggregate::RuntimeTypes,
    mut frames: Vec<Frame>,
    mut constants: crate::constants::Constants,
    execution: &mut Execution,
    entry_consumed: &mut bool,
) -> Result<(), Error> {
    let mut returned_snapshot = None;
    let mut tooling = None;
    let mut budgets = crate::bounded::Budgets::default();
    let mut task = 0;
    let mut runnable = std::collections::VecDeque::new();
    let mut parked: Vec<(u64, Vec<Frame>, Vec<crate::observation::FailureRecord>)> = Vec::new();
    operations.enter_task(task, frames.iter().any(|f| f.finalized.is_some()));
    loop {
        for start in operations.take_tasks() {
            let mut invocation_types = Default::default();
            let (definition, body, captures, sizes) = match &start.callee {
                Value::Closure {
                    definition,
                    body,
                    captures,
                    sizes,
                    type_arguments,
                    ..
                } => {
                    invocation_types = type_arguments.clone();
                    (
                        provider.definition(definition).map_err(Error::Query)?,
                        *body,
                        captures.clone(),
                        sizes.clone(),
                    )
                }
                Value::FunctionReference(reference) => {
                    let definition = provider
                        .definition(&reference.definition)
                        .map_err(Error::Query)?;
                    let body = definition.entry;
                    crate::type_arguments::bind(
                        &mut invocation_types,
                        definition.bodies[body.0 as usize].parameter_scope,
                        crate::type_arguments::Actuals {
                            types: reference.types.clone(),
                            arguments: reference.arguments.clone(),
                        },
                    );
                    let mut sizes = Default::default();
                    Frame::bind_sizes(
                        &mut sizes,
                        definition.bodies[body.0 as usize].parameter_scope,
                        reference.arguments.iter().map(|a| match a {
                            hirsel_types::GenericArgument::Size(hirsel_types::Size::Constant(
                                n,
                            )) => Some(*n),
                            _ => None,
                        }),
                    );
                    (definition, body, vec![], sizes)
                }
                _ => return Err(invalid("task body requires a callable")),
            };
            let mut child = Frame::new_body(definition, body, vec![], captures, sizes)?;
            child.type_arguments = invocation_types;
            if let Some(parent) = frames.last().and_then(|f| f.check.as_ref()) {
                let callee =
                    frames
                        .last()
                        .and_then(|f| f.operation.as_ref())
                        .and_then(|(_, state)| {
                            let bounds = state.bounds.as_ref()?;
                            let Value::Closure {
                                captures,
                                owned_captures,
                                ..
                            } = &start.callee
                            else {
                                return None;
                            };
                            let shadow = crate::checked::native_shadow(
                                &start.callee,
                                &state.arguments,
                                bounds,
                            );
                            Some(crate::checked::Callee {
                                captures: captures
                                    .iter()
                                    .enumerate()
                                    .map(|(i, value)| {
                                        value
                                            .as_ref()
                                            .map(|v| shadow.part(i).for_value(v))
                                            .unwrap_or_default()
                                    })
                                    .collect(),
                                owned: owned_captures.clone(),
                                lifetime: shadow.lifetime(),
                            })
                        });
                child.check = Some(enter_checked(&parent.state, &child, vec![], callee));
            }
            start.callee.begin_call();
            start.callee.discard();
            runnable.push_back((start.id, vec![child], vec![]));
        }
        let yield_now = operations.take_yield();
        // Poll only at suspension points. Pure code may starve other tasks;
        // checking host readiness on every interpreter instruction would make
        // a waiting socket slow all pure computation.
        if yield_now || frames.is_empty() {
            // Stable sorting keeps non-timer waits and equal-deadline timers in
            // suspension order. Poll timers by their original absolute ticks,
            // including when several deadlines passed during a blocking step.
            parked.sort_by_cached_key(|(_, stack, _)| {
                match &stack.last().unwrap().operation.as_ref().unwrap().0 {
                    OperationResult::Park { token, .. } => operations.wait_deadline(*token),
                    _ => None,
                }
            });
            let mut waiting = vec![];
            for (id, mut stack, failures) in parked.drain(..) {
                operations.enter_task(id, stack.iter().any(|f| f.finalized.is_some()));
                let frame: &mut Frame = stack.last_mut().unwrap();
                let (OperationResult::Park { token, .. }, state) =
                    frame.operation.as_ref().unwrap()
                else {
                    return Err(invalid("parked task has no wait"));
                };
                let result = operations.poll(
                    *token,
                    &mut OperationContext {
                        configuration,
                        sources,
                        stdout: &mut execution.stdout,
                        trace: &mut execution.trace,
                        trace_mode: runtime.trace_mode,
                        anchor: Some(&state.anchor),
                    },
                )?;
                if let Some(result) = result {
                    frame.operation.as_mut().unwrap().0 = result;
                    runnable.push_back((id, stack, failures));
                } else {
                    waiting.push((id, stack, failures));
                }
            }
            parked = waiting;
        }
        operations.enter_task(task, frames.iter().any(|f| f.finalized.is_some()));
        if frames.is_empty() || (yield_now && !runnable.is_empty()) {
            if !frames.is_empty() {
                runnable.push_back((
                    task,
                    std::mem::take(&mut frames),
                    std::mem::take(&mut execution.failures),
                ));
            }
            let choice = if runnable.is_empty() {
                0
            } else {
                operations
                    .choose_task(&runnable.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())?
            };
            if !runnable.is_empty() && choice >= runnable.len() {
                return Err(invalid(
                    "scheduler selected a task outside the runnable queue",
                ));
            }
            if let Some((id, stack, failures)) = runnable.remove(choice) {
                task = id;
                frames = stack;
                execution.failures = failures;
                operations.enter_task(task, frames.iter().any(|f| f.finalized.is_some()));
                let resumed = frames.last().unwrap();
                operations.task_resumed(
                    task,
                    &mut OperationContext {
                        configuration,
                        sources,
                        stdout: &mut execution.stdout,
                        trace: &mut execution.trace,
                        trace_mode: runtime.trace_mode,
                        anchor: Some(&anchor(resumed, site_of(resumed)?)),
                    },
                )?;
            } else {
                if operations.wait()? {
                    continue;
                }
                let waits = parked
                    .iter()
                    .filter_map(|(_, stack, _)| match &stack.last()?.operation.as_ref()?.0 {
                        OperationResult::Park { wait, .. } => Some(wait.clone()),
                        _ => None,
                    })
                    .collect();
                return Err(Error::Blocked(waits));
            }
        }

        let depth = frames.len();
        if let Some((result, mut state)) = frames
            .last_mut()
            .unwrap()
            .work
            .is_empty()
            .then(|| frames.last_mut().unwrap().operation.take())
            .flatten()
        {
            let frame = frames.last_mut().unwrap();
            match result {
                OperationResult::Park { token, wait } => {
                    frame.operation = Some((OperationResult::Park { token, wait }, state));
                    parked.push((
                        task,
                        std::mem::take(&mut frames),
                        std::mem::take(&mut execution.failures),
                    ));
                }
                OperationResult::FailureChain {
                    mut failures,
                    released,
                } => {
                    let first = failures
                        .first()
                        .ok_or_else(|| invalid("empty failure chain"))?
                        .failure
                        .clone();
                    let offset = execution.failures.len();
                    if let Some(last) = execution.failures.last_mut() {
                        last.cause = Some(offset);
                    }
                    for failure in &mut failures {
                        if let Some(cause) = &mut failure.cause {
                            *cause += offset;
                        }
                    }
                    execution.failures.extend(failures);
                    state.already_failed = true;
                    frame.ending = Some(crate::resources::Ending::Failed);
                    for value in released.into_iter().rev() {
                        frame.work.push_back(crate::resources::Work::Release(
                            value,
                            state.anchor.clone(),
                        ));
                    }
                    frame.operation = Some((OperationResult::Failed(first), state));
                }
                OperationResult::UndefinedBehavior(detected) => {
                    execution.outcome = Outcome::UndefinedBehavior(detected);
                    return Ok(());
                }
                OperationResult::Call {
                    callee,
                    arguments,
                    mut_arguments,
                    writebacks,
                    updates,
                    owned,
                    consumed,
                    continuation,
                } => {
                    let callback_inputs = state
                        .bounds
                        .as_ref()
                        .map(|bounds| {
                            arguments
                                .iter()
                                .map(|value| {
                                    let shadow = crate::checked::native_shadow(
                                        value,
                                        &state.arguments,
                                        bounds,
                                    )
                                    .for_value(value);
                                    crate::checked::Argument {
                                        lifetime: shadow.total(),
                                        shadow,
                                    }
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let callback_callee = state.bounds.as_ref().and_then(|bounds| {
                        if let Value::Closure {
                            captures,
                            owned_captures,
                            ..
                        } = &callee
                        {
                            let shadow =
                                crate::checked::native_shadow(&callee, &state.arguments, bounds);
                            Some(crate::checked::Callee {
                                captures: captures
                                    .iter()
                                    .enumerate()
                                    .map(|(i, v)| {
                                        v.as_ref()
                                            .map(|v| shadow.part(i).for_value(v))
                                            .unwrap_or_default()
                                    })
                                    .collect(),
                                owned: owned_captures.clone(),
                                lifetime: shadow.lifetime(),
                            })
                        } else {
                            None
                        }
                    });
                    validate_callback_writebacks(&state, &mut_arguments, &writebacks)?;
                    transfer_operation(&mut state, consumed)?;
                    update_operation(&mut state, updates)?;
                    state.owned = owned;
                    callee.begin_call();
                    let mut invocation_types = Default::default();
                    let (definition, body, captures, sizes) = match callee {
                        Value::Closure {
                            definition,
                            body,
                            captures,
                            owned_captures,
                            sizes,
                            type_arguments,
                            ..
                        } => {
                            if captures.len() != owned_captures.len() {
                                return Err(invalid("capture ownership count"));
                            }
                            invocation_types = type_arguments;
                            (
                                provider.definition(&definition).map_err(Error::Query)?,
                                body,
                                captures,
                                sizes,
                            )
                        }
                        Value::FunctionReference(reference) => {
                            let hirsel_types::TypeKind::Function(function) = &reference
                                .types
                                .get(reference.ty)
                                .ok_or_else(|| invalid("named callback type"))?
                                .kind
                            else {
                                return Err(invalid("named callback function type"));
                            };
                            let mut callback_depth = depth;
                            let mut suspended = &state;
                            while let Some(callback) = &suspended.callback {
                                callback_depth += 1;
                                suspended = &callback.state;
                            }
                            if callback_depth >= limits.call_depth
                                || limits.steps == Some(execution.steps)
                            {
                                let limit = ToolingLimit {
                                    kind: if callback_depth >= limits.call_depth {
                                        ToolingLimitKind::CallDepth
                                    } else {
                                        ToolingLimitKind::Steps
                                    },
                                    limit: if callback_depth >= limits.call_depth {
                                        limits.call_depth as u64
                                    } else {
                                        limits.steps.expect("step limit reached")
                                    },
                                    site: Some(state.anchor.site),
                                };
                                if tooling.is_none() {
                                    if runtime.trace_mode == TraceMode::Full {
                                        execution
                                            .trace
                                            .push(TraceEvent::ToolingLimit(limit.clone()));
                                    }
                                    tooling = Some(limit);
                                }
                                frame.ending = Some(crate::resources::Ending::Failed);
                                for (i, value) in arguments.into_iter().enumerate().rev() {
                                    if function.parameters.get(i).is_some_and(|p| {
                                        p.passing == hirsel_syntax::PassingMode::Take
                                    }) {
                                        frame.work.push_back(crate::resources::Work::Release(
                                            value,
                                            state.anchor.clone(),
                                        ));
                                    } else {
                                        value.discard();
                                    }
                                }
                                abandon_operation(frame, state)?;
                                continue;
                            }
                            if let Some(result) = operations.call(
                                OperationCall {
                                    runtime,
                                    path: &reference.definition,
                                    generics: &reference.arguments,
                                    arguments: &arguments,
                                    result_type: function.result,
                                    types: &reference.types,
                                    site: state.anchor.site,
                                },
                                &mut OperationContext {
                                    configuration,
                                    sources,
                                    stdout: &mut execution.stdout,
                                    trace: &mut execution.trace,
                                    trace_mode: runtime.trace_mode,
                                    anchor: Some(&state.anchor),
                                },
                            )? {
                                execution.steps = execution.steps.saturating_add(1);
                                let taken = function
                                    .parameters
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(i, p)| {
                                        (p.passing == hirsel_syntax::PassingMode::Take).then_some(i)
                                    })
                                    .collect();
                                let native = crate::resources::OperationState {
                                    arguments,
                                    owned: vec![],
                                    taken,
                                    finishing: None,
                                    mut_places: vec![],
                                    returns: state.returns,
                                    on_failure: state.on_failure,
                                    anchor: state.anchor.clone(),
                                    already_failed: false,
                                    callback: Some(Box::new(crate::resources::Callback {
                                        failure_start: execution.failures.len(),
                                        token: continuation,
                                        state,
                                        mut_parameters: vec![],
                                        writebacks,
                                    })),
                                    callback_mut_arguments: mut_arguments,
                                    bounds: frame.check.as_ref().map(|check| {
                                        operation_bounds(
                                            &check.state.borrow().config,
                                            &reference.definition,
                                            &callback_inputs,
                                            &[],
                                            &[],
                                        )
                                    }),
                                };
                                frame.operation = Some((result, native));
                                continue;
                            }
                            let definition = provider
                                .definition(&reference.definition)
                                .map_err(Error::Query)?;
                            let body = definition.entry;
                            let mut sizes = Default::default();
                            Frame::bind_sizes(
                                &mut sizes,
                                definition.bodies[body.0 as usize].parameter_scope,
                                reference.arguments.iter().map(|arg| match arg {
                                    hirsel_types::GenericArgument::Size(
                                        hirsel_types::Size::Constant(n),
                                    ) => Some(*n),
                                    _ => None,
                                }),
                            );
                            crate::type_arguments::bind(
                                &mut invocation_types,
                                definition.bodies[body.0 as usize].parameter_scope,
                                crate::type_arguments::Actuals {
                                    types: reference.types.clone(),
                                    arguments: reference.arguments.clone(),
                                },
                            );
                            (definition, body, vec![], sizes)
                        }
                        _ => return Err(invalid("operation callback requires function value")),
                    };
                    let path = definition.path.clone();
                    let mut callback =
                        Frame::new_body(definition, body, arguments, captures, sizes)?;
                    callback.type_arguments = invocation_types;
                    if let Some(check) = &frame.check {
                        callback.check = Some(enter_checked(
                            &check.state,
                            &callback,
                            callback_inputs,
                            callback_callee,
                        ));
                    }
                    let mut_parameters = mut_arguments
                        .into_iter()
                        .map(|i| {
                            callback
                                .body()
                                .parameters
                                .get(i)
                                .copied()
                                .map(|l| (i, l))
                                .ok_or_else(|| invalid("callback mut parameter"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    callback.mut_parameters = mut_parameters.iter().map(|(_, l)| *l).collect();
                    callback.callback = Some(crate::resources::Callback {
                        failure_start: execution.failures.len(),
                        token: continuation,
                        state,
                        mut_parameters,
                        writebacks,
                    });
                    callback.depth_failure = depth >= limits.call_depth;
                    frames.push(callback);
                    if runtime.trace_mode == TraceMode::Full {
                        execution.trace.push(TraceEvent::Enter {
                            path,
                            depth: depth + 1,
                        });
                    }
                }
                OperationResult::Complete {
                    value,
                    arguments,
                    consumed,
                    released,
                } => {
                    if let Some(failure_start) = state.finishing {
                        if execution.failures.len() > failure_start {
                            frame.work.push_back(crate::resources::Work::Release(
                                value,
                                state.anchor.clone(),
                            ));
                            if let Some(mut callback) = state.callback.take() {
                                writeback_native_callback(&mut callback, &state)?;
                                enqueue_owned(frame, &mut callback.state);
                                callback.state.already_failed = true;
                                let failure =
                                    execution.failures[callback.failure_start].failure.clone();
                                frame.work.push_back(crate::resources::Work::Resume(
                                    OperationResult::Failed(failure),
                                    *callback,
                                ));
                            } else {
                                enqueue_cleanup(frame, state.on_failure, state.anchor.site)?;
                            }
                        } else {
                            frame.ending = None;
                            if let Some(mut callback) = state.callback.take() {
                                writeback_native_callback(&mut callback, &state)?;
                                if let (Some(child), Some(parent)) =
                                    (&state.bounds, &mut callback.state.bounds)
                                {
                                    if let Some(check) = &frame.check {
                                        check
                                            .state
                                            .borrow()
                                            .access(&child.result, "native callback return")?;
                                    }
                                    let written: Vec<_> =
                                        child.updates.iter().map(|(_, s)| s.clone()).collect();
                                    parent.accept_callback(&child.result, &written);
                                }
                                enqueue_owned(frame, &mut callback.state);
                                let arguments = state
                                    .callback_mut_arguments
                                    .iter()
                                    .map(|&i| {
                                        (i, std::mem::replace(&mut state.arguments[i], Value::Unit))
                                    })
                                    .collect();
                                frame.work.push_back(crate::resources::Work::Resume(
                                    OperationResult::Complete {
                                        value,
                                        arguments,
                                        consumed: vec![],
                                        released: vec![],
                                    },
                                    *callback,
                                ));
                            } else {
                                let Some(CallReturn { destination, next }) = state.returns else {
                                    return Err(invalid("diverging operation returned"));
                                };
                                let shadow = state
                                    .bounds
                                    .as_ref()
                                    .map(|bounds| {
                                        bounds.returned_shadow(&value, &state.arguments).ownership(
                                            &value,
                                            frame
                                                .place(destination)
                                                .ok()
                                                .and_then(|p| frame.definition.types.get(p.ty))
                                                .is_some_and(|t| {
                                                    t.modes.uniqueness
                                                        == hirsel_types::Uniqueness::Unique
                                                }),
                                        )
                                    })
                                    .unwrap_or_default()
                                    .for_value(&value);
                                frame.assign_shadow(destination, value, shadow)?;
                                frame.goto(next);
                            }
                        }
                    } else {
                        transfer_operation(&mut state, consumed)?;
                        update_operation(&mut state, arguments)?;
                        restore_operation(frame, &mut state)?;
                        state.finishing = Some(execution.failures.len());
                        frame.ending = Some(crate::resources::Ending::Failed);
                        enqueue_taken(frame, &mut state);
                        for value in released.into_iter().rev() {
                            frame.work.push_back(crate::resources::Work::Release(
                                value,
                                state.anchor.clone(),
                            ));
                        }
                        frame.operation = Some((OperationResult::returned(value), state));
                    }
                }
                OperationResult::Failed(failure) => {
                    restore_operation(frame, &mut state)?;
                    if !state.already_failed {
                        record_failure(
                            runtime.trace_mode,
                            execution,
                            failure,
                            state.anchor.clone(),
                        );
                    }
                    frame.ending = Some(crate::resources::Ending::Failed);
                    enqueue_taken(frame, &mut state);
                    if let Some(mut callback) = state.callback.take() {
                        writeback_native_callback(&mut callback, &state)?;
                        enqueue_owned(frame, &mut callback.state);
                        callback.state.already_failed = true;
                        let failure = execution.failures[callback.failure_start].failure.clone();
                        frame.work.push_back(crate::resources::Work::Resume(
                            OperationResult::Failed(failure),
                            *callback,
                        ));
                    } else {
                        enqueue_cleanup(frame, state.on_failure, state.anchor.site)?;
                    }
                }
            }
            continue;
        }
        if let Some(work) = frames.last_mut().unwrap().work.pop_front() {
            let frame = frames.last_mut().unwrap();
            match work {
                crate::resources::Work::Resume(result, mut callback) => {
                    frame.ending = None;
                    if !matches!(result, OperationResult::Failed(_)) {
                        if let Some(f) = execution.failures.get(callback.failure_start) {
                            callback.state.already_failed = true;
                            release_operation_result(frame, result, &mut callback.state)?;
                            frame.ending = Some(crate::resources::Ending::Failed);
                            frame.work.push_back(crate::resources::Work::Resume(
                                OperationResult::Failed(f.failure.clone()),
                                callback,
                            ));
                            continue;
                        }
                    }
                    let failed = matches!(result, OperationResult::Failed(_));
                    let handles = operations.handles_failure(callback.token);
                    let result = if failed && handles {
                        callback.state.already_failed = false;
                        OperationResult::FailureChain {
                            failures: execution.failures.split_off(callback.failure_start),
                            released: vec![],
                        }
                    } else {
                        result
                    };
                    let resumed = operations.resume(
                        callback.token,
                        result,
                        &mut OperationContext {
                            configuration,
                            sources,
                            stdout: &mut execution.stdout,
                            trace: &mut execution.trace,
                            trace_mode: runtime.trace_mode,
                            anchor: Some(&callback.state.anchor),
                        },
                    )?;
                    let resumed = if failed && !handles {
                        release_operation_result(frame, resumed, &mut callback.state)?;
                        OperationResult::Failed(
                            execution.failures[callback.failure_start].failure.clone(),
                        )
                    } else {
                        resumed
                    };
                    frame.operation = Some((resumed, callback.state));
                }
                crate::resources::Work::Chain(id, anchor) => {
                    let node = frame
                        .body()
                        .cleanup_nodes
                        .get(id.0 as usize)
                        .ok_or_else(|| invalid("cleanup node out of range"))?
                        .clone();
                    if let Some(parent) = node.parent {
                        let mut next = anchor.clone();
                        if let Some((_, index)) = &mut next.cleanup {
                            *index += 1;
                        }
                        frame
                            .work
                            .push_front(crate::resources::Work::Chain(parent, next));
                    }
                    frame
                        .work
                        .push_front(crate::resources::Work::Action(node.action, anchor));
                }
                crate::resources::Work::Action(action, anchor) => {
                    if runtime.trace_mode == TraceMode::Full {
                        execution.trace.push(TraceEvent::Cleanup {
                            depth,
                            action: action.clone(),
                        });
                    }
                    if let Some(check) = &frame.check {
                        let mut state = check.state.borrow_mut();
                        state.site = anchor.site;
                        state.definition = Some(frame.definition.path.clone());
                    }
                    match action {
                        CleanupAction::Drop { place, if_live } => {
                            frame.access(place, "drop")?;
                            let value = match frame.slot(place) {
                                Ok(slot) => slot.take(),
                                Err(
                                    reason @ (crate::frame::AccessError::InactiveVariant
                                    | crate::frame::AccessError::OutOfBounds),
                                ) => {
                                    let (rule, kind) = match reason {
                                        crate::frame::AccessError::InactiveVariant => {
                                            ("MEM-13", "InactiveVariant")
                                        }
                                        _ => ("MEM-14", "OutOfBounds"),
                                    };
                                    record_failure(
                                        runtime.trace_mode,
                                        execution,
                                        failure(sources, anchor.site, rule, kind)?,
                                        anchor,
                                    );
                                    frame.ending = Some(crate::resources::Ending::Failed);
                                    continue;
                                }
                                Err(crate::frame::AccessError::Api(error)) => return Err(error),
                            };
                            if value.is_none() && !if_live {
                                return Err(invalid("unconditional drop of dead storage"));
                            }
                            if let Some(value) = value {
                                checked_release_order(frame, place, &value)?;
                                frame
                                    .work
                                    .push_front(crate::resources::Work::Release(value, anchor));
                            }
                        }
                        CleanupAction::EndLoan { .. } => (),
                        _ => return Err(Error::Unavailable("M2 cleanup".into())),
                    }
                }
                crate::resources::Work::Release(mut value, anchor) => {
                    if let Some(check) = &frame.check {
                        if matches!(
                            value,
                            Value::Resource { .. } | Value::ResourceVariant { .. }
                        ) {
                            check.state.borrow_mut().released += 1;
                        }
                    }
                    if let Value::Shared(root) = &value {
                        // The last root releases the frozen graph it held.
                        if let Some(graph) = root.release()? {
                            frame
                                .work
                                .push_front(crate::resources::Work::Release(graph, anchor));
                        }
                        continue;
                    }
                    if let Value::Handle { kind, id } = &value {
                        effect(
                            &mut execution.trace,
                            anchor.clone(),
                            crate::observation::Effect::HandleDropped {
                                kind: kind.clone(),
                                id: *id,
                            },
                        );
                        operations.drop_handle(
                            kind,
                            *id,
                            &mut OperationContext {
                                configuration,
                                sources,
                                stdout: &mut execution.stdout,
                                trace: &mut execution.trace,
                                trace_mode: runtime.trace_mode,
                                anchor: Some(&anchor),
                            },
                        )?;
                        for value in operations.take_releases() {
                            frame
                                .work
                                .push_front(crate::resources::Work::Release(value, anchor.clone()));
                        }
                        if let Some((rule, kind)) = operations.take_drop_failure() {
                            let mut at = anchor.clone();
                            at.site = dropped_binding_site(frame, &anchor).unwrap_or(anchor.site);
                            record_failure(
                                runtime.trace_mode,
                                execution,
                                failure(sources, at.site, &rule, &kind)?,
                                at,
                            );
                            frame.ending = Some(crate::resources::Ending::Failed);
                        }
                        continue;
                    }
                    if let Some(check) = &frame.check {
                        if check.noalloc {
                            if let Value::Resource { definition, .. }
                            | Value::ResourceVariant { definition, .. } = &value
                            {
                                if !check
                                    .state
                                    .borrow()
                                    .config
                                    .noalloc_drop
                                    .contains(definition)
                                {
                                    return Err(check.state.borrow().trap(
                                        crate::checked::TrapRule::AllocatingCallInNoalloc,
                                        format!(
                                            "implicit destruction of {definition:?} may allocate"
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                    if let Value::Resource {
                        definition, sizes, ..
                    }
                    | Value::ResourceVariant {
                        definition, sizes, ..
                    } = &value
                    {
                        if let Some((path, body)) =
                            runtime.get(definition).and_then(|n| n.finalizer.clone())
                        {
                            effect(
                                &mut execution.trace,
                                anchor.clone(),
                                crate::observation::Effect::Finalizer {
                                    definition: definition.clone(),
                                },
                            );
                            let definition = provider.definition(&path).map_err(Error::Query)?;
                            let mut bindings = Default::default();
                            Frame::bind_sizes(
                                &mut bindings,
                                definition.bodies[body.0 as usize].parameter_scope,
                                sizes.iter().copied(),
                            );
                            let sizes = bindings;
                            let mut callee =
                                Frame::new_body(definition, body, vec![value], vec![], sizes)?;
                            if let Some(check) = &frame.check {
                                callee.check =
                                    Some(enter_checked(&check.state, &callee, vec![], None));
                            }
                            let local = *callee
                                .body()
                                .parameters
                                .first()
                                .ok_or_else(|| invalid("finalizer needs self"))?;
                            callee.finalized = Some((local, anchor));
                            callee.depth_failure = depth >= limits.call_depth;
                            frames.push(callee);
                            if runtime.trace_mode == TraceMode::Full {
                                execution.trace.push(TraceEvent::Enter {
                                    path,
                                    depth: depth + 1,
                                });
                            }
                            continue;
                        }
                    }
                    release_fields(frame, &mut value, anchor)?;
                }
            }
            continue;
        }
        // Hidden counted loop owners survive borrowed parameter storage and are
        // released through the ordinary evaluator, including failure/tooling exit.
        if frames.last().unwrap().ending.is_some() && !frames.last().unwrap().loop_owned.is_empty()
        {
            let frame = frames.last_mut().unwrap();
            let released = frame.loop_owned.len();
            for (value, anchor) in frame.loop_owned.drain(..).rev() {
                frame
                    .work
                    .push_back(crate::resources::Work::Release(value, anchor));
            }
            if runtime.trace_mode == TraceMode::Full {
                execution
                    .trace
                    .push(TraceEvent::LoopOwnership { released, live: 0 });
            }
            continue;
        }
        // A detached result has no acquiring owner. Dispose it through ordinary
        // shielded cleanup before ending the child's frame or publishing completion.
        // A finalizer failure then belongs to this child and its scope's chain.
        if task != 0 && frames.len() == 1 && operations.discard_task_result(task) {
            let frame = frames.last_mut().unwrap();
            if let Some(crate::resources::Ending::Return(value)) = &mut frame.ending
                && *value != Value::Unit
            {
                let value = std::mem::replace(value, Value::Unit);
                let site = site_of(frame)?;
                frame
                    .work
                    .push_back(crate::resources::Work::Release(value, anchor(frame, site)));
                continue;
            }
        }
        // PROG-3 transfers the entry Result to the host. Keep only a tooling
        // snapshot for observation; release the actual owner on this same stack.
        if task == 0 && frames.len() == 1 && !*entry_consumed {
            let frame = frames.last_mut().unwrap();
            if let Some(crate::resources::Ending::Return(value)) = &frame.ending {
                let site = site_of(frame)?;
                let root_anchor = anchor(frame, site);
                if operations.entry_returned(
                    value,
                    runtime,
                    &mut OperationContext {
                        configuration,
                        sources,
                        stdout: &mut execution.stdout,
                        trace: &mut execution.trace,
                        trace_mode: runtime.trace_mode,
                        anchor: Some(&root_anchor),
                    },
                )? {
                    returned_snapshot = Some(value.clone());
                    *entry_consumed = true;
                    let Some(crate::resources::Ending::Return(value)) = frame.ending.take() else {
                        unreachable!()
                    };
                    frame.ending = Some(crate::resources::Ending::Return(Value::Unit));
                    frame
                        .work
                        .push_back(crate::resources::Work::Release(value, root_anchor));
                    continue;
                }
            }
        }
        if let Some(ending) = frames.last_mut().unwrap().ending.take() {
            let mut finished = frames.pop().unwrap();
            // Map the result and mut write-backs into the caller's lifetimes, then
            // end every instance the frame owns (MODE-1, MEM-10, MEM-4).
            let mut returned = Shadow::default();
            let mut written = vec![];
            if let Some(check) = &mut finished.check {
                let body = &finished.definition.bodies[finished.body_id.0 as usize];
                returned = check.returned_to_caller(body, &finished.definition.types);
                for local in &finished.mut_parameters {
                    if let Some(ordinal) = body.parameters.iter().position(|p| p == local) {
                        written.push((*local, check.written_back(ordinal, *local)));
                    }
                }
                check.exit();
            }
            if runtime.trace_mode == TraceMode::Full {
                execution.trace.push(TraceEvent::Return {
                    path: finished.definition.path.clone(),
                    depth,
                });
            }
            if let Some((local, anchor)) = finished.finalized.take() {
                if matches!(&ending,crate::resources::Ending::Return(value) if *value != Value::Unit)
                {
                    return Err(invalid("finalizer must return unit"));
                }
                let mut value = finished.slots[local.0 as usize]
                    .take()
                    .ok_or_else(|| invalid("finalizer consumed self"))?;
                release_fields(frames.last_mut().unwrap(), &mut value, anchor)?;
                if matches!(ending, crate::resources::Ending::Failed) {
                    let caller = frames.last_mut().unwrap();
                    let already_exiting = caller.ending.is_some();
                    if let Some(crate::resources::Ending::Return(value)) = caller.ending.take() {
                        caller.work.push_back(crate::resources::Work::Release(
                            value,
                            crate::observation::Anchor {
                                definition: caller.definition.path.clone(),
                                body: caller.body_id,
                                cleanup: None,
                                site: site_of(caller)?,
                            },
                        ));
                    }
                    caller.ending = Some(crate::resources::Ending::Failed);
                    if !already_exiting {
                        if let Some(id) = failure_cleanup_at(caller) {
                            enqueue_cleanup(caller, id, site_of(caller)?)?;
                        }
                    }
                }
                continue;
            }
            if let Some(mut callback) = finished.callback.take() {
                let updates = callback
                    .mut_parameters
                    .drain(..)
                    .map(|(i, l)| {
                        finished
                            .slots
                            .get_mut(l.0 as usize)
                            .and_then(Option::take)
                            .map(|v| (i, v))
                            .ok_or_else(|| invalid("callback consumed mut parameter"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                writeback_callback(&mut callback, &updates)?;
                let abandoned = matches!(ending, crate::resources::Ending::Failed)
                    && execution.failures.get(callback.failure_start).is_none();
                let released = operations.callback_finished(callback.token, &updates, abandoned)?;
                let caller = frames.last_mut().unwrap();
                for value in released.into_iter().rev() {
                    caller.work.push_back(crate::resources::Work::Release(
                        value,
                        callback.state.anchor.clone(),
                    ));
                }

                if let Some(check) = &finished.check {
                    if matches!(&ending, crate::resources::Ending::Return(_)) {
                        check
                            .state
                            .borrow()
                            .access(&returned, "callback return to operation")?;
                    }
                    let shadows: Vec<_> = written.iter().map(|(_, s)| s.clone()).collect();
                    for shadow in &shadows {
                        check
                            .state
                            .borrow()
                            .access(shadow, "callback mut writeback")?;
                    }
                    if let Some(bounds) = &mut callback.state.bounds {
                        bounds.accept_callback(&returned, &shadows);
                    }
                }
                let result = match ending {
                    crate::resources::Ending::Return(value) => {
                        if updates.is_empty() {
                            OperationResult::returned(value)
                        } else {
                            OperationResult::Complete {
                                consumed: vec![],
                                released: vec![],
                                value,
                                arguments: updates,
                            }
                        }
                    }
                    crate::resources::Ending::Failed => {
                        callback.state.already_failed = true;
                        if let Some(f) = execution.failures.get(callback.failure_start) {
                            OperationResult::Failed(f.failure.clone())
                        } else {
                            let caller = frames.last_mut().unwrap();
                            caller.ending = Some(crate::resources::Ending::Failed);
                            abandon_operation(caller, callback.state)?;
                            continue;
                        }
                    }
                };
                let caller = frames.last_mut().unwrap();
                // Mark the caller as suspended in cleanup, so a failing owned
                // finalizer cannot run the caller's failure list prematurely.
                caller.ending = Some(crate::resources::Ending::Failed);
                enqueue_owned(caller, &mut callback.state);
                caller
                    .work
                    .push_back(crate::resources::Work::Resume(result, callback));
                continue;
            }
            if let Some(caller) = frames.last_mut() {
                let continuation = caller
                    .continuation
                    .take()
                    .ok_or_else(|| invalid("missing continuation"))?;
                // MEM-6: a bounded callback's result is Ok, and its own OutOfSpace,
                // with no failure during the cleanup that followed, is Error(OutOfSpace).
                let ending = match (finished.bounded, ending) {
                    (None, ending) => ending,
                    (Some(boundary), ending) => {
                        let caught = budgets.close(boundary, execution.failures.len());
                        match ending {
                            crate::resources::Ending::Return(value) => {
                                returned = Shadow::Parts(vec![returned]);
                                crate::resources::Ending::Return(Value::Variant {
                                    variant: 0,
                                    fields: vec![Some(value)],
                                })
                            }
                            crate::resources::Ending::Failed if caught => {
                                execution.failures.truncate(boundary.failures);
                                if let Some(last) = execution.failures.last_mut()
                                    && last.cause == Some(boundary.failures)
                                {
                                    last.cause = None;
                                }
                                returned = Shadow::default();
                                crate::resources::Ending::Return(Value::Variant {
                                    variant: 1,
                                    fields: vec![Some(Value::Variant {
                                        variant: 0,
                                        fields: vec![],
                                    })],
                                })
                            }
                            failed => failed,
                        }
                    }
                };
                for (place, local) in continuation.writebacks {
                    let value = finished
                        .slots
                        .get_mut(local.0 as usize)
                        .ok_or_else(|| invalid("mut parameter missing"))?
                        .take();
                    let shadow = written
                        .iter()
                        .find(|(l, _)| *l == local)
                        .map(|(_, s)| s.clone())
                        .unwrap_or_default();
                    caller.put(place, value, shadow)?;
                }
                match ending {
                    crate::resources::Ending::Return(value) => {
                        let Some(CallReturn { destination, next }) = continuation.returns else {
                            return Err(invalid("diverging callee returned"));
                        };
                        caller.assign_shadow(destination, value, returned)?;
                        caller.goto(next);
                    }
                    crate::resources::Ending::Failed => {
                        caller.ending = Some(crate::resources::Ending::Failed);
                        enqueue_cleanup(caller, continuation.on_failure, site_of(caller)?)?;
                    }
                }
            } else {
                if task != 0 && tooling.is_none() {
                    let result = if execution.failures.is_empty() {
                        let crate::resources::Ending::Return(value) = ending else {
                            return Err(invalid("task failed without a failure"));
                        };
                        OperationResult::returned(value)
                    } else {
                        OperationResult::FailureChain {
                            failures: std::mem::take(&mut execution.failures),
                            released: vec![],
                        }
                    };
                    operations.task_finished(
                        task,
                        result,
                        &mut OperationContext {
                            configuration,
                            sources,
                            stdout: &mut execution.stdout,
                            trace: &mut execution.trace,
                            trace_mode: runtime.trace_mode,
                            anchor: Some(&anchor(&finished, site_of(&finished)?)),
                        },
                    )?;
                    continue;
                }
                execution.outcome = if let Some(limit) = tooling {
                    Outcome::ToolingLimit(limit)
                } else if let Some(f) = execution.failures.first() {
                    Outcome::Failed(f.failure.clone())
                } else {
                    match ending {
                        crate::resources::Ending::Return(value) => {
                            if !*entry_consumed && let Some(check) = &finished.check {
                                // The harness reads the result after every frame ended.
                                check.state.borrow().access(&returned, "returned value")?;
                            }
                            let value = returned_snapshot.take().unwrap_or(value);
                            Outcome::Returned {
                                exit_code: exit_code(&value),
                                value,
                            }
                        }
                        _ => return Err(invalid("failed without failure")),
                    }
                };
                return Ok(());
            }
            continue;
        }
        let frame = frames.last_mut().unwrap();
        let block = frame
            .body()
            .blocks
            .get(frame.block.0 as usize)
            .ok_or_else(|| invalid("block out of range"))?;
        let instruction = block.instructions.get(frame.instruction).cloned();
        let terminator = block.terminator.clone();
        let site = instruction.as_ref().map_or(block.span, |i| i.span);
        let cleanup = instruction
            .as_ref()
            .map_or_else(|| terminator_cleanup(&terminator), |i| Some(i.on_failure));
        // Issue #34: failure occurs at the next scheduled site after entry.
        // Control-only edges have no current-frame failure list in frozen TIR.
        if frame.depth_failure || limits.steps == Some(execution.steps) {
            let limit = ToolingLimit {
                kind: if frame.depth_failure {
                    ToolingLimitKind::CallDepth
                } else {
                    ToolingLimitKind::Steps
                },
                limit: if frame.depth_failure {
                    limits.call_depth as u64
                } else {
                    limits.steps.expect("step limit reached")
                },
                site: Some(site),
            };
            if tooling.is_none() {
                if runtime.trace_mode == TraceMode::Full {
                    execution
                        .trace
                        .push(TraceEvent::ToolingLimit(limit.clone()));
                }
                tooling = Some(limit);
            }
            frame.ending = Some(crate::resources::Ending::Failed);
            if let Some(id) = cleanup {
                enqueue_cleanup(frame, id, site)?;
            }
            continue;
        }
        execution.steps = execution.steps.saturating_add(1);
        if let Some(check) = &frame.check {
            let mut state = check.state.borrow_mut();
            state.site = site;
            state.definition = Some(frame.definition.path.clone());
        }
        let result = if let Some(instruction) = instruction {
            if runtime.trace_mode == TraceMode::Full {
                execution.trace.push(TraceEvent::Instruction {
                    depth,
                    block: frame.block,
                    index: frame.instruction as u32,
                    site,
                });
            }
            frame.instruction += 1;
            match instruction.kind {
                InstructionKind::Check { condition, rule } => match frame.operand(&condition) {
                    Ok(Value::Bool(true)) => Ok(Action::Continue),
                    Ok(Value::Bool(false)) => Ok(Action::Fail(rule)),
                    Ok(_) => Err(invalid("check needs bool").into()),
                    Err(e) => Err(e.into()),
                },
                InstructionKind::Drop { place, if_live } => {
                    arena_write(frame, place, false, &mut budgets).and_then(|()| {
                        frame
                            .access(place, "drop")
                            .and_then(|_| frame.slot(place))
                            .map(|slot| Action::Drop {
                                value: slot.take(),
                                if_live,
                            })
                            .map_err(EvalError::from)
                    })
                }
                InstructionKind::RunCleanup(id) => Ok(Action::Cleanup(id)),
                kind => execute_instruction(
                    frame,
                    kind,
                    runtime,
                    site,
                    &mut execution.trace,
                    provider,
                    &mut constants,
                    &mut budgets,
                )
                .map(|_| Action::Continue),
            }
        } else {
            if runtime.trace_mode == TraceMode::Full {
                execution.trace.push(TraceEvent::Terminator {
                    depth,
                    block: frame.block,
                    site,
                });
            }
            execute_terminator(frame, terminator, runtime, &mut budgets)
        };
        match result {
            Err(EvalError::Scalar(ScalarError::InvalidOperation)) => {
                return Err(invalid("invalid scalar operation or literal type"));
            }
            Err(EvalError::Scalar(reason)) => {
                record_failure(
                    runtime.trace_mode,
                    execution,
                    failure(sources, site, "SEM-1", &format!("{reason:?}"))?,
                    anchor(frame, site),
                );
                frame.ending = Some(crate::resources::Ending::Failed);
                if let Some(id) = cleanup {
                    enqueue_cleanup(frame, id, site)?;
                }
            }
            Err(EvalError::OutOfSpace(arena)) => {
                budgets.exhausted(arena, execution.failures.len());
                record_failure(
                    runtime.trace_mode,
                    execution,
                    failure(sources, site, "MEM-6", "OutOfSpace")?,
                    anchor(frame, site),
                );
                frame.ending = Some(crate::resources::Ending::Failed);
                if let Some(id) = cleanup {
                    enqueue_cleanup(frame, id, site)?;
                }
            }
            Err(EvalError::Depth) => {
                record_failure(
                    runtime.trace_mode,
                    execution,
                    failure(sources, site, "MEM-3", "Depth")?,
                    anchor(frame, site),
                );
                frame.ending = Some(crate::resources::Ending::Failed);
                if let Some(id) = cleanup {
                    enqueue_cleanup(frame, id, site)?;
                }
            }
            Err(reason @ (EvalError::InactiveVariant | EvalError::OutOfBounds)) => {
                let (rule, kind) = match reason {
                    EvalError::InactiveVariant => ("MEM-13", "InactiveVariant"),
                    _ => ("MEM-14", "OutOfBounds"),
                };
                let site = frame.place_failure_site.take().unwrap_or(site);
                record_failure(
                    runtime.trace_mode,
                    execution,
                    failure(sources, site, rule, kind)?,
                    anchor(frame, site),
                );
                frame.ending = Some(crate::resources::Ending::Failed);
                if let Some(id) = cleanup {
                    enqueue_cleanup(frame, id, site)?;
                }
            }
            Err(EvalError::Api(error)) => return Err(error),
            Ok(Action::Continue) => (),
            Ok(Action::Drop { value, if_live }) => {
                if value.is_none() && !if_live {
                    return Err(invalid("unconditional drop of dead storage"));
                }
                if let Some(value) = value {
                    frame
                        .work
                        .push_back(crate::resources::Work::Release(value, anchor(frame, site)));
                }
            }
            Ok(Action::Cleanup(id)) => enqueue_cleanup(frame, id, site)?,
            Ok(Action::Fail(rule)) => {
                record_failure(
                    runtime.trace_mode,
                    execution,
                    failure(sources, site, rule.as_str(), "Check")?,
                    anchor(frame, site),
                );
                frame.ending = Some(crate::resources::Ending::Failed);
                if let Some(id) = cleanup {
                    enqueue_cleanup(frame, id, site)?;
                }
            }
            Ok(Action::Return(value)) => {
                frame.ending = Some(crate::resources::Ending::Return(value));
                if let Some(id) = cleanup {
                    enqueue_cleanup(frame, id, site)?;
                }
            }
            Ok(Action::Call {
                callee,
                mut checks,
                mut arguments,
                mut_places,
                taken,
                returns,
                on_failure,
            }) => {
                let (callee, closes_arena, bounded) =
                    checked_callee(frame, callee, &mut arguments, &mut checks, &mut budgets)?;
                let mut closure_check = None;
                let mut invocation_types = Default::default();
                let (definition, body, captures, sizes) = match callee {
                    RuntimeCallee::Closure {
                        definition,
                        body,
                        captures,
                        mut sizes,
                        generic_sizes,
                        actuals,
                        type_arguments,
                        check,
                    } => {
                        invocation_types = type_arguments;
                        closure_check = check;
                        let definition = provider.definition(&definition).map_err(Error::Query)?;
                        if !actuals.arguments.is_empty() {
                            crate::type_arguments::bind(
                                &mut invocation_types,
                                definition.bodies[body.0 as usize].parameter_scope,
                                actuals,
                            );
                        }
                        Frame::bind_sizes(
                            &mut sizes,
                            definition.bodies[body.0 as usize].parameter_scope,
                            generic_sizes,
                        );
                        (definition, body, captures, sizes)
                    }
                    RuntimeCallee::Definition {
                        path,
                        mut generics,
                        types,
                    } => {
                        let mut call_types = None;
                        if let Some(types) = types {
                            let mut merged = frame.definition.types.clone();
                            for argument in &mut generics {
                                if let hirsel_types::GenericArgument::Type(ty) = argument {
                                    *ty = hirsel_types::import_type(&types, *ty, &mut merged)
                                        .map_err(|_| invalid("function reference generic type"))?;
                                }
                            }
                            call_types = Some(merged);
                        }
                        let concrete = crate::type_arguments::concrete(
                            call_types.as_ref().unwrap_or(&frame.definition.types),
                            &generics,
                            &frame.type_arguments,
                        )?;
                        generics = concrete.arguments.clone();
                        call_types = Some((*concrete.types).clone());
                        let tail = frame
                            .body()
                            .tail_sites
                            .iter()
                            .find(|tail| {
                                tail.call_block == frame.block
                                    && tail.owner_body == frame.body_id
                                    && tail.kind == TailKind::SelfTail
                            })
                            .cloned();
                        if path == frame.definition.path && tail.is_some() {
                            let tail = tail.unwrap();
                            let released = frame.loop_owned.len();
                            for (value, anchor) in frame.loop_owned.drain(..).rev() {
                                frame
                                    .work
                                    .push_back(crate::resources::Work::Release(value, anchor));
                            }
                            for (_, place) in tail.owned_loop_arguments {
                                let value = frame
                                    .slot(place)?
                                    .take()
                                    .ok_or_else(|| invalid("dead counted tail argument"))?;
                                frame.loop_owned.push((value, anchor(frame, site)));
                            }
                            if runtime.trace_mode == TraceMode::Full {
                                execution.trace.push(TraceEvent::LoopOwnership {
                                    released,
                                    live: frame.loop_owned.len(),
                                });
                            }
                            let scope = frame.body().parameter_scope;
                            Frame::bind_sizes(
                                &mut frame.sizes,
                                scope,
                                generics.iter().map(|arg| match arg {
                                    hirsel_types::GenericArgument::Size(
                                        hirsel_types::Size::Constant(n),
                                    ) => Some(*n),
                                    _ => None,
                                }),
                            );
                            crate::type_arguments::bind(
                                &mut frame.type_arguments,
                                scope,
                                concrete.clone(),
                            );
                            frame.restart_tail(arguments)?;
                            if let Some(old) = frame.check.take() {
                                let mut entered = enter_checked(&old.state, frame, checks, None);
                                entered.inherit(old)?;
                                frame.check = Some(entered);
                            }
                            continue;
                        }

                        if runtime.forget.as_ref() == Some(&path) {
                            if arguments.len() != 1 || !mut_places.is_empty() {
                                return Err(invalid("forget arity/mode"));
                            }
                            let value = arguments.pop().unwrap();
                            let (Value::Resource { definition, .. }
                            | Value::ResourceVariant { definition, .. }) = &value
                            else {
                                return Err(invalid("forget requires resource"));
                            };
                            if definition.0.package != frame.definition.path.0.package
                                || definition.0.modules != frame.definition.path.0.modules
                            {
                                return Err(invalid("cross-module forget"));
                            }
                            effect(
                                &mut execution.trace,
                                anchor(frame, site),
                                crate::observation::Effect::Forgotten {
                                    definition: definition.clone(),
                                },
                            );
                            if let Some(check) = &frame.check {
                                check.state.borrow_mut().forgotten +=
                                    crate::checked::resources_in(&value);
                            }
                            crate::aggregate::forget(value)?;
                            let Some(CallReturn { destination, next }) = returns else {
                                return Err(invalid("diverging forget"));
                            };
                            frame.assign(destination, Value::Unit)?;
                            frame.goto(next);
                            continue;
                        }
                        // A diverging operation is offered a Never result row.
                        let result_type = match returns {
                            Some(r) => frame.place(r.destination)?.ty,
                            None => call_types
                                .get_or_insert_with(|| frame.definition.types.clone())
                                .push(hirsel_types::Type {
                                    kind: hirsel_types::TypeKind::Primitive(
                                        hirsel_types::Primitive::Never,
                                    ),
                                    ..hirsel_types::Type::unit()
                                }),
                        };
                        let types = call_types.as_ref().unwrap_or(&frame.definition.types);
                        // MEM-6: a copy into a bounded arena charges its whole tree first,
                        // and builds every List and Array of it unshared there.
                        let exhausted = if path == hirsel_syntax::DefPath::core("copy_to") {
                            copy_to_arena(
                                runtime,
                                types,
                                result_type,
                                &mut arguments,
                                &mut budgets,
                            )?
                        } else {
                            list_write(&path, &mut arguments, &mut budgets)
                        };
                        let result = if let Some(arena) = exhausted {
                            budgets.exhausted(arena, execution.failures.len());
                            Some(OperationResult::Failed(failure(
                                sources,
                                site,
                                "MEM-6",
                                "OutOfSpace",
                            )?))
                        } else {
                            operations.call(
                                OperationCall {
                                    runtime,
                                    path: &path,
                                    generics: &generics,
                                    arguments: &arguments,
                                    result_type,
                                    types,
                                    site,
                                },
                                &mut OperationContext {
                                    configuration,
                                    sources,
                                    stdout: &mut execution.stdout,
                                    trace: &mut execution.trace,
                                    trace_mode: runtime.trace_mode,
                                    anchor: Some(&anchor(frame, site)),
                                },
                            )?
                        };
                        if let Some(result) = result {
                            let bounds = frame.check.as_ref().map(|check| {
                                operation_bounds(
                                    &check.state.borrow().config,
                                    &path,
                                    &checks,
                                    &mut_places,
                                    &taken,
                                )
                            });
                            frame.operation = Some((
                                result,
                                crate::resources::OperationState {
                                    arguments,
                                    owned: vec![],
                                    taken,
                                    finishing: None,
                                    mut_places,
                                    returns,
                                    on_failure,
                                    anchor: anchor(frame, site),
                                    already_failed: false,
                                    callback: None,
                                    callback_mut_arguments: vec![],
                                    bounds,
                                },
                            ));
                            continue;
                        }
                        // Generic TIR executes parametrically: scalar values retain
                        // their widths and aggregates retain their runtime shape.
                        // Static bounds and call types were checked at publication.
                        let d = provider.definition(&path).map_err(Error::Query)?;
                        let entry = d.entry;
                        let actuals = generics
                            .iter()
                            .map(|argument| match argument {
                                hirsel_types::GenericArgument::Size(
                                    hirsel_types::Size::Constant(n),
                                ) => Some(*n),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        let mut sizes = Default::default();
                        Frame::bind_sizes(
                            &mut sizes,
                            d.bodies[entry.0 as usize].parameter_scope,
                            actuals,
                        );
                        crate::type_arguments::bind(
                            &mut invocation_types,
                            d.bodies[entry.0 as usize].parameter_scope,
                            concrete,
                        );
                        (d, entry, vec![], sizes)
                    }
                };
                let path = definition.path.clone();
                let mut callee = Frame::new_body(definition, body, arguments, captures, sizes)?;
                callee.type_arguments = invocation_types;
                if let Some(check) = &frame.check {
                    let mut entered = enter_checked(&check.state, &callee, checks, closure_check);
                    entered.closes_arena = closes_arena;
                    callee.check = Some(entered);
                }
                // MEM-6: the callback never runs with an unbounded arena.
                callee.bounded = bounded.map(|(arena, cap)| {
                    budgets.open(arena, cap);
                    crate::bounded::Boundary {
                        arena,
                        failures: execution.failures.len(),
                    }
                });
                let writebacks = mut_places
                    .into_iter()
                    .map(|(i, p)| {
                        callee
                            .body()
                            .parameters
                            .get(i)
                            .copied()
                            .map(|l| (p, l))
                            .ok_or_else(|| invalid("mut parameter ordinal"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                callee.mut_parameters = writebacks.iter().map(|(_, l)| *l).collect();
                callee.depth_failure = depth >= limits.call_depth;
                frame.continuation = Some(Continuation {
                    returns,
                    on_failure,
                    writebacks,
                });
                frames.push(callee);
                if runtime.trace_mode == TraceMode::Full {
                    execution.trace.push(TraceEvent::Enter {
                        path,
                        depth: depth + 1,
                    });
                }
            }
        }
    }
}
fn failure(sources: &SourceMap, site: Span, rule: &str, kind: &str) -> Result<Failure, Error> {
    Ok(Failure {
        rule: RuleId::new(rule).map_err(|_| invalid("invalid failure rule"))?,
        line: sources
            .location(site)
            .ok_or_else(|| invalid("failure site absent from source map"))?
            .line,
        site,
        kind: kind.into(),
    })
}
fn exit_code(value: &Value) -> i32 {
    // PROG-3/FAIL-1 issue #34: provisional Result ordinal and exit mapping.
    // Core fixtures may return other values; those exit zero.
    match value {
        Value::Unit => 0,
        Value::Variant { variant: 0, .. } => 0,
        Value::Variant { variant: 1, .. } => 1,
        _ => 0,
    }
}
fn terminator_cleanup(terminator: &Terminator) -> Option<CleanupId> {
    match terminator {
        Terminator::Call { on_failure, .. } => Some(*on_failure),
        Terminator::Return { cleanup, .. } | Terminator::Fail { cleanup, .. } => Some(*cleanup),
        _ => None,
    }
}
fn anchor(frame: &Frame, site: Span) -> crate::observation::Anchor {
    crate::observation::Anchor {
        definition: frame.definition.path.clone(),
        body: frame.body_id,
        cleanup: None,
        site,
    }
}
fn site_of(frame: &Frame) -> Result<Span, Error> {
    Ok(frame
        .body()
        .blocks
        .get(frame.block.0 as usize)
        .ok_or_else(|| invalid("block"))?
        .span)
}
fn effect(
    trace: &mut Vec<TraceEvent>,
    anchor: crate::observation::Anchor,
    effect: crate::observation::Effect,
) {
    trace.push(TraceEvent::Effect(crate::observation::OrderedEffect {
        anchor,
        effect,
    }));
}
fn record_failure(
    trace_mode: TraceMode,
    execution: &mut Execution,
    failure: Failure,
    anchor: crate::observation::Anchor,
) {
    let index = execution.failures.len();
    if let Some(previous) = execution.failures.last_mut() {
        previous.cause = Some(index);
    }
    if trace_mode == TraceMode::Full {
        execution.trace.push(TraceEvent::Failure(failure.clone()));
    }
    execution.failures.push(crate::observation::FailureRecord {
        failure,
        anchor,
        cause: None,
        spawn_path: vec![],
    });
}
/// The binding whose Drop action a cleanup anchor names: a drop's own failure is
/// reported there, as the LIR cleanup anchor of that drop is.
fn dropped_binding_site(frame: &Frame, anchor: &crate::observation::Anchor) -> Option<Span> {
    let (id, index) = anchor.cleanup?;
    let body = frame.body();
    let mut node = body.cleanups.get(id.0 as usize)?.head?;
    for _ in 0..index {
        node = body.cleanup_nodes.get(node.0 as usize)?.parent?;
    }
    let CleanupAction::Drop { place, .. } = &body.cleanup_nodes.get(node.0 as usize)?.action else {
        return None;
    };
    let root = body.places.get(place.0 as usize)?.root;
    Some(body.locals.get(root.0 as usize)?.span)
}
fn enqueue_cleanup(frame: &mut Frame, id: CleanupId, site: Span) -> Result<(), Error> {
    let head = frame
        .body()
        .cleanups
        .get(id.0 as usize)
        .ok_or_else(|| invalid("cleanup out of range"))?
        .head;
    if let Some(head) = head {
        let mut a = anchor(frame, site);
        a.cleanup = Some((id, 0));
        frame.work.push_back(crate::resources::Work::Chain(head, a));
    }
    Ok(())
}
fn release_fields(
    frame: &mut Frame,
    value: &mut Value,
    anchor: crate::observation::Anchor,
) -> Result<(), Error> {
    if let Value::Closure {
        captures,
        owned_captures,
        invoked,
        ..
    } = value
    {
        if captures.len() != owned_captures.len() {
            return Err(invalid("capture ownership count"));
        }
        for (capture, owned) in captures.iter_mut().zip(owned_captures) {
            if let Some(v) = capture.take() {
                if *owned && !invoked.as_ref().is_some_and(|v| v.get().is_some()) {
                    frame
                        .work
                        .push_front(crate::resources::Work::Release(v, anchor.clone()));
                } else {
                    v.discard();
                }
            }
        }
    } else if let Some(fields) = value.fields_mut() {
        // Push front in declaration order: the worklist visits high to low (DROP-2).
        for field in fields.iter_mut() {
            if let Some(v) = field.take() {
                frame
                    .work
                    .push_front(crate::resources::Work::Release(v, anchor.clone()));
            }
        }
    }
    Ok(())
}
enum EvalError {
    InactiveVariant,
    OutOfBounds,
    Depth,
    /// A charge exceeded this bounded arena's budget (MEM-6).
    OutOfSpace(u64),
    Api(Error),
    Scalar(ScalarError),
}
impl From<crate::frame::AccessError> for EvalError {
    fn from(error: crate::frame::AccessError) -> Self {
        match error {
            crate::frame::AccessError::InactiveVariant => Self::InactiveVariant,
            crate::frame::AccessError::OutOfBounds => Self::OutOfBounds,
            crate::frame::AccessError::Api(error) => Self::Api(error),
        }
    }
}
impl From<Error> for EvalError {
    fn from(e: Error) -> Self {
        Self::Api(e)
    }
}
impl From<ScalarError> for EvalError {
    fn from(e: ScalarError) -> Self {
        Self::Scalar(e)
    }
}
enum Action {
    Drop {
        value: Option<Value>,
        if_live: bool,
    },
    Cleanup(CleanupId),
    Continue,
    Return(Value),
    Fail(RuleId),
    Call {
        callee: RuntimeCallee,
        checks: Vec<crate::checked::Argument>,
        arguments: Vec<Value>,
        mut_places: Vec<(usize, PlaceId)>,
        taken: Vec<usize>,
        returns: Option<CallReturn>,
        on_failure: CleanupId,
    },
}
fn rvalue(
    frame: &mut Frame,
    id: RvalueId,
    destination: PlaceId,
    runtime: &crate::aggregate::RuntimeTypes,
    provider: &mut (impl TirProvider + ?Sized),
    constants: &mut crate::constants::Constants,
    budgets: &mut crate::bounded::Budgets,
) -> Result<(Value, Shadow), EvalError> {
    let value = frame
        .body()
        .rvalues
        .get(id.0 as usize)
        .ok_or_else(|| invalid("rvalue out of range"))?
        .clone();
    let mut shadow = Shadow::default();
    let value = match value {
        Rvalue::Constant(c) => {
            let ty = frame.place(destination)?.ty;
            let ty = &frame
                .definition
                .types
                .get(ty)
                .ok_or_else(|| invalid("type out of range"))?
                .kind;
            let value = scalar::constant(&c, ty)?;
            if frame.check.is_some() {
                shadow = shadow.ownership(&value, true);
            }
            value
        }
        Rvalue::Size(parameter) => {
            let ty = frame.place(destination)?.ty;
            if !matches!(
                frame.definition.types.get(ty).map(|t| &t.kind),
                Some(hirsel_types::TypeKind::Primitive(
                    hirsel_types::Primitive::U64
                ))
            ) {
                return Err(invalid("generic size read must have type u64").into());
            }
            Value::Integer(
                crate::Integer::new(hirsel_types::Primitive::U64, frame.size(parameter)? as u128)
                    .ok_or_else(|| invalid("invalid generic size"))?,
            )
        }
        Rvalue::FunctionReference {
            definition,
            arguments,
        } => {
            let arguments = arguments
                .into_iter()
                .map(|argument| match argument {
                    hirsel_types::GenericArgument::Size(hirsel_types::Size::Parameter(p)) => {
                        frame.size(p).map(|n| {
                            hirsel_types::GenericArgument::Size(hirsel_types::Size::Constant(n))
                        })
                    }
                    other => Ok(other),
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let concrete = crate::type_arguments::concrete(
                &frame.definition.types,
                &arguments,
                &frame.type_arguments,
            )?;
            Value::FunctionReference(crate::value::NamedFunction {
                definition,
                arguments: concrete.arguments,
                types: concrete.types,
                ty: frame.place(destination)?.ty,
            })
        }
        Rvalue::ReadConstant { definition } => {
            let value = constants.read(provider, &definition, runtime)?;
            if frame.check.is_some() {
                shadow = shadow.ownership(&value, false);
            }
            value
        }
        Rvalue::Use(operand) => {
            let (value, bound) = frame.operand_shadow(&operand)?;
            shadow = bound;
            value
        }
        Rvalue::Unary {
            op: hirsel_syntax::UnaryOp::Unique,
            value,
        } => {
            if value.usage != hirsel_types::UseKind::Copy {
                return Err(invalid("deep unique requires copy extraction").into());
            }
            if frame.check.as_ref().is_some_and(|c| c.noalloc)
                && !frame.placed(frame.place(value.place)?.ty)
                && frame
                    .slot(value.place)?
                    .as_ref()
                    .is_some_and(crate::checked::holds_storage)
            {
                if let Some(check) = &frame.check {
                    return Err(check
                        .state
                        .borrow()
                        .trap(
                            crate::checked::TrapRule::AllocationInNoalloc,
                            "unique deep-copies owned blocks while its source remains live",
                        )
                        .into());
                }
            }
            // Value copies traverse owned fields; Shared roots clone their
            // handle identity and retain their existing contents as a boundary.
            let (mut value, bound) = frame.operand_shadow(&value)?;
            if let Some(check) = &frame.check {
                shadow = bound.fresh_copy(&check.state.borrow());
            }
            value.fresh_arena_storage();
            value
        }
        Rvalue::Unary { op, value } => scalar::unary(op, frame.operand(&value)?)?,
        Rvalue::Binary { op, left, right } => {
            let left = frame.operand(&left)?;
            let right = frame.operand(&right)?;
            if matches!(
                op,
                hirsel_syntax::BinaryOp::Eq
                    | hirsel_syntax::BinaryOp::Ne
                    | hirsel_syntax::BinaryOp::Lt
                    | hirsel_syntax::BinaryOp::Le
                    | hirsel_syntax::BinaryOp::Gt
                    | hirsel_syntax::BinaryOp::Ge
            ) {
                let result = crate::structural::binary(op, &left, &right).map_err(|e| match e {
                    crate::structural::StructuralError::Depth => EvalError::Depth,
                    _ => invalid("invalid structural operation").into(),
                });
                left.discard();
                right.discard();
                result?
            } else {
                scalar::binary(op, left, right)?
            }
        }
        Rvalue::Construct {
            constructor,
            fields,
            destination: placement,
        } => {
            // MEM-6: charge a bounded arena before any operand is consumed.
            if let Some(arena) = bounded_destination(frame, &placement, budgets) {
                let ty = frame.place(destination)?.ty;
                let bytes = crate::bounded::construction(
                    runtime,
                    &frame.definition.types,
                    ty,
                    &constructor,
                    fields.len(),
                )?;
                budgets
                    .charge(arena, bytes)
                    .map_err(EvalError::OutOfSpace)?;
            }
            let mut values = vec![None; fields.len()];
            let mut parts = vec![Shadow::default(); fields.len()];
            for field in fields {
                let (value, bound) = frame.operand_shadow(&field.value)?;
                let slot = values
                    .get_mut(field.field as usize)
                    .ok_or_else(|| invalid("constructor field out of range"))?;
                if slot.is_some() {
                    return Err(invalid("duplicate constructor field").into());
                }
                *slot = Some(value);
                parts[field.field as usize] = bound;
            }
            if frame.check.is_some() {
                let inline_sequence = matches!(
                    frame
                        .definition
                        .types
                        .get(frame.place(destination)?.ty)
                        .map(|t| &t.kind),
                    Some(hirsel_types::TypeKind::Builtin {
                        kind: hirsel_types::BuiltinType::Inline | hirsel_types::BuiltinType::Simd,
                        ..
                    })
                );
                let block = match constructor {
                    Constructor::Map | Constructor::Set | Constructor::Box => true,
                    Constructor::Sequence => !inline_sequence,
                    _ => false,
                };
                let region = if block {
                    checked_allocation(frame, &placement)?
                } else {
                    None
                };
                shadow = Shadow::Parts(parts);
                if let Destination::Arena(origin) = &placement {
                    if let Some(arena) = frame.arena_of(origin) {
                        shadow.add_all(arena);
                    }
                }
                if let Some(region) = region {
                    shadow.add_all(region);
                }
            }
            let kind = &frame
                .definition
                .types
                .get(frame.place(destination)?.ty)
                .ok_or_else(|| invalid("destination type"))?
                .kind;
            let sizes = if let hirsel_types::TypeKind::Named { arguments, .. } = kind {
                arguments
                    .iter()
                    .map(|argument| match argument {
                        hirsel_types::GenericArgument::Size(hirsel_types::Size::Constant(n)) => {
                            Ok(Some(*n))
                        }
                        hirsel_types::GenericArgument::Size(hirsel_types::Size::Parameter(p)) => {
                            frame.size(*p).map(Some)
                        }
                        _ => Ok(None),
                    })
                    .collect::<Result<Vec<_>, Error>>()?
            } else {
                vec![]
            };
            let mut value = crate::aggregate::construct(constructor, values, kind, runtime, sizes)?;
            // MEM-6: a List or Array built in an arena is unshared storage there.
            if let Destination::Arena(origin) = &placement
                && let Some(arena) = frame.arena_of(origin)
                && let Value::Sequence {
                    kind, arena: tag, ..
                } = &mut value
            {
                let arena = u64::from(arena);
                *tag = crate::bounded::storage(
                    runtime,
                    &frame.definition.types,
                    frame.place(destination)?.ty,
                    *kind,
                    arena,
                    budgets.bounded(arena),
                )?;
            }
            if frame.check.is_some() {
                shadow = shadow.ownership(&value, true);
            }
            value
        }
        Rvalue::Closure {
            body,
            captures,
            destination: placement,
        } => {
            if !captures.is_empty()
                && let Some(arena) = bounded_destination(frame, &placement, budgets)
            {
                let types = captures
                    .iter()
                    .map(|c| frame.place(c.place).map(|p| p.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let bytes = crate::bounded::environment(runtime, &frame.definition.types, &types)?;
                budgets
                    .charge(arena, bytes)
                    .map_err(EvalError::OutOfSpace)?;
            }
            let owned_captures = captures
                .iter()
                .map(|o| {
                    matches!(
                        o.usage,
                        hirsel_types::UseKind::Copy | hirsel_types::UseKind::Take
                    )
                })
                .collect();
            let region = if frame.check.is_some() && !captures.is_empty() {
                checked_allocation(frame, &placement)?
            } else {
                None
            };
            let mut parts = vec![];
            let mut values = vec![];
            for operand in &captures {
                let (value, mut bound) = frame.operand_shadow(operand)?;
                if matches!(
                    operand.usage,
                    hirsel_types::UseKind::Borrow | hirsel_types::UseKind::Mut
                ) {
                    // A borrowed capture refers to the captured slot's storage.
                    if let Some(home) = home(frame, operand.place)? {
                        bound = Shadow::View {
                            content: Box::new(bound),
                            home,
                        };
                    }
                }
                parts.push(bound);
                values.push(Some(value));
            }
            let captures = values;
            shadow = Shadow::Parts(parts);
            if let Some(region) = region {
                // The environment block lives in the region, so the closure
                // is reached through it; what a capture holds or views is not
                // stored there, so a call's result is not bounded by it.
                shadow = Shadow::View {
                    content: Box::new(shadow),
                    home: vec![region],
                };
            }
            Value::Closure {
                invoked: frame
                    .definition
                    .types
                    .get(frame.place(destination)?.ty)
                    .is_some_and(|t| t.modes.multiplicity == hirsel_types::Multiplicity::Once)
                    .then(|| std::sync::Arc::new(std::sync::OnceLock::new())),
                definition: frame.definition.path.clone(),
                body,
                captures,
                owned_captures,
                sizes: frame.sizes.clone(),
                type_arguments: frame.type_arguments.clone(),
            }
        }
        _ => {
            return Err(
                Error::Unavailable("rvalue requires aggregate/shape extension".into()).into(),
            );
        }
    };
    let shadow = if frame.check.is_some() {
        shadow.ownership(&value, true).for_value(&value)
    } else {
        shadow.for_value(&value)
    };
    Ok((value, shadow))
}
/// MEM-6: `copy_to(a, x)` charges a bounded arena `x`'s whole tree, then retags the
/// copied value's Lists and Arrays as fresh storage of `a` (of the heap for
/// `heap`). Returns the arena whose budget the copy exceeds.
fn copy_to_arena(
    runtime: &crate::aggregate::RuntimeTypes,
    types: &hirsel_types::TypeStore,
    ty: hirsel_types::TypeId,
    arguments: &mut [Value],
    budgets: &mut crate::bounded::Budgets,
) -> Result<Option<u64>, Error> {
    let [Value::Handle { kind, id }, value] = arguments else {
        return Ok(None);
    };
    let arena = (kind == crate::checked::ARENA_HANDLE).then_some(*id);
    if let Some(arena) = arena
        && budgets.bounded(arena)
    {
        let bytes = crate::bounded::copy(runtime, types, ty, value)?;
        if let Err(arena) = budgets.charge(arena, bytes) {
            return Ok(Some(arena));
        }
    }
    let arena = arena.map(|arena| (arena, budgets.bounded(arena)));
    crate::bounded::retag(runtime, types, ty, value, arena)?;
    Ok(None)
}

/// MEM-6: a push to an arena List charges its element before it consumes its
/// argument; a write to a shared arena List (`push`, `pop`, `remove`, `swap`)
/// first copies it within its arena, charged its length, and the operation
/// writes the unshared copy. A call that fails its bounds check or moves no
/// element (`pop` of an empty List, `swap(xs, i, i)`) writes nothing, and
/// `reserve` is only a hint there. Returns the arena whose budget is exceeded.
fn list_write(
    path: &hirsel_syntax::DefPath,
    arguments: &mut [Value],
    budgets: &mut crate::bounded::Budgets,
) -> Option<u64> {
    let name = ["list.push", "list.pop", "list.remove", "list.swap"]
        .into_iter()
        .find(|name| *path == hirsel_syntax::DefPath::core(name))?;
    let index = |at: usize| match arguments.get(at) {
        Some(Value::Integer(i)) => Some(i.bits()),
        _ => None,
    };
    let (a, c) = (index(1), index(2));
    let Some(Value::Sequence {
        kind: hirsel_types::BuiltinType::List,
        elements,
        arena: Some(storage),
    }) = arguments.first_mut()
    else {
        return None;
    };
    let len = elements.len() as u128;
    let writes = match name {
        "list.push" => true,
        "list.pop" => len > 0,
        "list.remove" => a.is_some_and(|a| a < len),
        _ => matches!((a, c), (Some(a), Some(c)) if a < len && c < len && a != c),
    };
    if !writes {
        return None;
    }
    if name == "list.push"
        && let Err(arena) = budgets.charge(storage.arena, storage.element)
    {
        return Some(arena);
    }
    if storage.shared() {
        let bytes = storage.element.saturating_mul(elements.len() as u64);
        if let Err(arena) = budgets.charge(storage.arena, bytes) {
            return Some(arena);
        }
        *storage = crate::value::ArenaStorage::fresh(storage.arena, storage.element);
        // The original and the copy both hold every element.
        elements
            .iter()
            .flatten()
            .for_each(crate::value::Value::share_arena_storage);
    }
    None
}

/// MEM-6: copy the shared arena Lists and Arrays a write or `mut` lend at `place`
/// writes through, charging each its copied length.
fn arena_write(
    frame: &mut Frame,
    place: PlaceId,
    lend: bool,
    budgets: &mut crate::bounded::Budgets,
) -> Result<(), EvalError> {
    match frame.arena_write(place, lend, |arena, bytes| budgets.charge(arena, bytes))? {
        Some(arena) => Err(EvalError::OutOfSpace(arena)),
        None => Ok(()),
    }
}
/// The bounded arena a construction's destination names, if any.
fn bounded_destination(
    frame: &Frame,
    placement: &Destination,
    budgets: &crate::bounded::Budgets,
) -> Option<u64> {
    let Destination::Arena(origin) = placement else {
        return None;
    };
    let arena = u64::from(frame.arena_of(origin)?);
    budgets.bounded(arena).then_some(arena)
}
/// DROP-2: later declarations in this scope and all live descendant owners
/// must release first, even when the exit uses several cleanup chains.
fn checked_release_order(frame: &Frame, place: PlaceId, value: &Value) -> Result<(), Error> {
    let Some(check) = &frame.check else {
        return Ok(());
    };
    let place = frame.place(place)?;
    let local = &frame.body().locals[place.root.0 as usize];
    if !place.projection.is_empty()
        || local.storage != Storage::Local
        || crate::checked::resources_in(value) == 0
    {
        return Ok(());
    }
    for (index, other) in frame.body().locals.iter().enumerate() {
        if other.storage != Storage::Local
            || index == place.root.0 as usize
            || !frame.slots[index]
                .as_ref()
                .is_some_and(|v| crate::checked::resources_in(v) != 0)
        {
            continue;
        }
        let later = other.scope == local.scope
            && (other.span.start, index) > (local.span.start, place.root.0 as usize);
        let mut scope = frame.body().scopes[other.scope.0 as usize].parent;
        let mut descendant = false;
        while let Some(id) = scope {
            if id == local.scope {
                descendant = true;
                break;
            }
            scope = frame.body().scopes[id.0 as usize].parent;
        }
        if later || descendant {
            return Err(check.state.borrow().trap(
                crate::checked::TrapRule::FinalizerOrder,
                format!(
                    "owner {} released before live later/inner owner {index}",
                    place.root.0
                ),
            ));
        }
    }
    Ok(())
}
/// Bind a new frame's lifetimes; `@noalloc` applies to a definition's entry body.
fn enter_checked(
    state: &crate::checked::Shared,
    frame: &Frame,
    arguments: Vec<crate::checked::Argument>,
    callee: Option<crate::checked::Callee>,
) -> crate::checked::FrameCheck {
    let noalloc = frame.body_id == frame.definition.entry
        && state
            .borrow()
            .config
            .noalloc
            .contains(&frame.definition.path);
    crate::checked::FrameCheck::enter(
        state.clone(),
        frame.body(),
        &frame.definition.types,
        &frame.slots,
        arguments,
        callee,
        noalloc,
    )
}
/// The home instance of a place's root slot, under checked execution.
fn home(frame: &mut Frame, place: PlaceId) -> Result<Option<crate::checked::Bound>, Error> {
    let root = frame.place(place)?.root;
    let body = frame.body_id;
    let Frame {
        check, definition, ..
    } = frame;
    Ok(check
        .as_mut()
        .map(|c| c.home(&definition.bodies[body.0 as usize], root)))
}
/// MEM-5: a `@noalloc` body never constructs a block on the counted heap. An
/// explicit heap or caller destination traps at once; an inferred block is
/// placed in the frame's own region (MEM-10), so it traps if it escapes the frame.
fn checked_allocation(frame: &Frame, placement: &Destination) -> Result<Option<u32>, Error> {
    let Some(check) = &frame.check else {
        return Ok(None);
    };
    if !check.noalloc {
        return Ok(None);
    }
    match placement {
        Destination::Arena(_) | Destination::Region(_) => Ok(None),
        Destination::Inferred => Ok(Some(check.region)),
        Destination::Heap | Destination::Caller(_) => Err(check.state.borrow().trap(
            crate::checked::TrapRule::AllocationInNoalloc,
            format!("block constructed with destination {placement:?}"),
        )),
    }
}
fn execute_instruction(
    frame: &mut Frame,
    kind: InstructionKind,
    runtime: &crate::aggregate::RuntimeTypes,
    site: Span,
    trace: &mut Vec<TraceEvent>,
    provider: &mut (impl TirProvider + ?Sized),
    constants: &mut crate::constants::Constants,
    budgets: &mut crate::bounded::Budgets,
) -> Result<(), EvalError> {
    match kind {
        InstructionKind::Defuse { place } => {
            let slot = frame.slot(place)?;
            let value = slot
                .take()
                .ok_or_else(|| invalid("defuse of dead storage"))?;
            *slot = Some(match value {
                Value::Resource {
                    definition, fields, ..
                } => Value::Record { definition, fields },
                Value::ResourceVariant {
                    variant, fields, ..
                } => Value::Variant { variant, fields },
                _ => return Err(invalid("defuse of non-resource storage").into()),
            });
            // The parent ownership obligation ends here; its remaining field
            // shell is released normally, without re-entering the finalizer.
            if let Some(check) = &frame.check {
                check.state.borrow_mut().released += 1;
            }
        }
        InstructionKind::StorageDead { place, if_live } => {
            let slot = frame.slot(place)?;
            if slot.take().is_none() && !if_live {
                return Err(invalid("StorageDead on dead storage").into());
            }
            frame.put(place, None, Shadow::default())?;
        }
        InstructionKind::EndScope(scope) => {
            let body = &frame.definition.bodies[frame.body_id.0 as usize];
            for (index, local) in body.locals.iter().enumerate() {
                let mut current = Some(local.scope);
                while let Some(id) = current {
                    if id == scope {
                        frame.slots[index].take();
                        if let Some(check) = &mut frame.check {
                            check.shadows[index] = Shadow::default();
                        }
                        break;
                    }
                    current = body.scopes[id.0 as usize].parent;
                }
            }
            if let Some(check) = &mut frame.check {
                check.end_scope(body, scope);
            }
        }
        InstructionKind::StorageLive(local) => {
            let slot = frame
                .slots
                .get_mut(local.0 as usize)
                .ok_or_else(|| invalid("local out of range"))?;
            if slot.is_some() {
                return Err(invalid("StorageLive on live storage").into());
            }
            if let Some(check) = &mut frame.check {
                check.storage_live(local);
            }
        }
        InstructionKind::Replace {
            destination,
            value,
            displaced,
            if_live,
        } => {
            // Resolve potentially failing projections before extracting ownership.
            if !if_live && frame.slot(destination)?.is_none() || frame.slot(displaced)?.is_some() {
                return Err(invalid(
                    "replacement requires live destination and dead displaced storage",
                )
                .into());
            }
            let (value, shadow) = frame.operand_shadow(&value)?;
            arena_write(frame, destination, false, budgets)?;
            frame.write_access(destination)?;
            let old_shadow = frame.shadow_of(destination)?;
            let old = frame.slot(destination)?.replace(value);
            frame.set_shadow(destination, shadow)?;
            if let Some(old) = old {
                frame.assign_shadow(displaced, old, old_shadow)?;
            }
        }
        InstructionKind::Assign { destination, value } => {
            let created = matches!(
                frame.body().rvalues.get(value.0 as usize),
                Some(Rvalue::Construct { .. })
            );
            let (value, shadow) = rvalue(
                frame,
                value,
                destination,
                runtime,
                provider,
                constants,
                budgets,
            )?;
            if let Value::Resource { definition, .. } | Value::ResourceVariant { definition, .. } =
                &value
            {
                if created {
                    if let Some(check) = &frame.check {
                        check.state.borrow_mut().constructed += 1;
                    }
                    effect(
                        trace,
                        anchor(frame, site),
                        crate::observation::Effect::ResourceCreated {
                            definition: definition.clone(),
                        },
                    );
                }
            }
            arena_write(frame, destination, false, budgets)?;
            frame.write_access(destination)?;
            frame.assign_shadow(destination, value, shadow)?;
        }
        InstructionKind::Snapshot { destination, value } => {
            let (value, shadow) = frame.operand_shadow(&value)?;
            frame.assign_shadow(destination, value, shadow)?;
        }
        InstructionKind::Drop { .. } | InstructionKind::RunCleanup(_) => unreachable!(),
        // Binding an origin computes both places. An index outside its
        // sequence or a field of an inactive variant still fails where the
        // place is accessed, with that access's cleanup, but at the site of
        // the expression naming the place (the diagnostics part's primary site is the
        // operation that fails, not the enclosing statement).
        InstructionKind::BindOrigin { source, place, .. } => {
            for id in [source, place] {
                if let Err(
                    crate::frame::AccessError::OutOfBounds
                    | crate::frame::AccessError::InactiveVariant,
                ) = frame.slot(id)
                    && frame.place_failure_site.is_none()
                {
                    frame.place_failure_site = Some(site);
                }
            }
        }
        InstructionKind::BeginLoan { .. }
        | InstructionKind::EndLoan { .. }
        | InstructionKind::BindFields { .. }
        | InstructionKind::EndOrigin(_) => {}
        InstructionKind::Check { .. } => {
            unreachable!("Check is handled in the instruction action wrapper")
        }
    }
    Ok(())
}
fn execute_terminator(
    frame: &mut Frame,
    terminator: Terminator,
    runtime: &crate::aggregate::RuntimeTypes,
    budgets: &mut crate::bounded::Budgets,
) -> Result<Action, EvalError> {
    Ok(match terminator {
        Terminator::Match { table, from } => {
            let table = &frame.body().matches[table.0 as usize];
            let mut selected = None;
            for arm in table.arms.iter().skip(from as usize) {
                if frame.pattern_matches(table, arm.pattern)? {
                    selected = Some(arm.guard.unwrap_or(arm.target));
                    break;
                }
            }
            frame.goto(selected.ok_or_else(|| invalid("non-exhaustive match table"))?);
            Action::Continue
        }
        Terminator::Goto(block) => {
            frame.goto(block);
            Action::Continue
        }
        Terminator::Branch { condition, yes, no } => {
            let Value::Bool(condition) = frame.operand(&condition)? else {
                return Err(invalid("branch needs bool").into());
            };
            frame.goto(if condition { yes } else { no });
            Action::Continue
        }
        Terminator::Switch {
            value,
            arms,
            otherwise,
        } => {
            let value = frame.operand(&value)?;
            let tag = match &value {
                Value::Variant { variant, .. } | Value::ResourceVariant { variant, .. } => {
                    Some(*variant)
                }
                Value::Integer(i) => u32::try_from(i.bits()).ok(),
                Value::Bool(b) => Some(u32::from(*b)),
                _ => {
                    value.discard();
                    return Err(invalid("Switch requires an integer, bool or variant tag").into());
                }
            };
            value.discard();
            let next = arms
                .iter()
                .find(|(key, _)| Some(*key) == tag)
                .map_or(otherwise, |(_, block)| *block);
            frame.goto(next);
            Action::Continue
        }
        Terminator::Return { value, .. } => {
            let (value, shadow) = frame.operand_shadow(&value)?;
            if let Some(check) = &mut frame.check {
                check.returned = Some(shadow);
            }
            Action::Return(value)
        }
        Terminator::Fail { rule, .. } => Action::Fail(rule),
        Terminator::Call {
            callee,
            arguments,
            returns,
            on_failure,
        } => {
            if let Callee::Definition { path, .. } = &callee {
                if runtime.forget.as_ref() == Some(path)
                    && (arguments.len() != 1
                        || arguments[0].value.usage != hirsel_types::UseKind::Take)
                {
                    return Err(invalid("forget requires one take argument").into());
                }
            }
            let callee = match callee {
                Callee::Definition { path, arguments } => {
                    let generics = arguments
                        .into_iter()
                        .map(|argument| match argument {
                            hirsel_types::GenericArgument::Size(hirsel_types::Size::Parameter(
                                p,
                            )) => frame.size(p).map(|n| {
                                hirsel_types::GenericArgument::Size(hirsel_types::Size::Constant(n))
                            }),
                            argument => Ok(argument),
                        })
                        .collect::<Result<Vec<_>, Error>>()?;
                    RuntimeCallee::Definition {
                        path,
                        generics,
                        types: None,
                    }
                }
                Callee::Indirect {
                    value: operand,
                    arguments,
                } => {
                    let (value, shadow) = frame.operand_shadow(&operand)?;
                    let check = callee_lifetime(frame, operand.place, &value, shadow)?;
                    value.begin_call();
                    match value {
                        Value::FunctionReference(reference) => RuntimeCallee::Definition {
                            path: reference.definition,
                            generics: reference.arguments,
                            types: Some(reference.types),
                        },
                        Value::Closure {
                            definition,
                            body,
                            captures,
                            owned_captures,
                            sizes,
                            type_arguments,
                            ..
                        } => {
                            if captures.len() != owned_captures.len() {
                                return Err(invalid("capture ownership count").into());
                            }
                            let generic_sizes = arguments
                                .iter()
                                .map(|argument| match argument {
                                    hirsel_types::GenericArgument::Type(_) => Ok(None),
                                    hirsel_types::GenericArgument::Size(
                                        hirsel_types::Size::Constant(n),
                                    ) => Ok(Some(*n)),
                                    hirsel_types::GenericArgument::Size(
                                        hirsel_types::Size::Parameter(p),
                                    ) => frame.size(*p).map(Some),
                                })
                                .collect::<Result<Vec<_>, Error>>()?;
                            RuntimeCallee::Closure {
                                definition,
                                body,
                                captures,
                                sizes,
                                generic_sizes,
                                actuals: crate::type_arguments::concrete(
                                    &frame.definition.types,
                                    &arguments,
                                    &frame.type_arguments,
                                )?,
                                type_arguments,
                                check,
                            }
                        }
                        _ => return Err(invalid("indirect call requires closure").into()),
                    }
                }
            };
            let mut_places = arguments
                .iter()
                .filter(|a| a.value.usage == hirsel_types::UseKind::Mut)
                .map(|a| (a.parameter.0 as usize, a.value.place))
                .collect();
            let taken = arguments
                .iter()
                .filter(|a| a.value.usage == hirsel_types::UseKind::Take)
                .map(|a| a.parameter.0 as usize)
                .collect();
            let borrowed_protocol = runtime.checked.as_ref().is_some_and(|checked| {
                crate::checked::protocol_call(
                    &frame.definition,
                    frame.body_id,
                    frame.block,
                    checked,
                )
            });
            let mut values = vec![None; arguments.len()];
            let mut checks = vec![crate::checked::Argument::default(); arguments.len()];
            // The argument places already contain caller temporaries, in explicit source order.
            for argument in arguments {
                let mut operand = argument.value.clone();
                if operand.usage == hirsel_types::UseKind::Mut {
                    arena_write(frame, operand.place, true, budgets)?;
                    operand.usage = hirsel_types::UseKind::Take;
                }
                let (value, shadow) = frame.operand_shadow(&operand)?;
                let slot = values
                    .get_mut(argument.parameter.0 as usize)
                    .ok_or_else(|| invalid("parameter ordinal out of range"))?;
                if slot.is_some() {
                    return Err(invalid("duplicate call parameter").into());
                }
                *slot = Some(value);
                if frame.check.is_some() {
                    // Ordinary local results include a borrowed receiving home.
                    // Protocol results compose owned contents through open/next;
                    // their access slots are not additional source lifetimes.
                    let mut lifetime = if borrowed_protocol {
                        shadow.source_bound()
                    } else {
                        shadow.total()
                    };
                    if !borrowed_protocol
                        && !lifetime.is_empty()
                        && matches!(
                            argument.value.usage,
                            hirsel_types::UseKind::Borrow | hirsel_types::UseKind::Mut
                        )
                    {
                        lifetime.extend(home(frame, operand.place)?.unwrap_or_default());
                        lifetime.sort_unstable();
                        lifetime.dedup();
                    }
                    checks[argument.parameter.0 as usize] =
                        crate::checked::Argument { shadow, lifetime };
                }
            }
            Action::Call {
                callee,
                checks,
                arguments: values.into_iter().map(|v| v.unwrap()).collect(),
                mut_places,
                taken,
                returns,
                on_failure,
            }
        }
        Terminator::Unreachable => return Err(invalid("executed Unreachable").into()),
    })
}

enum RuntimeCallee {
    Definition {
        path: hirsel_syntax::DefPath,
        generics: Vec<hirsel_types::GenericArgument>,
        types: Option<std::sync::Arc<hirsel_types::TypeStore>>,
    },
    Closure {
        definition: hirsel_syntax::DefPath,
        body: BodyId,
        captures: Vec<Option<Value>>,
        sizes: crate::value::SizeBindings,
        generic_sizes: Vec<Option<u64>>,
        actuals: crate::type_arguments::Actuals,
        type_arguments: crate::type_arguments::Bindings,
        check: Option<crate::checked::Callee>,
    },
}
/// `arena.with` becomes a call of its callback with a fresh arena, which ends
/// when the callback returns (MEM-4); placement is unobservable (MEM-10), so
/// normal execution needs nothing more. `arena.bounded` does the same with a
/// fresh bounded arena, the third result: its handle id and capacity (MEM-6).
/// Checked execution also poisons the arena at its end, and `@noalloc` bodies
/// call only `@noalloc` definitions (MEM-5).
fn checked_callee(
    frame: &Frame,
    callee: RuntimeCallee,
    arguments: &mut Vec<Value>,
    checks: &mut Vec<crate::checked::Argument>,
    budgets: &mut crate::bounded::Budgets,
) -> Result<(RuntimeCallee, Option<u32>, Option<(u64, u64)>), Error> {
    let RuntimeCallee::Definition { path, .. } = &callee else {
        return Ok((callee, None, None));
    };
    let bounded = *path == hirsel_syntax::DefPath::core(crate::library::ARENA_BOUNDED_OPERATION);
    let Some(check) = &frame.check else {
        if !bounded && *path != hirsel_syntax::DefPath::core(crate::library::ARENA_WITH_OPERATION) {
            return Ok((callee, None, None));
        }
        let cap = bounded.then(|| bounded_capacity(arguments)).transpose()?;
        let (definition, body, captures, _, sizes, type_arguments) = arena_callback(arguments)?;
        let id = if bounded { budgets.issue() } else { 0 };
        *arguments = vec![Value::Handle {
            kind: crate::checked::ARENA_HANDLE.into(),
            id,
        }];
        let callee = RuntimeCallee::Closure {
            definition,
            body,
            captures,
            sizes,
            generic_sizes: vec![],
            actuals: Default::default(),
            type_arguments,
            check: None,
        };
        return Ok((callee, None, cap.map(|cap| (id, cap))));
    };
    let mut state = check.state.borrow_mut();
    if check.noalloc && !state.config.noalloc.contains(path) {
        return Err(state.trap(
            crate::checked::TrapRule::AllocatingCallInNoalloc,
            format!("call to {:?}", path.0.name),
        ));
    }
    if !bounded && state.config.arena_with.as_ref() != Some(path) {
        return Ok((callee, None, None));
    }
    let cap = bounded.then(|| bounded_capacity(arguments)).transpose()?;
    let (definition, body, captures, owned_captures, sizes, type_arguments) =
        arena_callback(arguments)?;
    let arena = state.arena();
    let shadow = checks.pop().unwrap_or_default().shadow;
    let check = crate::checked::Callee {
        captures: (0..captures.len()).map(|i| shadow.capture(i)).collect(),
        owned: owned_captures,
        lifetime: shadow.total(),
    };
    *arguments = vec![Value::Handle {
        kind: crate::checked::ARENA_HANDLE.into(),
        id: arena.into(),
    }];
    *checks = vec![crate::checked::Argument {
        shadow: Shadow::of(vec![arena]),
        lifetime: vec![arena],
    }];
    let callee = RuntimeCallee::Closure {
        definition,
        body,
        captures,
        sizes,
        generic_sizes: vec![],
        actuals: Default::default(),
        type_arguments,
        check: Some(check),
    };
    Ok((callee, Some(arena), cap.map(|cap| (arena.into(), cap))))
}
/// Remove `arena.bounded`'s leading capacity argument.
fn bounded_capacity(arguments: &mut Vec<Value>) -> Result<u64, Error> {
    if arguments.len() != 2 {
        return Err(invalid("arena.bounded takes a capacity and one callback"));
    }
    match arguments.remove(0) {
        Value::Integer(cap) => {
            u64::try_from(cap.bits()).map_err(|_| invalid("arena.bounded capacity"))
        }
        _ => Err(invalid("arena.bounded capacity is not a u64")),
    }
}
type Callback = (
    DefPath,
    hirsel_tir::BodyId,
    Vec<Option<Value>>,
    Vec<bool>,
    crate::value::SizeBindings,
    crate::type_arguments::Bindings,
);
/// The one closure argument of `arena.with`.
fn arena_callback(arguments: &mut Vec<Value>) -> Result<Callback, Error> {
    let [
        Value::Closure {
            definition,
            body,
            captures,
            owned_captures,
            sizes,
            type_arguments,
            ..
        },
    ] = <[Value; 1]>::try_from(std::mem::take(arguments))
        .map_err(|_| invalid("arena.with takes one callback"))?
    else {
        return Err(invalid("arena.with callback must be a closure"));
    };
    Ok((
        definition,
        body,
        captures,
        owned_captures,
        sizes,
        type_arguments,
    ))
}
/// What a closure call's environment is bounded by, under checked execution.
fn callee_lifetime(
    frame: &mut Frame,
    place: PlaceId,
    value: &Value,
    shadow: Shadow,
) -> Result<Option<crate::checked::Callee>, Error> {
    if frame.check.is_none() {
        return Ok(None);
    }
    let Value::Closure {
        captures,
        owned_captures,
        ..
    } = value
    else {
        return Ok(None);
    };
    let mut lifetime = shadow.total();
    lifetime.extend(home(frame, place)?.unwrap_or_default());
    lifetime.sort_unstable();
    lifetime.dedup();
    Ok(Some(crate::checked::Callee {
        captures: (0..captures.len()).map(|i| shadow.capture(i)).collect(),
        owned: owned_captures.clone(),
        lifetime,
    }))
}

fn restore_operation(
    frame: &mut Frame,
    state: &mut crate::resources::OperationState,
) -> Result<(), Error> {
    for (index, place) in &state.mut_places {
        let value = std::mem::replace(&mut state.arguments[*index], Value::Unit);
        let bound = state
            .bounds
            .as_ref()
            .and_then(|bounds| bounds.updates.iter().find(|(i, _)| i == index))
            .map(|(_, b)| b.clone())
            .unwrap_or_default();
        let shadow = bound.for_value(&value);
        frame.put(*place, Some(value), shadow)?;
    }
    Ok(())
}
/// Section 22 declarations select result sources; ownership transport supplies
/// the values retained by native state. Declared local results include the home
/// of a borrowed binding, while transferred cursor items retain value bounds.
fn operation_bounds(
    config: &crate::checked::CheckedRuntime,
    path: &hirsel_syntax::DefPath,
    checks: &[crate::checked::Argument],
    mut_places: &[(usize, PlaceId)],
    taken: &[usize],
) -> crate::checked::OperationCheck {
    let total = |i: usize| checks.get(i).map(|c| c.shadow.total()).unwrap_or_default();
    let signature = config.operations.get(path);
    let function = signature.and_then(|s| match &s.kind {
        hirsel_types::SignatureKind::Function(f) => Some(f),
        _ => None,
    });
    let taken: Vec<_> = function
        .map(|f| {
            f.parameters
                .iter()
                .enumerate()
                .filter_map(|(i, p)| (p.passing == hirsel_syntax::PassingMode::Take).then_some(i))
                .collect()
        })
        .unwrap_or_else(|| taken.to_vec());
    let mut moved = vec![];
    for i in &taken {
        moved.extend(total(*i));
    }
    moved.sort_unstable();
    moved.dedup();
    let mut retained = moved.clone();
    for (i, _) in mut_places {
        retained.extend(
            checks
                .get(*i)
                .map(|c| c.shadow.content_bound())
                .unwrap_or_default(),
        );
    }
    retained.sort_unstable();
    retained.dedup();
    let result = if let (Some(signature), Some(function)) = (signature, function) {
        let mut result_inputs = checks.to_vec();
        for (i, parameter) in function.parameters.iter().enumerate() {
            if parameter.passing == hirsel_syntax::PassingMode::Mut {
                if let Some(input) = result_inputs.get_mut(i) {
                    input.lifetime = input.shadow.content_bound();
                }
            }
        }
        crate::checked::operation_result(
            &signature.types,
            function.result,
            &result_inputs,
            &retained,
        )
    } else {
        Shadow::of(retained)
    };
    let updates = mut_places
        .iter()
        .map(|(i, _)| {
            let mut shadow = checks.get(*i).map(|c| c.shadow.clone()).unwrap_or_default();
            shadow.add_bound(&moved);
            (*i, shadow)
        })
        .collect();
    crate::checked::OperationCheck {
        result,
        updates,
        inputs: checks.to_vec(),
        callbacks: vec![],
    }
}
fn failure_cleanup_at(frame: &Frame) -> Option<CleanupId> {
    let block = frame.body().blocks.get(frame.block.0 as usize)?;
    if frame.instruction > 0 {
        block
            .instructions
            .get(frame.instruction - 1)
            .map(|i| i.on_failure)
    } else {
        terminator_cleanup(&block.terminator)
    }
}

fn enqueue_owned(frame: &mut Frame, state: &mut crate::resources::OperationState) {
    for value in state.owned.drain(..).rev() {
        frame
            .work
            .push_back(crate::resources::Work::Release(value, state.anchor.clone()));
    }
}
fn release_operation_result(
    frame: &mut Frame,
    result: OperationResult,
    state: &mut crate::resources::OperationState,
) -> Result<(), Error> {
    let anchor = state.anchor.clone();
    match result {
        OperationResult::Complete {
            value,
            arguments,
            consumed,
            released,
        } => {
            transfer_operation(state, consumed)?;
            update_operation(state, arguments)?;
            frame
                .work
                .push_back(crate::resources::Work::Release(value, anchor.clone()));
            for value in released.into_iter().rev() {
                frame
                    .work
                    .push_back(crate::resources::Work::Release(value, anchor.clone()));
            }
        }
        OperationResult::Call {
            callee,
            arguments,
            owned,
            updates,
            consumed,
            ..
        } => {
            transfer_operation(state, consumed)?;
            update_operation(state, updates)?;
            callee.discard();
            for value in arguments {
                value.discard();
            }
            for value in owned.into_iter().rev() {
                frame
                    .work
                    .push_back(crate::resources::Work::Release(value, anchor.clone()));
            }
        }
        OperationResult::Failed(_)
        | OperationResult::UndefinedBehavior(_)
        | OperationResult::Park { .. }
        | OperationResult::FailureChain { .. } => (),
    }
    Ok(())
}

/// Abandon all suspended native callbacks without running success continuations.
/// Each mutable snapshot is forwarded before the original caller is restored.
fn abandon_operation(
    frame: &mut Frame,
    mut state: crate::resources::OperationState,
) -> Result<(), Error> {
    loop {
        restore_operation(frame, &mut state)?;
        enqueue_owned(frame, &mut state);
        enqueue_taken(frame, &mut state);
        if let Some(mut callback) = state.callback.take() {
            writeback_native_callback(&mut callback, &state)?;
            state = callback.state;
        } else {
            enqueue_cleanup(frame, state.on_failure, state.anchor.site)?;
            return Ok(());
        }
    }
}

fn validate_callback_writebacks(
    state: &crate::resources::OperationState,
    parameters: &[usize],
    writebacks: &[crate::CallbackWriteback],
) -> Result<(), Error> {
    let mut seen = std::collections::BTreeSet::new();
    for target in writebacks {
        if !parameters.contains(&target.parameter)
            || !state.mut_places.iter().any(|(i, _)| *i == target.argument)
                && !state.callback_mut_arguments.contains(&target.argument)
            || !seen.insert(target.parameter)
        {
            return Err(invalid("invalid callback mut writeback"));
        }
        let mut value = state
            .arguments
            .get(target.argument)
            .ok_or_else(|| invalid("callback writeback argument"))?;
        for &field in &target.fields {
            value = value
                .fields()
                .and_then(|fields| fields.get(field))
                .and_then(Option::as_ref)
                .ok_or_else(|| invalid("callback writeback field"))?;
        }
    }
    Ok(())
}

fn writeback_callback(
    callback: &mut crate::resources::Callback,
    updates: &[(usize, Value)],
) -> Result<(), Error> {
    for target in &callback.writebacks {
        let value = updates
            .iter()
            .find(|(i, _)| *i == target.parameter)
            .ok_or_else(|| invalid("missing callback mut writeback"))?
            .1
            .clone();
        let mut saved = &mut callback.state.arguments[target.argument];
        for &field in &target.fields {
            saved = saved
                .fields_mut()
                .and_then(|fields| fields.get_mut(field))
                .and_then(Option::as_mut)
                .ok_or_else(|| invalid("callback writeback field"))?;
        }
        std::mem::replace(saved, value).discard();
    }
    Ok(())
}

fn writeback_native_callback(
    callback: &mut crate::resources::Callback,
    state: &crate::resources::OperationState,
) -> Result<(), Error> {
    let updates = state
        .callback_mut_arguments
        .iter()
        .map(|&i| (i, state.arguments[i].clone()))
        .collect::<Vec<_>>();
    writeback_callback(callback, &updates)?;
    if let (Some(child), Some(parent)) = (&state.bounds, &mut callback.state.bounds) {
        let written = child
            .updates
            .iter()
            .map(|(_, shadow)| shadow.clone())
            .collect::<Vec<_>>();
        parent.accept_callback(&child.result, &written);
    }
    Ok(())
}

fn update_operation(
    state: &mut crate::resources::OperationState,
    updates: Vec<(usize, Value)>,
) -> Result<(), Error> {
    let mut seen = std::collections::BTreeSet::new();
    for (index, _) in &updates {
        if !seen.insert(*index) {
            return Err(invalid("operation updated an argument twice"));
        }
        if !state.mut_places.iter().any(|(i, _)| i == index)
            && !state.callback_mut_arguments.contains(index)
        {
            return Err(invalid("operation updated a non-mut argument"));
        }
    }
    for (index, mut value) in updates {
        value.canonicalize();
        std::mem::replace(&mut state.arguments[index], value).discard();
    }
    Ok(())
}

fn transfer_operation(
    state: &mut crate::resources::OperationState,
    consumed: Vec<usize>,
) -> Result<(), Error> {
    let mut seen = std::collections::BTreeSet::new();
    for index in consumed {
        if !seen.insert(index) || !state.taken.contains(&index) {
            return Err(invalid("operation transferred an argument it does not own"));
        }
        state.taken.retain(|i| *i != index);
        std::mem::replace(&mut state.arguments[index], Value::Unit).discard();
    }
    Ok(())
}
fn enqueue_taken(frame: &mut Frame, state: &mut crate::resources::OperationState) {
    for index in state.taken.drain(..).rev() {
        let value = std::mem::replace(&mut state.arguments[index], Value::Unit);
        frame
            .work
            .push_back(crate::resources::Work::Release(value, state.anchor.clone()));
    }
}
