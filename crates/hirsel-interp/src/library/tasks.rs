//! Structured task state. Frames and destruction belong to the evaluator.
use super::dispatch::{arg, variant};
use super::{Library, error, none, ok, some, u64_value};
use crate::observation::{Anchor, FailureRecord};
use crate::{
    BlockedWait, Error, OperationCall, OperationContext, OperationResult, TaskStart, Value, invalid,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Tasks {
    pub current: u64,
    pub yield_now: bool,
    pub shielded: bool,
    spawn_counts: BTreeMap<u64, u32>,
    active_scopes: BTreeMap<u64, Vec<u64>>,
    scope_canceled: BTreeMap<u64, u32>,
    next: u64,
    starts: Vec<TaskStart>,
    tasks: BTreeMap<u64, Child>,
    scopes: BTreeMap<u64, ScopeState>,
    pub pending: BTreeMap<u64, Pending>,
    /// A host adapter observes scope deadlines on its own clock (`take_deadlines`); the
    /// simulation expires them at its virtual readings.
    pub host_deadlines: bool,
    /// Deadline scopes entered since the host adapter last took them: scope, clock, ticks.
    entered: Vec<(u64, Value, u64)>,
    /// Live task sets (CONC-2): their scope and members in add order, `None` once awaited.
    sets: BTreeMap<u64, TaskSet>,
    /// Children completed so far: the next completion's place in completion order.
    completions: u64,
    /// The run's scheduler policy (`inputs.scheduler`) and the root task's priority.
    pub schedule: hirsel_kernels::schedule::Schedule,
    pub root_priority: u64,
    /// A callee run's root scope, cancelled by its caller (RUN-9): the cause.
    root_canceled: Option<u32>,
}
struct TaskSet {
    scope: u64,
    members: Vec<Option<u64>>,
}
/// A scope's deadline (CONC-3): when it is due, whether it fired, and the entry's anchor,
/// where its expiry is recorded.
pub(super) struct ScopeDeadline {
    pub at: u64,
    pub expired: bool,
    anchor: Option<Anchor>,
}
#[derive(Default)]
struct ScopeState {
    /// The task whose body runs in the scope.
    owner: u64,
    deadline: Option<ScopeDeadline>,
    children: Vec<u64>,
    // Completed detached failures still belong to the scope. Keep each child's
    // chain separately so completion order cannot change spawn-order reporting.
    failures: BTreeMap<Vec<u32>, Vec<FailureRecord>>,
}
struct Child {
    scope: u64,
    path: Vec<u32>,
    canceled: Option<u32>,
    finished: bool,
    detached: bool,
    /// Its place in completion order, which `task.await_any` reports first.
    completed: u64,
    result: Option<OperationResult>,
    /// Its scheduling priority under `pct@1`, drawn when it was spawned.
    priority: u64,
}
pub(super) enum Pending {
    Scope {
        scope: u64,
        result: Option<bool>,
    },
    Isolate,
    Join {
        scope: u64,
        result: OperationResult,
        released: Vec<Value>,
    },
    Await(u64),
    AwaitAny(u64),
    /// `task.cancel_all`'s wait for its canceled members, and what their detach released.
    Drain {
        members: Vec<u64>,
        released: Vec<Value>,
    },
}
fn handle(kind: &str, id: u64) -> Value {
    Value::Handle {
        kind: kind.into(),
        id,
    }
}
pub(super) fn id(value: &Value, expected: &str) -> Result<u64, Error> {
    match value {
        Value::Handle { kind, id } if kind == expected => Ok(*id),
        _ => Err(invalid(format!("expected {expected}"))),
    }
}
impl Tasks {
    /// The logical bytes of every finished child's result not yet taken.
    pub(super) fn retained_bytes(&self) -> u64 {
        self.tasks
            .values()
            .filter_map(|child| child.result.as_ref())
            .map(crate::resources::result_bytes)
            .fold(0, u64::saturating_add)
    }

    /// The priority of live task `id` (the root is 0).
    pub fn priority(&mut self, id: u64) -> Option<&mut u64> {
        if id == 0 {
            return Some(&mut self.root_priority);
        }
        self.tasks.get_mut(&id).map(|child| &mut child.priority)
    }
    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
    pub fn take_starts(&mut self) -> Vec<TaskStart> {
        std::mem::take(&mut self.starts)
    }
    pub fn canceled(&self) -> Option<u32> {
        if self.shielded {
            return None;
        }
        self.canceled_task(self.current)
    }
    /// Whether `task` is cancelled: itself, a scope it runs, or, for the root task, the
    /// run's root scope.
    pub fn canceled_task(&self, task: u64) -> Option<u32> {
        self.tasks
            .get(&task)
            .and_then(|t| t.canceled)
            .or_else(|| {
                self.active_scopes
                    .get(&task)?
                    .iter()
                    .rev()
                    .find_map(|id| self.scope_canceled.get(id).copied())
            })
            .or(if task == 0 { self.root_canceled } else { None })
    }
    /// Cancel the run's root scope with `cause`: its root task and every task under it.
    pub fn cancel_root(&mut self, cause: u32) {
        self.root_canceled.get_or_insert(cause);
        for child in self.tasks.values_mut() {
            child.canceled.get_or_insert(1);
        }
    }
    /// A child of `scope` that runs no frames: the library finishes it (`finished`), as
    /// `runner.spawn`'s call does.
    pub(super) fn native_child(&mut self, scope: u64) -> Result<u64, Error> {
        let task = self.fresh();
        let mut path = self.path();
        let children = self
            .scopes
            .get_mut(&scope)
            .ok_or_else(|| invalid("expired task scope"))?;
        let ordinal = self.spawn_counts.entry(self.current).or_default();
        path.push(*ordinal);
        *ordinal += 1;
        children.children.push(task);
        let canceled = self.canceled().map(|_| 1);
        self.tasks.insert(
            task,
            Child {
                scope,
                path,
                canceled,
                finished: false,
                detached: false,
                completed: 0,
                result: None,
                // It runs no frames, so the scheduler never picks it.
                priority: 0,
            },
        );
        Ok(task)
    }
    pub fn path(&self) -> Vec<u32> {
        self.tasks
            .get(&self.current)
            .map(|t| t.path.clone())
            .unwrap_or_default()
    }
    fn cancel(&mut self, task: u64, cause: u32) {
        let path = self.tasks[&task].path.clone();
        for child in self.tasks.values_mut() {
            if child.path.starts_with(&path) {
                child
                    .canceled
                    .get_or_insert(if child.path == path { cause } else { 1 });
            }
        }
    }
    pub fn detached(&self, task: u64) -> bool {
        self.tasks.get(&task).is_some_and(|child| child.detached)
    }
    fn detach(&mut self, task: u64) -> Result<Vec<Value>, Error> {
        let child = self
            .tasks
            .get_mut(&task)
            .ok_or_else(|| invalid("unknown detached task"))?;
        child.detached = true;
        if child.finished {
            self.retire(task)
        } else {
            Ok(vec![])
        }
    }
    fn retire(&mut self, task: u64) -> Result<Vec<Value>, Error> {
        let child = self
            .tasks
            .remove(&task)
            .ok_or_else(|| invalid("unknown retired task"))?;
        let scope = self
            .scopes
            .get_mut(&child.scope)
            .ok_or_else(|| invalid("expired child scope"))?;
        scope.children.retain(|id| *id != task);
        match child.result {
            Some(OperationResult::Complete {
                value,
                mut released,
                ..
            }) => {
                if value != Value::Unit {
                    released.push(value);
                }
                Ok(released)
            }
            Some(OperationResult::FailureChain { failures, released }) => {
                scope.failures.insert(child.path, failures);
                Ok(released)
            }
            _ => Err(invalid("retired task has no completed result")),
        }
    }
    /// The task set `set` of `scope`.
    fn set(&mut self, set: u64, scope: u64) -> Result<&mut TaskSet, Error> {
        self.sets
            .get_mut(&set)
            .filter(|row| row.scope == scope)
            .ok_or_else(|| invalid("task set of another scope"))
    }
    /// The add-order index of the member that completed first among those not awaited, if
    /// any has: completion order is the scheduler's, so a replayed schedule chooses the same.
    fn first_completed(&self, set: u64) -> Option<u64> {
        self.sets[&set]
            .members
            .iter()
            .enumerate()
            .filter_map(|(index, member)| {
                let child = &self.tasks[member.as_ref()?];
                child.finished.then_some((child.completed, index as u64))
            })
            .min()
            .map(|(_, index)| index)
    }
    /// A dropped task set's members stay children of its scope, joined at its exit.
    pub fn drop_set(&mut self, set: u64) {
        self.sets.remove(&set);
    }
    pub fn finished(&mut self, task: u64, mut result: OperationResult) -> Result<(), Error> {
        let child = self
            .tasks
            .get_mut(&task)
            .ok_or_else(|| invalid("unknown completed task"))?;
        if let OperationResult::FailureChain { failures, .. } = &mut result {
            for failure in failures {
                if failure.spawn_path.is_empty() {
                    failure.spawn_path = child.path.clone();
                }
            }
            let scope = child.scope;
            self.scope_canceled.entry(scope).or_insert(2);
            let siblings = self.scopes[&scope].children.clone();
            for sibling in siblings {
                if sibling != task {
                    self.cancel(sibling, 2);
                }
            }
        }
        let child = self.tasks.get_mut(&task).unwrap();
        child.finished = true;
        child.completed = self.completions;
        self.completions += 1;
        child.result = Some(result);
        let detached = child.detached;
        self.spawn_counts.remove(&task);
        self.active_scopes.remove(&task);
        if detached && !self.retire(task)?.is_empty() {
            return Err(invalid(
                "detached task completed before disposing its result",
            ));
        }
        Ok(())
    }
}
impl Library {
    fn task_callback(
        &mut self,
        pending: Pending,
        callee: Value,
        arguments: Vec<Value>,
    ) -> OperationResult {
        let token = self.next_callback;
        self.next_callback += 1;
        let consumed = if matches!(pending, Pending::Isolate) {
            vec![1]
        } else {
            vec![]
        };
        self.tasks.pending.insert(token, pending);
        OperationResult::Call {
            callee,
            arguments,
            mut_arguments: vec![],
            writebacks: vec![],
            updates: vec![],
            owned: vec![],
            consumed,
            continuation: token,
        }
    }
    fn task_wait(
        &mut self,
        pending: Pending,
        context: &OperationContext<'_>,
        kind: &str,
        targets: Vec<u64>,
    ) -> OperationResult {
        let token = self.next_callback;
        self.next_callback += 1;
        self.tasks.pending.insert(token, pending);
        OperationResult::Park {
            token,
            wait: BlockedWait {
                spawn_path: self.tasks.path(),
                anchor: context.anchor().unwrap().clone(),
                kind: kind.into(),
                targets,
            },
        }
    }
    pub(super) fn task_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let a = &*call.arguments;
        self.tasks.yield_now = true;
        self.simulation.record(
            "Tasks",
            if name == "task.await" {
                "task.await.start"
            } else {
                name
            },
            super::channels::request(self.tasks.current, a),
            vec![],
            context,
        )?;
        Ok(match name {
            "task.scope" | "task.scope_until" => {
                self.simulation.id(arg(a, 0)?, "Tasks")?;
                let until = name == "task.scope_until";
                // `scope_until` returns `Bounded[R]`: the error exit reads R.
                let mut body_type = call.result_type;
                if until {
                    match &call
                        .types
                        .get(body_type)
                        .ok_or_else(|| invalid("scope result type"))?
                        .kind
                    {
                        hirsel_types::TypeKind::Named { arguments, .. } => {
                            match arguments.first() {
                                Some(hirsel_types::GenericArgument::Type(r)) => body_type = *r,
                                _ => return Err(invalid("Bounded without its result type")),
                            }
                        }
                        _ => return Err(invalid("task.scope_until returns Bounded")),
                    }
                }
                let deadline = if until {
                    let Value::Record { fields, .. } = arg(a, 1)? else {
                        return Err(invalid("expected a Deadline"));
                    };
                    let (Some(Some(clock)), Some(Some(Value::Handle { kind, id: at }))) =
                        (fields.first(), fields.get(1))
                    else {
                        return Err(invalid("a Deadline without its fields"));
                    };
                    if kind != "Instant" {
                        return Err(invalid("a Deadline without its Instant"));
                    }
                    if !self.tasks.host_deadlines {
                        self.simulation.id(clock, "MonotonicClock")?;
                    }
                    Some((clock.clone(), *at))
                } else {
                    None
                };
                let scope = self.tasks.fresh();
                self.tasks.scopes.insert(
                    scope,
                    ScopeState {
                        owner: self.tasks.current,
                        deadline: deadline.as_ref().map(|(_, at)| ScopeDeadline {
                            at: *at,
                            expired: false,
                            anchor: context.anchor().cloned(),
                        }),
                        ..Default::default()
                    },
                );
                self.tasks
                    .active_scopes
                    .entry(self.tasks.current)
                    .or_default()
                    .push(scope);
                if let Some((clock, at)) = deadline {
                    if self.tasks.host_deadlines {
                        self.tasks.entered.push((scope, clock, at));
                    } else if at <= self.simulation.monotonic_now() {
                        // A deadline already reached cancels the scope before its body runs.
                        self.expire_scope(scope, context)?;
                    }
                }
                self.task_callback(
                    Pending::Scope {
                        scope,
                        result: match &call
                            .types
                            .get(body_type)
                            .ok_or_else(|| invalid("scope result type"))?
                            .kind
                        {
                            hirsel_types::TypeKind::Parameter(_) => None,
                            hirsel_types::TypeKind::Named { definition, .. } => {
                                Some(definition.is_core("Result"))
                            }
                            _ => Some(false),
                        },
                    },
                    arg(a, if until { 2 } else { 1 })?.clone(),
                    vec![handle("Scope", scope)],
                )
            }
            "task.isolate" => {
                self.simulation.id(arg(a, 0)?, "Tasks")?;
                self.task_callback(Pending::Isolate, arg(a, 1)?.clone(), vec![])
            }
            "task.async" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let task = self.tasks.fresh();
                let mut path = self.tasks.path();
                let children = self
                    .tasks
                    .scopes
                    .get_mut(&scope)
                    .ok_or_else(|| invalid("expired task scope"))?;
                let ordinal = self
                    .tasks
                    .spawn_counts
                    .entry(self.tasks.current)
                    .or_default();
                path.push(*ordinal);
                *ordinal += 1;
                children.children.push(task);
                let canceled = self.tasks.canceled().map(|_| 1);
                self.tasks.tasks.insert(
                    task,
                    Child {
                        scope,
                        path,
                        canceled,
                        finished: false,
                        detached: false,
                        completed: 0,
                        result: None,
                        priority: self.tasks.schedule.admit(),
                    },
                );
                self.tasks.starts.push(TaskStart {
                    site: context.anchor().map(|anchor| anchor.site),
                    id: task,
                    callee: arg(a, 1)?.clone(),
                });
                OperationResult::returned(handle("Task", task)).transferring([1])
            }
            "task.await" | "task.cancel" | "task.detach" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let task = id(arg(a, 1)?, "Task")?;
                if self.tasks.tasks.get(&task).is_none_or(|t| t.scope != scope) {
                    return Err(invalid("task belongs to another scope"));
                }
                if name == "task.cancel" {
                    self.tasks.cancel(task, 0);
                    OperationResult::returned(Value::Unit)
                } else if name == "task.detach" {
                    OperationResult::Complete {
                        value: Value::Unit,
                        arguments: vec![],
                        consumed: vec![1],
                        released: self.tasks.detach(task)?,
                    }
                } else if let Some(result) = self.tasks.tasks.get_mut(&task).unwrap().result.take()
                {
                    self.record_join(task, &result, context)?;
                    result
                } else {
                    self.task_wait(Pending::Await(task), context, "task.await", vec![task])
                }
            }
            "task.set" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                if !self.tasks.scopes.contains_key(&scope) {
                    return Err(invalid("expired task scope"));
                }
                let set = self.tasks.fresh();
                self.tasks.sets.insert(
                    set,
                    TaskSet {
                        scope,
                        members: vec![],
                    },
                );
                OperationResult::returned(handle("TaskSet", set))
            }
            "task.add" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let task = id(arg(a, 2)?, "Task")?;
                if self.tasks.tasks.get(&task).is_none_or(|t| t.scope != scope) {
                    return Err(invalid("task belongs to another scope"));
                }
                self.tasks
                    .set(id(arg(a, 1)?, "TaskSet")?, scope)?
                    .members
                    .push(Some(task));
                OperationResult::Complete {
                    value: Value::Unit,
                    arguments: vec![],
                    consumed: vec![2],
                    released: vec![],
                }
            }
            "task.await_any" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let set = id(arg(a, 1)?, "TaskSet")?;
                let waiting: Vec<u64> = self
                    .tasks
                    .set(set, scope)?
                    .members
                    .iter()
                    .flatten()
                    .copied()
                    .collect();
                if waiting.is_empty() {
                    OperationResult::returned(none())
                } else if let Some(index) = self.tasks.first_completed(set) {
                    self.record_any(index, context)?;
                    OperationResult::returned(some(u64_value(index)))
                } else {
                    self.task_wait(Pending::AwaitAny(set), context, "task.await_any", waiting)
                }
            }
            "task.await_at" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let index = super::dispatch::index(arg(a, 2)?)?;
                let member = self
                    .tasks
                    .set(id(arg(a, 1)?, "TaskSet")?, scope)?
                    .members
                    .get_mut(index)
                    .and_then(Option::take);
                let Some(task) = member else {
                    return Ok(OperationResult::Failed(super::dispatch::fail(
                        call,
                        context,
                        "CONC-2",
                        "the task set has no unawaited member at this index",
                    )));
                };
                if let Some(result) = self.tasks.tasks.get_mut(&task).unwrap().result.take() {
                    self.record_join(task, &result, context)?;
                    result
                } else {
                    self.task_wait(Pending::Await(task), context, "task.await", vec![task])
                }
            }
            "task.cancel_all" | "task.detach_all" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let set = id(arg(a, 1)?, "TaskSet")?;
                self.tasks.set(set, scope)?;
                let members: Vec<u64> = self
                    .tasks
                    .sets
                    .remove(&set)
                    .unwrap()
                    .members
                    .into_iter()
                    .flatten()
                    .collect();
                let mut released = vec![];
                for &task in &members {
                    if name == "task.cancel_all" {
                        self.tasks.cancel(task, 0);
                    }
                    released.extend(self.tasks.detach(task)?);
                }
                if name == "task.cancel_all" {
                    self.drain(members, released, context)
                } else {
                    drained(released)
                }
            }
            "task.check_canceled" => {
                id(arg(a, 0)?, "Scope")?;
                OperationResult::returned(match self.tasks.canceled() {
                    Some(cause) => error(variant(0, vec![variant(cause, vec![])])),
                    None => ok(Value::Unit),
                })
            }
            _ => return Err(invalid("unknown task operation")),
        })
    }
    pub(super) fn resume_task(
        &mut self,
        pending: Pending,
        result: OperationResult,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        Ok(match pending {
            Pending::Isolate => match result {
                OperationResult::Complete { value, .. } => OperationResult::returned(ok(value)),
                OperationResult::FailureChain { mut failures, .. } => {
                    for failure in &mut failures {
                        if failure.spawn_path.is_empty() {
                            failure.spawn_path = self.tasks.path();
                        }
                    }
                    OperationResult::returned(error(failure_value(&failures, context)?))
                }
                _ => return Err(invalid("isolate callback did not complete")),
            },
            Pending::Scope {
                scope,
                result: is_result,
            } => {
                let active = self
                    .tasks
                    .active_scopes
                    .get_mut(&self.tasks.current)
                    .ok_or_else(|| invalid("missing active scope"))?;
                if active.pop() != Some(scope) {
                    return Err(invalid("scope exit order"));
                }
                let error_value = matches!(
                    &result,
                    OperationResult::Complete {
                        value: Value::Variant { variant: 1, .. },
                        ..
                    }
                );
                if error_value && is_result.is_none() {
                    return Err(Error::Unavailable("task.scope needs the concrete result nominal for an erased generic error exit".into()));
                }
                if matches!(&result, OperationResult::FailureChain { .. })
                    || error_value && is_result == Some(true)
                {
                    for child in self.tasks.scopes[&scope].children.clone() {
                        self.tasks.cancel(child, 1);
                    }
                }
                let pending = Pending::Join {
                    scope,
                    result,
                    released: vec![],
                };
                self.join_scope(pending, context)?
            }
            _ => return Err(invalid("invalid task callback continuation")),
        })
    }
    fn join_scope(
        &mut self,
        pending: Pending,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let Pending::Join {
            scope,
            mut result,
            mut released,
        } = pending
        else {
            return Err(invalid("invalid scope join"));
        };
        let children = self.tasks.scopes[&scope].children.clone();
        if children.iter().any(|id| !self.tasks.tasks[id].finished) {
            return Ok(self.task_wait(
                Pending::Join {
                    scope,
                    result,
                    released,
                },
                context,
                "task.scope",
                children,
            ));
        }
        let scope_state = self.tasks.scopes.remove(&scope).unwrap();
        let bounded = scope_state.deadline.as_ref().map(|d| d.expired);
        let mut completed: Vec<_> = scope_state
            .failures
            .into_iter()
            .map(|(path, failures)| {
                (
                    path,
                    OperationResult::FailureChain {
                        failures,
                        released: vec![],
                    },
                )
            })
            .collect();
        for id in children {
            let child = self.tasks.tasks.remove(&id).unwrap();
            if let Some(result) = child.result {
                completed.push((child.path, result));
            }
        }
        completed.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, child) in completed {
            match child {
                OperationResult::Complete { value, .. } => released.push(value),
                OperationResult::FailureChain {
                    failures: chain, ..
                } => {
                    if let OperationResult::FailureChain {
                        failures: primary, ..
                    } = &mut result
                    {
                        if chain.first().map(|f| &f.spawn_path)
                            < primary.first().map(|f| &f.spawn_path)
                        {
                            let mut earlier = chain;
                            append_chain(&mut earlier, std::mem::take(primary));
                            *primary = earlier;
                        } else {
                            append_chain(primary, chain);
                        }
                    } else {
                        if let OperationResult::Complete { value, .. } = result {
                            released.push(value);
                        }
                        result = OperationResult::FailureChain {
                            failures: chain,
                            released: vec![],
                        };
                    }
                }
                _ => return Err(invalid("unfinished child result")),
            }
        }
        self.tasks.scope_canceled.remove(&scope);
        // `task.scope_until`: `Finished(r)`, or `Expired(r)` when its deadline fired.
        if let (Some(expired), OperationResult::Complete { value, .. }) = (bounded, &mut result) {
            let body = std::mem::replace(value, Value::Unit);
            *value = variant(u32::from(expired), vec![body]);
        }
        match &mut result {
            OperationResult::Complete {
                released: pending, ..
            }
            | OperationResult::FailureChain {
                released: pending, ..
            } => pending.extend(released),
            _ => return Err(invalid("invalid scope result")),
        }
        Ok(result)
    }
    fn record_join(
        &mut self,
        task: u64,
        result: &OperationResult,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let output = match result {
            OperationResult::Complete { value, .. } => super::dispatch::encode_value(value),
            OperationResult::FailureChain { failures, .. } => {
                super::dispatch::encode_value(&failure_value(failures, context)?)
            }
            _ => return Err(invalid("unfinished joined task")),
        };
        self.simulation.record(
            "Tasks",
            "task.await.complete",
            task.to_le_bytes().to_vec(),
            output,
            context,
        )
    }
    /// `task.await_any`'s choice, an event of the run as a join's completion is.
    fn record_any(&mut self, index: u64, context: &mut OperationContext<'_>) -> Result<(), Error> {
        self.simulation.record(
            "Tasks",
            "task.await_any.complete",
            self.tasks.current.to_le_bytes().to_vec(),
            index.to_le_bytes().to_vec(),
            context,
        )
    }
    /// `task.cancel_all` returns once every canceled member has finished; detached, each
    /// one's result was released as it completed.
    fn drain(
        &mut self,
        members: Vec<u64>,
        released: Vec<Value>,
        context: &OperationContext<'_>,
    ) -> OperationResult {
        let members: Vec<u64> = members
            .into_iter()
            .filter(|task| self.tasks.tasks.contains_key(task))
            .collect();
        if members.is_empty() {
            drained(released)
        } else {
            let targets = members.clone();
            self.task_wait(
                Pending::Drain { members, released },
                context,
                "task.cancel_all",
                targets,
            )
        }
    }
    pub(super) fn poll_task(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let pending = self
            .tasks
            .pending
            .remove(&token)
            .ok_or_else(|| invalid("unknown task wait"))?;
        match pending {
            Pending::Await(task) => {
                let result = self
                    .tasks
                    .tasks
                    .get_mut(&task)
                    .ok_or_else(|| invalid("expired awaited task"))?
                    .result
                    .take();
                if let Some(result) = &result {
                    self.record_join(task, result, context)?;
                } else {
                    self.tasks.pending.insert(token, Pending::Await(task));
                }
                Ok(result)
            }
            Pending::Join { scope, .. }
                if scope != 0
                    && self.tasks.scopes[&scope]
                        .children
                        .iter()
                        .any(|id| !self.tasks.tasks[id].finished) =>
            {
                self.tasks.pending.insert(token, pending);
                Ok(None)
            }
            pending @ Pending::Join { .. } => self.join_scope(pending, context).map(Some),
            Pending::AwaitAny(set) => match self.tasks.first_completed(set) {
                Some(index) => {
                    self.record_any(index, context)?;
                    Ok(Some(OperationResult::returned(some(u64_value(index)))))
                }
                None => {
                    self.tasks.pending.insert(token, pending);
                    Ok(None)
                }
            },
            Pending::Drain { ref members, .. }
                if members
                    .iter()
                    .any(|task| self.tasks.tasks.contains_key(task)) =>
            {
                self.tasks.pending.insert(token, pending);
                Ok(None)
            }
            Pending::Drain { released, .. } => Ok(Some(drained(released))),
            _ => Err(invalid("callback is not a wait")),
        }
    }
}
/// `CancelCause` ordinals (core library).
const CANCEL_PARENT: u32 = 1;
pub(super) const CANCEL_DEADLINE: u32 = 3;
impl Library {
    /// Cancel `scope` for its deadline (CONC-3): the scope's later blocking calls and its
    /// children see `DeadlineExceeded`, the children of scopes its owner entered inside it
    /// `Parent`, and its exit reports `Expired`. The expiry is a recorded scheduling event.
    pub(super) fn expire_scope(
        &mut self,
        scope: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let state = self
            .tasks
            .scopes
            .get_mut(&scope)
            .ok_or_else(|| invalid("an exited scope's deadline expired"))?;
        let Some(deadline) = &mut state.deadline else {
            return Err(invalid("a scope without a deadline expired"));
        };
        if deadline.expired {
            return Ok(());
        }
        deadline.expired = true;
        let (at, anchor, owner) = (deadline.at, deadline.anchor.clone(), state.owner);
        let children = state.children.clone();
        self.tasks
            .scope_canceled
            .entry(scope)
            .or_insert(CANCEL_DEADLINE);
        for child in children {
            self.tasks.cancel(child, CANCEL_DEADLINE);
        }
        let inner: Vec<u64> = self
            .tasks
            .active_scopes
            .get(&owner)
            .map(|stack| {
                stack
                    .iter()
                    .skip_while(|s| **s != scope)
                    .skip(1)
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for inner in inner {
            for child in self.tasks.scopes[&inner].children.clone() {
                self.tasks.cancel(child, CANCEL_PARENT);
            }
        }
        let anchor = anchor.as_ref().or(context.anchor);
        self.simulation.record(
            "Tasks",
            "task.scope_until.expire",
            at.to_le_bytes().to_vec(),
            vec![],
            &mut OperationContext {
                configuration: context.configuration,
                sources: context.sources,
                stdout: &mut *context.stdout,
                trace: &mut *context.trace,
                trace_mode: context.trace_mode,
                anchor,
            },
        )
    }
    /// The simulation's scope deadlines its virtual reading reached, by deadline then entry.
    pub(super) fn expire_due(&mut self, context: &mut OperationContext<'_>) -> Result<(), Error> {
        if self.tasks.host_deadlines {
            return Ok(());
        }
        let now = self.simulation.monotonic_now();
        let mut due: Vec<(u64, u64)> = self
            .tasks
            .scopes
            .iter()
            .filter_map(|(id, s)| {
                s.deadline
                    .as_ref()
                    .filter(|d| !d.expired && d.at <= now)
                    .map(|d| (d.at, *id))
            })
            .collect();
        // Scope identities grow in entry order.
        due.sort();
        for (_, scope) in due {
            self.expire_scope(scope, context)?;
        }
        Ok(())
    }
    /// The earliest simulated scope deadline still pending: an outstanding event source.
    pub(super) fn next_scope_deadline(&self) -> Option<u64> {
        if self.tasks.host_deadlines {
            return None;
        }
        self.tasks
            .scopes
            .values()
            .filter_map(|s| s.deadline.as_ref().filter(|d| !d.expired).map(|d| d.at))
            .min()
    }
    /// A host adapter observes scope deadlines on its own clock instead of virtual time.
    pub fn host_deadlines(&mut self) {
        self.tasks.host_deadlines = true;
    }
    /// The deadline scopes entered since the last call: scope, clock and deadline ticks.
    pub fn take_deadlines(&mut self) -> Vec<(u64, Value, u64)> {
        std::mem::take(&mut self.tasks.entered)
    }
    /// Whether `scope` is live and its deadline has not fired.
    pub fn deadline_pending(&self, scope: u64) -> bool {
        self.tasks
            .scopes
            .get(&scope)
            .and_then(|s| s.deadline.as_ref())
            .is_some_and(|d| !d.expired)
    }
    /// The host's timer for `scope` fired: cancel it as the simulation would.
    pub fn expire_deadline(
        &mut self,
        scope: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        self.expire_scope(scope, context)
    }
}
/// `task.cancel_all` or `task.detach_all` done: the set was taken, the released results remain.
fn drained(released: Vec<Value>) -> OperationResult {
    OperationResult::Complete {
        value: Value::Unit,
        arguments: vec![],
        consumed: vec![1],
        released,
    }
}
pub(super) fn append_chain(primary: &mut Vec<FailureRecord>, mut chain: Vec<FailureRecord>) {
    let offset = primary.len();
    if let Some(last) = primary.last_mut() {
        last.cause = Some(offset);
    }
    for record in &mut chain {
        if let Some(cause) = &mut record.cause {
            *cause += offset;
        }
    }
    primary.extend(chain);
}
fn record(name: &str, fields: Vec<Value>) -> Value {
    Value::Record {
        definition: (&hirsel_syntax::DefPath::core(name)).into(),
        fields: fields.into_iter().map(Some).collect(),
    }
}
pub(super) fn failure_value(
    chain: &[FailureRecord],
    context: &OperationContext<'_>,
) -> Result<Value, Error> {
    let mut cause = none();
    for failure in chain.iter().rev() {
        let f = &failure.failure;
        let site = context
            .sources
            .location(f.site)
            .ok_or_else(|| invalid("failure source missing"))?;
        let kind = match f.kind.as_str() {
            "Depth" => 2,
            "OutOfSpace" => 3,
            "DeadMutex" => 4,
            "Contract" => 1,
            _ => 0,
        };
        let value = record(
            "Failure",
            vec![
                variant(kind, vec![]),
                Value::String(f.rule.as_str().into()),
                record(
                    "SourceSite",
                    vec![
                        Value::String(failure.anchor.definition.to_string()),
                        u64_value(site.line as u64),
                        u64_value(site.column as u64),
                    ],
                ),
                Value::Sequence {
                    kind: hirsel_types::BuiltinType::List,
                    elements: failure
                        .spawn_path
                        .iter()
                        .map(|i| Some(u64_value(*i as u64)))
                        .collect(),
                    arena: None,
                },
                cause,
            ],
        );
        cause = some(Value::Box(vec![Some(value)]));
    }
    let Value::Variant { mut fields, .. } = cause else {
        return Err(invalid("empty isolated failure"));
    };
    let Some(Value::Box(mut boxed)) = fields.pop().flatten() else {
        return Err(invalid("invalid failure cause"));
    };
    boxed
        .pop()
        .flatten()
        .ok_or_else(|| invalid("invalid failure value"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Operations, TraceMode, observation::Anchor};
    use hirsel_span::{SourceMap, Span};
    use hirsel_syntax::DefPath;
    use hirsel_tir::{BodyId, RunConfiguration};
    use hirsel_types::{Type, TypeKind, TypeStore};

    fn call(
        library: &mut Library,
        name: &str,
        arguments: &[Value],
        cx: &mut OperationContext<'_>,
    ) -> OperationResult {
        let mut types = TypeStore::default();
        let unit = types.push(Type::with_kind(TypeKind::Tuple(vec![].into())));
        library
            .task_call(
                name,
                &OperationCall {
                    runtime: &Default::default(),
                    path: &DefPath::core(name),
                    arguments: &mut arguments.to_vec(),
                    generics: &[],
                    types: &types,
                    result_type: unit,
                    site: Span::default(),
                },
                cx,
            )
            .unwrap()
    }

    // A corpus program cannot inspect the runtime's private child tables. Exercise
    // the real spawn/detach/completion hooks with both possible completion orders.
    #[test]
    fn detached_children_leave_bounded_scope_tables() {
        let run = RunConfiguration {
            entry: DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let sources = SourceMap::default();
        let anchor = Anchor {
            definition: run.entry.clone(),
            body: BodyId(0),
            cleanup: None,
            site: Span::default(),
        };
        let mut stdout = vec![];
        let mut trace = vec![];
        let mut cx = OperationContext {
            configuration: &run,
            sources: &sources,
            stdout: &mut stdout,
            trace: &mut trace,
            trace_mode: TraceMode::Effects,
            anchor: Some(&anchor),
        };
        let mut library = Library::new(&run, Default::default()).unwrap();
        let scope = library.tasks.fresh();
        library.tasks.scopes.insert(scope, Default::default());
        for ordinal in 0..4096 {
            let closure = Value::Closure {
                definition: (&DefPath::core("child")).into(),
                body: BodyId(0),
                invoked: Some(Default::default()),
                captures: vec![],
                environment: Box::new(crate::value::Environment {
                    owned_captures: vec![],
                    sizes: Default::default(),
                    type_arguments: Default::default(),
                }),
            };
            let OperationResult::Complete { value: child, .. } = call(
                &mut library,
                "task.async",
                &[handle("Scope", scope), closure],
                &mut cx,
            ) else {
                panic!("spawn")
            };
            let task = id(&child, "Task").unwrap();
            assert_eq!(library.take_tasks().len(), 1);
            assert_eq!(library.tasks.tasks[&task].path, vec![ordinal]);
            if ordinal % 2 == 0 {
                call(
                    &mut library,
                    "task.detach",
                    &[handle("Scope", scope), child],
                    &mut cx,
                );
                library
                    .task_finished(task, OperationResult::returned(Value::Unit), &mut cx)
                    .unwrap();
            } else {
                library
                    .task_finished(task, OperationResult::returned(Value::Unit), &mut cx)
                    .unwrap();
                call(
                    &mut library,
                    "task.detach",
                    &[handle("Scope", scope), child],
                    &mut cx,
                );
            }
            assert!(
                library.tasks.tasks.is_empty(),
                "completed detached child retained at {ordinal}"
            );
            assert!(
                library.tasks.scopes[&scope].children.is_empty(),
                "scope child row retained at {ordinal}"
            );
            assert!(library.tasks.spawn_counts.len() <= 1);
            assert!(library.tasks.active_scopes.len() <= 1);
        }

        // Failures survive row retirement and are joined in spawn order even
        // when detached children finish in a different order.
        let mut ids = vec![];
        for _ in 0..3 {
            let closure = Value::Closure {
                definition: (&DefPath::core("child")).into(),
                body: BodyId(0),
                invoked: Some(Default::default()),
                captures: vec![],
                environment: Box::new(crate::value::Environment {
                    owned_captures: vec![],
                    sizes: Default::default(),
                    type_arguments: Default::default(),
                }),
            };
            let OperationResult::Complete { value: child, .. } = call(
                &mut library,
                "task.async",
                &[handle("Scope", scope), closure],
                &mut cx,
            ) else {
                panic!("spawn")
            };
            ids.push(id(&child, "Task").unwrap());
            library.take_tasks();
            call(
                &mut library,
                "task.detach",
                &[handle("Scope", scope), child],
                &mut cx,
            );
        }
        for index in [2, 0, 1] {
            let failure = FailureRecord {
                operation: None,
                failure: crate::Failure {
                    rule: hirsel_diag::RuleId::of("SEM-1"),
                    site: Span::default(),
                    line: 1,
                    kind: "Arithmetic".into(),
                },
                anchor: anchor.clone(),
                spawn_path: vec![],
                cause: None,
            };
            library
                .task_finished(
                    ids[index],
                    OperationResult::FailureChain {
                        failures: vec![failure],
                        released: vec![],
                    },
                    &mut cx,
                )
                .unwrap();
        }
        assert!(library.tasks.tasks.is_empty());
        assert!(library.tasks.scopes[&scope].children.is_empty());
        let joined = library
            .join_scope(
                Pending::Join {
                    scope,
                    result: OperationResult::returned(Value::Unit),
                    released: vec![],
                },
                &cx,
            )
            .unwrap();
        let OperationResult::FailureChain { failures, .. } = joined else {
            panic!("detached failure lost")
        };
        assert_eq!(
            failures
                .iter()
                .map(|f| f.spawn_path.clone())
                .collect::<Vec<_>>(),
            vec![vec![4096], vec![4097], vec![4098]]
        );
        assert_eq!(
            failures.iter().map(|f| f.cause).collect::<Vec<_>>(),
            vec![Some(1), Some(2), None]
        );
        assert!(library.tasks.scopes.is_empty());
    }
}
