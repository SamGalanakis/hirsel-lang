//! Structured tasks (CONC-1, CONC-2, FAIL-2): the native scheduler of
//! `hirsel-rt/src/tasks.rs`, with each task's activity stack in place of its
//! fiber, so H observes the interleaving native code and the interpreter do.
//!
//! A switch is requested inside an operation and made once the running frame
//! is back on its stack ([`Machine::finish_switch`]). A task that blocks in a
//! task operation leaves its call terminator unexecuted and records where it
//! stopped ([`Wait`]); the terminator runs again when the task resumes and
//! completes the operation. A task that must wait parks with what it waits for
//! ([`Park`]); parked tasks are polled in park order at every yield and
//! whenever the running task stops, as the runtime polls them, and a task due
//! to park after its operation's yield parks when it reaches the front. What the runtime keeps per task in `TaskCtx` H
//! keeps per task here: its failure chain, with the spawn path of each
//! failure a child contributed. A child's closure is a harness root while the
//! scheduler holds it, and its environment is released after its frame
//! returns, as the runtime releases it.
use super::clock::{CANCEL_DEADLINE, ScopeDeadline, Suspended};
use super::*;
use std::collections::VecDeque;

/// `CancelCause` ordinals (core library).
const CANCEL_REQUESTED: u32 = 0;
const CANCEL_PARENT: u32 = 1;
const CANCEL_CHILD_FAILED: u32 = 2;
/// Harness root keys of the closures the scheduler holds.
const MAIN_ROOT: u64 = 1 << 62;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Ready,
    Parked,
    Holding,
    Done,
}

/// Where a task stopped inside a task operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Wait {
    /// A join of `child`, parked until a poll found it finished.
    Join(u32),
    /// A join that handed off to holding `child`; it completes once the child completed,
    /// with the operation's yield when it had not parked.
    JoinHanded(u32, bool),
    /// A scope exit after its first part: waiting for, or discarding, children.
    Exit(u32),
    /// A finished child holding its result until a join or a discard resumes it.
    Finish,
    /// A detach that handed off to a holding child.
    Detach(u32),
    /// A receive, parked until a poll found a message, a closed channel or a cancellation.
    Receive,
    /// A suspended operation (a timer wait, or a wait on a stalled scripted source), parked
    /// until a poll ended it; its completion is the task's [`Suspended`].
    Suspended,
    /// `task.await_any` on task set `set`, parked until a member completed.
    Any(u32),
    /// `task.cancel_all` or `task.detach_all` releasing task set `set`'s members from `next`,
    /// after handing off to a holding one.
    Release { set: u32, next: usize },
    /// `task.cancel_all`, parked until every member of task set `set` completed.
    Drain(u32),
}

/// What a parked task waits for; its poll performs the wait's effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Park {
    /// A join of `child`, which moves its result to `slot`.
    Join { child: u32, slot: Ptr },
    /// A scope exit, until every child of `scope` holds its result or completed.
    Exit(u32),
    /// A receive on `channel`, which moves a message into `slot` (CONC-6).
    Receive {
        channel: u32,
        slot: Ptr,
        layout: LayoutId,
    },
    /// A timer wait due at `until` (`clock.sleep_until`).
    Timer { until: u64 },
    /// A wait on a stalled scripted peer or child, which only its source or a cancellation ends.
    Pending,
    /// `task.await_any`, until a member of task set `set` holds its result or failed.
    Any(u32),
    /// `task.cancel_all`, until every member of task set `set` completed.
    Drain(u32),
}

/// A task set (CONC-2): its scope and its members' Task words in add order, `None` once
/// awaited.
#[derive(Clone)]
struct TaskSet {
    generation: u32,
    scope: u32,
    members: Vec<Option<u64>>,
}

/// Task set words are channel endpoint words with this bit too, as the runtime makes them.
pub(super) const TASK_SET: u64 = 1 << 62;

/// A holding child's result: its slot, the slot's layout and its `None` image.
#[derive(Clone)]
struct Slot {
    at: Ptr,
    layout: LayoutId,
    none: Extent,
}

#[derive(Clone)]
pub(super) struct Task {
    cpu_scoped: bool,
    generation: u32,
    stack: Vec<Activity>,
    failures: Vec<ObservedFailure>,
    /// Per failure, the spawn path of the child it came from (None: this task's own).
    spawns: Vec<Option<Vec<u32>>>,
    unanchored: std::collections::BTreeSet<usize>,
    pub(super) isolations: Vec<super::isolate::Invocation>,
    state: State,
    scope: Option<u32>,
    pub(super) path: Vec<u32>,
    spawned: u32,
    canceled: Option<u32>,
    scopes: Vec<u32>,
    pub(super) wait: Option<Wait>,
    /// What the task waits for while parked, or once at the front (`parking`).
    park: Option<Park>,
    /// Queued after its operation's yield, to park when it reaches the front.
    parking: bool,
    /// The status a receive's poll produced.
    pub(super) status: u32,
    slot: Option<Slot>,
    handoff: Option<u32>,
    detached: bool,
    /// The completed frame failed, or the environment's release did.
    failed: bool,
    /// The frame returned and the environment's release is running.
    exited: bool,
    chain: Option<Vec<(ObservedFailure, Vec<u32>)>>,
    /// The layout of the closure the scheduler holds for this child.
    main: Option<LayoutId>,
    entry_site: Option<CallerSite>,
    /// Its place in completion order, set when it holds its result or failed.
    completed: u64,
}

impl Task {
    fn new(generation: u32, scope: Option<u32>, path: Vec<u32>, canceled: Option<u32>) -> Task {
        Task {
            cpu_scoped: false,
            generation,
            stack: Vec::new(),
            failures: Vec::new(),
            spawns: Vec::new(),
            unanchored: Default::default(),
            isolations: Vec::new(),
            state: State::Ready,
            scope,
            path,
            spawned: 0,
            canceled,
            scopes: Vec::new(),
            wait: None,
            park: None,
            parking: false,
            status: 0,
            slot: None,
            handoff: None,
            detached: false,
            failed: false,
            exited: false,
            chain: None,
            main: None,
            entry_site: None,
            completed: 0,
        }
    }
}

#[derive(Clone)]
struct Scope {
    generation: u32,
    /// The task that entered it.
    owner: u32,
    children: Vec<u32>,
    canceled: Option<u32>,
    /// `task.scope_until`'s deadline (CONC-3).
    deadline: Option<ScopeDeadline>,
}

#[derive(Clone)]
pub(super) struct Tasks {
    cpu: Option<u32>,
    tasks: Vec<Option<Task>>,
    scopes: Vec<Option<Scope>>,
    generation: u32,
    pub(super) current: u32,
    runnable: VecDeque<u32>,
    /// Parked tasks, in park order.
    parked: Vec<u32>,
    pub(super) channels: super::channels::Channels,
    pub(super) sync: super::sync::Synchronizers,
    /// The switch the running operation requested, and whether the running
    /// task completed (its stack is not kept).
    switch: Option<(u32, bool)>,
    /// The running task's spawn path per failure (`Task::spawns` while it runs).
    spawns: Vec<Option<Vec<u32>>>,
    /// Each task's suspended operation.
    pub(super) suspended: std::collections::BTreeMap<u32, Suspended>,
    /// The live scopes' deadlines that have not fired, as (deadline, scope): by deadline, ties
    /// in entry order.
    armed: std::collections::BTreeSet<(u64, u32)>,
    sets: Vec<Option<TaskSet>>,
    /// Children that held their result or failed so far: the next one's completion order.
    completions: u64,
}

fn handle(index: u32, generation: u32) -> u64 {
    u64::from(generation) << 32 | u64::from(index)
}

/// The task operation `id` names, if it is one.
pub(super) fn operation(id: &hirsel_core_contract::OperationId) -> Option<&'static str> {
    let path = &id.definition.0;
    if !path.package.name_is("core") {
        return None;
    }
    let name = path
        .modules
        .iter()
        .chain(std::iter::once(&path.name))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(".");
    [
        "$intrinsic.CpuMapScoped",
        "$intrinsic.TaskScopeEnter",
        "$intrinsic.TaskScopeEnterUntil",
        "$intrinsic.TaskScopeExit",
        "$intrinsic.TaskScopeExitUntil",
        "$intrinsic.TaskSpawn",
        "$intrinsic.TaskFinish",
        "$intrinsic.TaskJoin",
        "$intrinsic.TaskCanceled",
        "task.cancel",
        "task.detach",
        "task.set",
        "task.add",
        "task.await_any",
        "$intrinsic.TaskSetHas",
        "$intrinsic.TaskSetTake",
        "task.cancel_all",
        "task.detach_all",
        "$intrinsic.ChannelOpen",
        "sender.dup",
        "$intrinsic.ChannelSend",
        "$intrinsic.ChannelReceive",
        "$intrinsic.MutexCreate",
        "$intrinsic.MutexTake",
        "$intrinsic.MutexPut",
        "$intrinsic.MutexKill",
        "$intrinsic.MutexCause",
        "$intrinsic.AtomicCreate",
        "$intrinsic.AtomicLoad",
        "$intrinsic.AtomicStore",
        "$intrinsic.SharedThawTurn",
        "$intrinsic.IsolateBegin",
        "$intrinsic.IsolateEnd",
        "$intrinsic.IsolateCount",
        "$intrinsic.IsolateKind",
        "$intrinsic.IsolateRule",
        "$intrinsic.IsolateDefinition",
        "$intrinsic.IsolateLine",
        "$intrinsic.IsolateColumn",
        "$intrinsic.IsolateSpawn",
    ]
    .into_iter()
    .find(|n| *n == name)
}

impl Tasks {
    fn new() -> Tasks {
        Tasks {
            cpu: None,
            tasks: vec![Some(Task::new(1, None, Vec::new(), None))],
            scopes: Vec::new(),
            generation: 1,
            current: 0,
            runnable: VecDeque::new(),
            parked: Vec::new(),
            channels: Default::default(),
            sync: Default::default(),
            suspended: Default::default(),
            armed: Default::default(),
            switch: None,
            spawns: Vec::new(),
            sets: Vec::new(),
            completions: 0,
        }
    }
    /// `task` holds its result or failed: its place in completion order.
    fn order(&mut self, task: u32) -> Checked<()> {
        let order = self.completions;
        self.completions += 1;
        self.task(task)?.completed = order;
        Ok(())
    }
    /// The task set a TaskSet word names, which must belong to `scope`.
    fn set_handle(&self, word: u64, scope: u32) -> Checked<u32> {
        let index = word as u32;
        match self.sets.get(index as usize).and_then(Option::as_ref) {
            Some(set)
                if word & TASK_SET != 0
                    && set.generation
                        == ((word & !(super::channels::ENDPOINT | TASK_SET)) >> 32) as u32
                    && set.scope == scope =>
            {
                Ok(index)
            }
            _ => fail(
                V::Malformed,
                "a TaskSet handle of another scope or a closed set",
            ),
        }
    }
    fn set(&mut self, index: u32) -> Checked<&mut TaskSet> {
        self.sets
            .get_mut(index as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| err(V::Malformed, "a vacant task set row"))
    }
    /// The row a member's Task word names while the child has one.
    fn member(&self, word: u64) -> Option<&Task> {
        self.tasks
            .get(word as u32 as usize)
            .and_then(Option::as_ref)
            .filter(|t| t.generation == (word >> 32) as u32)
    }
    /// The add-order index of the unawaited member of `set` that completed first, if one has.
    fn first_completed(&self, set: u32) -> Option<u64> {
        self.sets[set as usize]
            .as_ref()?
            .members
            .iter()
            .enumerate()
            .filter_map(|(i, word)| {
                let row = self.member((*word)?)?;
                matches!(row.state, State::Holding | State::Done)
                    .then_some((row.completed, i as u64))
            })
            .min()
            .map(|(_, i)| i)
    }
    /// Whether every unawaited member of `set` has completed.
    fn drained(&self, set: u32) -> bool {
        self.sets[set as usize].as_ref().is_some_and(|row| {
            row.members
                .iter()
                .flatten()
                .all(|&word| self.member(word).is_none_or(|t| t.state == State::Done))
        })
    }
    pub(super) fn task(&mut self, index: u32) -> Checked<&mut Task> {
        self.tasks
            .get_mut(index as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| err(V::Malformed, "a vacant task row"))
    }
    fn scope(&mut self, index: u32) -> Checked<&mut Scope> {
        self.scopes
            .get_mut(index as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| err(V::Malformed, "a vacant scope row"))
    }
    fn fresh_generation(&mut self) -> u32 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.generation
    }
    fn scope_handle(&self, word: u64) -> Checked<u32> {
        let index = word as u32;
        match self.scopes.get(index as usize).and_then(Option::as_ref) {
            Some(scope) if scope.generation == (word >> 32) as u32 => Ok(index),
            _ => fail(V::Malformed, "a Scope handle outside its callback"),
        }
    }
    fn task_handle(&self, word: u64, scope: u32) -> Checked<u32> {
        let index = word as u32;
        match self.tasks.get(index as usize).and_then(Option::as_ref) {
            Some(task) if task.generation == (word >> 32) as u32 && task.scope == Some(scope) => {
                Ok(index)
            }
            _ => fail(
                V::Malformed,
                "a Task handle of another scope or a completed run",
            ),
        }
    }
    /// The running task's cancellation, none while it is `shielded` ([`Machine::shielded`]).
    pub(super) fn canceled(&self, shielded: bool) -> Option<u32> {
        self.canceled_of(self.current, shielded)
    }
    /// `task`'s cancellation, none while it is `shielded`.
    pub(super) fn canceled_task(&self, task: u32, shielded: bool) -> Checked<Option<u32>> {
        if self.tasks.get(task as usize).is_none_or(Option::is_none) {
            return fail(V::Malformed, "a vacant task row");
        }
        Ok(self.canceled_of(task, shielded))
    }
    fn canceled_of(&self, task: u32, shielded: bool) -> Option<u32> {
        if shielded {
            return None;
        }
        let task = self.tasks[task as usize].as_ref()?;
        task.canceled.or_else(|| {
            task.scopes
                .iter()
                .rev()
                .find_map(|&s| self.scopes[s as usize].as_ref().and_then(|s| s.canceled))
        })
    }
    fn cancel(&mut self, task: u32, cause: u32) -> Checked<()> {
        let path = self.task(task)?.path.clone();
        for row in self.tasks.iter_mut().flatten() {
            if row.path.starts_with(&path) && row.scope.is_some() {
                row.canceled.get_or_insert(if row.path.len() == path.len() {
                    cause
                } else {
                    CANCEL_PARENT
                });
            }
        }
        Ok(())
    }
    fn settled(&self, scope: u32) -> bool {
        self.scopes[scope as usize].as_ref().is_some_and(|s| {
            s.children.iter().all(|&c| {
                self.tasks[c as usize]
                    .as_ref()
                    .is_some_and(|t| matches!(t.state, State::Holding | State::Done))
            })
        })
    }
    /// `scope`'s deadline fired (CONC-3): the scope is canceled `DeadlineExceeded`, as is each
    /// of its children, and the children of the scopes its owner entered inside it `Parent`.
    pub(super) fn expire(&mut self, scope: u32) -> Checked<()> {
        let row = self.scope(scope)?;
        let Some(deadline) = &mut row.deadline else {
            return fail(V::Malformed, "a scope without a deadline expired");
        };
        if deadline.expired {
            return Ok(());
        }
        deadline.expired = true;
        let at = deadline.at;
        row.canceled.get_or_insert(CANCEL_DEADLINE);
        let (owner, children) = (row.owner, row.children.clone());
        self.armed.remove(&(at, scope));
        for child in children {
            self.cancel(child, CANCEL_DEADLINE)?;
        }
        let inner: Vec<u32> = self
            .task(owner)?
            .scopes
            .iter()
            .skip_while(|&&s| s != scope)
            .skip(1)
            .copied()
            .collect();
        for inner in inner {
            for child in self.scope(inner)?.children.clone() {
                self.cancel(child, CANCEL_PARENT)?;
            }
        }
        Ok(())
    }
    /// Each live scope's deadline that has not fired, by deadline with ties in entry order.
    pub(super) fn armed(&self) -> impl Iterator<Item = (u32, ScopeDeadline)> + '_ {
        self.armed.iter().filter_map(|&(_, index)| {
            Some((index, self.scopes[index as usize].as_ref()?.deadline?))
        })
    }
    /// Park the running task on `park`.
    fn park_current(&mut self, park: Park) -> Checked<()> {
        let current = self.current;
        let row = self.task(current)?;
        row.state = State::Parked;
        row.park = Some(park);
        self.parked.push(current);
        Ok(())
    }
    /// Resume holding `child` directly; the running task parks until it completes.
    fn hand_off(&mut self, child: u32) -> Checked<()> {
        let current = self.current;
        self.task(current)?.state = State::Parked;
        let row = self.task(child)?;
        row.state = State::Ready;
        row.handoff = Some(current);
        self.switch = Some((child, false));
        Ok(())
    }
    pub(super) fn task_spawns(&mut self) -> &mut Vec<Option<Vec<u32>>> {
        &mut self.spawns
    }
    fn vacate(&mut self, task: u32) {
        self.tasks[task as usize] = None;
    }
}

impl<S: Stage> Machine<'_, S> {
    pub(super) fn scheduler(&mut self) -> &mut Tasks {
        self.tasks.get_or_insert_with(|| Box::new(Tasks::new()))
    }

    /// The handle word of a `Scope` or `Task` argument, by value or by address.
    pub(super) fn task_word(&mut self, f: &Frame, argument: &CallArgument) -> Checked<u64> {
        let val = match argument.value {
            ArgumentValue::Value(v) => self.read_use(f, v)?,
            ArgumentValue::Address { address, .. } => {
                let r = self.resolve_use(f, address)?;
                let size = self.size(r.layout)?;
                let image = self
                    .ledger
                    .check(r.ptr)?
                    .extent
                    .copy(r.ptr.offset, size, None);
                let repr = self
                    .lir_type(f.unit, self.unit(f.unit).addresses[address.index()].ty)?
                    .repr;
                self.val_from(image, repr, r.layout)?.0
            }
        };
        match val {
            Val::Handle(word) => Ok(word),
            Val::Ptr(ptr) => Ok(self.scan_scalar(ptr, 8)? as u64),
            Val::Bits { bits, .. } => Ok(bits as u64),
            _ => fail(V::TypeConfusion, "a task handle that is not a word"),
        }
    }

    /// The argument of declared parameter `ordinal`.
    pub(super) fn task_argument(arguments: &[CallArgument], ordinal: u32) -> Checked<CallArgument> {
        arguments
            .iter()
            .find(|a| a.parameter == ordinal)
            .copied()
            .ok_or_else(|| err(V::Malformed, "a task operation argument is missing"))
    }

    /// A `mut` argument's place, and the version its write-back defines.
    pub(super) fn task_slot(&mut self, f: &Frame, argument: &CallArgument) -> Checked<Resolved> {
        let ArgumentValue::Address { address, .. } = argument.value else {
            return fail(V::Malformed, "a result slot passed by value");
        };
        let r = self.resolve_use(f, address)?;
        self.check_writable(&r)?;
        Ok(r)
    }

    /// Finish a task operation normally: its `mut` arguments' versions, its result.
    pub(super) fn task_return(
        &mut self,
        f: &mut Frame,
        arguments: &[CallArgument],
        result: Option<(ValueId, Val)>,
        normal: Option<EdgeId>,
    ) -> Checked<Control> {
        self.task_written(f, arguments)?;
        if let Some((value, val)) = result {
            self.define_fresh(f, value, val)?;
        }
        self.jump(f, returning(normal)?)?;
        Ok(Control::Continue)
    }

    fn task_written(&mut self, f: &mut Frame, arguments: &[CallArgument]) -> Checked<()> {
        for arg in arguments {
            if arg.passing == hirsel_core_contract::Passing::Mut
                && let ArgumentValue::Address { address, version } = arg.value
                && let Some(slot) = self.resolve_use(f, address)?.root_slot
            {
                f.versions[slot.index()] = Some(version);
                self.define_version(f, version)?;
            }
        }
        Ok(())
    }

    /// Fail a task operation along `failure` with the running task's chain.
    fn task_fail(
        &mut self,
        f: &mut Frame,
        arguments: &[CallArgument],
        failure: FailureId,
    ) -> Checked<Control> {
        self.task_written(f, arguments)?;
        self.fail_at(f, failure, false)
    }

    /// Move a holding child's `Some` image into `out` and leave its `None` behind.
    fn deliver(&mut self, slot: Slot, out: Ptr) -> Checked<()> {
        let size = self.size(slot.layout)?;
        let owned = self.resources_of(slot.layout)?;
        // The awaiter's slot holds `None`: plain bits with nothing to release.
        self.take_part(out, size, &owned);
        let image = self.take_part(slot.at, size, &owned);
        let (val, _, credited) = self.val_from(image, ValueRepr::Aggregate, slot.layout)?;
        self.write_value(
            out,
            slot.layout,
            Moved {
                val,
                credited,
                borrow: None,
            },
        )?;
        self.write_value(
            slot.at,
            slot.layout,
            Moved {
                val: Val::Image(slot.none),
                credited: false,
                borrow: None,
            },
        )
    }

    /// The running task's failures, each attributed to the task it occurred in.
    fn take_chain(&mut self, own: &[u32]) -> Vec<(ObservedFailure, Vec<u32>)> {
        let failures = std::mem::take(&mut self.failures);
        self.unanchored_failures.clear();
        let spawns = std::mem::take(self.scheduler().task_spawns());
        failures
            .into_iter()
            .zip(spawns.into_iter().chain(std::iter::repeat(None)))
            .map(|(f, spawn)| (f, spawn.unwrap_or_else(|| own.to_vec())))
            .collect()
    }

    /// The running task's failures, as [`Self::take_chain`] gives them, kept (FAIL-4: a dead
    /// mutex keeps the chain of the body that killed it).
    pub(super) fn chain(&mut self, own: &[u32]) -> Vec<(ObservedFailure, Vec<u32>)> {
        let failures = self.failures.clone();
        let spawns = self.scheduler().task_spawns().clone();
        failures
            .into_iter()
            .zip(spawns.into_iter().chain(std::iter::repeat(None)))
            .map(|(f, spawn)| (f, spawn.unwrap_or_else(|| own.to_vec())))
            .collect()
    }

    /// Continue the running task's chain with a child's (a join).
    pub(super) fn append_chain(&mut self, chain: Vec<(ObservedFailure, Vec<u32>)>) {
        for (failure, spawn) in chain {
            let len = self.failures.len();
            let spawns = self.scheduler().task_spawns();
            spawns.resize(len, None);
            spawns.push(Some(spawn));
            self.failures.push(failure);
        }
    }

    /// Merge a child's chain at its scope's exit: the earlier first spawn path leads (FAIL-2).
    fn merge_chain(&mut self, own: &[u32], chain: Vec<(ObservedFailure, Vec<u32>)>) {
        let Some((_, first)) = chain.first() else {
            return;
        };
        let empty = self.failures.is_empty();
        let leading = match self.scheduler().task_spawns().first() {
            _ if empty => true,
            None => first.as_slice() < own,
            Some(spawn) => first.as_slice() < spawn.as_deref().unwrap_or(own),
        };
        if leading {
            let unanchored = std::mem::take(&mut self.unanchored_failures);
            let earlier = self.take_chain(own);
            self.append_chain(chain);
            let offset = self.failures.len();
            self.append_chain(earlier);
            self.unanchored_failures = unanchored.into_iter().map(|i| i + offset).collect();
        } else {
            self.append_chain(chain);
        }
    }

    pub(super) fn task_operation(
        &mut self,
        f: &mut Frame,
        site: &CallSite,
        name: &str,
        normal: Option<EdgeId>,
        failure: FailureId,
    ) -> Checked<Control> {
        let arguments = self
            .pool(site.arguments, &self.unit(f.unit).pools.call_arguments)?
            .to_vec();
        // A task operation the running task starts, not one it resumes, is a timer choice point.
        let resumed = self.tasks.as_deref().is_some_and(|s| {
            s.tasks[s.current as usize]
                .as_ref()
                .is_some_and(|t| t.wait.is_some())
        });
        if !resumed {
            self.timer_choice()?;
        }
        match name {
            "$intrinsic.CpuMapScoped" => {
                let width = self.task_word(f, &Self::task_argument(&arguments, 1)?)? as u32;
                let action = self.task_word(f, &Self::task_argument(&arguments, 2)?)? as u32;
                let count = self.task_word(f, &Self::task_argument(&arguments, 3)?)?;
                let shielded = self.shielded(f);
                let grant = self.config.cpu_max_workers;
                let free = self.config.cpu_free;
                let scheduler = self.scheduler();
                let current = scheduler.current;
                let member = scheduler.task(current)?.cpu_scoped;
                let canceled = scheduler.canceled(shielded);
                let status = match action {
                    0 | 4 => {
                        if !matches!(width, 1 | 4 | 8 | 16) {
                            1
                        } else if grant.is_none() {
                            2
                        } else if member {
                            5
                        } else if !free && action != 4 {
                            11
                        } else if let Some(cause) = canceled {
                            16 + cause
                        } else if grant.is_some_and(|max| width > max) {
                            4
                        } else if scheduler.cpu.is_some() {
                            3
                        } else {
                            scheduler.cpu = Some(width);
                            scheduler.task(current)?.cpu_scoped = true;
                            0
                        }
                    }
                    1 => {
                        scheduler.cpu = None;
                        scheduler.task(current)?.cpu_scoped = false;
                        canceled.map_or(0, |cause| 16 + cause)
                    }
                    2 | 3 | 5 => {
                        if scheduler.cpu != Some(width) {
                            return fail(V::Malformed, "CPU receipt without an admitted batch");
                        }
                        if (action == 2 && count > u64::from(width) * 2)
                            || (action != 2 && count >= u64::from(width) * 2)
                        {
                            6
                        } else {
                            canceled.map_or(0, |cause| 16 + cause)
                        }
                    }
                    _ => return fail(V::Malformed, "unknown CPU control action"),
                };
                if action != 5 || free {
                    self.task_yield()?;
                }
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "CPU control without a result"))?;
                self.task_return(
                    f,
                    &arguments,
                    Some((
                        result,
                        Val::Bits {
                            scalar: ScalarKind::U32,
                            bits: u128::from(status),
                        },
                    )),
                    normal,
                )
            }
            "$intrinsic.ChannelOpen"
            | "sender.dup"
            | "$intrinsic.ChannelSend"
            | "$intrinsic.ChannelReceive" => {
                self.channel_operation(f, site, name, &arguments, normal)
            }
            name if name.starts_with("$intrinsic.Mutex")
                || name.starts_with("$intrinsic.Atomic") =>
            {
                self.sync_operation(f, site, name, &arguments, normal)
            }
            "$intrinsic.TaskCanceled" => {
                let shielded = self.shielded(f);
                let status = self
                    .scheduler()
                    .canceled(shielded)
                    .map_or(0, |cause| cause + 1);
                self.task_yield()?;
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a cancellation check without a status"))?;
                let val = Val::Bits {
                    scalar: ScalarKind::U32,
                    bits: u128::from(status),
                };
                self.task_return(f, &arguments, Some((result, val)), normal)
            }
            name if name.starts_with("$intrinsic.Isolate") => {
                self.isolate_operation(f, site, name, &arguments, normal)
            }
            "$intrinsic.TaskScopeEnter" | "$intrinsic.TaskScopeEnterUntil" => {
                // The deadline's clock handle, then its Instant's ticks.
                let deadline = match name {
                    "$intrinsic.TaskScopeEnterUntil" => {
                        let (clock, at) =
                            self.deadline_words(f, &Self::task_argument(&arguments, 1)?)?;
                        let now = self.ops.monotonic(clock).ok_or_else(|| {
                            err(
                                V::Malformed,
                                "a scope deadline on a clock the model did not grant",
                            )
                        })?;
                        Some((
                            ScopeDeadline {
                                clock,
                                at,
                                expired: false,
                            },
                            at <= now,
                        ))
                    }
                    _ => None,
                };
                let s = self.scheduler();
                let generation = s.fresh_generation();
                let current = s.current;
                s.scopes.push(Some(Scope {
                    generation,
                    owner: current,
                    children: Vec::new(),
                    canceled: None,
                    deadline: deadline.map(|(d, _)| d),
                }));
                let index = (s.scopes.len() - 1) as u32;
                s.task(current)?.scopes.push(index);
                if let Some((d, reached)) = deadline {
                    s.armed.insert((d.at, index));
                    // A deadline already reached cancels the scope before its body runs.
                    if reached {
                        s.expire(index)?;
                    }
                }
                self.task_yield()?;
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a scope entry without a result"))?;
                self.task_return(
                    f,
                    &arguments,
                    Some((result, Val::Handle(handle(index, generation)))),
                    normal,
                )
            }
            "$intrinsic.TaskSpawn" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let ArgumentValue::Value(main) = Self::task_argument(&arguments, 1)?.value else {
                    return fail(V::Malformed, "a spawned closure passed by address");
                };
                let main_layout = self.value_type(f.unit, main)?.layout;
                let moved = self.take_owned(f, main, "spawned closure")?;
                let Val::Callable {
                    code: Some(code),
                    env,
                } = moved.val
                else {
                    return fail(V::TypeConfusion, "a spawned task without code");
                };
                let mut frame = self.frame(code, Role::Callee, env);
                let actual = self
                    .caller_sites(f, self.source_range(f, site.anchor), false)
                    .1;
                let header = &self.unit(code).header;
                if header.track_caller || header.origin == UnitOrigin::Library {
                    frame.caller = actual;
                }
                frame.operation = actual;
                self.bind_parameter_slots(&mut frame, &[])?;
                self.bind_captures(&mut frame)?;
                let shielded = self.shielded(f);
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let canceled = s.canceled(shielded).map(|_| CANCEL_PARENT);
                let current = s.current;
                let parent = s.task(current)?;
                let mut path = parent.path.clone();
                path.push(parent.spawned);
                parent.spawned += 1;
                let generation = s.fresh_generation();
                let mut task = Task::new(generation, Some(scope), path, canceled);
                task.cpu_scoped = s.task(s.current)?.cpu_scoped;
                task.main = Some(main_layout);
                task.entry_site = actual;
                task.stack.push(Activity::Frame(Box::new(frame)));
                s.tasks.push(Some(task));
                let index = (s.tasks.len() - 1) as u32;
                s.scope(scope)?.children.push(index);
                s.runnable.push_back(index);
                // The scheduler holds the closure until the child's frame returns.
                self.set_root(
                    MAIN_ROOT | u64::from(index),
                    Some(Held {
                        val: Val::Callable {
                            code: Some(code),
                            env,
                        },
                        credits: u32::from(moved.credited),
                        borrow: None,
                    }),
                );
                self.task_yield()?;
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a spawn without a result"))?;
                self.task_return(
                    f,
                    &arguments,
                    Some((result, Val::Handle(handle(index, generation)))),
                    normal,
                )
            }
            "$intrinsic.TaskFinish" => {
                let s = self.scheduler();
                let current = s.current;
                if s.task(current)?.wait == Some(Wait::Finish) {
                    // A join or a discard resumed the holding child.
                    s.task(current)?.wait = None;
                    return self.task_return(f, &arguments, None, normal);
                }
                let r = self.task_slot(f, &Self::task_argument(&arguments, 0)?)?;
                let ArgumentValue::Value(none) = Self::task_argument(&arguments, 1)?.value else {
                    return fail(V::Malformed, "a None image passed by address");
                };
                let none = match self.read_use(f, none)? {
                    Val::Image(image) => image,
                    _ => return fail(V::TypeConfusion, "a None image that is not an aggregate"),
                };
                let slot = Slot {
                    at: r.ptr,
                    layout: r.layout,
                    none,
                };
                let s = self.scheduler();
                let row = s.task(current)?;
                if row.scope.is_none() {
                    return fail(V::Malformed, "the root task finished as a child");
                }
                if row.detached {
                    return self.task_return(f, &arguments, None, normal);
                }
                row.slot = Some(slot);
                row.state = State::Holding;
                row.wait = Some(Wait::Finish);
                s.order(current)?;
                self.run_next(false)?;
                Ok(Control::Continue)
            }
            "$intrinsic.TaskJoin" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let task = self.task_word(f, &Self::task_argument(&arguments, 1)?)?;
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let child = s.task_handle(task, scope)?;
                let current = s.current;
                match s.task(current)?.wait {
                    None => {
                        let state = s.task(child)?.state;
                        match state {
                            State::Holding => {
                                let held = s.task(child)?.slot.take().ok_or_else(|| {
                                    err(V::Malformed, "a holding child without a slot")
                                })?;
                                let r = self.task_slot(f, &Self::task_argument(&arguments, 2)?)?;
                                self.deliver(held, r.ptr)?;
                                let s = self.scheduler();
                                s.task(current)?.wait = Some(Wait::JoinHanded(child, true));
                                s.hand_off(child)?;
                                return Ok(Control::Continue);
                            }
                            State::Done => self.task_yield()?,
                            State::Ready | State::Parked => {
                                let r = self.task_slot(f, &Self::task_argument(&arguments, 2)?)?;
                                self.scheduler().task(current)?.wait = Some(Wait::Join(child));
                                self.park_until(Park::Join { child, slot: r.ptr }, true)?;
                                return Ok(Control::Continue);
                            }
                        }
                    }
                    Some(Wait::Join(waited)) if waited == child => {
                        // The poll moved a holding child's result; its frame finishes now.
                        if s.task(child)?.state == State::Holding {
                            s.task(current)?.wait = Some(Wait::JoinHanded(child, false));
                            s.hand_off(child)?;
                            return Ok(Control::Continue);
                        }
                        s.task(current)?.wait = None;
                    }
                    Some(Wait::JoinHanded(waited, yields)) if waited == child => {
                        s.task(current)?.wait = None;
                        if yields {
                            self.task_yield()?;
                        }
                    }
                    _ => return fail(V::Malformed, "a task resumed in another operation"),
                }
                let s = self.scheduler();
                let chain = s.task(child)?.chain.take();
                s.scope(scope)?.children.retain(|&c| c != child);
                s.vacate(child);
                match chain {
                    Some(chain) => {
                        self.append_chain(chain);
                        self.task_fail(f, &arguments, failure)
                    }
                    None => self.task_return(f, &arguments, None, normal),
                }
            }
            "$intrinsic.TaskScopeExit" | "$intrinsic.TaskScopeExitUntil" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let error = match self.read_use(
                    f,
                    match Self::task_argument(&arguments, 1)?.value {
                        ArgumentValue::Value(v) => v,
                        _ => return fail(V::Malformed, "a scope exit flag by address"),
                    },
                )? {
                    Val::Bits { bits, .. } => bits != 0,
                    _ => return fail(V::TypeConfusion, "a scope exit flag that is not a bool"),
                };
                let failing = !self.failures.is_empty();
                let s = self.scheduler();
                let index = s.scope_handle(word)?;
                let current = s.current;
                match s.task(current)?.wait {
                    None => {
                        if s.task(current)?.scopes.pop() != Some(index) {
                            return fail(
                                V::Malformed,
                                "a scope exits before a scope entered inside it",
                            );
                        }
                        if failing || error {
                            for child in s.scope(index)?.children.clone() {
                                s.cancel(child, CANCEL_PARENT)?;
                            }
                        }
                        s.task(current)?.wait = Some(Wait::Exit(index));
                    }
                    Some(Wait::Exit(waited)) if waited == index => {}
                    _ => return fail(V::Malformed, "a task resumed in another operation"),
                }
                if !s.settled(index) {
                    // The exit is not a yield.
                    self.park_until(Park::Exit(index), false)?;
                    return Ok(Control::Continue);
                }
                let s = self.scheduler();
                // Discard the results nobody awaited, the last spawned first.
                let children = s.scope(index)?.children.clone();
                for &child in children.iter().rev() {
                    if s.task(child)?.state == State::Holding {
                        s.task(child)?.slot = None;
                        s.hand_off(child)?;
                        return Ok(Control::Continue);
                    }
                }
                s.task(current)?.wait = None;
                let own = s.task(current)?.path.clone();
                for &child in &children {
                    let s = self.scheduler();
                    let chain = s.task(child)?.chain.take();
                    s.vacate(child);
                    if let Some(chain) = chain {
                        self.merge_chain(&own, chain);
                    }
                }
                // An exit removes the scope's deadline; whether it fired is the result.
                let s = self.scheduler();
                let deadline = s.scopes[index as usize].take().and_then(|row| row.deadline);
                if let Some(d) = deadline {
                    s.armed.remove(&(d.at, index));
                }
                let expired = deadline.map(|d| d.expired);
                if self.failures.is_empty() {
                    let result = match (name, expired) {
                        ("$intrinsic.TaskScopeExitUntil", Some(expired)) => {
                            let result = site.result.ok_or_else(|| {
                                err(V::Malformed, "a deadline scope's exit without its flag")
                            })?;
                            Some((
                                result,
                                Val::Bits {
                                    scalar: ScalarKind::Bool,
                                    bits: u128::from(expired),
                                },
                            ))
                        }
                        ("$intrinsic.TaskScopeExit", None) => None,
                        _ => return fail(V::Malformed, "a scope exits as another kind"),
                    };
                    self.task_return(f, &arguments, result, normal)
                } else {
                    self.task_fail(f, &arguments, failure)
                }
            }
            "task.cancel" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let task = self.task_word(f, &Self::task_argument(&arguments, 1)?)?;
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let child = s.task_handle(task, scope)?;
                s.cancel(child, CANCEL_REQUESTED)?;
                self.task_yield()?;
                self.task_return(f, &arguments, None, normal)
            }
            "task.detach" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let task = self.task_word(f, &Self::task_argument(&arguments, 1)?)?;
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let current = s.current;
                match s.task(current)?.wait {
                    None => {
                        let child = s.task_handle(task, scope)?;
                        s.task(child)?.detached = true;
                        if s.task(child)?.state == State::Holding {
                            s.task(child)?.slot = None;
                            s.task(current)?.wait = Some(Wait::Detach(child));
                            s.hand_off(child)?;
                            return Ok(Control::Continue);
                        }
                        if s.task(child)?.state == State::Done && s.task(child)?.chain.is_none() {
                            s.scope(scope)?.children.retain(|&c| c != child);
                            s.vacate(child);
                        }
                    }
                    Some(Wait::Detach(child)) => {
                        s.task(current)?.wait = None;
                        if s.tasks[child as usize]
                            .as_ref()
                            .is_some_and(|t| t.state == State::Done && t.chain.is_none())
                        {
                            s.scope(scope)?.children.retain(|&c| c != child);
                            s.vacate(child);
                        }
                    }
                    _ => return fail(V::Malformed, "a task resumed in another operation"),
                }
                self.task_yield()?;
                // The handle word is taken: nothing of it remains to release.
                self.task_return(f, &arguments, None, normal)
            }
            "task.set" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let generation = s.fresh_generation() & 0x3fff_ffff;
                s.sets.push(Some(TaskSet {
                    generation,
                    scope,
                    members: Vec::new(),
                }));
                let set = super::channels::ENDPOINT
                    | TASK_SET
                    | u64::from(generation) << 32
                    | (s.sets.len() - 1) as u64;
                self.ledger
                    .handles
                    .insert(set, (crate::ledger::HandleState::Open, false));
                self.task_yield()?;
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a task set without a result"))?;
                self.task_return(f, &arguments, Some((result, Val::Handle(set))), normal)
            }
            "task.add" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let set = self.task_word(f, &Self::task_argument(&arguments, 1)?)?;
                let task = self.task_word(f, &Self::task_argument(&arguments, 2)?)?;
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                s.task_handle(task, scope)?;
                let set = s.set_handle(set, scope)?;
                s.set(set)?.members.push(Some(task));
                self.task_yield()?;
                // The handle word is taken: the set holds it now.
                self.task_return(f, &arguments, None, normal)
            }
            "task.await_any" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let set = self.task_word(f, &Self::task_argument(&arguments, 1)?)?;
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let set = s.set_handle(set, scope)?;
                let current = s.current;
                let index = match s.task(current)?.wait {
                    None => {
                        let waiting = s.set(set)?.members.iter().any(Option::is_some);
                        let first = s.first_completed(set);
                        if waiting && first.is_none() {
                            s.task(current)?.wait = Some(Wait::Any(set));
                            self.park_until(Park::Any(set), true)?;
                            return Ok(Control::Continue);
                        }
                        self.task_yield()?;
                        first
                    }
                    Some(Wait::Any(waited)) if waited == set => {
                        s.task(current)?.wait = None;
                        Some(s.first_completed(set).ok_or_else(|| {
                            err(V::Malformed, "await_any resumed with no member completed")
                        })?)
                    }
                    _ => return fail(V::Malformed, "a task resumed in another operation"),
                };
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "await_any without a result"))?;
                let layout = self.value_type(f.unit, result)?.layout;
                let value = match index {
                    Some(index) => OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::Bits {
                            scalar: ScalarKind::U64,
                            bits: u128::from(index),
                        }),
                    },
                    None => OpValue::Variant {
                        tag: 0,
                        value: Box::new(OpValue::Unit),
                    },
                };
                let image =
                    self.operation_image(f, layout, value, &arguments, &mut Default::default())?;
                let repr = self.value_type(f.unit, result)?.repr;
                let val = self.val_from(image, repr, layout)?.0;
                self.task_return(f, &arguments, Some((result, val)), normal)
            }
            // The Lean model's uniqueness test of a `Shared` root, as `Inst::IsUnique` reads
            // it: the sole root. Not a yield outside free mode.
            "$intrinsic.SharedThawTurn" => {
                let root = Self::task_argument(&arguments, 0)?;
                let ArgumentValue::Value(root) = root.value else {
                    return fail(V::Malformed, "a thaw turn's root passed by address");
                };
                let Val::Ptr(p) = self.read_use(f, root)? else {
                    return fail(V::TypeConfusion, "a thaw turn of a non-block");
                };
                let last = matches!(
                    self.ledger.check(p)?.count,
                    Some(Count::Finite(1) | Count::Root(1))
                );
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a thaw turn without a result"))?;
                let val = Val::Bits {
                    scalar: ScalarKind::Bool,
                    bits: u128::from(last),
                };
                self.task_return(f, &arguments, Some((result, val)), normal)
            }
            "$intrinsic.TaskSetHas" | "$intrinsic.TaskSetTake" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let set = self.task_word(f, &Self::task_argument(&arguments, 1)?)?;
                let ArgumentValue::Value(index) = Self::task_argument(&arguments, 2)?.value else {
                    return fail(V::Malformed, "a member index passed by address");
                };
                let index = match self.read_use(f, index)? {
                    Val::Bits { bits, .. } => usize::try_from(bits).ok(),
                    _ => return fail(V::TypeConfusion, "a member index that is not a u64"),
                };
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let set = s.set_handle(set, scope)?;
                let member = index.and_then(|i| s.set(set).ok()?.members.get_mut(i));
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a task set entry without a result"))?;
                // Neither is a yield: they are parts of `task.await_at`, whose join yields.
                let val = if name == "$intrinsic.TaskSetHas" {
                    Val::Bits {
                        scalar: ScalarKind::Bool,
                        bits: u128::from(member.is_some_and(|m| m.is_some())),
                    }
                } else {
                    Val::Handle(
                        member
                            .and_then(Option::take)
                            .ok_or_else(|| err(V::Malformed, "a task set member taken twice"))?,
                    )
                };
                self.task_return(f, &arguments, Some((result, val)), normal)
            }
            "task.cancel_all" | "task.detach_all" => {
                let word = self.task_word(f, &Self::task_argument(&arguments, 0)?)?;
                let set_word = self.task_word(f, &Self::task_argument(&arguments, 1)?)?;
                let cancel = name == "task.cancel_all";
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let set = s.set_handle(set_word, scope)?;
                let current = s.current;
                let next = match s.task(current)?.wait {
                    None => {
                        if cancel {
                            for word in s.set(set)?.members.clone().into_iter().flatten() {
                                let child = s.task_handle(word, scope)?;
                                s.cancel(child, CANCEL_REQUESTED)?;
                            }
                        }
                        Some(0)
                    }
                    Some(Wait::Release { set: waited, next }) if waited == set => Some(next),
                    Some(Wait::Drain(waited)) if waited == set => None,
                    _ => return fail(V::Malformed, "a task resumed in another operation"),
                };
                if let Some(next) = next {
                    let members = s.set(set)?.members.clone();
                    for (i, word) in members.iter().enumerate().skip(next) {
                        let Some(word) = *word else { continue };
                        let s = self.scheduler();
                        if let Some(child) = s.member(word).map(|_| word as u32) {
                            let row = s.task(child)?;
                            row.detached = true;
                            if row.state == State::Holding {
                                row.slot = None;
                                s.task(current)?.wait = Some(Wait::Release { set, next: i + 1 });
                                s.hand_off(child)?;
                                return Ok(Control::Continue);
                            }
                            if row.state == State::Done && row.chain.is_none() {
                                s.scope(scope)?.children.retain(|&c| c != child);
                                s.vacate(child);
                            }
                        }
                    }
                    let s = self.scheduler();
                    s.task(current)?.wait = None;
                    if cancel && !s.drained(set) {
                        s.task(current)?.wait = Some(Wait::Drain(set));
                        self.park_until(Park::Drain(set), true)?;
                        return Ok(Control::Continue);
                    }
                    self.task_yield()?;
                } else {
                    s.task(current)?.wait = None;
                }
                // The set word is taken: its handle closes with its row.
                self.scheduler().sets[set as usize] = None;
                match self.ledger.handles.get_mut(&set_word) {
                    Some((state @ crate::ledger::HandleState::Open, _)) => {
                        *state = crate::ledger::HandleState::Closed
                    }
                    _ => return fail(V::DoubleClose, "a task set closed twice"),
                }
                self.task_return(f, &arguments, None, normal)
            }
            _ => unreachable!("a task operation"),
        }
    }

    /// A dropped task set's row closes; its members stay children of their scope.
    pub(super) fn close_task_set(&mut self, word: u64) -> Checked<()> {
        let s = self.scheduler();
        let index = word as u32 as usize;
        match s.sets.get(index).and_then(Option::as_ref) {
            Some(set)
                if set.generation
                    == ((word & !(super::channels::ENDPOINT | TASK_SET)) >> 32) as u32 =>
            {
                s.sets[index] = None;
                Ok(())
            }
            _ => fail(V::DoubleClose, "a task set closed twice"),
        }
    }

    /// Poll every parked task in park order: each whose wait is over has its
    /// effect performed and joins the back of the queue.
    fn poll_parked(&mut self) -> Checked<()> {
        // Scope deadlines first, so the parked tasks' polls see what they cancel (CONC-3).
        self.expire_due()?;
        let s = self.scheduler();
        let mut parked = std::mem::take(&mut s.parked);
        // Timer waits by deadline, ties in park order, after the other waits in park order.
        parked.sort_by_cached_key(|&task| match s.tasks[task as usize].as_ref()?.park? {
            Park::Timer { until } => Some(until),
            _ => None,
        });
        let mut waiting = Vec::with_capacity(parked.len());
        for task in parked {
            if self.poll(task)? {
                let s = self.scheduler();
                let row = s.task(task)?;
                row.state = State::Ready;
                row.park = None;
                s.runnable.push_back(task);
            } else {
                waiting.push(task);
            }
        }
        // A poll never parks a task.
        self.scheduler().parked = waiting;
        Ok(())
    }

    /// Whether parked `task`'s wait is over, after performing its effect if it is.
    fn poll(&mut self, task: u32) -> Checked<bool> {
        let s = self.scheduler();
        match s.task(task)?.park {
            Some(Park::Join { child, slot }) => match s.task(child)?.state {
                State::Holding => {
                    let held = s
                        .task(child)?
                        .slot
                        .take()
                        .ok_or_else(|| err(V::Malformed, "a holding child without a slot"))?;
                    self.deliver(held, slot)?;
                    Ok(true)
                }
                State::Done => Ok(true),
                State::Ready | State::Parked => Ok(false),
            },
            Some(Park::Exit(scope)) => Ok(s.settled(scope)),
            Some(Park::Timer { .. } | Park::Pending) => self.poll_suspended(task),
            Some(Park::Any(set)) => Ok(s.first_completed(set).is_some()),
            Some(Park::Drain(set)) => Ok(s.drained(set)),
            Some(Park::Receive {
                channel,
                slot,
                layout,
            }) => match {
                let shielded = self.task_shielded(task)?;
                self.receive_ready(task, shielded, channel, slot, layout)?
            } {
                Some(status) => {
                    self.scheduler().task(task)?.status = status;
                    Ok(true)
                }
                None => Ok(false),
            },
            None => fail(V::Malformed, "a parked task without a wait"),
        }
    }

    /// Take the front of the queue; a task due to park parks there, the
    /// parked tasks are polled, and the next front is taken.
    fn front(&mut self) -> Checked<u32> {
        loop {
            let s = self.scheduler();
            let Some(next) = s.runnable.pop_front() else {
                // A script's next signal arrives first, at this idle point.
                match self.ops.deliver_signal() {
                    SignalDelivery::Pending => {
                        self.poll_parked()?;
                        continue;
                    }
                    SignalDelivery::Default(kind) => {
                        self.signaled = Some(kind);
                        return fail(V::Signaled, "a signal's default action ended the run");
                    }
                    SignalDelivery::None => {}
                }
                // Virtual time: a pending deadline is an outstanding event source.
                if self.advance()? {
                    self.poll_parked()?;
                    continue;
                }
                // CONC-8: the run's outcome, which `run` reports as Blocked.
                return fail(V::Blocked, "every task of the run is blocked (CONC-8)");
            };
            let row = s.task(next)?;
            if !row.parking {
                return Ok(next);
            }
            row.parking = false;
            row.state = State::Parked;
            s.parked.push(next);
            self.poll_parked()?;
        }
    }

    /// The running task stopped (it parked, holds its result or completed):
    /// poll the parked tasks and run the front, which may be the running task.
    fn run_next(&mut self, departing: bool) -> Checked<()> {
        self.poll_parked()?;
        let next = self.front()?;
        self.scheduler().switch = Some((next, departing));
        Ok(())
    }

    /// The yield after an operation: poll the parked tasks and, when another
    /// task is runnable, queue the running task and run the front.
    pub(super) fn task_yield(&mut self) -> Checked<()> {
        if self.tasks.is_none() || self.scheduler().switch.is_some() {
            return Ok(());
        }
        self.poll_parked()?;
        let s = self.scheduler();
        if s.runnable.is_empty() {
            return Ok(());
        }
        let current = s.current;
        s.runnable.push_back(current);
        let next = self.front()?;
        self.scheduler().switch = Some((next, false));
        Ok(())
    }

    /// Wait for `park`; the operation runs again once a poll performed its
    /// effect. `yielded`: the operation yields first, and the task parks when
    /// it is next at the front.
    pub(super) fn park_until(&mut self, park: Park, yielded: bool) -> Checked<()> {
        if yielded {
            self.poll_parked()?;
            let s = self.scheduler();
            if !s.runnable.is_empty() {
                let current = s.current;
                let row = s.task(current)?;
                row.park = Some(park);
                row.parking = true;
                s.runnable.push_back(current);
                let next = self.front()?;
                self.scheduler().switch = Some((next, false));
                return Ok(());
            }
        }
        self.scheduler().park_current(park)?;
        self.run_next(false)
    }

    /// Whether the running task is a child (the root's frames finish the run).
    pub(super) fn in_child(&self) -> bool {
        self.tasks.as_ref().is_some_and(|s| s.current != 0)
    }

    /// A child's `$native.task_main` frame returned or failed: release its closure's
    /// environment, as the runtime does, then complete ([`Self::task_complete`]).
    pub(super) fn task_exited(&mut self, failed: bool) -> Checked<()> {
        let s = self.scheduler();
        let current = s.current;
        let row = s.task(current)?;
        row.failed |= failed;
        row.exited = true;
        let callable = row
            .main
            .ok_or_else(|| err(V::Malformed, "a child without its closure's layout"))?;
        let key = MAIN_ROOT | u64::from(current);
        let held = self
            .roots
            .get(&key)
            .cloned()
            .ok_or_else(|| err(V::Malformed, "a child without its closure"))?;
        self.set_root(key, None);
        if held.credits == 0 || !matches!(held.val, Val::Callable { env: Some(_), .. }) {
            return Ok(());
        }
        // Release the environment block through the callable's own drop step.
        let temp = self.temporary(callable)?;
        self.write_value(
            temp,
            callable,
            Moved {
                val: held.val,
                credited: true,
                borrow: None,
            },
        )?;
        let actual = self.scheduler().task(current)?.entry_site;
        self.stack.push(Activity::Drain(Drain {
            caller: actual,
            operation: actual,
            work: vec![
                Work::Death {
                    alloc: temp.alloc,
                    raw: None,
                },
                Work::DropAt {
                    at: temp,
                    layout: callable,
                    mask: None,
                },
            ],
            failed: false,
        }));
        Ok(())
    }

    /// A drain with no frame below it finished on a child's stack.
    pub(super) fn task_drained(&mut self, failed: bool) -> Checked<()> {
        if self.in_child() {
            let s = self.scheduler();
            let current = s.current;
            s.task(current)?.failed |= failed;
        }
        Ok(())
    }

    /// The running child's stack emptied after its frame returned: record its
    /// outcome and run another task.
    pub(super) fn task_complete(&mut self) -> Checked<()> {
        let s = self.scheduler();
        let index = s.current;
        if !s.task(index)?.exited {
            return fail(
                V::Malformed,
                "a child's stack emptied before its frame returned",
            );
        }
        let path = s.task(index)?.path.clone();
        let failed = s.task(index)?.failed;
        let chain = self.take_chain(&path);
        if failed == chain.is_empty() {
            return fail(
                V::Malformed,
                "a task's status disagrees with its failure state",
            );
        }
        let s = self.scheduler();
        let row = s.task(index)?;
        row.state = State::Done;
        let scope = row
            .scope
            .ok_or_else(|| err(V::Malformed, "the root task completed as a child"))?;
        let handoff = row.handoff.take();
        let detached = row.detached;
        if failed {
            row.chain = Some(chain);
            s.order(index)?;
            let scope_row = s.scope(scope)?;
            scope_row.canceled.get_or_insert(CANCEL_CHILD_FAILED);
            for sibling in scope_row.children.clone() {
                if sibling != index {
                    s.cancel(sibling, CANCEL_CHILD_FAILED)?;
                }
            }
        }
        if detached && !failed {
            s.scope(scope)?.children.retain(|&c| c != index);
            s.vacate(index);
        }
        match handoff {
            // A join or a discard ran the rest of a holding child: no schedule point.
            Some(next) => s.switch = Some((next, true)),
            None => self.run_next(true)?,
        }
        self.finish_switch()
    }

    /// Make the switch the last operation requested, once its frame is back on its stack.
    pub(super) fn finish_switch(&mut self) -> Checked<()> {
        let Some(s) = self.tasks.as_deref_mut() else {
            return Ok(());
        };
        let Some((next, departing)) = s.switch.take() else {
            return Ok(());
        };
        let from = s.current;
        if next == from {
            return Ok(());
        }
        let stack = std::mem::take(&mut self.stack);
        let failures = std::mem::take(&mut self.failures);
        let unanchored = std::mem::take(&mut self.unanchored_failures);
        let s = self.tasks.as_deref_mut().unwrap();
        let spawns = std::mem::take(&mut s.spawns);
        if departing {
            if !stack.is_empty() || !failures.is_empty() {
                return fail(V::Malformed, "a task completed with state left");
            }
        } else if let Some(row) = s.tasks[from as usize].as_mut() {
            row.stack = stack;
            row.failures = failures;
            row.spawns = spawns;
            row.unanchored = unanchored;
        }
        let row = s.task(next)?;
        row.state = State::Ready;
        self.stack = std::mem::take(&mut row.stack);
        self.failures = std::mem::take(&mut row.failures);
        self.unanchored_failures = std::mem::take(&mut row.unanchored);
        let spawns = std::mem::take(&mut row.spawns);
        s.spawns = spawns;
        s.current = next;
        Ok(())
    }

    /// Every activity of every task: the running stack and the suspended ones.
    pub(super) fn all_activities(&self) -> Vec<&Activity> {
        activities(&self.stack, self.tasks.as_deref())
    }
}

/// A task whose activities are `stack` runs destruction: a drain, a finalizer, or an inline drop
/// body. Its finalizer shield (`hirsel-rt/src/finalize.rs`) is up, so no cancellation check
/// observes its cancellation (CONC-3).
fn stack_shields(stack: &[Activity]) -> bool {
    stack.iter().any(|activity| match activity {
        Activity::Frame(frame) => frame.shields(),
        Activity::Drain(_) => true,
    })
}

impl<S: Stage> Machine<'_, S> {
    /// Whether the running task, whose operation's frame is `f`, is shielded.
    pub(super) fn shielded(&self, f: &Frame) -> bool {
        f.shields() || stack_shields(&self.stack)
    }
    /// Whether a task polled while parked is shielded, from its kept stack.
    pub(super) fn task_shielded(&mut self, task: u32) -> Checked<bool> {
        if task == self.scheduler().current {
            return Ok(stack_shields(&self.stack));
        }
        Ok(stack_shields(&self.scheduler().task(task)?.stack))
    }
}

/// The running `stack` and every suspended task's.
pub(super) fn activities<'a>(stack: &'a [Activity], tasks: Option<&'a Tasks>) -> Vec<&'a Activity> {
    let mut out: Vec<&Activity> = stack.iter().collect();
    if let Some(s) = tasks {
        for row in s.tasks.iter().flatten() {
            out.extend(row.stack.iter());
        }
    }
    out
}

#[cfg(test)]
pub(super) fn sealed_owner_test_task(frame: Frame) -> Tasks {
    let mut tasks = Tasks::new();
    let mut receiver = Task::new(2, None, vec![0], None);
    receiver.stack.push(Activity::Frame(Box::new(frame)));
    tasks.tasks.push(Some(receiver));
    tasks
}
