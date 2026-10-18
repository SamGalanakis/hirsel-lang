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
//! to park after its operation's yield parks when it reaches the front. What the runtime keeps per task in `RunCtx` H
//! keeps per task here: its failure chain, with the spawn path of each
//! failure a child contributed. A child's closure is a harness root while the
//! scheduler holds it, and its environment is released after its frame
//! returns, as the runtime releases it.
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
}

/// A holding child's result: its slot, the slot's layout and its `None` image.
#[derive(Clone)]
struct Slot {
    at: Ptr,
    layout: LayoutId,
    none: Extent,
}

#[derive(Clone)]
pub(super) struct Task {
    generation: u32,
    stack: Vec<Activity>,
    failures: Vec<ObservedFailure>,
    /// Per failure, the spawn path of the child it came from (None: this task's own).
    spawns: Vec<Option<Vec<u32>>>,
    unanchored: std::collections::BTreeSet<usize>,
    state: State,
    scope: Option<u32>,
    path: Vec<u32>,
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
}

impl Task {
    fn new(generation: u32, scope: Option<u32>, path: Vec<u32>, canceled: Option<u32>) -> Task {
        Task {
            generation,
            stack: Vec::new(),
            failures: Vec::new(),
            spawns: Vec::new(),
            unanchored: Default::default(),
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
        }
    }
}

#[derive(Clone)]
struct Scope {
    generation: u32,
    children: Vec<u32>,
    canceled: Option<u32>,
}

#[derive(Clone)]
pub(super) struct Tasks {
    tasks: Vec<Option<Task>>,
    scopes: Vec<Option<Scope>>,
    generation: u32,
    pub(super) current: u32,
    runnable: VecDeque<u32>,
    /// Parked tasks, in park order.
    parked: Vec<u32>,
    pub(super) channels: super::channels::Channels,
    /// The switch the running operation requested, and whether the running
    /// task completed (its stack is not kept).
    switch: Option<(u32, bool)>,
    /// The running task's spawn path per failure (`Task::spawns` while it runs).
    spawns: Vec<Option<Vec<u32>>>,
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
        "$intrinsic.TaskScopeEnter",
        "$intrinsic.TaskScopeExit",
        "$intrinsic.TaskSpawn",
        "$intrinsic.TaskFinish",
        "$intrinsic.TaskJoin",
        "task.cancel",
        "task.detach",
        "channel.open",
        "sender.dup",
        "$intrinsic.ChannelSend",
        "$intrinsic.ChannelReceive",
    ]
    .into_iter()
    .find(|n| *n == name)
}

impl Tasks {
    fn new() -> Tasks {
        Tasks {
            tasks: vec![Some(Task::new(1, None, Vec::new(), None))],
            scopes: Vec::new(),
            generation: 1,
            current: 0,
            runnable: VecDeque::new(),
            parked: Vec::new(),
            channels: Default::default(),
            switch: None,
            spawns: Vec::new(),
        }
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
    pub(super) fn canceled(&self) -> Option<u32> {
        self.canceled_of(self.current)
    }
    pub(super) fn canceled_task(&self, task: u32) -> Checked<Option<u32>> {
        if self.tasks.get(task as usize).is_none_or(Option::is_none) {
            return fail(V::Malformed, "a vacant task row");
        }
        Ok(self.canceled_of(task))
    }
    fn canceled_of(&self, task: u32) -> Option<u32> {
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
    fn task_spawns(&mut self) -> &mut Vec<Option<Vec<u32>>> {
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

    /// Continue the running task's chain with a child's (a join).
    fn append_chain(&mut self, chain: Vec<(ObservedFailure, Vec<u32>)>) {
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
        match name {
            "channel.open"
            | "sender.dup"
            | "$intrinsic.ChannelSend"
            | "$intrinsic.ChannelReceive" => {
                self.channel_operation(f, site, name, &arguments, normal)
            }
            "$intrinsic.TaskScopeEnter" => {
                let s = self.scheduler();
                let generation = s.fresh_generation();
                s.scopes.push(Some(Scope {
                    generation,
                    children: Vec::new(),
                    canceled: None,
                }));
                let index = (s.scopes.len() - 1) as u32;
                let current = s.current;
                s.task(current)?.scopes.push(index);
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
                self.bind_parameter_slots(&mut frame, &[])?;
                self.bind_captures(&mut frame)?;
                let s = self.scheduler();
                let scope = s.scope_handle(word)?;
                let canceled = s.canceled().map(|_| CANCEL_PARENT);
                let current = s.current;
                let parent = s.task(current)?;
                let mut path = parent.path.clone();
                path.push(parent.spawned);
                parent.spawned += 1;
                let generation = s.fresh_generation();
                let mut task = Task::new(generation, Some(scope), path, canceled);
                task.main = Some(main_layout);
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
            "$intrinsic.TaskScopeExit" => {
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
                self.scheduler().scopes[index as usize] = None;
                if self.failures.is_empty() {
                    self.task_return(f, &arguments, None, normal)
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
            _ => unreachable!("a task operation"),
        }
    }

    /// Poll every parked task in park order: each whose wait is over has its
    /// effect performed and joins the back of the queue.
    fn poll_parked(&mut self) -> Checked<()> {
        let parked = std::mem::take(&mut self.scheduler().parked);
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
            Some(Park::Receive {
                channel,
                slot,
                layout,
            }) => match self.receive_ready(task, channel, slot, layout)? {
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
                // CONC-8: environmental in the runtime, never a valid program's schedule here.
                return fail(V::Unsupported, "every task of the run is blocked (CONC-8)");
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
        self.stack.push(Activity::Drain(Drain {
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
