//! Structured task state. Frames and destruction belong to the evaluator.
use super::dispatch::{arg, variant};
use super::{Library, error, none, ok, some, u64_value};
use crate::observation::{Anchor, FailureRecord};
use crate::{
    BlockedWait, Error, OperationCall, OperationContext, OperationResult, TaskRest, TaskStart,
    Value, invalid,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Tasks {
    pub current: u64,
    pub yield_now: bool,
    pub shielded: bool,
    /// Free mode gives every returned frame a distinct completion op.
    pub free_rests: bool,
    /// Deadline scopes retained until their owner's called cleanup returns.
    closing_deadlines: BTreeMap<u64, u64>,
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
    /// The run's scheduler policy (`inputs.scheduler`) and the root task's priority.
    pub schedule: hirsel_kernels::schedule::Schedule,
    pub root_priority: u64,
    /// A callee run's root scope, cancelled by its caller (RUN-9): the cause.
    root_canceled: Option<u32>,
    /// Cancellations so far, of a task, a scope or the root scope: a parked task's
    /// cancellation can change only when this does.
    pub cancellations: u64,
    /// Holding children whose result an operation discarded, with that operation's task:
    /// each child drops its result in a turn of its own (its rest) before the operation
    /// returns, as native `hand_off` (detach, `detach_all`, `cancel_all`, scope exit).
    rests: Vec<(u64, TaskRest)>,
    /// The children in their rest: whose failure a dropped result raises belongs to them.
    resting: BTreeMap<u64, Resting>,
    /// Provider-owned runner waiters run no evaluator frames.
    pub frameless: BTreeSet<u64>,
    /// Tasks whose operation yields once its rests return: the yield follows the hand-offs.
    deferred_yield: BTreeSet<u64>,
    /// Failures raised in the rests of a task's scope exit, by spawn path: they join its
    /// result when the rests return (FAIL-2).
    late: BTreeMap<u64, Vec<(Vec<u32>, Vec<FailureRecord>)>>,
}
/// A child in its rest, whose row is retired: its scope, spawn path and cancellation, and
/// the task whose scope exit discarded its result, when a scope exit did.
struct Resting {
    scope: u64,
    path: Vec<u32>,
    canceled: Option<u32>,
    exit: Option<u64>,
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
    /// Ranks always advance, even before any set exists or contains a child.
    completions: u64,
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
        Value::Mutex(root) if expected == "Mutex" => Ok(root.id()),
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
                let rest = self.resting.get(&task)?;
                rest.canceled
                    .or_else(|| self.scope_canceled.get(&rest.scope).copied())
            })
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
        self.cancellations += 1;
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
        self.frameless.insert(task);
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
    /// The spawn path of live child `task`.
    pub(super) fn child_path(&self, task: u64) -> Option<Vec<u32>> {
        self.tasks
            .get(&task)
            .map(|t| t.path.clone())
            .or_else(|| self.resting.get(&task).map(|r| r.path.clone()))
    }
    /// Whether each member of task set `set`, in add order, has finished (an awaited
    /// member has not).
    pub(super) fn member_finished(&self, set: u64) -> Vec<bool> {
        self.sets.get(&set).map_or_else(Vec::new, |row| {
            row.members
                .iter()
                .map(|m| m.is_some_and(|t| self.tasks.get(&t).is_some_and(|c| c.finished)))
                .collect()
        })
    }
    pub fn path(&self) -> Vec<u32> {
        self.tasks
            .get(&self.current)
            .map(|t| &t.path)
            .or_else(|| self.resting.get(&self.current).map(|r| &r.path))
            .cloned()
            .unwrap_or_default()
    }
    fn cancel(&mut self, task: u64, cause: u32) {
        self.cancellations += 1;
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
    /// Detach `task`: a holding child drops its result in its rest, before the operation
    /// returns; a running one drops it at its completion.
    fn detach(&mut self, task: u64) -> Result<(), Error> {
        let child = self
            .tasks
            .get_mut(&task)
            .ok_or_else(|| invalid("unknown detached task"))?;
        child.detached = true;
        if child.finished {
            let (scope, path, canceled) = (child.scope, child.path.clone(), child.canceled);
            let holds = matches!(child.result, Some(OperationResult::Complete { .. }));
            let values = self.retire(task)?;
            if holds {
                self.rest(task, scope, path, canceled, values, false);
            } else {
                self.frameless.remove(&task);
            }
        }
        Ok(())
    }
    /// The running task's operation discarded holding child `task`'s `values`: the child
    /// drops them in its rest.
    fn rest(
        &mut self,
        task: u64,
        scope: u64,
        path: Vec<u32>,
        canceled: Option<u32>,
        values: Vec<Value>,
        exit: bool,
    ) {
        if values.is_empty() && !self.free_rests {
            self.frameless.remove(&task);
            return;
        }
        self.resting.insert(
            task,
            Resting {
                scope,
                path,
                canceled,
                exit: exit.then_some(self.current),
            },
        );
        self.rests.push((
            self.current,
            TaskRest {
                child: task,
                values,
            },
        ));
    }
    /// The running task's operation yields after its rests, if it has any.
    fn defer_yield(&mut self) {
        if self.rests.iter().any(|(parent, _)| *parent == self.current) {
            self.yield_now = false;
            self.deferred_yield.insert(self.current);
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
    /// Whether finished child `task` holds a result nobody took yet.
    pub fn holds_result(&self, task: u64) -> bool {
        self.tasks.get(&task).is_some_and(|child| {
            child.finished
                && child.result.as_ref().is_some_and(|result| {
                    self.free_rests
                        || !matches!(
                            result,
                            OperationResult::Complete {
                                value: Value::Unit,
                                ..
                            }
                        )
                })
        })
    }
    pub(super) fn has_rests(&self, parent: u64) -> bool {
        self.rests.iter().any(|(owner, _)| *owner == parent)
    }

    /// The rests the operation of `parent` queued, in hand-off order.
    pub fn take_rests(&mut self, parent: u64) -> Vec<TaskRest> {
        if self.rests.is_empty() {
            return vec![];
        }
        let (mine, others) = std::mem::take(&mut self.rests)
            .into_iter()
            .partition::<Vec<_>, _>(|(task, _)| *task == parent);
        self.rests = others;
        mine.into_iter().map(|(_, rest)| rest).collect()
    }
    /// `task`'s rest ended, with the failures dropping its result raised. A detached child's
    /// belong to its scope and cancel its siblings, as its completion's would (FAIL-2); a
    /// scope exit's join that exit's result.
    pub fn rest_finished(
        &mut self,
        task: u64,
        mut failures: Vec<FailureRecord>,
    ) -> Result<(), Error> {
        let resting = self
            .resting
            .remove(&task)
            .ok_or_else(|| invalid("an unknown rest ended"))?;
        self.frameless.remove(&task);
        if failures.is_empty() {
            return Ok(());
        }
        for failure in &mut failures {
            if failure.spawn_path.is_empty() {
                failure.spawn_path = resting.path.clone();
            }
        }
        if let Some(parent) = resting.exit {
            self.late
                .entry(parent)
                .or_default()
                .push((resting.path, failures));
            return Ok(());
        }
        let scope = resting.scope;
        let Some(state) = self.scopes.get_mut(&scope) else {
            return Err(invalid("a detached rest outlived its scope"));
        };
        state.failures.insert(resting.path, failures);
        self.scope_canceled.entry(scope).or_insert(2);
        for sibling in self.scopes[&scope].children.clone() {
            self.cancel(sibling, 2);
        }
        Ok(())
    }
    /// The rests of `parent`'s operation returned to it with `result` pending: a scope exit's
    /// rest failures join it, and a deferred yield follows.
    pub fn rests_returned(&mut self, parent: u64, result: &mut OperationResult) {
        if self.deferred_yield.remove(&parent) {
            self.yield_now = true;
        }
        let Some(mut late) = self.late.remove(&parent) else {
            return;
        };
        late.sort_by(|a, b| a.0.cmp(&b.0));
        let mut released = vec![];
        for (_, chain) in late {
            join_failure(result, &mut released, chain);
        }
        if let OperationResult::FailureChain {
            released: pending, ..
        } = result
        {
            pending.extend(released);
        }
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
            self.cancellations += 1;
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
        let scope = self
            .scopes
            .get_mut(&child.scope)
            .expect("a live child's scope");
        child.completed = scope.completions;
        scope.completions = scope
            .completions
            .checked_add(1)
            .ok_or_else(|| invalid("a scope's completion rank overflowed"))?;
        child.result = Some(result);
        let detached = child.detached;
        self.spawn_counts.remove(&task);
        self.active_scopes.remove(&task);
        if detached {
            if !self.retire(task)?.is_empty() {
                return Err(invalid(
                    "detached task completed before disposing its result",
                ));
            }
            self.frameless.remove(&task);
        }
        Ok(())
    }
}
impl Library {
    /// Apply the bounded wrapper only when the scope exit can publish Done.
    /// Its retained timer may expire while a child drops a discarded result.
    pub(super) fn free_deadline_done(
        &mut self,
        mut result: OperationResult,
    ) -> Result<OperationResult, Error> {
        let Some(scope) = self.tasks.closing_deadlines.remove(&self.tasks.current) else {
            return Ok(result);
        };
        let row = self
            .tasks
            .scopes
            .remove(&scope)
            .ok_or_else(|| invalid("missing closing deadline scope"))?;
        let live = row
            .deadline
            .ok_or_else(|| invalid("closing scope has no deadline"))?
            .expired;
        self.tasks.scope_canceled.remove(&scope);
        let path = self.tasks.path();
        let expired = self
            .free
            .as_mut()
            .unwrap()
            .session
            .expired(&path, live)
            .map_err(|error| invalid(format!("free-mode {error}")))?;
        if let OperationResult::Complete { value, .. } = &mut result {
            let body = std::mem::replace(value, Value::Unit);
            *value = variant(u32::from(expired), vec![body]);
        }
        Ok(result)
    }

    pub(super) fn cpu_scope(&mut self) -> u64 {
        let scope = self.tasks.fresh();
        self.tasks.scopes.insert(
            scope,
            ScopeState {
                owner: self.tasks.current,
                ..Default::default()
            },
        );
        self.tasks
            .active_scopes
            .entry(self.tasks.current)
            .or_default()
            .push(scope);
        scope
    }

    pub(super) fn cpu_child_result(&mut self, child: u64) -> Option<OperationResult> {
        self.tasks.tasks.get_mut(&child)?.result.take()
    }

    pub(super) fn spawn_child(
        &mut self,
        scope: u64,
        callee: Value,
        arguments: Vec<Value>,
        borrowed: bool,
        argument_paths: Vec<Vec<usize>>,
        context: &OperationContext<'_>,
    ) -> Result<u64, Error> {
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
        // Free mode: the child registers in this spawn's section; a replay never
        // starts a child its cut does not hold.
        let path = self.tasks.tasks[&task].path.clone();
        let starts = self
            .free
            .as_mut()
            .is_none_or(|free| free.session.start(&path));
        if starts {
            self.tasks.starts.push(TaskStart {
                site: context.anchor().map(|anchor| anchor.site),
                id: task,
                parent: self.tasks.current,
                callee,
                arguments,
                argument_paths,
                borrowed,
            });
        }
        if self.cpu_active.contains(&self.tasks.current) {
            self.cpu_active.insert(task);
        }
        Ok(task)
    }

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
        // Native isolate changes only the task's failure handler. Its body
        // supplies every Free operation; entering isolate adds no ordinal.
        if self.native_free() && name == "task.isolate" {
            self.tasks.yield_now = false;
            return self.task_action(name, call, context, None);
        }
        // The compiled await_at body checks and removes a member in two
        // administrative operations before joining its handle. Keep these
        // native F1 ordinals without changing the cooperative F0 convention.
        let mut native_member = None;
        if self.native_free() && name == "task.await_at" {
            match self.free_enter(hirsel_free::phase::Phase::Done, context)? {
                super::free::Enter::Park(park) => return Ok(park),
                super::free::Enter::Run => {}
                super::free::Enter::Off => unreachable!(),
            }
            let scope = id(arg(a, 0)?, "Scope")?;
            let set = id(arg(a, 1)?, "TaskSet")?;
            let index = super::dispatch::index(arg(a, 2)?)?;
            let has = self
                .tasks
                .set(set, scope)?
                .members
                .get(index)
                .is_some_and(Option::is_some);
            let checked = self.free_done(OperationResult::returned(Value::Bool(has)), context)?;
            if !matches!(checked, OperationResult::Complete { .. }) {
                return Ok(checked);
            }
            if !has {
                self.tasks.yield_now = false;
                return Ok(OperationResult::Failed(super::dispatch::fail(
                    call,
                    context,
                    "CONC-2",
                    "the task set has no unawaited member at this index",
                )));
            }
            match self.free_enter(hirsel_free::phase::Phase::Done, context)? {
                super::free::Enter::Park(park) => return Ok(park),
                super::free::Enter::Run => {}
                super::free::Enter::Off => unreachable!(),
            }
            native_member = self
                .tasks
                .set(set, scope)?
                .members
                .get_mut(index)
                .and_then(Option::take);
            let taken = self.free_done(OperationResult::returned(Value::Unit), context)?;
            if !matches!(taken, OperationResult::Complete { .. }) {
                return Ok(taken);
            }
        }
        let waiting = matches!(
            name,
            "task.await" | "task.await_at" | "task.await_any" | "task.cancel_all"
        );
        let calls = matches!(
            name,
            "task.await" | "task.await_at" | "task.detach" | "task.detach_all" | "task.cancel_all"
        );
        let entry = if calls {
            self.free_enter_call(context)?
        } else {
            self.free_enter(
                if waiting {
                    hirsel_free::phase::Phase::Entered
                } else {
                    hirsel_free::phase::Phase::Done
                },
                context,
            )?
        };
        let free = match entry {
            super::free::Enter::Off => false,
            super::free::Enter::Park(park) => return Ok(park),
            super::free::Enter::Run => true,
        };
        let result = self.task_action(name, call, context, native_member)?;
        if !free {
            Ok(result)
        } else if waiting {
            self.free_waiting(result, context)
        } else {
            self.free_done(result, context)
        }
    }
    fn task_action(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
        native_member: Option<u64>,
    ) -> Result<OperationResult, Error> {
        let a = &*call.arguments;
        Ok(match name {
            "task.scope" | "task.scope_until" => {
                self.simulation.id(arg(a, 0)?, "Tasks")?;
                let until = name == "task.scope_until";
                // CONC-2: only a nominal Result error cancels on scope exit. A
                // generic caller's destination can still be its parameter R;
                // invocation actuals have already been closed by the evaluator.
                // Read that actual rather than guessing from the value's tag.
                let mut body_type = match call.generics.first() {
                    Some(hirsel_types::GenericArgument::Type(ty)) => *ty,
                    Some(_) => return Err(invalid("scope result generic must be a type")),
                    None => call.result_type,
                };
                // Calls without explicit actuals supply their concrete result
                // row; `scope_until`'s row is Bounded[R], so unwrap it then.
                if until && call.generics.is_empty() {
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
                let task =
                    self.spawn_child(scope, arg(a, 1)?.clone(), vec![], false, vec![], context)?;
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
                    self.tasks.detach(task)?;
                    self.tasks.defer_yield();
                    OperationResult::Complete {
                        value: Value::Unit,
                        arguments: vec![],
                        consumed: vec![1],
                        released: vec![],
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
                } else if let Some(index) = {
                    let live = self.tasks.first_completed(set);
                    self.free_any(live, set)?
                } {
                    self.record_any(index, context)?;
                    OperationResult::returned(some(u64_value(index)))
                } else {
                    self.task_wait(Pending::AwaitAny(set), context, "task.await_any", waiting)
                }
            }
            "task.await_at" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let index = super::dispatch::index(arg(a, 2)?)?;
                let member = match native_member {
                    Some(task) => Some(task),
                    None => self
                        .tasks
                        .set(id(arg(a, 1)?, "TaskSet")?, scope)?
                        .members
                        .get_mut(index)
                        .and_then(Option::take),
                };
                let Some(task) = member else {
                    // The check precedes the await: the failing operation does not yield.
                    self.tasks.yield_now = false;
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
                for &task in &members {
                    if name == "task.cancel_all" {
                        self.tasks.cancel(task, 0);
                    }
                    self.tasks.detach(task)?;
                }
                self.tasks.defer_yield();
                if name == "task.cancel_all" {
                    self.drain(members, vec![], context)
                } else {
                    drained(vec![])
                }
            }
            "task.check_canceled" => {
                id(arg(a, 0)?, "Scope")?;
                OperationResult::returned(match self.free_canceled()? {
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
                // Free mode: a scope exit is a waiting op.
                let free = match self.free_enter_call(context)? {
                    super::free::Enter::Off => false,
                    super::free::Enter::Park(park) => return Ok(park),
                    super::free::Enter::Run => true,
                };
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
                let joined = self.join_scope(pending, context)?;
                // A scope exit that waits for its children parks: a scheduling point that is
                // not a yield, as the native exit's `block`.
                if matches!(joined, OperationResult::Park { .. })
                    && let Some(trace) = &mut self.simulation.root_trace
                {
                    trace.point(self.tasks.path());
                }
                if free {
                    self.free_waiting(joined, context)?
                } else {
                    joined
                }
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
        let mut scope_state = self.tasks.scopes.remove(&scope).unwrap();
        let bounded = if self.free.is_some() {
            if let Some(deadline) = scope_state.deadline.take() {
                self.tasks.scopes.insert(
                    scope,
                    ScopeState {
                        owner: scope_state.owner,
                        deadline: Some(deadline),
                        ..Default::default()
                    },
                );
                self.tasks
                    .closing_deadlines
                    .insert(self.tasks.current, scope);
            }
            None
        } else {
            scope_state.deadline.as_ref().map(|d| d.expired)
        };
        let mut completed: Vec<_> = scope_state
            .failures
            .into_iter()
            .map(|(path, failures)| {
                (
                    path,
                    None,
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
                completed.push((child.path, Some((id, child.canceled)), result));
            }
        }
        completed.sort_by(|a, b| a.0.cmp(&b.0));
        // Results nobody awaited: each holding child drops its own in its rest, the last
        // spawned first.
        let mut rests = vec![];
        for (path, id, child) in completed {
            match child {
                OperationResult::Complete { value, .. } => match id {
                    Some((id, canceled)) if value != Value::Unit || self.free.is_some() => {
                        rests.push((id, path, canceled, value))
                    }
                    _ => released.push(value),
                },
                OperationResult::FailureChain {
                    failures: chain, ..
                } => join_failure(&mut result, &mut released, chain),
                _ => return Err(invalid("unfinished child result")),
            }
        }
        for (id, path, canceled, value) in rests.into_iter().rev() {
            self.tasks
                .rest(id, scope, path, canceled, vec![value], true);
        }
        self.tasks
            .frameless
            .retain(|id| self.tasks.tasks.contains_key(id) || self.tasks.resting.contains_key(id));
        if !self.tasks.closing_deadlines.values().any(|&id| id == scope) {
            self.tasks.scope_canceled.remove(&scope);
        }
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
    pub(super) fn record_join(
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
        if self.free.is_some() && matches!(result, OperationResult::Complete { .. }) {
            let child = &self.tasks.tasks[&task];
            let (scope, path, canceled) = (child.scope, child.path.clone(), child.canceled);
            // A moved result still has an empty native cleanup rest. Its actual
            // completion is a separate Free op, just as for a discarded result.
            self.tasks.rest(task, scope, path, canceled, vec![], false);
            self.tasks.defer_yield();
        }
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
            Pending::AwaitAny(set) => match {
                let live = self.tasks.first_completed(set);
                self.free_any(live, set)?
            } {
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
        self.tasks.cancellations += 1;
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
/// Join a failed child's `chain` to a scope exit's `result`, the earliest spawn path leading
/// (FAIL-2); a body result the failure displaces is `released`.
fn join_failure(
    result: &mut OperationResult,
    released: &mut Vec<Value>,
    chain: Vec<FailureRecord>,
) {
    if let OperationResult::FailureChain {
        failures: primary, ..
    } = result
    {
        if chain.first().map(|f| &f.spawn_path) < primary.first().map(|f| &f.spawn_path) {
            let mut earlier = chain;
            append_chain(&mut earlier, std::mem::take(primary));
            *primary = earlier;
        } else {
            append_chain(primary, chain);
        }
    } else {
        let displaced = std::mem::replace(
            result,
            OperationResult::FailureChain {
                failures: chain,
                released: vec![],
            },
        );
        if let OperationResult::Complete { value, .. } = displaced {
            released.push(value);
        }
    }
}
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
    let Some(
        Value::Box(mut boxed)
        | Value::ArenaBox {
            fields: mut boxed, ..
        },
    ) = fields.pop().flatten()
    else {
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
    use hirsel_types::{Type, TypeInterner, TypeKind};

    fn call(
        library: &mut Library,
        name: &str,
        arguments: &[Value],
        cx: &mut OperationContext<'_>,
    ) -> OperationResult {
        let runtime = crate::aggregate::RuntimeTypes::new(TypeInterner::new(None));
        let unit = runtime
            .types
            .intern(Type::with_kind(TypeKind::Tuple(vec![].into())));
        library
            .task_call(
                name,
                &OperationCall {
                    runtime: &runtime,
                    path: &DefPath::core(name),
                    arguments: &mut arguments.to_vec(),
                    generics: &[],
                    types: &runtime.types,
                    result_type: unit,
                    site: Span::default(),
                },
                cx,
            )
            .unwrap()
    }

    #[test]
    fn free_deadline_outcome_waits_for_called_cleanup_and_substitutes_on_replay() {
        use hirsel_free::session::{Recording, Session};
        for expired in [false, true] {
            let mut tape: Option<Vec<u8>> = None;
            for replaying in [false, true] {
                let run = RunConfiguration {
                    entry: DefPath::core("main"),
                    hash_seed: 0,
                    inputs: vec![],
                };
                let sources = SourceMap::default();
                let anchor = Anchor {
                    definition: (run.entry.clone()).into(),
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
                let session = if replaying {
                    Session::replay(tape.as_ref().unwrap()).unwrap()
                } else {
                    Session::record(b"scope rest deadline", Recording::default())
                };
                library.enable_free(session, 1);
                let scope = library.tasks.fresh();
                library.tasks.scopes.insert(
                    scope,
                    ScopeState {
                        deadline: Some(ScopeDeadline {
                            at: 100,
                            expired: false,
                            anchor: Some(anchor.clone()),
                        }),
                        ..Default::default()
                    },
                );
                let closure = Value::Closure {
                    definition: (&DefPath::core("child")).into(),
                    body: BodyId(0),
                    invoked: Some(Default::default()),
                    captures: vec![],
                    environment: Box::new(crate::value::Environment {
                        storage: None,
                        capture_types: vec![],
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
                let child = id(&child, "Task").unwrap();
                library.tasks.current = child;
                crate::Operations::task_finished(
                    &mut library,
                    child,
                    OperationResult::returned(u64_value(7)),
                    &mut cx,
                )
                .unwrap();
                library.tasks.current = 0;
                library.free_enter_call(&cx).unwrap();
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
                let mut result = library.free_waiting(joined, &cx).unwrap();
                assert!(
                    matches!(
                        result,
                        OperationResult::Complete {
                            value: Value::Unit,
                            ..
                        }
                    ),
                    "no bounded wrapper before child cleanup"
                );
                assert!(library.deadline_pending(scope));
                let rests = crate::Operations::take_rests(&mut library, 0);
                assert_eq!(rests.len(), 1);
                if expired != replaying {
                    library.expire_scope(scope, &mut cx).unwrap();
                }
                library.tasks.current = child;
                crate::Operations::rest_finished(&mut library, child, vec![]).unwrap();
                library.tasks.current = 0;
                library.free_rests_completed(0, &mut result, &cx).unwrap();
                if replaying {
                    // Done at the cut parks before the caller's next segment.
                    assert!(matches!(result, OperationResult::Park { .. }));
                    library.take_free().unwrap().finish().unwrap();
                } else {
                    assert!(
                        matches!(result, OperationResult::Complete { value: Value::Variant { variant, .. }, .. } if variant == u32::from(expired))
                    );
                    assert!(!library.tasks.scopes.contains_key(&scope));
                    library.free_stop("returned");
                    tape = Some(library.take_free().unwrap().report().tape);
                }
            }
        }
    }

    #[test]
    fn provider_owned_child_markers_end_with_task_or_cleanup_rest() {
        for free in [false, true] {
            for detach_first in [false, true] {
                let mut tasks = Tasks::default();
                tasks.free_rests = free;
                tasks.scopes.insert(1, ScopeState::default());
                let child = tasks.native_child(1).unwrap();
                assert!(tasks.frameless.contains(&child));
                if detach_first {
                    tasks.detach(child).unwrap();
                }
                tasks
                    .finished(child, OperationResult::returned(Value::Unit))
                    .unwrap();
                if !detach_first {
                    tasks.detach(child).unwrap();
                }
                let rests = tasks.take_rests(0);
                assert_eq!(rests.len(), usize::from(free && !detach_first));
                if !rests.is_empty() {
                    assert!(tasks.frameless.contains(&child));
                    tasks.rest_finished(child, vec![]).unwrap();
                }
                assert!(tasks.frameless.is_empty());
                assert!(tasks.tasks.is_empty());
                assert!(tasks.resting.is_empty());
            }
        }
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
            definition: (run.entry.clone()).into(),
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
                    storage: None,
                    capture_types: vec![],
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
                    storage: None,
                    capture_types: vec![],
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
