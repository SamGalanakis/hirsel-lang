//! A single iterative scheduling loop: Hirsel calls never recurse on the host stack.
use crate::Failure;
use crate::checked::Shadow;
use crate::frame::{CallerSite, Continuation, Frame};
use crate::scalar::{self, ScalarError};
use crate::*;
use hirsel_diag::RuleId;
use hirsel_span::{SourceMap, Span};
use hirsel_tir::*;
use hirsel_types::Destination;

pub fn run(
    provider: &mut (impl TirProvider + ?Sized),
    types: &std::sync::Arc<hirsel_types::TypeInterner>,
    sources: &SourceMap,
    configuration: &RunConfiguration,
    arguments: Vec<Value>,
    limits: Limits,
) -> Result<Execution, ExecutionError> {
    run_with_operations(
        provider,
        types,
        sources,
        configuration,
        arguments,
        limits,
        &mut NoOperations,
    )
}
pub fn run_with_operations(
    provider: &mut (impl TirProvider + ?Sized),
    types: &std::sync::Arc<hirsel_types::TypeInterner>,
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
        &crate::aggregate::RuntimeTypes::new(types.clone()),
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
    run_observed(
        provider,
        sources,
        configuration,
        arguments,
        limits,
        operations,
        runtime,
        &mut crate::ownership_trace::NoTrace,
    )
}

pub(crate) fn run_observed<O: crate::ownership_trace::Observer>(
    provider: &mut (impl TirProvider + ?Sized),
    sources: &SourceMap,
    configuration: &RunConfiguration,
    arguments: Vec<Value>,
    limits: Limits,
    operations: &mut impl Operations,
    runtime: &crate::aggregate::RuntimeTypes,
    observer: &mut O,
) -> Result<Execution, ExecutionError> {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut immediate = crate::provider::ImmediateTirProvider::new(provider);
    let mut execution = std::pin::pin!(run_observed_async(
        &mut immediate,
        sources,
        configuration,
        arguments,
        limits,
        operations,
        runtime,
        observer
    ));
    match execution
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(result) => result,
        Poll::Pending => unreachable!("an immediate TIR provider suspended"),
    }
}

/// Execute with saved compiler dependency continuations. Poll and drop this
/// future on one worker: it retains the interpreter's Rc/RefCell ownership.
pub async fn run_with_runtime_async(
    provider: &mut (impl AsyncTirProvider + ?Sized),
    sources: &SourceMap,
    configuration: &RunConfiguration,
    arguments: Vec<Value>,
    limits: Limits,
    operations: &mut impl Operations,
    runtime: &crate::aggregate::RuntimeTypes,
) -> Result<Execution, ExecutionError> {
    run_observed_async(
        provider,
        sources,
        configuration,
        arguments,
        limits,
        operations,
        runtime,
        &mut crate::ownership_trace::NoTrace,
    )
    .await
}

async fn run_observed_async<O: crate::ownership_trace::Observer>(
    provider: &mut (impl AsyncTirProvider + ?Sized),
    sources: &SourceMap,
    configuration: &RunConfiguration,
    arguments: Vec<Value>,
    limits: Limits,
    operations: &mut impl Operations,
    runtime: &crate::aggregate::RuntimeTypes,
    observer: &mut O,
) -> Result<Execution, ExecutionError> {
    let checker = runtime.checked.clone().map(crate::checked::state);
    let root = crate::runs::Run::new(
        provider,
        configuration.clone(),
        arguments,
        limits,
        runtime,
        runtime.budgets,
        checker.as_ref(),
    )
    .await?;
    let (mut root, result) = crate::runs::execute(
        provider, sources, limits, operations, runtime, root, observer,
    )
    .await;
    let execution = &mut root.execution;
    let result = match (result, &checker) {
        (Err(Error::Blocked(waits)), _) => {
            execution.outcome = Outcome::Blocked(waits);
            Ok(())
        }
        (Err(Error::BudgetExceeded(kind)), _) => {
            execution.outcome = Outcome::BudgetExceeded(kind);
            Ok(())
        }
        (Err(Error::Exhausted(exhaustion)), _) => {
            execution.outcome = Outcome::Exhausted(*exhaustion);
            Ok(())
        }
        (Err(Error::Signaled(kind)), _) => {
            execution.outcome = Outcome::Signaled(kind);
            Ok(())
        }
        (Err(Error::Environmental(cause)), _) => {
            execution.outcome = Outcome::Environmental(cause);
            Ok(())
        }
        (Err(Error::ProviderFailed(record)), _) => {
            execution.outcome = Outcome::Failed(record.failure.clone());
            execution.failures.push(*record);
            Ok(())
        }
        (Err(Error::Ended(failure)), _) => {
            execution.outcome = Outcome::Failed(failure);
            Ok(())
        }
        (Err(Error::Trapped(trap)), Some(_)) => {
            execution.outcome = Outcome::Trapped(*trap);
            Ok(())
        }
        (Ok(()), Some(state)) => {
            execution.outcome =
                audit_resources(&state.borrow(), &execution.outcome, root.entry_consumed)
                    .map_or(execution.outcome.clone(), |trap| Outcome::Trapped(*trap));
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
    Ok(root.execution)
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

/// One run's view of its provider: a program's definitions are immutable for the
/// run, so each is looked up once rather than at every call. An error is never
/// retained; it is asked again.
struct Resolved<'p, P: ?Sized> {
    provider: &'p mut P,
    definitions: std::collections::BTreeMap<hirsel_syntax::DefPath, DefinitionRef>,
    /// Declarations with their own body: CT-5 instances never replace them.
    own_bodies: std::collections::BTreeSet<hirsel_syntax::DefPath>,
}
impl<'p, P: AsyncTirProvider + ?Sized> Resolved<'p, P> {
    fn new(provider: &'p mut P) -> Self {
        Self {
            provider,
            definitions: Default::default(),
            own_bodies: Default::default(),
        }
    }
}
impl<P: AsyncTirProvider + ?Sized> AsyncTirProvider for Resolved<'_, P> {
    async fn definition(
        &mut self,
        path: &hirsel_syntax::DefPath,
    ) -> Result<DefinitionRef, hirsel_types::QueryError> {
        if let Some(definition) = self.definitions.get(path) {
            return Ok(definition.clone());
        }
        let definition = self.provider.definition(path).await?;
        self.definitions.insert(path.clone(), definition.clone());
        Ok(definition)
    }
    async fn constant(
        &mut self,
        path: &hirsel_syntax::DefPath,
    ) -> Result<MaterializedRef, hirsel_types::QueryError> {
        self.provider.constant(path).await
    }
    async fn constant_instance(
        &mut self,
        path: &hirsel_syntax::DefPath,
        arguments: &[u64],
    ) -> Result<MaterializedRef, hirsel_types::QueryError> {
        self.provider.constant_instance(path, arguments).await
    }
    async fn constant_definitions(
        &mut self,
    ) -> Result<Vec<hirsel_syntax::DefPath>, hirsel_types::QueryError> {
        self.provider.constant_definitions().await
    }
    async fn instance_body(
        &mut self,
        path: &hirsel_syntax::DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<hirsel_syntax::DefPath>, hirsel_types::QueryError> {
        if self.own_bodies.contains(path) {
            return Ok(None);
        }
        let instance = self.provider.instance_body(path, arguments).await?;
        if instance.is_none() {
            self.own_bodies.insert(path.clone());
        }
        Ok(instance)
    }
}
/// What one run's scheduler holds between steps: its task stacks, budgets and
/// constants, and what its entry returned so far.
pub(crate) struct Scheduler {
    returned_snapshot: Option<Value>,
    tooling: Option<hirsel_tir::ToolingLimit>,
    budgets: crate::bounded::Budgets,
    task: u64,
    runnable: std::collections::VecDeque<(u64, Vec<Frame>, Vec<crate::observation::FailureRecord>)>,
    parked: crate::parked::Parked,
    frames: Vec<Frame>,
    constants: crate::constants::Constants,
    /// The dispatch forms of the bodies its frames run.
    codes: crate::code::Codes,
    /// The ended frames of finished children holding a result nobody took yet, where a
    /// discard drops it; pruned when the table doubles.
    holding: std::collections::HashMap<u64, Frame>,
    holding_mark: usize,
    /// Tasks whose turn is handed to holding children dropping discarded results.
    hand_offs: Vec<HandOff>,
}
/// A task's turn handed to the rests of its pending operation: the child in its rest, the
/// rests after it, and the task's own stack, continued when the last rest returns.
struct HandOff {
    parent: u64,
    frames: Vec<Frame>,
    failures: Vec<crate::observation::FailureRecord>,
    child: u64,
    rests: std::collections::VecDeque<crate::TaskRest>,
}
/// Holding child `rest.child`'s stack for its rest: its ended frame, dropping the values.
fn rest_stack<O: crate::ownership_trace::Observer>(
    holding: &mut std::collections::HashMap<u64, Frame>,
    rest: crate::TaskRest,
    template: &Frame,
    operations: &(impl Operations + ?Sized),
    observer: &mut O,
    sources: &SourceMap,
) -> Result<(u64, Vec<Frame>), Error> {
    let mut frame = match holding.remove(&rest.child) {
        Some(frame) => frame,
        None if operations.frameless_task(rest.child) => template.cleanup_shell()?,
        None => return Err(invalid("a discarded result's child holds no frame")),
    };
    frame.ending = Some(crate::resources::Ending::Return(Value::Unit));
    let site = site_of(&frame)?;
    for value in rest.values.into_iter().rev() {
        frame.work.push_back(crate::resources::Work::Release(
            value,
            anchor(&frame, site),
            frame.caller.clone(),
        ));
    }
    if O::ENABLED {
        observer.sync(rest.child, std::slice::from_ref(&frame), sources);
        observer.rest_entered(rest.child, &frame);
    }
    Ok((rest.child, vec![frame]))
}
impl Scheduler {
    pub(crate) fn new(
        frame: Frame,
        constants: crate::constants::Constants,
        budgets: crate::RunBudgets,
    ) -> Self {
        Self {
            returned_snapshot: None,
            tooling: None,
            budgets: crate::bounded::Budgets::new(budgets),
            task: 0,
            runnable: Default::default(),
            parked: Default::default(),
            frames: vec![frame],
            constants,
            codes: Default::default(),
            holding: std::collections::HashMap::new(),
            holding_mark: HOLDING_MARK,
            hand_offs: vec![],
        }
    }
    /// The waits of its parked tasks, in suspension order (CONC-8).
    pub(crate) fn waits(&self) -> Vec<BlockedWait> {
        self.parked
            .iter()
            .filter_map(|(_, stack, _)| {
                let frame = stack.last()?;
                let result = if let Some(drop) = &frame.drop_wait {
                    &drop.result
                } else {
                    &frame.operation.as_ref()?.0
                };
                match result {
                    OperationResult::Park { wait, .. } => Some(wait.clone()),
                    _ => None,
                }
            })
            .collect()
    }
}
/// Where a run's step stopped.
pub(crate) enum Step {
    /// Its entry ended; the run's `Execution` holds the outcome.
    Finished,
    /// None of its tasks can proceed until a wait is ready.
    Idle,
}
/// The holding table's size before its first pruning.
const HOLDING_MARK: usize = 32;
pub(crate) async fn execute_frames<O: crate::ownership_trace::Observer>(
    provider: &mut (impl AsyncTirProvider + ?Sized),
    sources: &SourceMap,
    configuration: &RunConfiguration,
    limits: Limits,
    operations: &mut (impl Operations + ?Sized),
    runtime: &crate::aggregate::RuntimeTypes,
    state: &mut Option<Scheduler>,
    execution: &mut Execution,
    entry_consumed: &mut bool,
    observer: &mut O,
) -> Result<Step, Error> {
    let profile = crate::profile::Profile::active(&configuration.entry);
    let provider = &mut Resolved::new(provider);
    let Scheduler {
        mut returned_snapshot,
        mut tooling,
        mut budgets,
        mut task,
        mut runnable,
        mut parked,
        mut frames,
        mut constants,
        mut codes,
        mut holding,
        mut holding_mark,
        mut hand_offs,
    } = state
        .take()
        .ok_or_else(|| invalid("a finished run resumed"))?;
    operations.enter_task(task, frames.iter().any(|f| f.finalized.is_some()));
    loop {
        // Parking moves the stack into the parked queue; its observation stays
        // suspended until a runnable stack is selected below.
        if !frames.is_empty() {
            observer.sync(task, &frames, sources);
        }
        for start in operations.take_tasks() {
            let parent_frames = if start.parent == task {
                &frames
            } else if let Some((_, stack, _)) =
                runnable.iter().find(|(id, _, _)| *id == start.parent)
            {
                stack
            } else if let Some((_, stack, _)) = parked.iter().find(|(id, _, _)| *id == start.parent)
            {
                stack
            } else {
                return Err(invalid("spawned task has no parent stack"));
            };
            let mut invocation_types = Default::default();
            let (definition, body, captures, sizes) = match &start.callee {
                Value::Closure {
                    definition,
                    body,
                    captures,
                    environment,
                    ..
                } => {
                    invocation_types = environment.type_arguments.clone();
                    (
                        provider
                            .definition(definition.path())
                            .await
                            .map_err(Error::Query)?,
                        *body,
                        captures.clone(),
                        environment.sizes.clone(),
                    )
                }
                Value::FunctionReference(reference) => {
                    let definition = provider
                        .definition(&reference.definition)
                        .await
                        .map_err(Error::Query)?;
                    let body = definition.entry;
                    crate::type_arguments::bind(
                        &mut invocation_types,
                        definition.bodies[body.0 as usize].parameter_scope,
                        crate::type_arguments::Actuals {
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
            let mut child = Frame::new_body(
                runtime.types.clone(),
                definition,
                body,
                start.arguments.clone(),
                captures,
                sizes,
            )?;
            if child.definition.tracks_caller(body) {
                child.caller = start
                    .site
                    .and_then(|site| {
                        parent_frames
                            .last()
                            .map(|frame| CallerSite::new(frame, site, sources))
                    })
                    .transpose()?;
            }
            child.type_arguments = invocation_types;
            if let Some(parent) = parent_frames.last().and_then(|f| f.check.as_ref()) {
                let callee = parent_frames
                    .last()
                    .and_then(|f| f.operation.as_deref())
                    .and_then(|(_, state)| {
                        let bounds = state.bounds.as_ref()?;
                        let Value::Closure {
                            captures,
                            environment,
                            ..
                        } = &start.callee
                        else {
                            return None;
                        };
                        let shadow =
                            crate::checked::native_shadow(&start.callee, &state.arguments, bounds);
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
                            owned: environment
                                .owned_captures
                                .iter()
                                .map(|owned| *owned && !start.borrowed)
                                .collect(),
                            lifetime: shadow.lifetime(),
                        })
                    });
                // OWN-6: a taken capture's blocks become the child's.
                if let Some(callee) = &callee {
                    for (capture, owned) in callee.captures.iter().zip(&callee.owned) {
                        if *owned {
                            capture.release();
                        }
                    }
                }
                let arguments = parent_frames
                    .last()
                    .and_then(|f| f.operation.as_deref())
                    .and_then(|(_, state)| Some((state, state.bounds.as_ref()?)))
                    .map(|(state, bounds)| {
                        start
                            .arguments
                            .iter()
                            .enumerate()
                            .map(|(parameter, value)| {
                                let shadow = if let Some(path) = start.argument_paths.get(parameter)
                                {
                                    crate::checked::task_argument_shadow(bounds, path, value)?
                                } else {
                                    crate::checked::native_shadow(value, &state.arguments, bounds)
                                        .for_value(value)
                                };
                                shadow.release();
                                Ok(crate::checked::Argument {
                                    lifetime: shadow.total(),
                                    shadow,
                                    mutable: false,
                                })
                            })
                            .collect::<Result<Vec<_>, Error>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                child.check = Some(enter_checked(&parent.state, &child, arguments, callee)?);
            }
            if O::ENABLED {
                observer.task_started(
                    start.parent,
                    start.id,
                    operations.ownership_task_path(start.id),
                    &child,
                );
            }
            if !start.borrowed {
                budgets.storage.environment_entry(&start.callee)?;
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
            operations.expire_deadlines(&mut OperationContext {
                configuration,
                sources,
                stdout: &mut execution.stdout,
                trace: &mut execution.trace,
                trace_mode: runtime.trace_mode,
                anchor: None,
            })?;
            // Parked tasks are kept in poll order: non-timer waits and equal-deadline
            // timers in suspension order, timers by their original absolute ticks,
            // including when several deadlines passed during a blocking step. A
            // watched wait that is not woken would poll None, so it is not asked.
            let woken = operations.woken();
            for key in parked.due(&woken) {
                let (id, stack, _) = parked
                    .get_mut(key)
                    .ok_or_else(|| invalid("parked task left early"))?;
                operations.enter_task(*id, stack.iter().any(|f| f.finalized.is_some()));
                observer.sync(*id, stack, sources);
                let frame: &mut Frame = stack.last_mut().unwrap();
                let (token, anchor) = if let Some(drop) = &frame.drop_wait {
                    let OperationResult::Park { token, .. } = &drop.result else {
                        return Err(invalid("parked destructor has no wait"));
                    };
                    (*token, &drop.anchor)
                } else {
                    let (OperationResult::Park { token, .. }, state) =
                        frame.operation.as_deref().unwrap()
                    else {
                        return Err(invalid("parked task has no wait"));
                    };
                    (*token, &state.anchor)
                };
                let result = operations.poll(
                    token,
                    &mut OperationContext {
                        configuration,
                        sources,
                        stdout: &mut execution.stdout,
                        trace: &mut execution.trace,
                        trace_mode: runtime.trace_mode,
                        anchor: Some(anchor),
                    },
                )?;
                if let Some(result) = result {
                    observer.builtin_completed(*id, frame, &result);
                    let (id, mut stack, failures) = parked.remove(key).unwrap();
                    let frame = stack.last_mut().unwrap();
                    if let Some(drop) = &mut frame.drop_wait {
                        drop.result = result;
                    } else {
                        frame.operation.as_mut().unwrap().0 = result;
                    }
                    runnable.push_back((id, stack, failures));
                }
            }
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
                if let Some(check) = frames.last().and_then(|f| f.check.as_ref()) {
                    check.state.borrow_mut().task = task;
                }
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
                // Nothing of this run can proceed: its execution decides what next
                // (external readiness, another run, or CONC-8 Blocked).
                *state = Some(Scheduler {
                    returned_snapshot,
                    tooling,
                    budgets,
                    task,
                    runnable,
                    parked,
                    frames,
                    constants,
                    codes,
                    holding,
                    holding_mark,
                    hand_offs,
                });
                return Ok(Step::Idle);
            }
        }

        // A scheduler choice changes the observer state before any resumed cleanup.
        observer.sync(task, &frames, sources);

        // The pending operation discarded holding children's results: each child drops its
        // own in its rest, and this task continues when the last returns, with no scheduling
        // point between (detach, `detach_all`, `cancel_all`, scope exit; native `hand_off`).
        if frames
            .last()
            .is_some_and(|f| f.work.is_empty() && f.operation.is_some())
        {
            let mut rests: std::collections::VecDeque<_> = operations.take_rests(task).into();
            if let Some(first) = rests.pop_front() {
                hand_offs.push(HandOff {
                    parent: task,
                    frames: std::mem::take(&mut frames),
                    failures: std::mem::take(&mut execution.failures),
                    child: first.child,
                    rests,
                });
                (task, frames) = rest_stack(
                    &mut holding,
                    first,
                    hand_offs.last().unwrap().frames.last().unwrap(),
                    operations,
                    observer,
                    sources,
                )?;
                operations.enter_task(task, false);
                continue;
            }
            let root_anchor = anchor(frames.last().unwrap(), site_of(frames.last().unwrap())?);
            if let Some(operation) = &mut frames.last_mut().unwrap().operation {
                operations.rests_completed(
                    task,
                    &mut operation.0,
                    &mut OperationContext {
                        configuration,
                        sources,
                        stdout: &mut execution.stdout,
                        trace: &mut execution.trace,
                        trace_mode: runtime.trace_mode,
                        anchor: Some(&root_anchor),
                    },
                )?;
            }
        }
        if let Some(drop) = frames.last_mut().unwrap().drop_wait.take() {
            match drop.result {
                OperationResult::Park { token, .. } => {
                    frames.last_mut().unwrap().drop_wait = Some(drop);
                    operations.parked(token);
                    parked.push(
                        (
                            task,
                            std::mem::take(&mut frames),
                            std::mem::take(&mut execution.failures),
                        ),
                        token,
                        operations.wait_deadline(token),
                        operations.watched(token),
                    );
                }
                OperationResult::Complete {
                    value: Value::Unit,
                    arguments,
                    consumed,
                    released,
                } if arguments.is_empty() && consumed.is_empty() && released.is_empty() => {
                    finish_handle_drop(
                        frames.last_mut().unwrap(),
                        operations,
                        runtime.trace_mode,
                        execution,
                        sources,
                        &drop.anchor,
                        drop.caller.as_ref(),
                        observer,
                        task,
                    )?;
                }
                _ => return Err(invalid("invalid destructor completion")),
            }
            continue;
        }
        let depth = frames.len();
        // Test before taking: moving the large empty slot on every step costs.
        let top = frames.last_mut().unwrap();
        if top.work.is_empty()
            && top.operation.is_some()
            && let Some((result, mut state)) = top.operation.take().map(|operation| *operation)
        {
            let frame = frames.last_mut().unwrap();
            match result {
                OperationResult::SealArena { arena, root_type } => {
                    if !state.taken.contains(&0)
                        || !state.taken.contains(&1)
                        || !matches!(state.arguments.first(), Some(Value::Handle { kind, id })
                            if kind == crate::checked::ARENA_HANDLE && *id == arena)
                    {
                        return Err(invalid("seal request does not own its grant and root"));
                    }
                    let root = state
                        .arguments
                        .get(1)
                        .ok_or_else(|| invalid("missing seal root"))?;
                    let charge = budgets
                        .storage
                        .seal_charge(runtime, arena, root_type, root)?;
                    let charged = match charge {
                        crate::inventory::Charge::Bytes(bytes) => budgets.charge(arena, bytes),
                        crate::inventory::Charge::Overflow if budgets.bounded(arena) => Err(arena),
                        crate::inventory::Charge::Overflow => Ok(()),
                    };
                    let result = if let Err(arena) = charged {
                        budgets.exhausted(arena, execution.failures.len());
                        OperationResult::Failed(failure(
                            sources,
                            state.anchor.site,
                            "MEM-6",
                            "OutOfSpace",
                        )?)
                    } else {
                        let prepared = budgets.storage.prepare_owner(
                            runtime,
                            arena,
                            root_type,
                            root,
                            &operation_allocation_site(frame, &state.anchor)?,
                        )?;
                        let id = budgets.storage.commit_owner(prepared)?;
                        if let Some(check) = &frame.check {
                            check.state.borrow_mut().constructed += 1;
                        }
                        OperationResult::returned(Value::Handle {
                            kind: "Region".into(),
                            id,
                        })
                        .transferring([0, 1])
                    };
                    frame.operation = Some(Box::new((result, state)));
                }
                OperationResult::BorrowRegion {
                    owner,
                    root_type,
                    mut result,
                } => {
                    if state.root_loan.is_some() {
                        return Err(invalid("duplicate Region read request"));
                    }
                    if !state.mut_places.iter().any(|(index, _)| *index == 0)
                        || !matches!(state.arguments.first(), Some(Value::Handle { kind, id })
                            if kind == "Region" && *id == owner)
                    {
                        return Err(invalid("Region read request lacks exclusive owner custody"));
                    }
                    let OperationResult::Call { arguments, .. } = result.as_mut() else {
                        return Err(invalid("Region read did not schedule a callback"));
                    };
                    if arguments.len() != 1 {
                        return Err(invalid("Region root callback arity"));
                    }
                    let (root, loan) = budgets.storage.borrow_owner(
                        owner,
                        root_type,
                        frame.check.as_ref().map(|c| &c.state),
                    )?;
                    arguments[0] = root;
                    state.root_loan = Some(loan);
                    frame.operation = Some(Box::new((*result, state)));
                }
                OperationResult::Park { token, wait } => {
                    if operations.immediate_poll(token)
                        && let Some(next) = operations.poll(
                            token,
                            &mut OperationContext {
                                configuration,
                                sources,
                                stdout: &mut execution.stdout,
                                trace: &mut execution.trace,
                                trace_mode: runtime.trace_mode,
                                anchor: Some(&state.anchor),
                            },
                        )?
                    {
                        observer.builtin_completed(task, frame, &next);
                        frame.operation = Some(Box::new((next, state)));
                        continue;
                    }
                    frame.operation =
                        Some(Box::new((OperationResult::Park { token, wait }, state)));
                    operations.parked(token);
                    parked.push(
                        (
                            task,
                            std::mem::take(&mut frames),
                            std::mem::take(&mut execution.failures),
                        ),
                        token,
                        operations.wait_deadline(token),
                        operations.watched(token),
                    );
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
                            state.caller.clone(),
                        ));
                    }
                    frame.operation = Some(Box::new((OperationResult::Failed(first), state)));
                }
                OperationResult::UndefinedBehavior(detected) => {
                    execution.outcome = Outcome::UndefinedBehavior(detected);
                    return Ok(Step::Finished);
                }
                OperationResult::Call {
                    callee,
                    mut arguments,
                    mut_arguments,
                    writebacks,
                    mut updates,
                    owned,
                    consumed,
                    continuation,
                } => {
                    // The primitive already accepted these COW payloads. Their
                    // retained credits must exist while the callback runs or
                    // when its own entry is rejected.
                    for (index, value) in &mut updates {
                        if let Some((_, place)) = state.mut_places.iter().find(|(i, _)| i == index)
                        {
                            let ty = closed_storage_type(frame, runtime, frame.place(*place)?.ty)?;
                            budgets.storage.record(
                                runtime,
                                ty,
                                value,
                                &operation_allocation_site(frame, &state.anchor)?,
                                true,
                            )?;
                        }
                    }
                    record_operation_storage(frame, &mut state, runtime, &mut budgets)?;
                    let Some(activation) = frame.depth.admit() else {
                        let failed = failure(sources, state.anchor.site, "LANG-10", "Depth")?;
                        record_failure(
                            runtime.trace_mode,
                            execution,
                            failed.clone(),
                            state.anchor.clone(),
                            state.caller.as_ref(),
                        );
                        release_operation_result(
                            frame,
                            OperationResult::Call {
                                callee,
                                arguments,
                                mut_arguments,
                                writebacks,
                                updates,
                                owned,
                                consumed,
                                continuation,
                            },
                            &mut state,
                        )?;
                        state.already_failed = true;
                        frame.work.push_back(crate::resources::Work::Resume(
                            OperationResult::Failed(failed),
                            Box::new(crate::resources::Callback {
                                failure_start: execution.failures.len() - 1,
                                token: continuation,
                                state,
                                mut_parameters: vec![],
                                writebacks: vec![],
                            }),
                        ));
                        continue;
                    };
                    validate_callback_writebacks(&state, &mut_arguments, &writebacks)?;
                    let callback_inputs = state
                        .bounds
                        .as_ref()
                        .map(|bounds| {
                            arguments
                                .iter()
                                .enumerate()
                                .map(|(parameter, value)| {
                                    // A lent slot has an exact origin. Structural
                                    // equality can also match the lookup key or
                                    // another argument with a shorter lifetime.
                                    let shadow = if parameter == 0 && state.root_loan.is_some() {
                                        state.root_loan.as_ref().unwrap().shadow()
                                    } else if let Some(target) = writebacks
                                        .iter()
                                        .find(|target| target.parameter == parameter)
                                    {
                                        let mut shadow =
                                            bounds.inputs[target.argument].shadow.clone();
                                        for &field in &target.fields {
                                            shadow = shadow.part(field);
                                        }
                                        shadow
                                    } else {
                                        crate::checked::native_shadow(
                                            value,
                                            &state.arguments,
                                            bounds,
                                        )
                                    }
                                    .for_value(value);
                                    crate::checked::Argument {
                                        lifetime: shadow.total(),
                                        shadow,
                                        mutable: false,
                                    }
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let callback_callee = state.bounds.as_ref().and_then(|bounds| {
                        if let Value::Closure {
                            captures,
                            environment,
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
                                owned: environment.owned_captures.clone(),
                                lifetime: shadow.lifetime(),
                            })
                        } else {
                            None
                        }
                    });
                    transfer_operation(&mut state, consumed)?;
                    update_operation(&mut state, updates)?;
                    state.owned = owned;
                    budgets.storage.environment_entry(&callee)?;
                    callee.begin_call();
                    let mut invocation_types = Default::default();
                    let (definition, body, captures, sizes) = match callee {
                        Value::Closure {
                            definition,
                            body,
                            captures,
                            environment,
                            ..
                        } => {
                            if captures.len() != environment.owned_captures.len() {
                                return Err(invalid("capture ownership count"));
                            }
                            let crate::value::Environment {
                                sizes,
                                type_arguments,
                                ..
                            } = *environment;
                            invocation_types = type_arguments;
                            (
                                provider
                                    .definition(definition.path())
                                    .await
                                    .map_err(Error::Query)?,
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
                            let before_limit = limits.steps == Some(execution.steps);
                            let cost = primitive_work(&reference.definition, &arguments)?;
                            let precharged =
                                cost != 0 && callback_depth < limits.call_depth && !before_limit;
                            let mut primitive_limit = false;
                            if precharged {
                                // Named callbacks of runtime operations enter here, without
                                // an Action::Call frame. Count their ordinary call step once,
                                // then the same primitive work as a direct/indirect source call.
                                let allowed = limits.steps.map_or(cost, |limit| {
                                    cost.min(
                                        limit.saturating_sub(execution.steps).saturating_sub(1),
                                    )
                                });
                                budgets
                                    .run
                                    .primitive(allowed)
                                    .map_err(Error::BudgetExceeded)?;
                                operations.fuel_charged(allowed);
                                execution.steps = execution.steps.saturating_add(1 + allowed);
                                profile.add(frame.definition.path.path(), 1 + allowed);
                                primitive_limit = allowed != cost;
                            }
                            if callback_depth >= limits.call_depth
                                || before_limit
                                || primitive_limit
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
                                if operations.stop_at_tooling_limit() {
                                    execution.outcome =
                                        Outcome::ToolingLimit(tooling.take().unwrap());
                                    return Ok(Step::Finished);
                                }
                                frame.ending = Some(crate::resources::Ending::Failed);
                                for (i, value) in arguments.into_iter().enumerate().rev() {
                                    if function.parameters.get(i).is_some_and(|p| {
                                        p.passing == hirsel_syntax::PassingMode::Take
                                    }) {
                                        frame.work.push_back(crate::resources::Work::Release(
                                            value,
                                            state.anchor.clone(),
                                            state.caller.clone(),
                                        ));
                                    } else {
                                        value.discard();
                                    }
                                }
                                abandon_operation(frame, state)?;
                                continue;
                            }
                            if reference.definition.is_core("cpu.map_scoped") {
                                let certified = crate::cpu_mapper::certified(
                                    provider,
                                    &runtime.types,
                                    arguments
                                        .get(3)
                                        .ok_or_else(|| invalid("CPU mapper argument"))?,
                                )
                                .await?;
                                operations.cpu_mapper_certified(certified);
                            }
                            let operation = operations.resolve(&reference.definition);
                            if let Some(result) = match operation {
                                None => None,
                                Some(operation) => operations.call_resolved(
                                    operation,
                                    OperationCall {
                                        runtime,
                                        path: &reference.definition,
                                        generics: &reference.arguments,
                                        arguments: &mut arguments,
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
                                )?,
                            } {
                                if !precharged {
                                    execution.steps = execution.steps.saturating_add(1);
                                    profile.add(frame.definition.path.path(), 1);
                                }
                                let taken = function
                                    .parameters
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(i, p)| {
                                        (p.passing == hirsel_syntax::PassingMode::Take).then_some(i)
                                    })
                                    .collect();
                                let native = crate::resources::OperationState {
                                    activation: Some(activation),
                                    caller: None,
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
                                    root_loan: None,
                                    bounds: frame.check.as_ref().map(|check| {
                                        operation_bounds(
                                            &runtime.types,
                                            &check.state.borrow(),
                                            &reference.definition,
                                            &callback_inputs,
                                            &[],
                                            &[],
                                        )
                                    }),
                                };
                                frame.operation = Some(Box::new((result, native)));
                                continue;
                            }
                            let definition = provider
                                .definition(&reference.definition)
                                .await
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
                                    arguments: reference.arguments.clone(),
                                },
                            );
                            (definition, body, vec![], sizes)
                        }
                        _ => return Err(invalid("operation callback requires function value")),
                    };
                    let path = definition.path.path().clone();
                    let mut callback = Frame::new_body(
                        runtime.types.clone(),
                        definition,
                        body,
                        arguments,
                        captures,
                        sizes,
                    )?;
                    callback.depth = frame.depth.clone();
                    callback.activation = Some(activation);
                    if callback.definition.tracks_caller(body) {
                        callback.caller = Some(CallerSite::new(frame, state.anchor.site, sources)?);
                    }
                    callback.type_arguments = invocation_types;
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
                    if let Some(check) = &frame.check {
                        callback.check = Some(enter_checked(
                            &check.state,
                            &callback,
                            callback_inputs,
                            callback_callee,
                        )?);
                    }
                    callback.callback = Some(Box::new(crate::resources::Callback {
                        failure_start: execution.failures.len(),
                        token: continuation,
                        state,
                        mut_parameters,
                        writebacks,
                    }));
                    callback.depth_failure = depth >= limits.call_depth;
                    observer.callback_entered(task, frame, &callback);
                    frames.push(callback);
                    if runtime.trace_mode == TraceMode::Full {
                        execution.trace.push(TraceEvent::Enter {
                            path,
                            depth: depth + 1,
                        });
                    }
                }
                OperationResult::ArenaCharge {
                    arena,
                    bytes,
                    result,
                } => {
                    let result = match budgets.charge(arena, bytes) {
                        Ok(()) => *result,
                        Err(arena) => {
                            crate::resources::discard_result(*result);
                            budgets.exhausted(arena, execution.failures.len());
                            OperationResult::Failed(failure(
                                sources,
                                state.anchor.site,
                                "MEM-6",
                                "OutOfSpace",
                            )?)
                        }
                    };
                    frame.operation = Some(Box::new((result, state)));
                }
                OperationResult::Complete {
                    mut value,
                    arguments,
                    consumed,
                    released,
                } => {
                    if let Some(returns) = state.returns {
                        let ty = closed_storage_type(
                            frame,
                            runtime,
                            frame.place(returns.destination)?.ty,
                        )?;
                        budgets.storage.record(
                            runtime,
                            ty,
                            &mut value,
                            &operation_allocation_site(frame, &state.anchor)?,
                            false,
                        )?;
                    }
                    if budgets.run.metered() {
                        // What the result holds beyond the owners it took or updated.
                        let held = std::iter::once(&value)
                            .chain(arguments.iter().map(|(_, v)| v))
                            .map(Value::shallow_bytes)
                            .fold(0, u64::saturating_add);
                        let given = consumed
                            .iter()
                            .chain(arguments.iter().map(|(i, _)| i))
                            .filter_map(|&i| state.arguments.get(i))
                            .map(Value::shallow_bytes)
                            .fold(0, u64::saturating_add);
                        budgets.run.charge(held.saturating_sub(given));
                    }
                    if let Some(failure_start) = state.finishing {
                        if execution.failures.len() > failure_start {
                            frame.work.push_back(crate::resources::Work::Release(
                                value,
                                state.anchor.clone(),
                                state.caller.clone(),
                            ));
                            retire_operation(frame, &mut state);
                            if let Some(mut callback) = state.callback.take() {
                                writeback_native_callback(&mut callback, &state)?;
                                enqueue_owned(frame, &mut callback.state);
                                callback.state.already_failed = true;
                                let failure =
                                    execution.failures[callback.failure_start].failure.clone();
                                frame.work.push_back(crate::resources::Work::Resume(
                                    OperationResult::Failed(failure),
                                    callback,
                                ));
                            } else {
                                enqueue_cleanup(frame, state.on_failure, state.anchor.site)?;
                            }
                        } else {
                            frame.ending = None;
                            retire_operation(frame, &mut state);
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
                                    if operations.retains_task_arguments(callback.token) {
                                        parent.task_arguments =
                                            Some((value.clone(), child.result.clone()));
                                    }
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
                                    callback,
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
                                                .and_then(|p| runtime.types.get(p.ty))
                                                .is_some_and(|t| {
                                                    t.modes.uniqueness
                                                        == hirsel_types::Uniqueness::Unique
                                                }),
                                        )
                                    })
                                    .unwrap_or_default()
                                    .for_value(&value);
                                frame.assign_shadow(destination, value, shadow)?;
                                observer.builtin_returned(task, frame, frame.observe(destination)?);
                                frame.goto(next);
                            }
                        }
                    } else {
                        transfer_operation(&mut state, consumed)?;
                        update_operation(&mut state, arguments)?;
                        record_operation_storage(frame, &mut state, runtime, &mut budgets)?;
                        restore_operation(frame, &mut state)?;
                        state.finishing = Some(execution.failures.len());
                        frame.ending = Some(crate::resources::Ending::Failed);
                        enqueue_taken(frame, &mut state);
                        for value in released.into_iter().rev() {
                            frame.work.push_back(crate::resources::Work::Release(
                                value,
                                state.anchor.clone(),
                                state.caller.clone(),
                            ));
                        }
                        frame.operation = Some(Box::new((OperationResult::returned(value), state)));
                    }
                }
                OperationResult::Failed(failure) => {
                    record_operation_storage(frame, &mut state, runtime, &mut budgets)?;
                    restore_operation(frame, &mut state)?;
                    if !state.already_failed {
                        record_failure(
                            runtime.trace_mode,
                            execution,
                            failure,
                            state.anchor.clone(),
                            state.caller.as_ref(),
                        );
                    }
                    frame.ending = Some(crate::resources::Ending::Failed);
                    enqueue_taken(frame, &mut state);
                    retire_operation(frame, &mut state);
                    if let Some(mut callback) = state.callback.take() {
                        writeback_native_callback(&mut callback, &state)?;
                        enqueue_owned(frame, &mut callback.state);
                        callback.state.already_failed = true;
                        let failure = execution.failures[callback.failure_start].failure.clone();
                        frame.work.push_back(crate::resources::Work::Resume(
                            OperationResult::Failed(failure),
                            callback,
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
                crate::resources::Work::Retire(activation) => drop(activation),
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
                        &mut callback.state.arguments,
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
                    observer.builtin_completed(task, frame, &resumed);
                    frame.operation = Some(Box::new((resumed, callback.state)));
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
                        state.definition = Some(frame.definition.path.path().clone());
                    }
                    match action {
                        CleanupAction::Drop { place, if_live } => {
                            let shadow = frame.access(place, "drop")?;
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
                                        frame.caller.as_ref(),
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
                                frame.set_shadow(place, Default::default())?;
                                if frame.check.is_some() {
                                    frame
                                        .work
                                        .push_front(crate::resources::Work::RetireShadow(shadow));
                                }
                                observer.drop_place(
                                    task,
                                    frame,
                                    place,
                                    &value,
                                    anchor.site,
                                    sources,
                                );
                                frame.work.push_front(crate::resources::Work::Release(
                                    value,
                                    anchor,
                                    frame.caller.clone(),
                                ));
                            } else {
                                frame.set_shadow(place, Default::default())?;
                            }
                        }
                        CleanupAction::EndLoan { .. } => (),
                        _ => return Err(Error::Unavailable("M2 cleanup".into())),
                    }
                }
                crate::resources::Work::RetireShadow(shadow) => drop(shadow),
                crate::resources::Work::Release(mut value, anchor, caller) => {
                    if value.enrolled_arena_block() {
                        value.discard();
                        continue;
                    }
                    if let Value::Handle { kind, id } = &value
                        && kind == "Region"
                    {
                        let mut owner = budgets.storage.release_owner(*id)?;
                        observer.released_without_finalizer(
                            task,
                            frame,
                            &value,
                            anchor.site,
                            sources,
                        );
                        if let Some(check) = &frame.check {
                            check.state.borrow_mut().released += 1;
                        }
                        frame.work.push_back(crate::resources::Work::Release(
                            std::mem::replace(&mut owner.root, Value::Unit),
                            anchor.clone(),
                            caller.clone(),
                        ));
                        for (_, mut retained) in std::mem::take(&mut owner.retained) {
                            release_fields(
                                frame,
                                &mut retained.value,
                                retained.source.anchor,
                                caller.clone(),
                            )?;
                            retained.value.discard();
                        }
                        continue;
                    }
                    // The lexical scope guard owns arena storage. Consuming a
                    // grant token is mechanical and exposes no checker-local ID.
                    if matches!(&value,Value::Handle {kind,..} if kind==crate::checked::ARENA_HANDLE)
                    {
                        continue;
                    }
                    if let Some(check) = &frame.check {
                        if matches!(
                            value,
                            Value::Resource { .. } | Value::ResourceVariant { .. }
                        ) {
                            check.state.borrow_mut().released += 1;
                        }
                    }
                    if let Value::Shared(root) = &value {
                        if operations.defer_release(root) {
                            continue;
                        }
                        let checking = frame.check.is_some();
                        let graph = root.release()?;
                        if checking && graph.is_some() {
                            anchored_root(&frames, root.id())?;
                        }
                        operations.shared_released(
                            root.id(),
                            &mut OperationContext {
                                configuration,
                                sources,
                                stdout: &mut execution.stdout,
                                trace: &mut execution.trace,
                                trace_mode: runtime.trace_mode,
                                anchor: Some(&anchor),
                            },
                        );
                        // The last root releases the frozen graph it held.
                        if let Some(graph) = graph {
                            frames.last_mut().unwrap().work.push_front(
                                crate::resources::Work::Release(graph, anchor, caller.clone()),
                            );
                        }
                        continue;
                    }
                    if let Value::Mutex(root) = &value {
                        if !root.release()? {
                            continue;
                        }
                        value = Value::Handle {
                            kind: "Mutex".into(),
                            id: root.id(),
                        };
                    }
                    if let Value::Handle { kind, id } = &value {
                        if O::ENABLED {
                            observer.before_handle_drop(
                                frame,
                                kind,
                                *id,
                                operations.ownership_channel(*id),
                            );
                        }
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
                        let suspended = operations.take_drop_wait();
                        if let Some(result) = suspended {
                            frame.drop_wait = Some(Box::new(crate::frame::DropWait {
                                result,
                                anchor,
                                caller,
                            }));
                        } else {
                            finish_handle_drop(
                                frame,
                                operations,
                                runtime.trace_mode,
                                execution,
                                sources,
                                &anchor,
                                caller.as_ref(),
                                observer,
                                task,
                            )?;
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
                                    .contains(definition.path())
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
                        if let Some((path, body)) = runtime
                            .get(definition.path())
                            .and_then(|n| n.finalizer.clone())
                        {
                            let Some(activation) = frame.depth.finalizer() else {
                                record_failure(
                                    runtime.trace_mode,
                                    execution,
                                    failure(sources, anchor.site, "LANG-10", "Depth")?,
                                    anchor.clone(),
                                    caller.as_ref(),
                                );
                                frame.ending = Some(crate::resources::Ending::Failed);
                                observer.released_without_finalizer(
                                    task,
                                    frame,
                                    &value,
                                    anchor.site,
                                    sources,
                                );
                                release_fields(frame, &mut value, anchor, caller)?;
                                continue;
                            };
                            effect(
                                &mut execution.trace,
                                anchor.clone(),
                                crate::observation::Effect::Finalizer {
                                    definition: definition.path().clone(),
                                },
                            );
                            let definition =
                                provider.definition(&path).await.map_err(Error::Query)?;
                            let mut bindings = Default::default();
                            Frame::bind_sizes(
                                &mut bindings,
                                definition.bodies[body.0 as usize].parameter_scope,
                                sizes.iter().copied(),
                            );
                            let sizes = bindings;
                            let mut callee = Frame::new_body(
                                runtime.types.clone(),
                                definition,
                                body,
                                vec![value],
                                vec![],
                                sizes,
                            )?;
                            callee.depth = frame.depth.clone();
                            callee.activation = Some(activation);
                            if let Some(check) = &frame.check {
                                callee.check =
                                    Some(enter_checked(&check.state, &callee, vec![], None)?);
                            }
                            let local = *callee
                                .body()
                                .parameters
                                .first()
                                .ok_or_else(|| invalid("finalizer needs self"))?;
                            callee.finalized = Some(Box::new((local, anchor, caller)));
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
                    observer.released_without_finalizer(task, frame, &value, anchor.site, sources);
                    release_fields(frame, &mut value, anchor, caller)?;
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
                frame.work.push_back(crate::resources::Work::Release(
                    value,
                    anchor,
                    frame.caller.clone(),
                ));
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
            if let Some(crate::resources::Ending::Return(value)) = &frame.ending {
                observer.return_discarded(task, frame, value);
            }
            if let Some(crate::resources::Ending::Return(value)) = &mut frame.ending
                && *value != Value::Unit
            {
                let value = std::mem::replace(value, Value::Unit);
                let site = site_of(frame)?;
                frame.work.push_back(crate::resources::Work::Release(
                    value,
                    anchor(frame, site),
                    frame.caller.clone(),
                ));
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
                    observer.return_discarded(task, frame, value);
                    returned_snapshot = Some(value.clone());
                    *entry_consumed = true;
                    let Some(crate::resources::Ending::Return(value)) = frame.ending.take() else {
                        unreachable!()
                    };
                    frame.ending = Some(crate::resources::Ending::Return(Value::Unit));
                    frame.work.push_back(crate::resources::Work::Release(
                        value,
                        root_anchor,
                        frame.caller.clone(),
                    ));
                    continue;
                }
            }
        }
        // A rest ended: the next rest of its hand-off runs, or the task that handed off
        // continues with its pending operation.
        if frames.len() == 1
            && frames[0].ending.is_some()
            && let Some(index) = hand_offs.iter().position(|h| h.child == task)
        {
            // Rest cleanup owns a real frame too; retire its observation before
            // handing control back to the waiting parent.
            observer.finish_frame(
                task,
                &frames[0],
                frames[0].ending.as_ref().unwrap(),
                sources,
            );
            frames.clear();
            operations.rest_finished(task, std::mem::take(&mut execution.failures))?;
            let hand_off = &mut hand_offs[index];
            if let Some(next) = hand_off.rests.pop_front() {
                hand_off.child = next.child;
                (task, frames) = rest_stack(
                    &mut holding,
                    next,
                    hand_off.frames.last().unwrap(),
                    operations,
                    observer,
                    sources,
                )?;
            } else {
                let hand_off = hand_offs.swap_remove(index);
                task = hand_off.parent;
                frames = hand_off.frames;
                execution.failures = hand_off.failures;
                operations.enter_task(task, frames.iter().any(|f| f.finalized.is_some()));
                let root_anchor = anchor(frames.last().unwrap(), site_of(frames.last().unwrap())?);
                if let Some(operation) = &mut frames.last_mut().unwrap().operation {
                    operations.rests_returned(task, &mut operation.0);
                    operations.rests_completed(
                        task,
                        &mut operation.0,
                        &mut OperationContext {
                            configuration,
                            sources,
                            stdout: &mut execution.stdout,
                            trace: &mut execution.trace,
                            trace_mode: runtime.trace_mode,
                            anchor: Some(&root_anchor),
                        },
                    )?;
                }
            }
            operations.enter_task(task, frames.iter().any(|f| f.finalized.is_some()));
            continue;
        }
        if frames.last().unwrap().ending.is_some() {
            let frame = frames.last_mut().unwrap();
            if let Some(arena) = frame.arena_scope.take() {
                if let Some(inventory) = budgets.storage.close(arena) {
                    for (_, entry) in inventory.into_entries() {
                        let mut retained = entry.payload;
                        release_fields(
                            frame,
                            &mut retained.value,
                            retained.source.anchor,
                            frame.caller.clone(),
                        )?;
                        retained.value.discard();
                    }
                }
                // Drain outgoing owners before retiring the lexical arena guard.
                if !frame.work.is_empty() {
                    continue;
                }
            }
        }
        if frames.last().unwrap().ending.is_some()
            && let Some(ending) = frames.last_mut().unwrap().ending.take()
        {
            observer.finish_frame(task, frames.last().unwrap(), &ending, sources);
            let mut finished = frames.pop().unwrap();
            // Map the result and mut write-backs into the caller's lifetimes, then
            // end every instance the frame owns (MODE-1, MEM-10, MEM-4).
            let mut returned = Shadow::default();
            let mut written = vec![];
            if let Some(check) = &mut finished.check {
                let body = &finished.definition.bodies[finished.body_id.0 as usize];
                returned = check.returned_to_caller(body, &runtime.types);
                for local in &finished.mut_parameters {
                    if let Some(ordinal) = body.parameters.iter().position(|p| p == local) {
                        written.push((*local, check.written_back(ordinal, *local)));
                    }
                }
                check.exit();
            }
            if runtime.trace_mode == TraceMode::Full {
                execution.trace.push(TraceEvent::Return {
                    path: finished.definition.path.path().clone(),
                    depth,
                });
            }
            if let Some((local, anchor, caller)) =
                finished.finalized.take().map(|finalized| *finalized)
            {
                if matches!(&ending,crate::resources::Ending::Return(value) if *value != Value::Unit)
                {
                    return Err(invalid("finalizer must return unit"));
                }
                let mut value = finished.slots[local.0 as usize]
                    .take()
                    .ok_or_else(|| invalid("finalizer consumed self"))?;
                if let Some(activation) = finished.activation.take() {
                    frames
                        .last_mut()
                        .unwrap()
                        .work
                        .push_front(crate::resources::Work::Retire(activation));
                }
                release_fields(frames.last_mut().unwrap(), &mut value, anchor, caller)?;
                if matches!(ending, crate::resources::Ending::Failed) {
                    let caller = frames.last_mut().unwrap();
                    let already_exiting = caller.ending.is_some();
                    if let Some(crate::resources::Ending::Return(value)) = caller.ending.take() {
                        caller.work.push_back(crate::resources::Work::Release(
                            value,
                            crate::observation::Anchor {
                                definition: caller.definition.path,
                                body: caller.body_id,
                                cleanup: None,
                                site: site_of(caller)?,
                            },
                            caller.caller.clone(),
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
                observer.callback_released(
                    caller,
                    &released,
                    abandoned,
                    &callback.state.anchor,
                    sources,
                );
                for value in released.into_iter().rev() {
                    caller.work.push_back(crate::resources::Work::Release(
                        value,
                        callback.state.anchor.clone(),
                        callback.state.caller.clone(),
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
                        if operations.retains_task_arguments(callback.token)
                            && let crate::resources::Ending::Return(value) = &ending
                        {
                            bounds.task_arguments = Some((value.clone(), returned.clone()));
                        }
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
                    let holds = matches!(&result, OperationResult::Complete { .. });
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
                    // A holding child keeps its ended frame, where a discard drops its result.
                    if holds && operations.holds_result(task) {
                        for value in finished.slots.drain(..).flatten() {
                            value.discard();
                        }
                        holding.insert(task, finished);
                        if holding.len() > holding_mark {
                            holding.retain(|id, _| operations.holds_result(*id));
                            holding_mark = (holding.len() * 2).max(HOLDING_MARK);
                        }
                    }
                    continue;
                }
                let root_end = task == 0 && tooling.is_none();
                execution.outcome = if let Some(limit) = tooling.take() {
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
                if root_end
                    && operations.root_finished()?
                    && !(runnable.is_empty() && parked.iter().next().is_none())
                {
                    // A free-mode replay: the other tasks still run to their cut.
                    continue;
                }
                return Ok(Step::Finished);
            }
            continue;
        }
        // Fuel is charged at function entries and back-edges, and the memory model is
        // measured before the step after a charge that may pass the budget.
        if frames.last().is_some_and(|f| f.fuel_due) {
            frames.last_mut().unwrap().fuel_due = false;
            budgets.run.fuel().map_err(Error::BudgetExceeded)?;
            operations.fuel_charged(1);
        }
        if budgets.run.due() {
            let live = frames
                .iter()
                .chain(runnable.iter().flat_map(|(_, stack, _)| stack))
                .chain(parked.iter().flat_map(|(_, stack, _)| stack))
                .chain(hand_offs.iter().flat_map(|h| &h.frames))
                .map(Frame::logical_bytes)
                .fold(
                    operations
                        .retained_bytes()
                        .saturating_add(budgets.storage.logical_bytes()),
                    u64::saturating_add,
                );
            budgets.run.measured(live).map_err(Error::BudgetExceeded)?;
        }
        let frame = frames.last_mut().unwrap();
        let code = match frame.code {
            Some(code) => code,
            None => {
                let code = codes.id(&runtime.types, &frame.definition, frame.body_id);
                frame.code = Some(code);
                frame.places = Some(codes.get(code).places.clone());
                code
            }
        };
        let before = execution.steps;
        let general = if O::ENABLED {
            true
        } else {
            dispatch(
                frame,
                codes.get(code),
                depth,
                limits,
                runtime.trace_mode,
                execution,
            )
        };
        profile.add(frame.definition.path.path(), execution.steps - before);
        if !general {
            continue;
        }
        let call = codes
            .get(code)
            .op(frame.block, frame.instruction)
            .call
            .clone();
        // The step reads its code through its own handle on the immutable
        // definition, so the frame stays free for mutation without a copy.
        let definition = frame.definition.clone();
        let block = definition
            .bodies
            .get(frame.body_id.0 as usize)
            .and_then(|body| body.blocks.get(frame.block.0 as usize))
            .ok_or_else(|| invalid("block out of range"))?;
        let instruction = block.instructions.get(frame.instruction);
        let terminator = &block.terminator;
        let site = instruction.map_or(block.span, |i| i.span);
        let cleanup =
            instruction.map_or_else(|| terminator_cleanup(terminator), |i| Some(i.on_failure));
        // A source-budget charge is not an execution step (CT-4). The event past
        // the budget is denied before it executes, and nothing further runs.
        if let Some(Instruction {
            kind: InstructionKind::Charge(event),
            span,
            ..
        }) = instruction
        {
            if let Some(budget) = limits.source {
                if execution.charged == budget {
                    return Err(Error::Exhausted(Box::new(crate::Exhaustion {
                        budget,
                        event: *event,
                        definition: frame.definition.path.path().clone(),
                        site: *span,
                    })));
                }
                execution.charged += 1;
            }
            frame.instruction += 1;
            continue;
        }
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
            if operations.stop_at_tooling_limit() {
                execution.outcome = Outcome::ToolingLimit(tooling.take().unwrap());
                return Ok(Step::Finished);
            }
            frame.ending = Some(crate::resources::Ending::Failed);
            if let Some(id) = cleanup {
                enqueue_cleanup(frame, id, site)?;
            }
            continue;
        }
        // An opt-in simulator point precedes the store, before any taken call
        // arguments leave their slots. Resuming executes this same call once.
        let atomic_store = instruction.is_none()
            && match terminator {
                Terminator::Call {
                    callee: Callee::Definition { path, .. },
                    ..
                } => path.0.name == "store" && path.is_core("atomic.store"),
                Terminator::Call {
                    callee: Callee::Indirect { value, .. },
                    ..
                } => matches!(
                frame.observe(value.place), Ok(Value::FunctionReference(reference))
                    if reference.definition.is_core("atomic.store")),
                _ => false,
            };
        if atomic_store {
            if !frame.store_point_passed
                && operations
                    .simulation_point(crate::library::simulation::SchedulePoint::AtomicStore)
            {
                frame.store_point_passed = true;
                execution.trace.push(TraceEvent::SimulationPoint(
                    crate::library::simulation::SchedulePoint::AtomicStore,
                ));
                continue;
            }
            frame.store_point_passed = false;
        }
        execution.steps = execution.steps.saturating_add(1);
        profile.add(frame.definition.path.path(), 1);
        if let Some(check) = &frame.check {
            let mut state = check.state.borrow_mut();
            state.site = site;
            state.definition = Some(frame.definition.path.path().clone());
        }
        observer.before_step(
            task,
            frame,
            instruction.map(|i| &i.kind),
            terminator,
            site,
            sources,
        );
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
            match &instruction.kind {
                InstructionKind::Check { condition, rule } => match frame.operand(condition) {
                    Ok(Value::Bool(true)) => Ok(Action::Continue),
                    Ok(Value::Bool(false)) => Ok(Action::Fail(rule.clone())),
                    Ok(_) => Err(invalid("check needs bool").into()),
                    Err(e) => Err(e.into()),
                },
                &InstructionKind::Drop { place, if_live } => {
                    arena_write(frame, place, false, &mut budgets).and_then(|()| {
                        let shadow = frame.access(place, "drop")?;
                        let value = frame.slot(place)?.take();
                        frame.set_shadow(place, Default::default())?;
                        Ok(Action::Drop {
                            value,
                            if_live,
                            shadow,
                        })
                    })
                }
                &InstructionKind::RunCleanup(id) => Ok(Action::Cleanup(id)),
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
                .await
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
            execute_terminator(frame, terminator, call.as_ref(), runtime, &mut budgets)
        };
        observer.after_step(task, frame, result.is_ok());
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
                    frame.caller.as_ref(),
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
                    frame.caller.as_ref(),
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
                    frame.caller.as_ref(),
                );
                frame.ending = Some(crate::resources::Ending::Failed);
                if let Some(id) = cleanup {
                    enqueue_cleanup(frame, id, site)?;
                }
            }
            Err(reason @ (EvalError::InvocationDepth | EvalError::KeyDepth)) => {
                let rule = if matches!(reason, EvalError::KeyDepth) {
                    "MEM-3"
                } else {
                    "LANG-10"
                };
                record_failure(
                    runtime.trace_mode,
                    execution,
                    failure(sources, site, rule, "Depth")?,
                    anchor(frame, site),
                    frame.caller.as_ref(),
                );
                frame.ending = Some(crate::resources::Ending::Failed);
                // TIR's entered-callee failure list excludes taken actuals.
                // Rejected admission leaves them here; release them first in
                // reverse evaluation order, then run that ordinary failure list.
                if let Terminator::Call {
                    callee, arguments, ..
                } = terminator
                {
                    for argument in arguments.iter().rev() {
                        if argument.value.usage == hirsel_types::UseKind::Take {
                            let value = frame.operand(&argument.value)?;
                            frame.work.push_back(crate::resources::Work::Release(
                                value,
                                anchor(frame, site),
                                frame.caller.clone(),
                            ));
                        }
                    }
                    if let Callee::Indirect { value, .. } = callee
                        && value.usage == hirsel_types::UseKind::Take
                    {
                        let value = frame.operand(value)?;
                        frame.work.push_back(crate::resources::Work::Release(
                            value,
                            anchor(frame, site),
                            frame.caller.clone(),
                        ));
                    }
                }
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
                    frame.caller.as_ref(),
                );
                frame.ending = Some(crate::resources::Ending::Failed);
                if let Some(id) = cleanup {
                    enqueue_cleanup(frame, id, site)?;
                }
            }
            Err(EvalError::Api(error)) => return Err(error),
            Ok(Action::Continue) => (),
            Ok(Action::Drop {
                value,
                if_live,
                shadow,
            }) => {
                if value.is_none() && !if_live {
                    return Err(invalid("unconditional drop of dead storage"));
                }
                if let Some(value) = value {
                    if let Some(Instruction {
                        kind: InstructionKind::Drop { place, .. },
                        ..
                    }) = instruction
                    {
                        observer.drop_place(task, frame, *place, &value, site, sources);
                    }
                    frame.work.push_back(crate::resources::Work::Release(
                        value,
                        anchor(frame, site),
                        frame.caller.clone(),
                    ));
                    if frame.check.is_some() {
                        frame
                            .work
                            .push_back(crate::resources::Work::RetireShadow(shadow));
                    }
                }
            }
            Ok(Action::Cleanup(id)) => enqueue_cleanup(frame, id, site)?,
            Ok(Action::Fail(rule)) => {
                record_failure(
                    runtime.trace_mode,
                    execution,
                    failure(sources, site, rule.as_str(), "Check")?,
                    anchor(frame, site),
                    frame.caller.as_ref(),
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
                activation,
                restart,
                forward_caller,
                callee,
                mut checks,
                mut arguments,
                mut_places,
                taken,
                returns,
                on_failure,
            }) => {
                let operation_caller = if forward_caller {
                    frame.caller.clone()
                } else {
                    None
                };
                // The arena adapter is mechanical, but its declared primitive and
                // callback are two independently admitted language invocations.
                // Admit the callback before consuming its closure/environment.
                let arena_callback = matches!(&callee, RuntimeCallee::Definition { path, .. }
                    if path.path().is_core(crate::library::ARENA_WITH_OPERATION)
                        || path.path().is_core(crate::library::ARENA_BOUNDED_OPERATION));
                let callback_activation = if arena_callback {
                    match frame.depth.admit() {
                        Some(guard) => Some(guard),
                        None => {
                            frame.operation = Some(Box::new((
                                OperationResult::Failed(failure(
                                    sources, site, "LANG-10", "Depth",
                                )?),
                                crate::resources::OperationState {
                                    activation,
                                    caller: operation_caller,
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
                                    root_loan: None,
                                    bounds: None,
                                },
                            )));
                            continue;
                        }
                    }
                } else {
                    None
                };
                let (callee, closes_arena, bounded) =
                    checked_callee(frame, callee, &mut arguments, &mut checks, &mut budgets)?;
                let mut closure_check = None;
                let mut invocation_types;
                let mut callee_code = None;
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
                        let definition = provider
                            .definition(definition.path())
                            .await
                            .map_err(Error::Query)?;
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
                    RuntimeCallee::Definition { path, generics } => {
                        let target = match path {
                            CalleePath::Site(call)
                                if call.closed
                                    && (generics.is_empty() || frame.type_arguments.is_empty()) =>
                            {
                                match call.target.get() {
                                    Some(target) => target.clone(),
                                    None => {
                                        let target = std::rc::Rc::new(
                                            target(
                                                frame,
                                                provider,
                                                runtime,
                                                call.callee.clone(),
                                                generics,
                                            )
                                            .await?,
                                        );
                                        let _ = call.target.set(target.clone());
                                        target
                                    }
                                }
                            }
                            path => std::rc::Rc::new(
                                target(frame, provider, runtime, path.into_path(), generics)
                                    .await?,
                            ),
                        };
                        if forward_caller && let Some(tail) = &target.tail {
                            let released = frame.loop_owned.len();
                            for (value, anchor) in frame.loop_owned.drain(..).rev() {
                                frame.work.push_back(crate::resources::Work::Release(
                                    value,
                                    anchor,
                                    frame.caller.clone(),
                                ));
                            }
                            for &(_, place) in &tail.owned_loop_arguments {
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
                                target.actuals.arguments.iter().map(|arg| match arg {
                                    hirsel_types::GenericArgument::Size(
                                        hirsel_types::Size::Constant(n),
                                    ) => Some(*n),
                                    _ => None,
                                }),
                            );
                            crate::type_arguments::bind(
                                &mut frame.type_arguments,
                                scope,
                                target.actuals.clone(),
                            );
                            frame.restart_tail(arguments)?;
                            if let Some(old) = frame.check.take() {
                                let mut entered = enter_checked(&old.state, frame, checks, None)?;
                                entered.inherit(old)?;
                                frame.check = Some(entered);
                            }
                            continue;
                        }
                        let path = &target.path;
                        // Pure borrowed primitive work has the same deterministic charge in
                        // reference and checked H. Preflight before doing work or publishing
                        // any result; the ordinary call terminator remains its own step.
                        let cost = primitive_work(path, &arguments)?;
                        if cost != 0 {
                            let allowed = limits.steps.map_or(cost, |limit| {
                                cost.min(limit.saturating_sub(execution.steps))
                            });
                            budgets
                                .run
                                .primitive(allowed)
                                .map_err(Error::BudgetExceeded)?;
                            operations.fuel_charged(allowed);
                            execution.steps = execution.steps.saturating_add(allowed);
                            profile.add(frame.definition.path.path(), allowed);
                            if allowed != cost {
                                let limit = ToolingLimit {
                                    kind: ToolingLimitKind::Steps,
                                    limit: limits.steps.unwrap(),
                                    site: Some(site),
                                };
                                if runtime.trace_mode == TraceMode::Full {
                                    execution
                                        .trace
                                        .push(TraceEvent::ToolingLimit(limit.clone()));
                                }
                                if operations.stop_at_tooling_limit() {
                                    execution.outcome = Outcome::ToolingLimit(limit);
                                    return Ok(Step::Finished);
                                }
                                tooling.get_or_insert(limit);
                                frame.ending = Some(crate::resources::Ending::Failed);
                                enqueue_cleanup(frame, on_failure, site)?;
                                continue;
                            }
                        }
                        if target.forget {
                            if arguments.len() != 1 || !mut_places.is_empty() {
                                return Err(invalid("forget arity/mode"));
                            }
                            let value = arguments.pop().unwrap();
                            let (Value::Resource { definition, .. }
                            | Value::ResourceVariant { definition, .. }) = &value
                            else {
                                return Err(invalid("forget requires resource"));
                            };
                            if definition.path().0.package != frame.definition.path.path().0.package
                                || definition.path().0.modules
                                    != frame.definition.path.path().0.modules
                            {
                                return Err(invalid("cross-module forget"));
                            }
                            effect(
                                &mut execution.trace,
                                anchor(frame, site),
                                crate::observation::Effect::Forgotten {
                                    definition: definition.path().clone(),
                                },
                            );
                            if let Some(check) = &frame.check {
                                check.state.borrow_mut().forgotten +=
                                    crate::checked::resources_in(&value);
                            }
                            crate::aggregate::forget(value)?;
                            observer.forgotten(task, frame, site, sources);
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
                            None => runtime.types.intern(hirsel_types::Type {
                                kind: hirsel_types::TypeKind::Primitive(
                                    hirsel_types::Primitive::Never,
                                ),
                                ..hirsel_types::Type::unit()
                            }),
                        };
                        let types = &*runtime.types;
                        // MEM-6: a copy into a bounded arena charges its whole tree first,
                        // and builds every List and Array of it unshared there.
                        let exhausted = match target.arena {
                            crate::code::ArenaCall::None => None,
                            crate::code::ArenaCall::Copy => {
                                copy_to_arena(runtime, result_type, &mut arguments, &mut budgets)?
                            }
                            crate::code::ArenaCall::List(name) => {
                                list_write(name, &mut arguments, &mut budgets)
                            }
                            crate::code::ArenaCall::Keyed(name) => {
                                keyed_write(name, &mut arguments, &mut budgets)
                            }
                            crate::code::ArenaCall::Read(name) => {
                                read_write(name, &mut arguments, &mut budgets)
                            }
                        };
                        let result = if let Some(arena) = exhausted {
                            budgets.exhausted(arena, execution.failures.len());
                            Some(OperationResult::Failed(failure(
                                sources,
                                site,
                                "MEM-6",
                                "OutOfSpace",
                            )?))
                        } else if let Some(operation) = *target.operation.get_or_init(|| {
                            operations.resolve(path).map(|id| crate::code::Operation {
                                id,
                                immediate: operations.immediate(id),
                            })
                        }) {
                            if path.is_core("cpu.map_scoped") {
                                let certified = crate::cpu_mapper::certified(
                                    provider,
                                    &runtime.types,
                                    arguments
                                        .get(3)
                                        .ok_or_else(|| invalid("CPU mapper argument"))?,
                                )
                                .await?;
                                operations.cpu_mapper_certified(certified);
                            }
                            operations.call_resolved(
                                operation.id,
                                OperationCall {
                                    runtime,
                                    path,
                                    generics: &target.actuals.arguments,
                                    arguments: &mut arguments,
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
                        } else {
                            None
                        };
                        if let Some(mut result) = result {
                            if O::ENABLED {
                                let channel = match arguments.get(1) {
                                    Some(Value::Handle { id, .. }) => {
                                        operations.ownership_channel(*id)
                                    }
                                    _ => None,
                                };
                                observer.builtin_entered(task, frame, &arguments, &result, channel);
                            }
                            if frame.check.is_none()
                                && !budgets.run.metered()
                                && target
                                    .operation
                                    .get()
                                    .copied()
                                    .flatten()
                                    .is_some_and(|op| op.immediate)
                                && complete_immediate(
                                    frame,
                                    &mut result,
                                    &mut arguments,
                                    &mut_places,
                                    &taken,
                                    returns,
                                    target.arena,
                                )?
                            {
                                if let Some(returns) = returns {
                                    observer.builtin_returned(
                                        task,
                                        frame,
                                        frame.observe(returns.destination)?,
                                    );
                                }
                                continue;
                            }
                            // MODE-2: a native operation's parameter declared
                            // `unique`, such as `shared.freeze`'s sole-owner
                            // precondition (MEM-11), received no value another
                            // live holder shares; checked execution counts
                            // holders, independently of the proofs. A callee
                            // with a frame is checked at its entry instead.
                            if let Some(check) = &frame.check
                                && let Some(ordinal) = shared_unique_input(
                                    &runtime.types,
                                    &check.state.borrow().config,
                                    &path,
                                    &checks,
                                )
                            {
                                return Err(check.state.borrow().trap(
                                    crate::checked::TrapRule::UniqueShared,
                                    format!(
                                        "{} parameter {ordinal} is declared unique, but its block has another live holder",
                                        path.0.name
                                    ),
                                ));
                            }
                            let bounds = frame.check.as_ref().map(|check| {
                                let state = check.state.borrow();
                                let mut bounds = operation_bounds(
                                    &runtime.types,
                                    &state,
                                    path,
                                    &checks,
                                    &mut_places,
                                    &taken,
                                );
                                bounds.list_operation(&state, path, &arguments, &result);
                                bounds
                            });
                            frame.operation = Some(Box::new((
                                result,
                                crate::resources::OperationState {
                                    activation,
                                    caller: operation_caller,
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
                                    root_loan: None,
                                    bounds,
                                },
                            )));
                            continue;
                        }
                        // Generic TIR executes parametrically: scalar values retain
                        // their widths and aggregates retain their runtime shape.
                        // Static bounds and call types were checked at publication.
                        let entry = match target.entry.get() {
                            Some(entry) => entry,
                            None => {
                                let definition =
                                    provider.definition(path).await.map_err(Error::Query)?;
                                let body = definition.entry;
                                let scope = definition.bodies[body.0 as usize].parameter_scope;
                                let mut sizes = Default::default();
                                Frame::bind_sizes(
                                    &mut sizes,
                                    scope,
                                    target.actuals.arguments.iter().map(
                                        |argument| match argument {
                                            hirsel_types::GenericArgument::Size(
                                                hirsel_types::Size::Constant(n),
                                            ) => Some(*n),
                                            _ => None,
                                        },
                                    ),
                                );
                                let mut type_arguments = Default::default();
                                crate::type_arguments::bind(
                                    &mut type_arguments,
                                    scope,
                                    target.actuals.clone(),
                                );
                                let code = codes.id(&runtime.types, &definition, body);
                                target.entry.get_or_init(|| crate::code::Entry {
                                    definition,
                                    body,
                                    sizes,
                                    type_arguments,
                                    code,
                                })
                            }
                        };
                        invocation_types = entry.type_arguments.clone();
                        callee_code = Some(entry.code);
                        (
                            entry.definition.clone(),
                            entry.body,
                            vec![],
                            entry.sizes.clone(),
                        )
                    }
                };
                let path =
                    (runtime.trace_mode == TraceMode::Full).then(|| definition.path.path().clone());
                let mut callee = Frame::new_body(
                    runtime.types.clone(),
                    definition,
                    body,
                    arguments,
                    captures,
                    sizes,
                )?;
                callee.depth = frame.depth.clone();
                if arena_callback {
                    callee.activation = callback_activation;
                    callee.enclosing_activation = activation;
                } else {
                    callee.activation = activation;
                }
                if callee.definition.tracks_caller(body) {
                    callee.caller =
                        if forward_caller && frame.definition.tracks_caller(frame.body_id) {
                            frame.caller.clone()
                        } else {
                            Some(CallerSite::new(frame, site, sources)?)
                        };
                }
                callee.type_arguments = invocation_types;
                callee.code = callee_code;
                callee.places = callee_code.map(|id| codes.get(id).places.clone());
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
                if let Some(check) = &frame.check {
                    let mut entered = enter_checked(&check.state, &callee, checks, closure_check)?;
                    entered.closes_arena = closes_arena;
                    callee.check = Some(entered);
                }
                callee.arena_scope = closes_arena.map(u64::from);
                // MEM-6: the callback never runs with an unbounded arena.
                callee.bounded = bounded.map(|(arena, cap)| {
                    budgets.open(arena, cap);
                    crate::bounded::Boundary {
                        arena,
                        failures: execution.failures.len(),
                    }
                });
                callee.restarts = frame.restarts + usize::from(restart);
                callee.depth_failure = depth - callee.restarts >= limits.call_depth;
                frame.continuation = Some(Continuation {
                    returns,
                    on_failure,
                    writebacks,
                });
                observer.call_entered(task, frame, &callee);
                frames.push(callee);
                if let Some(path) = path {
                    execution.trace.push(TraceEvent::Enter {
                        path,
                        depth: depth + 1,
                    });
                }
            }
        }
    }
}
/// Step the top frame through its resolved ops while each one completes without
/// failure, a call or queued work, with exactly the general step's effects, step
/// count and trace. Returns true when the general step must execute the current op,
/// and false when the scheduler must look at the frame again first (a jump that
/// charges fuel).
fn dispatch(
    frame: &mut Frame,
    code: &crate::code::Code,
    depth: usize,
    limits: Limits,
    trace_mode: TraceMode,
    execution: &mut Execution,
) -> bool {
    use crate::code::{CleanupStep, OpKind};
    // Neither checked state nor a depth failure can change within scalar
    // dispatch. Both return to the general step before fetching any operation.
    if frame.depth_failure || frame.check.is_some() {
        return true;
    }
    let mut ops = code.block_ops(frame.block);
    loop {
        if limits.steps == Some(execution.steps) {
            return true;
        }
        let op = &ops[frame.instruction];
        // Each arm decides whether its op completes before changing anything; `step`
        // then counts and traces it exactly as the general step does.
        let step = |frame: &Frame, execution: &mut Execution, terminator: bool| {
            execution.steps = execution.steps.saturating_add(1);
            if trace_mode == TraceMode::Full {
                execution.trace.push(if terminator {
                    TraceEvent::Terminator {
                        depth,
                        block: frame.block,
                        site: op.site,
                    }
                } else {
                    TraceEvent::Instruction {
                        depth,
                        block: frame.block,
                        index: frame.instruction as u32,
                        site: op.site,
                    }
                });
            }
        };
        match &op.kind {
            OpKind::General => return true,
            OpKind::Charge => {
                // The general step denies the charge past the budget.
                if limits.source == Some(execution.charged) {
                    return true;
                }
                if limits.source.is_some() {
                    execution.charged += 1;
                }
                frame.instruction += 1;
                continue;
            }
            OpKind::Nop => step(frame, execution, false),
            &OpKind::Live(slot) => {
                if frame.slots[slot as usize].is_some() {
                    return true;
                }
                step(frame, execution, false);
            }
            &OpKind::Dead { slot, if_live } => {
                if frame.slots[slot as usize].is_none() && !if_live {
                    return true;
                }
                step(frame, execution, false);
                frame.slots[slot as usize] = None;
            }
            OpKind::EndScope(slots) => {
                step(frame, execution, false);
                for &slot in slots.iter() {
                    frame.slots[slot as usize] = None;
                }
            }
            OpKind::Constant { slot, value } => {
                if frame.slots[*slot as usize].is_some() {
                    return true;
                }
                step(frame, execution, false);
                frame.slots[*slot as usize] = Some(value.clone());
            }
            &OpKind::Use { slot, value } => {
                if frame.slots[slot as usize].is_some() {
                    return true;
                }
                let Some(value) = frame.local_operand(value) else {
                    return true;
                };
                step(frame, execution, false);
                frame.slots[slot as usize] = Some(value);
            }
            &OpKind::Unary { op, slot, value } => {
                let Some(operand) = frame.peek(value) else {
                    return true;
                };
                if frame.slots[slot as usize].is_some() {
                    return true;
                }
                let Ok(result) = scalar::unary(op, operand.clone()) else {
                    return true;
                };
                step(frame, execution, false);
                frame.commit(value);
                frame.slots[slot as usize] = Some(result);
            }
            &OpKind::Binary {
                op,
                slot,
                left,
                right,
            } => {
                let (Some(a), Some(b)) = (frame.peek(left), frame.peek(right)) else {
                    return true;
                };
                let moved = left.usage == hirsel_types::UseKind::Take
                    || right.usage == hirsel_types::UseKind::Take;
                if moved && left.slot == right.slot || frame.slots[slot as usize].is_some() {
                    return true;
                }
                let result = match (a, b) {
                    // Structural order on two integers of one kind is their scalar order.
                    (&Value::Integer(a), &Value::Integer(b)) if a.kind() == b.kind() => {
                        scalar::integer(op, a, b).ok()
                    }
                    _ if matches!(
                        op,
                        hirsel_syntax::BinaryOp::Eq
                            | hirsel_syntax::BinaryOp::Ne
                            | hirsel_syntax::BinaryOp::Lt
                            | hirsel_syntax::BinaryOp::Le
                            | hirsel_syntax::BinaryOp::Gt
                            | hirsel_syntax::BinaryOp::Ge
                    ) =>
                    {
                        crate::structural::binary(op, a, b).ok()
                    }
                    _ => scalar::binary(op, a.clone(), b.clone()).ok(),
                };
                let Some(result) = result else {
                    return true;
                };
                step(frame, execution, false);
                frame.commit(left);
                frame.commit(right);
                frame.slots[slot as usize] = Some(result);
            }
            &OpKind::Replace {
                slot,
                value,
                displaced,
                if_live,
            } => {
                if !if_live && frame.slots[slot as usize].is_none()
                    || frame.slots[displaced as usize].is_some()
                    || value.slot == displaced
                {
                    return true;
                }
                let Some(value) = frame.local_operand(value) else {
                    return true;
                };
                step(frame, execution, false);
                if let Some(old) = frame.slots[slot as usize].replace(value) {
                    frame.slots[displaced as usize] = Some(old);
                }
            }
            &OpKind::Check(condition) => {
                if !matches!(frame.peek(condition), Some(Value::Bool(true))) {
                    return true;
                }
                step(frame, execution, false);
                frame.commit(condition);
            }
            OpKind::Cleanup { id, steps } => {
                // Dropping a dead `if_live` slot or an inert value queues nothing
                // and releases nothing, so the whole list runs here, in order.
                let inert = steps.iter().all(|step| match *step {
                    CleanupStep::Drop { slot, if_live } => frame.slots[slot as usize]
                        .as_ref()
                        .map_or(if_live, Value::inert),
                    CleanupStep::EndLoan => true,
                });
                if !inert {
                    return true;
                }
                step(frame, execution, false);
                if trace_mode == TraceMode::Full {
                    for action in frame.body().cleanup_actions(*id) {
                        execution.trace.push(TraceEvent::Cleanup {
                            depth,
                            action: action.clone(),
                        });
                    }
                }
                for step in steps.iter() {
                    if let CleanupStep::Drop { slot, .. } = *step {
                        frame.slots[slot as usize] = None;
                    }
                }
            }
            &OpKind::Goto(block) => {
                step(frame, execution, true);
                frame.goto(block);
                if frame.fuel_due {
                    return false;
                }
                ops = code.block_ops(frame.block);
                continue;
            }
            &OpKind::Branch { condition, yes, no } => {
                let Some(&Value::Bool(value)) = frame.peek(condition) else {
                    return true;
                };
                step(frame, execution, true);
                frame.commit(condition);
                frame.goto(if value { yes } else { no });
                if frame.fuel_due {
                    return false;
                }
                ops = code.block_ops(frame.block);
                continue;
            }
        }
        frame.instruction += 1;
    }
}
/// Closed operation identity, independent of source spelling or callback routing.
fn primitive_work(path: &hirsel_syntax::DefPath, arguments: &[Value]) -> Result<u64, Error> {
    if path.0.package.package_is("core", "1")
        && path.0.modules == ["bytes"]
        && let Some(algorithm) = hirsel_kernels::sha::Algorithm::from_operation(&path.0.name)
    {
        let Some(Value::Bytes(data)) = arguments.first() else {
            return Err(invalid("SHA requires a byte extent"));
        };
        Ok(algorithm.charge(data.len() as u64))
    } else {
        Ok(crate::library::numeric_work(path, arguments)?.unwrap_or(0))
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
    match value {
        Value::Record { definition, fields }
            if definition.path().package_is("std", "1")
                && definition.path().0.modules.as_ref() == ["process"]
                && definition.path().0.name == "ExitCode" =>
        {
            match fields.first().and_then(Option::as_ref) {
                Some(Value::Integer(n)) => n.bits() as i32,
                _ => 0,
            }
        }
        Value::Variant { variant: 0, fields } => match fields.first().and_then(Option::as_ref) {
            Some(value @ Value::Record { .. }) => exit_code(value),
            _ => 0,
        },
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
/// The root anchor check of checked interpretation (S-14, RC-ROOT): a `Shared` root dies
/// only when no live binding of the releasing task still reaches it. Every holder is
/// released at its TIR cleanup action, after its binding gave the value up, so a binding
/// still reaching a dead root is a holder dropped before its cleanup.
fn anchored_root(frames: &[Frame], id: u64) -> Result<(), Error> {
    let bound = frames
        .iter()
        .flat_map(|frame| frame.slots.iter().flatten())
        .any(|value| value.reaches_root(id));
    if bound {
        return Err(invalid(format!(
            "Shared#{id} lost its last root while a live binding still reaches it (S-14)"
        )));
    }
    Ok(())
}

fn anchor(frame: &Frame, site: Span) -> crate::observation::Anchor {
    crate::observation::Anchor {
        definition: frame.definition.path,
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
    mut failure: Failure,
    mut anchor: crate::observation::Anchor,
    caller: Option<&CallerSite>,
) {
    let operation = caller
        .filter(|caller| caller.site != anchor.site)
        .map(|caller| {
            failure.site = caller.site;
            failure.line = caller.line;
            std::mem::replace(&mut anchor, caller.anchor())
        });
    let index = execution.failures.len();
    if let Some(previous) = execution.failures.last_mut() {
        previous.cause = Some(index);
    }
    if trace_mode == TraceMode::Full {
        execution.trace.push(TraceEvent::Failure(failure.clone()));
    }
    execution.failures.push(crate::observation::FailureRecord {
        operation,
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
    caller: Option<CallerSite>,
) -> Result<(), Error> {
    if let Value::Closure {
        captures,
        environment,
        invoked,
        ..
    } = value
    {
        if captures.len() != environment.owned_captures.len() {
            return Err(invalid("capture ownership count"));
        }
        for (capture, owned) in captures.iter_mut().zip(&environment.owned_captures) {
            if let Some(v) = capture.take() {
                if *owned && !invoked.as_ref().is_some_and(|v| v.get().is_some()) {
                    frame.work.push_front(crate::resources::Work::Release(
                        v,
                        anchor.clone(),
                        caller.clone(),
                    ));
                } else {
                    v.discard();
                }
            }
        }
    } else if let Some(fields) = value.fields_mut() {
        // Push front in declaration order: the worklist visits high to low (DROP-2).
        for field in fields.iter_mut() {
            if let Some(v) = field.take() {
                frame.work.push_front(crate::resources::Work::Release(
                    v,
                    anchor.clone(),
                    caller.clone(),
                ));
            }
        }
    }
    Ok(())
}
enum EvalError {
    InactiveVariant,
    OutOfBounds,
    Depth,
    InvocationDepth,
    /// Structural key rejection before any actual leaves caller storage.
    KeyDepth,
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
        shadow: crate::checked::Shadow,
    },
    Cleanup(CleanupId),
    Continue,
    Return(Value),
    Fail(RuleId),
    Call {
        activation: Option<crate::depth::Guard>,
        /// A constructor-tail site (LANG-10): the callee continues this
        /// frame's logical activation and is no deeper for the tooling guard.
        restart: bool,
        forward_caller: bool,
        callee: RuntimeCallee,
        checks: Vec<crate::checked::Argument>,
        arguments: Vec<Value>,
        mut_places: Vec<(usize, PlaceId)>,
        taken: Vec<usize>,
        returns: Option<CallReturn>,
        on_failure: CleanupId,
    },
}
async fn rvalue(
    frame: &mut Frame,
    id: RvalueId,
    destination: PlaceId,
    site: Span,
    runtime: &crate::aggregate::RuntimeTypes,
    provider: &mut (impl AsyncTirProvider + ?Sized),
    constants: &mut crate::constants::Constants,
    budgets: &mut crate::bounded::Budgets,
) -> Result<(Value, Shadow), EvalError> {
    let code = frame.definition.clone();
    let value = code.bodies[frame.body_id.0 as usize]
        .rvalues
        .get(id.0 as usize)
        .ok_or_else(|| invalid("rvalue out of range"))?;
    let mut shadow = Shadow::default();
    let value = match *value {
        Rvalue::Constant(ref c) => {
            let ty = frame.place(destination)?.ty;
            let ty = &runtime
                .types
                .get(ty)
                .ok_or_else(|| invalid("type out of range"))?
                .kind;
            // A receiving lifetime is not an allocation destination. Literal
            // backing stays heap/static; only an explicit construction/copy
            // destination enrolls a semantic arena allocation.
            let value = scalar::constant(c, ty)?;
            if frame.check.is_some() {
                shadow = shadow.ownership(&value, true);
            }
            value
        }
        // CT-3: a shape is a payload-free token; `type_shape.info` reads its type actual.
        Rvalue::Shape(_) => Value::Unit,
        Rvalue::Size(parameter) => {
            let ty = frame.place(destination)?.ty;
            if !matches!(
                runtime.types.get(ty).map(|t| &t.kind),
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
            ref definition,
            ref arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|argument| match *argument {
                    hirsel_types::GenericArgument::Size(hirsel_types::Size::Parameter(p)) => {
                        frame.size(p).map(|n| {
                            hirsel_types::GenericArgument::Size(hirsel_types::Size::Constant(n))
                        })
                    }
                    ref other => Ok(other.clone()),
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let concrete =
                crate::type_arguments::concrete(runtime, &arguments, &frame.type_arguments)?;
            Value::FunctionReference(Box::new(crate::value::NamedFunction {
                definition: definition.clone(),
                arguments: concrete.arguments,
                types: runtime.types.clone(),
                ty: frame.place(destination)?.ty,
            }))
        }
        Rvalue::ReadConstant {
            ref definition,
            ref arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|size| match size {
                    hirsel_types::Size::Constant(n) => Ok(*n),
                    hirsel_types::Size::Parameter(parameter) => frame.size(*parameter),
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let value = constants
                .read_instance(provider, definition, &arguments, runtime)
                .await?;
            if frame.check.is_some() {
                shadow = shadow.ownership(&value, false);
            }
            value
        }
        Rvalue::Use(ref operand) => {
            let (value, bound) = frame.operand_shadow(operand)?;
            shadow = bound;
            value
        }
        Rvalue::Unary {
            op: hirsel_syntax::UnaryOp::Unique,
            ref value,
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
            let (mut value, bound) = frame.operand_shadow(value)?;
            if let Some(check) = &frame.check {
                shadow = bound.fresh_copy(&check.state.borrow());
            }
            value.acquire_arena_roots();
            value.fresh_arena_storage();
            value
        }
        Rvalue::Unary { op, ref value } => scalar::unary(op, frame.operand(value)?)?,
        Rvalue::Binary {
            op,
            ref left,
            ref right,
        } => {
            let left = frame.operand(left)?;
            let right = frame.operand(right)?;
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
            ref constructor,
            ref fields,
            destination: ref placement,
        } => {
            // MEM-6: charge a bounded arena before any operand is consumed.
            if let Some(arena) = bounded_destination(frame, placement, budgets) {
                let ty = frame.place(destination)?.ty;
                let bytes = crate::bounded::construction(runtime, ty, constructor, fields.len())?;
                budgets
                    .charge(arena, bytes)
                    .map_err(EvalError::OutOfSpace)?;
            }
            let (mut values, mut parts) = if let Constructor::SortedMap { by } = constructor {
                let (value, shadow) = frame.operand_shadow(by)?;
                (vec![Some(value)], vec![shadow])
            } else {
                (
                    vec![None; fields.len()],
                    vec![Shadow::default(); fields.len()],
                )
            };
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
                    runtime
                        .types
                        .get(frame.place(destination)?.ty)
                        .map(|t| &t.kind),
                    Some(hirsel_types::TypeKind::Builtin {
                        kind: hirsel_types::BuiltinType::Inline | hirsel_types::BuiltinType::Simd,
                        ..
                    })
                );
                let block = match constructor {
                    Constructor::Map
                    | Constructor::Set
                    | Constructor::SortedMap { .. }
                    | Constructor::Box => true,
                    Constructor::Sequence => !inline_sequence,
                    _ => false,
                };
                let region = if block {
                    checked_allocation(frame, placement)?
                } else {
                    None
                };
                shadow = Shadow::Parts(parts);
                if let Destination::Arena(origin) = placement {
                    if let Some(arena) = frame.arena_of(origin) {
                        shadow.add_all(arena);
                    }
                }
                if let Some(region) = region {
                    shadow.add_all(region);
                }
            }
            let kind = &runtime
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
            // MEM-6: a List, Array, Map or Set built in an arena is unshared
            // storage there.
            if let Destination::Arena(origin) = placement
                && let Some(arena) = frame.arena_of(origin)
            {
                let kind = match &value {
                    Value::Sequence { kind, .. } => Some(*kind),
                    Value::Map { .. } => Some(hirsel_types::BuiltinType::Map),
                    Value::Set { .. } => Some(hirsel_types::BuiltinType::Set),
                    Value::Record { definition, .. } if definition.is_core("SortedMap") => {
                        Some(hirsel_types::BuiltinType::SortedMap)
                    }
                    _ => None,
                };
                if kind.is_none() && value.is_block() {
                    value.retag_own_block(Some(crate::value::ArenaStorage::fresh(arena.into(), 0)));
                }
                if let Some(kind) = kind {
                    let arena = u64::from(arena);
                    let storage = crate::bounded::storage(
                        runtime,
                        frame.place(destination)?.ty,
                        kind,
                        arena,
                        budgets.bounded(arena),
                    )?;
                    if let Some(tag) = value.arena_storage_mut() {
                        *tag = storage;
                    }
                }
            }
            if frame.check.is_some() {
                shadow = shadow.ownership(&value, true);
            }
            if budgets.run.metered() {
                budgets.run.charge(value.shallow_bytes());
            }
            value
        }
        Rvalue::Closure {
            context: _,
            body,
            ref captures,
            destination: ref placement,
        } => {
            if !captures.is_empty()
                && let Some(arena) = bounded_destination(frame, placement, budgets)
            {
                let types = captures
                    .iter()
                    .map(|c| frame.place(c.place).map(|p| p.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let bytes = crate::bounded::environment(runtime, &types)?;
                budgets
                    .charge(arena, bytes)
                    .map_err(EvalError::OutOfSpace)?;
            }
            let capture_arguments = captures
                .iter()
                .map(|capture| {
                    frame
                        .place(capture.place)
                        .map(|place| hirsel_types::GenericArgument::Type(place.ty))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let capture_types = crate::type_arguments::concrete(
                runtime,
                &capture_arguments,
                &frame.type_arguments,
            )?
            .arguments
            .into_iter()
            .map(|argument| match argument {
                hirsel_types::GenericArgument::Type(ty) => Ok(ty),
                _ => Err(invalid("closed capture type")),
            })
            .collect::<Result<Vec<_>, _>>()?;
            let storage = match placement {
                Destination::Arena(origin) => frame
                    .arena_of(origin)
                    .map(|arena| crate::value::ArenaStorage::fresh(arena.into(), 0)),
                _ => None,
            };
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
                checked_allocation(frame, placement)?
            } else {
                None
            };
            let mut parts = vec![];
            let mut values = vec![];
            for operand in captures {
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
                invoked: runtime
                    .types
                    .get(frame.place(destination)?.ty)
                    .is_some_and(|t| t.modes.multiplicity == hirsel_types::Multiplicity::Once)
                    .then(|| std::sync::Arc::new(std::sync::OnceLock::new())),
                definition: frame.definition.path,
                body,
                captures,
                environment: Box::new(crate::value::Environment {
                    storage,
                    capture_types,
                    owned_captures,
                    sizes: frame.sizes.clone(),
                    type_arguments: frame.type_arguments.clone(),
                }),
            }
        }
    };
    // Bounds exist only under checked execution.
    let shadow = if frame.check.is_some() {
        shadow.ownership(&value, true).for_value(&value)
    } else {
        Shadow::default()
    };
    let mut value = value;
    let ty = closed_storage_type(frame, runtime, frame.place(destination)?.ty)?;
    budgets.storage.record(
        runtime,
        ty,
        &mut value,
        &allocation_site(
            frame,
            anchor(frame, site),
            frame.instruction.saturating_sub(1),
        )?,
        false,
    )?;
    Ok((value, shadow))
}

fn allocation_site(
    frame: &Frame,
    anchor: crate::observation::Anchor,
    statement: usize,
) -> Result<crate::sealed_storage::AllocationSite, Error> {
    Ok(crate::sealed_storage::AllocationSite {
        anchor,
        block: frame.block,
        statement: u32::try_from(statement)
            .map_err(|_| invalid("allocation statement ordinal overflow"))?,
        path: vec![],
    })
}
fn operation_allocation_site(
    frame: &Frame,
    anchor: &crate::observation::Anchor,
) -> Result<crate::sealed_storage::AllocationSite, Error> {
    allocation_site(
        frame,
        anchor.clone(),
        frame.body().blocks[frame.block.0 as usize]
            .instructions
            .len(),
    )
}
fn closed_storage_type(
    frame: &Frame,
    runtime: &crate::aggregate::RuntimeTypes,
    ty: hirsel_types::TypeId,
) -> Result<hirsel_types::TypeId, Error> {
    let actuals = crate::type_arguments::concrete(
        runtime,
        &[hirsel_types::GenericArgument::Type(ty)],
        &frame.type_arguments,
    )?;
    match actuals.arguments.as_slice() {
        [hirsel_types::GenericArgument::Type(ty)] => Ok(*ty),
        _ => Err(invalid("closed storage type")),
    }
}

fn record_operation_storage(
    frame: &Frame,
    state: &mut crate::resources::OperationState,
    runtime: &crate::aggregate::RuntimeTypes,
    budgets: &mut crate::bounded::Budgets,
) -> Result<(), Error> {
    for (index, place) in &state.mut_places {
        let ty = closed_storage_type(frame, runtime, frame.place(*place)?.ty)?;
        budgets.storage.record(
            runtime,
            ty,
            &mut state.arguments[*index],
            &operation_allocation_site(frame, &state.anchor)?,
            true,
        )?;
    }
    Ok(())
}
/// MEM-6: `copy_to(a, x)` charges a bounded arena `x`'s whole tree, then retags the
/// copied value's Lists and Arrays as fresh storage of `a` (of the heap for
/// `heap`). Returns the arena whose budget the copy exceeds.
fn copy_to_arena(
    runtime: &crate::aggregate::RuntimeTypes,
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
        let bytes = crate::bounded::copy(runtime, ty, value)?;
        if let Err(arena) = budgets.charge(arena, bytes) {
            return Ok(Some(arena));
        }
    }
    let arena = arena.map(|arena| (arena, budgets.bounded(arena)));
    crate::bounded::retag(runtime, ty, value, arena)?;
    Ok(None)
}

/// MEM-6: a push to an arena List charges its element before it consumes its
/// argument; a write to a shared arena List (`push`, `pop`, `remove`, `swap`)
/// first copies it within its arena, charged its length, and the operation
/// writes the unshared copy. A call that fails its bounds check or moves no
/// element (`pop` of an empty List, `swap(xs, i, i)`) writes nothing, and
/// `reserve` is only a hint there. Returns the arena whose budget is exceeded.
fn list_write(
    name: &str,
    arguments: &mut [Value],
    budgets: &mut crate::bounded::Budgets,
) -> Option<u64> {
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
        *storage = storage.detached();
        // The original and the copy both hold every element.
        elements
            .iter()
            .flatten()
            .for_each(crate::value::Value::share_arena_storage);
    }
    None
}

/// MEM-6: a bulk byte append or an Io read into an arena List (`stdin.read`,
/// `file.read`, `net.read`) charges its input or maximum bytes before its effect, whatever it then
/// reads, and first copies a shared List within its arena, charged its length,
/// so an `OutOfSpace` reads nothing. Returns the arena whose budget is
/// exceeded.
fn read_write(
    name: &str,
    arguments: &mut [Value],
    budgets: &mut crate::bounded::Budgets,
) -> Option<u64> {
    let (buffer, max) = if name == "bytes.replace_range" {
        let Some(Value::Sequence { elements, .. }) = arguments.first() else {
            return None;
        };
        let Some(Value::Integer(start)) = arguments.get(1) else {
            return None;
        };
        let Some(Value::Bytes(data) | Value::ArenaBytes { bytes: data, .. }) = arguments.get(2)
        else {
            return None;
        };
        let start = u64::try_from(start.bits()).ok()?;
        let len = elements.len() as u64;
        // Invalid and empty calls never preflight a destination copy.
        if start > len || data.len() as u64 > len - start || data.is_empty() {
            return None;
        }
        (0, 0)
    } else if name == "bytes.append_from_history" {
        let Some(Value::Sequence { elements, .. }) = arguments.first() else {
            return None;
        };
        let Some(Value::Integer(distance)) = arguments.get(1) else {
            return None;
        };
        let Some(Value::Integer(count)) = arguments.get(2) else {
            return None;
        };
        let (distance, count) = (
            u64::try_from(distance.bits()).ok()?,
            u64::try_from(count.bits()).ok()?,
        );
        // Bad arguments fail before arena charging; empty valid appends do no work.
        if distance == 0
            || distance > elements.len() as u64
            || count == 0
            || count > ((1u64 << 62) - 17).saturating_sub(elements.len() as u64)
        {
            return None;
        }
        (0, count)
    } else if name == "bytes.append" {
        let Some(Value::Bytes(data) | Value::ArenaBytes { bytes: data, .. }) = arguments.get(1)
        else {
            return None;
        };
        if data.is_empty() {
            return None;
        }
        (0, data.len() as u64)
    } else {
        let (buffer, max) = if name == "stdin.read" {
            (1, 2)
        } else if name == "file.read"
            || name == "net.read"
            || name == "read_half.read"
            || name == "udp.recv_from"
        {
            (2, 3)
        } else {
            return None;
        };
        let max = match arguments.get(max) {
            Some(Value::Integer(i)) => u64::try_from(i.bits()).ok()?,
            _ => return None,
        };
        (buffer, max)
    };
    let Some(Value::Sequence {
        kind: hirsel_types::BuiltinType::List,
        elements,
        arena: Some(storage),
    }) = arguments.get_mut(buffer)
    else {
        return None;
    };
    let shared = if storage.shared() {
        elements.len() as u64
    } else {
        0
    };
    let bytes = storage
        .element
        .saturating_mul(max)
        .saturating_add(storage.element.saturating_mul(shared));
    if let Err(arena) = budgets.charge(storage.arena, bytes) {
        return Some(arena);
    }
    if storage.shared() {
        *storage = storage.detached();
    }
    None
}

/// MEM-6: an insert of a new key into an arena Map or Set charges its entry
/// before it consumes its arguments; a write to a shared one (an insert of a
/// new key, a Map insert that replaces a value, a remove of a key it holds)
/// first copies it within its arena, charged its entry count, and the
/// operation writes the unshared copy. A Set insert of a member it holds and a
/// remove of an absent key write nothing, and a key that cannot be hashed
/// (`Depth`) charges nothing: the operation fails. Returns the arena whose
/// budget is exceeded.
fn keyed_write(
    name: &str,
    arguments: &mut [Value],
    budgets: &mut crate::bounded::Budgets,
) -> Option<u64> {
    let (container, rest) = arguments.split_first_mut()?;
    if container.arena_storage().is_none() {
        return None;
    }
    let hit = crate::library::collection::contains(container, rest.first()?).ok()?;
    // Whether the operation writes, and whether it adds an entry.
    let (writes, adds) = match name {
        "map.insert" => (true, !hit),
        "set.insert" => (!hit, !hit),
        _ => (hit, false),
    };
    let (Value::Map {
        entries,
        arena: Some(storage),
    }
    | Value::Set {
        entries,
        arena: Some(storage),
    }) = container
    else {
        return None;
    };
    if !writes {
        return None;
    }
    if adds && let Err(arena) = budgets.charge(storage.arena, storage.element) {
        return Some(arena);
    }
    if storage.shared() {
        let bytes = storage.element.saturating_mul(entries.live_len() as u64);
        if let Err(arena) = budgets.charge(storage.arena, bytes) {
            return Some(arena);
        }
        *storage = storage.detached();
        // The original and the copy both hold every entry.
        entries
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
pub(crate) fn enter_checked(
    state: &crate::checked::Shared,
    frame: &Frame,
    mut arguments: Vec<crate::checked::Argument>,
    callee: Option<crate::checked::Callee>,
) -> Result<crate::checked::FrameCheck, Error> {
    for (argument, parameter) in arguments.iter_mut().zip(&frame.body().parameters) {
        argument.mutable = frame.mut_parameters.contains(parameter);
    }
    let noalloc = frame.body_id == frame.definition.entry
        && state
            .borrow()
            .config
            .noalloc
            .contains(frame.definition.path.path());
    let entered = crate::checked::FrameCheck::enter(
        state.clone(),
        frame.body(),
        &frame.types,
        &frame.slots,
        arguments,
        callee,
        noalloc,
    );
    entered.unique_parameters(frame.body(), &frame.types)?;
    Ok(entered)
}
/// The home instance of a place's root slot, under checked execution.
/// MODE-1: relations compose. A borrowed local that is not a semantic copy and
/// whose content lives only in instances this frame does not own (its
/// caller's, mapped through parameters and captures) is a view of that
/// content: a result related to it reaches the content, not the local's own
/// storage, so it lends no home. A copy is owned and lends only itself.
fn composed_view(
    frame: &mut Frame,
    operand: &Operand,
    lifetime: &crate::checked::Bound,
) -> Result<bool, Error> {
    let root = frame.place(operand.place)?.root;
    let body = &frame.definition.bodies[frame.body_id.0 as usize];
    if operand.usage != hirsel_types::UseKind::Borrow {
        return Ok(false);
    }
    let storage = body.locals[root.0 as usize].storage;
    let borrowed_carrier = storage == Storage::Temporary
        && frame
            .check
            .as_ref()
            .is_some_and(|check| check.borrowed.contains(&root.0));
    if storage != Storage::Local && !borrowed_carrier {
        return Ok(false);
    }
    Ok(frame
        .check
        .as_ref()
        .is_some_and(|check| !check.copies.contains(&root.0) && check.caller_owned(lifetime)))
}
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
async fn execute_instruction(
    frame: &mut Frame,
    kind: &InstructionKind,
    runtime: &crate::aggregate::RuntimeTypes,
    site: Span,
    trace: &mut Vec<TraceEvent>,
    provider: &mut (impl AsyncTirProvider + ?Sized),
    constants: &mut crate::constants::Constants,
    budgets: &mut crate::bounded::Budgets,
) -> Result<(), EvalError> {
    match *kind {
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
                        // Drop in place: moving the dead slot out copies it.
                        frame.slots[index] = None;
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
            ref value,
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
            let (value, shadow) = frame.operand_shadow(value)?;
            arena_write(frame, destination, false, budgets)?;
            frame.write_access(destination)?;
            let old_shadow = frame.displaced_shadow(destination)?;
            let old = frame.slot(destination)?.replace(value);
            frame.set_shadow(destination, shadow)?;
            if let Some(old) = old {
                frame.assign_shadow(displaced, old, old_shadow)?;
            }
            frame.record_storage(
                destination,
                runtime,
                &mut budgets.storage,
                &allocation_site(
                    frame,
                    anchor(frame, site),
                    frame.instruction.saturating_sub(1),
                )?,
            )?;
        }
        InstructionKind::Assign { destination, value } => {
            let created = matches!(
                frame.body().rvalues.get(value.0 as usize),
                Some(Rvalue::Construct { .. })
            );
            let borrowed = frame.check.as_ref().is_some_and(|check| {
                let body = frame.body();
                match body.rvalues.get(value.0 as usize) {
                    Some(Rvalue::Use(operand)) => match operand.usage {
                        hirsel_types::UseKind::Borrow | hirsel_types::UseKind::Mut => true,
                        hirsel_types::UseKind::Take => check
                            .borrowed
                            .contains(&body.places[operand.place.0 as usize].root.0),
                        hirsel_types::UseKind::Copy => false,
                    },
                    _ => false,
                }
            });
            let copied = frame.check.as_ref().is_some_and(|check| {
                let body = frame.body();
                let from = |operand: &Operand| {
                    operand.usage == hirsel_types::UseKind::Copy
                        || check
                            .copies
                            .contains(&body.places[operand.place.0 as usize].root.0)
                };
                match body.rvalues.get(value.0 as usize) {
                    Some(Rvalue::Use(operand)) => from(operand),
                    Some(Rvalue::Construct { fields, .. }) => {
                        fields.iter().any(|field| from(&field.value))
                    }
                    _ => false,
                }
            });
            let (value, shadow) = rvalue(
                frame,
                value,
                destination,
                site,
                runtime,
                provider,
                constants,
                budgets,
            )
            .await?;
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
                            definition: definition.path().clone(),
                        },
                    );
                }
            }
            arena_write(frame, destination, false, budgets)?;
            frame.write_access(destination)?;
            frame.assign_shadow(destination, value, shadow)?;
            if !frame.place(destination)?.projection.is_empty() {
                frame.record_storage(
                    destination,
                    runtime,
                    &mut budgets.storage,
                    &allocation_site(
                        frame,
                        anchor(frame, site),
                        frame.instruction.saturating_sub(1),
                    )?,
                )?;
            }
            if borrowed {
                let place = frame.place(destination)?;
                if place.projection.is_empty() {
                    let root = place.root.0;
                    frame.check.as_mut().unwrap().borrowed.insert(root);
                }
            }
            if copied {
                let place = frame.place(destination)?;
                if place.projection.is_empty() {
                    let root = place.root.0;
                    frame.check.as_mut().unwrap().copies.insert(root);
                }
            }
        }
        InstructionKind::Snapshot {
            destination,
            ref value,
        } => {
            let (value, shadow) = frame.operand_shadow(value)?;
            frame.assign_shadow(destination, value, shadow)?;
        }
        // The scheduler charges before it steps an instruction.
        InstructionKind::Drop { .. }
        | InstructionKind::RunCleanup(_)
        | InstructionKind::Charge(_) => unreachable!(),
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
    terminator: &Terminator,
    call: Option<&std::rc::Rc<crate::code::CallSite>>,
    runtime: &crate::aggregate::RuntimeTypes,
    budgets: &mut crate::bounded::Budgets,
) -> Result<Action, EvalError> {
    Ok(match *terminator {
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
        Terminator::Branch {
            ref condition,
            yes,
            no,
        } => {
            let Value::Bool(condition) = frame.operand(condition)? else {
                return Err(invalid("branch needs bool").into());
            };
            frame.goto(if condition { yes } else { no });
            Action::Continue
        }
        Terminator::Switch {
            ref value,
            ref arms,
            otherwise,
        } => {
            let value = frame.operand(value)?;
            let tag = match &value {
                Value::Variant { variant, .. } | Value::ResourceVariant { variant, .. } => {
                    Some(*variant)
                }
                Value::Integer(i) => u32::try_from(i.bits()).ok(),
                Value::Char(c) => Some(u32::from(*c)),
                Value::Bool(b) => Some(u32::from(*b)),
                _ => {
                    value.discard();
                    return Err(
                        invalid("Switch requires an integer, char, bool or variant tag").into(),
                    );
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
        Terminator::Return { ref value, .. } => {
            if frame.check.is_none() {
                return Ok(Action::Return(frame.operand(value)?));
            }
            let (value, shadow) = frame.operand_shadow(value)?;
            if let Some(check) = &mut frame.check {
                check.returned = Some(shadow);
            }
            Action::Return(value)
        }
        Terminator::Fail { ref rule, .. } => Action::Fail(rule.clone()),
        Terminator::Call {
            ref callee,
            ref arguments,
            returns,
            on_failure,
        } => {
            let self_tail = matches!(callee, Callee::Definition { path, .. }
                if path == frame.definition.path.path())
                && frame.body().tail_sites.iter().any(|site| {
                    site.owner_body == frame.body_id
                        && site.call_block == frame.block
                        && site.kind == TailKind::SelfTail
                });
            let adapter = matches!(
                callee,
                Callee::Indirect {
                    forward_caller: true,
                    ..
                }
            );
            let leaf = matches!(callee, Callee::Definition { path, .. }
                if !hirsel_core_contract::depth::adds_activation(path));
            let restart = matches!(callee, Callee::Definition { path, .. }
                if path == frame.definition.path.path())
                && frame.body().tail_sites.iter().any(|site| {
                    site.owner_body == frame.body_id
                        && site.call_block == frame.block
                        && matches!(site.kind, TailKind::ConstructorHole(_))
                });
            let activation = if self_tail || restart || adapter || leaf {
                None
            } else {
                Some(frame.depth.admit().ok_or(EvalError::InvocationDepth)?)
            };
            let primitive = match callee {
                Callee::Definition { path, .. } => Some(path),
                Callee::Indirect { value, .. } => match frame.observe(value.place)? {
                    Value::FunctionReference(reference) => Some(&reference.definition),
                    _ => None,
                },
            };
            if primitive.is_some_and(|path| {
                ["map.insert", "set.insert", "map.get", "set.contains"]
                    .iter()
                    .any(|name| path.is_core(name))
            }) {
                let key = arguments
                    .iter()
                    .find(|a| a.parameter.0 == 1)
                    .ok_or_else(|| invalid("keyed primitive has no key argument"))?;
                let key = frame
                    .slot_ref(key.value.place)?
                    .as_ref()
                    .ok_or_else(|| invalid("keyed primitive key is dead"))?;
                crate::structural::preflight(key).map_err(|error| match error {
                    crate::structural::StructuralError::Depth => EvalError::KeyDepth,
                    _ => invalid("invalid structural key").into(),
                })?;
            }
            if let Callee::Definition { path, .. } = callee {
                if runtime.forget.as_ref() == Some(path)
                    && (arguments.len() != 1
                        || arguments[0].value.usage != hirsel_types::UseKind::Take)
                {
                    return Err(invalid("forget requires one take argument").into());
                }
            }
            let forward_caller = match callee {
                Callee::Definition { .. } => true,
                Callee::Indirect { forward_caller, .. } => *forward_caller,
            };
            let callee = match callee {
                Callee::Definition { path, arguments } => {
                    let generics = arguments
                        .iter()
                        .map(|argument| match *argument {
                            hirsel_types::GenericArgument::Size(hirsel_types::Size::Parameter(
                                p,
                            )) => frame.size(p).map(|n| {
                                hirsel_types::GenericArgument::Size(hirsel_types::Size::Constant(n))
                            }),
                            ref argument => Ok(argument.clone()),
                        })
                        .collect::<Result<Vec<_>, Error>>()?;
                    RuntimeCallee::Definition {
                        path: match call {
                            Some(call) => CalleePath::Site(call.clone()),
                            None => CalleePath::Value(path.clone()),
                        },
                        generics,
                    }
                }
                Callee::Indirect {
                    forward_caller: _,
                    value: operand,
                    arguments,
                } => {
                    let (value, shadow) = frame.operand_shadow(operand)?;
                    let check = callee_lifetime(frame, operand.place, &value, shadow)?;
                    budgets.storage.environment_entry(&value)?;
                    value.begin_call();
                    match value {
                        Value::FunctionReference(reference) => RuntimeCallee::Definition {
                            path: CalleePath::Value(reference.definition),
                            generics: reference.arguments,
                        },
                        Value::Closure {
                            definition,
                            body,
                            captures,
                            environment,
                            ..
                        } => {
                            if captures.len() != environment.owned_captures.len() {
                                return Err(invalid("capture ownership count").into());
                            }
                            let crate::value::Environment {
                                sizes,
                                type_arguments,
                                ..
                            } = *environment;
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
                                    runtime,
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
                    &runtime.types,
                    &frame.definition,
                    frame.body_id,
                    frame.block,
                    checked,
                )
            });
            let mut values = vec![None; arguments.len()];
            let mut checks = if frame.check.is_some() {
                vec![crate::checked::Argument::default(); arguments.len()]
            } else {
                vec![]
            };
            // The argument places already contain caller temporaries, in explicit source order.
            for argument in arguments {
                let mut operand = argument.value.clone();
                if operand.usage == hirsel_types::UseKind::Mut {
                    arena_write(frame, operand.place, true, budgets)?;
                    operand.usage = hirsel_types::UseKind::Take;
                }
                let (value, shadow) = if frame.check.is_some() {
                    frame.operand_shadow(&operand)?
                } else {
                    (frame.operand(&operand)?, Shadow::default())
                };
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
                        && !composed_view(frame, &operand, &lifetime)?
                    {
                        lifetime.extend(home(frame, operand.place)?.unwrap_or_default());
                        lifetime.sort_unstable();
                        lifetime.dedup();
                    }
                    checks[argument.parameter.0 as usize] = crate::checked::Argument {
                        shadow,
                        lifetime,
                        mutable: false,
                    };
                }
            }
            Action::Call {
                activation,
                restart,
                forward_caller,
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

/// A named callee: a direct call's site, or a function reference's definition.
enum CalleePath {
    Site(std::rc::Rc<crate::code::CallSite>),
    Value(hirsel_syntax::DefPath),
}
impl CalleePath {
    fn path(&self) -> &hirsel_syntax::DefPath {
        match self {
            Self::Site(call) => &call.callee,
            Self::Value(path) => path,
        }
    }
    fn into_path(self) -> hirsel_syntax::DefPath {
        match self {
            Self::Site(call) => call.callee.clone(),
            Self::Value(path) => path,
        }
    }
}
/// Resolve a direct callee for a call from `frame`: close its generic arguments,
/// select its CT-5 instance and classify the call.
async fn target(
    frame: &Frame,
    provider: &mut (impl AsyncTirProvider + ?Sized),
    runtime: &crate::aggregate::RuntimeTypes,
    mut path: hirsel_syntax::DefPath,
    generics: Vec<hirsel_types::GenericArgument>,
) -> Result<crate::code::Target, Error> {
    let mut actuals = if generics.is_empty() || frame.type_arguments.is_empty() {
        hirsel_types::Actuals {
            arguments: generics,
        }
    } else {
        crate::type_arguments::concrete(runtime, &generics, &frame.type_arguments)?
    };
    // CT-5: a generic generated declaration runs the monomorphic body of its
    // selected instance.
    if let Some(instance) = provider
        .instance_body(&path, &actuals.arguments)
        .await
        .map_err(Error::Query)?
    {
        path = instance;
        actuals.arguments.clear();
    }
    let tail = (path == *frame.definition.path.path())
        .then(|| {
            frame
                .body()
                .tail_sites
                .iter()
                .find(|tail| {
                    tail.call_block == frame.block
                        && tail.owner_body == frame.body_id
                        && tail.kind == TailKind::SelfTail
                })
                .cloned()
        })
        .flatten();
    Ok(crate::code::Target {
        forget: runtime.forget.as_ref() == Some(&path),
        arena: crate::code::ArenaCall::resolve(&path),
        path,
        actuals,
        tail,
        entry: Default::default(),
        operation: Default::default(),
    })
}
enum RuntimeCallee {
    Definition {
        path: CalleePath,
        generics: Vec<hirsel_types::GenericArgument>,
    },
    Closure {
        definition: hirsel_syntax::DefId,
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
    let path = path.path();
    // The common ordinary call needs no arena dispatch or path comparison.
    if frame.check.is_none() && !matches!(path.0.name.as_str(), "with" | "bounded") {
        return Ok((callee, None, None));
    }
    let bounded = path.is_core(crate::library::ARENA_BOUNDED_OPERATION);
    let Some(check) = &frame.check else {
        if !bounded && !path.is_core(crate::library::ARENA_WITH_OPERATION) {
            return Ok((callee, None, None));
        }
        let cap = bounded.then(|| bounded_capacity(arguments)).transpose()?;
        let (callee, _) = arena_callback(arguments, &mut budgets.storage)?;
        let id = budgets.issue();
        budgets.storage.open(id)?;
        *arguments = vec![Value::Handle {
            kind: crate::checked::ARENA_HANDLE.into(),
            id,
        }];
        return Ok((
            callee,
            Some(u32::try_from(id).map_err(|_| invalid("arena identity exhausted"))?),
            cap.map(|cap| (id, cap)),
        ));
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
    let (mut callee, owned_captures) = arena_callback(arguments, &mut budgets.storage)?;
    let arena = state.arena();
    budgets.storage.open(arena.into())?;
    let shadow = checks.pop().unwrap_or_default().shadow;
    if let RuntimeCallee::Closure {
        captures, check, ..
    } = &mut callee
    {
        *check = Some(crate::checked::Callee {
            captures: (0..captures.len()).map(|i| shadow.capture(i)).collect(),
            owned: owned_captures,
            lifetime: shadow.total(),
        });
    }
    *arguments = vec![Value::Handle {
        kind: crate::checked::ARENA_HANDLE.into(),
        id: arena.into(),
    }];
    *checks = vec![crate::checked::Argument {
        shadow: Shadow::of(vec![arena]),
        lifetime: vec![arena],
        mutable: false,
    }];
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
/// The one callable argument of an arena scope, using the same named and
/// environment-carrying invocation paths as an ordinary indirect call.
fn arena_callback(
    arguments: &mut Vec<Value>,
    storage: &mut crate::sealed_storage::Storage,
) -> Result<(RuntimeCallee, Vec<bool>), Error> {
    let [callback] = <[Value; 1]>::try_from(std::mem::take(arguments))
        .map_err(|_| invalid("arena.with takes one callback"))?;
    // Callback admission has succeeded before this adapter runs. Transfer a
    // Once environment's owned captures just as an ordinary indirect call
    // does, so its caller's snapshot no longer releases them after the body.
    storage.environment_entry(&callback)?;
    callback.begin_call();
    match callback {
        Value::FunctionReference(reference) => Ok((
            RuntimeCallee::Definition {
                path: CalleePath::Value(reference.definition),
                generics: reference.arguments,
            },
            vec![],
        )),
        Value::Closure {
            definition,
            body,
            captures,
            environment,
            ..
        } => {
            let crate::value::Environment {
                owned_captures,
                sizes,
                type_arguments,
                ..
            } = *environment;
            Ok((
                RuntimeCallee::Closure {
                    definition,
                    body,
                    captures,
                    sizes,
                    generic_sizes: vec![],
                    actuals: Default::default(),
                    type_arguments,
                    check: None,
                },
                owned_captures,
            ))
        }
        _ => Err(invalid("arena.with callback must be a function")),
    }
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
        environment,
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
        owned: environment.owned_captures.clone(),
        lifetime,
    }))
}

/// The scheduled Complete path with no user release, callback, checker or
/// memory-meter or retained-arena work. The provider promised there are no
/// task rests to collect.
fn complete_immediate(
    frame: &mut Frame,
    result: &mut OperationResult,
    arguments: &mut Vec<Value>,
    mut_places: &[(usize, PlaceId)],
    taken: &[usize],
    returns: Option<CallReturn>,
    arena_operation: crate::code::ArenaCall,
) -> Result<bool, Error> {
    let OperationResult::Complete {
        value,
        arguments: updates,
        consumed,
        released,
        ..
    } = result
    else {
        return Ok(false);
    };
    // The scheduled completion records all retained mutations and new arena
    // allocations before publishing them. Immediate completion has no such
    // journal step. The preflight-classified collection operations mutate only
    // their direct block; other mutable shapes need a complete storage check.
    let arena_result =
        value.arena_storage().is_some() || !value.inert() && value.contains_arena_storage();
    let arena_mutation = mut_places.iter().any(|(index, _)| {
        let value = &arguments[*index];
        match arena_operation {
            crate::code::ArenaCall::List(_)
            | crate::code::ArenaCall::Keyed(_)
            | crate::code::ArenaCall::Read(_) => value.arena_storage().is_some(),
            _ => {
                value.arena_storage().is_some() || !value.inert() && value.contains_arena_storage()
            }
        }
    });
    if arena_result || arena_mutation {
        return Ok(false);
    }
    if !updates.is_empty()
        || !released.is_empty()
        || taken
            .iter()
            .any(|i| !consumed.contains(i) && !arguments[*i].inert())
    {
        return Ok(false);
    }
    for (ordinal, index) in consumed.iter().enumerate() {
        if consumed[..ordinal].contains(index) || !taken.contains(index) {
            return Err(invalid("operation transferred an argument it does not own"));
        }
    }
    // Only the returned payload moves into the caller. Keep the empty update
    // and release vectors, and the consumed ordinals, in their original result
    // rather than moving and replacing the entire completion.
    let value = std::mem::replace(value, Value::Unit);
    for &(index, place) in mut_places {
        let value = std::mem::replace(&mut arguments[index], Value::Unit);
        frame.put(place, Some(value), Shadow::default())?;
    }
    // Transferred owners are snapshots, and remaining taken inputs are inert.
    // Discarding snapshots never performs a semantic release.
    for value in arguments.drain(..) {
        value.discard();
    }
    let Some(CallReturn { destination, next }) = returns else {
        return Err(invalid("diverging operation returned"));
    };
    frame.assign(destination, value)?;
    frame.goto(next);
    Ok(true)
}

fn finish_handle_drop<O: crate::ownership_trace::Observer>(
    frame: &mut Frame,
    operations: &mut (impl Operations + ?Sized),
    trace_mode: TraceMode,
    execution: &mut Execution,
    sources: &hirsel_span::SourceMap,
    anchor: &crate::observation::Anchor,
    caller: Option<&crate::frame::CallerSite>,
    observer: &mut O,
    task: u64,
) -> Result<(), Error> {
    let values = operations.take_releases();
    observer.handle_released(task, frame, &values, anchor, sources);
    for value in values {
        frame.work.push_front(crate::resources::Work::Release(
            value,
            anchor.clone(),
            caller.cloned(),
        ));
    }
    if let Some((rule, kind)) = operations.take_drop_failure() {
        let mut at = anchor.clone();
        at.site = dropped_binding_site(frame, anchor).unwrap_or(anchor.site);
        record_failure(
            trace_mode,
            execution,
            failure(sources, at.site, &rule, &kind)?,
            at,
            caller,
        );
        frame.ending = Some(crate::resources::Ending::Failed);
    }
    Ok(())
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
/// The first argument whose operation parameter is declared `unique` but
/// whose value shares a block with another live holder.
fn shared_unique_input(
    types: &hirsel_types::TypeInterner,
    config: &crate::checked::CheckedRuntime,
    path: &hirsel_syntax::DefPath,
    checks: &[crate::checked::Argument],
) -> Option<usize> {
    let signature = config.operations.get(path)?;
    let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
        return None;
    };
    function.parameters.iter().enumerate().find_map(|(i, p)| {
        (types.get(p.ty)?.modes.uniqueness == hirsel_types::Uniqueness::Unique
            && !checks.get(i)?.shadow.sole_owner())
        .then_some(i)
    })
}
/// Section 22 declarations select result sources; ownership transport supplies
/// the values retained by native state. Declared local results include the home
/// of a borrowed binding, while transferred cursor items retain value bounds.
fn operation_bounds(
    types: &hirsel_types::TypeInterner,
    state: &crate::checked::State,
    path: &hirsel_syntax::DefPath,
    checks: &[crate::checked::Argument],
    mut_places: &[(usize, PlaceId)],
    taken: &[usize],
) -> crate::checked::OperationCheck {
    let total = |i: usize| checks.get(i).map(|c| c.shadow.total()).unwrap_or_default();
    let signature = state.config.operations.get(path);
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
        // Content the operation moves out of `mut` storage is the caller's, as
        // for Replace: it keeps where it lives, not this frame's access root.
        if let Some(check) = checks.get(*i) {
            let mut content = check.shadow.clone();
            state.caller_content(&mut content);
            retained.extend(content.content_bound());
        }
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
        crate::checked::operation_result(types, function.result, &result_inputs, &retained)
    } else {
        Shadow::of(retained)
    };
    let updates = mut_places
        .iter()
        .map(|(i, _)| {
            let mut shadow = checks.get(*i).map(|c| c.shadow.clone()).unwrap_or_default();
            shadow.add_bound(&moved);
            for taken in &taken {
                if let Some(input) = checks.get(*taken) {
                    shadow.retain(&input.shadow);
                }
            }
            (*i, shadow)
        })
        .collect();
    crate::checked::OperationCheck {
        result,
        updates,
        inputs: checks.to_vec(),
        callbacks: vec![],
        task_arguments: None,
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
        frame.work.push_back(crate::resources::Work::Release(
            value,
            state.anchor.clone(),
            state.caller.clone(),
        ));
    }
}

fn retire_operation(frame: &mut Frame, state: &mut crate::resources::OperationState) {
    state.root_loan.take();
    if let Some(activation) = state.activation.take() {
        frame
            .work
            .push_back(crate::resources::Work::Retire(activation));
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
            frame.work.push_back(crate::resources::Work::Release(
                value,
                anchor.clone(),
                state.caller.clone(),
            ));
            for value in released.into_iter().rev() {
                frame.work.push_back(crate::resources::Work::Release(
                    value,
                    anchor.clone(),
                    state.caller.clone(),
                ));
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
                frame.work.push_back(crate::resources::Work::Release(
                    value,
                    anchor.clone(),
                    state.caller.clone(),
                ));
            }
        }
        OperationResult::ArenaCharge { result, .. }
        | OperationResult::BorrowRegion { result, .. } => crate::resources::discard_result(*result),
        OperationResult::SealArena { .. } => (),
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
        retire_operation(frame, &mut state);
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
        let saved = callback.state.arguments[target.argument].project_mut(&target.fields)?;
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
    for (index, value) in updates {
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
        frame.work.push_back(crate::resources::Work::Release(
            value,
            state.anchor.clone(),
            state.caller.clone(),
        ));
    }
}
