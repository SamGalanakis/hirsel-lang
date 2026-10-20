//! The Process capability (docs/spec/core-library.md#processes): exact program rules
//! checked before any effect, scripted worlds, and the host provider.
//!
//! The host provider is Rust std's `Command` with an empty environment and an explicit
//! working directory. Its stream loop adapts std's unix `read_output`
//! (library/std/src/sys/process/unix/common.rs, f6c1107f; third-party/NOTICES.md):
//! one `poll` over nonblocking pipes, here also feeding stdin, bounding each stream and
//! watching a deadline and a cancellation descriptor, so neither pipe can stall the other.
//! Under a cooperative scheduler a run is a [`HostJob`]: the same loop on a helper thread,
//! whose completion wakes a descriptor the scheduler polls, so the task parks instead of
//! blocking the thread.
use crate::handles::Grant;
use crate::host_stages::{Ended, Stages};
use crate::io::{AdapterError, Completion, Request, State};
pub use hirsel_run_config::{
    ChildRun, ChildStep, ChildStream, Command, ExitStatus, GroupOutput, PipelineOutcome,
    ProcessFailure, ProcessOutcome, ProcessRule, ProcessRun, ScriptChild, ScriptGroup, ScriptRead,
    Signal, canonical_program,
};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};

/// A run's `Deadline`: its MonotonicClock handle and the Instant's nanosecond ticks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Deadline {
    pub clock: u64,
    pub ticks: u64,
}

/// The grace a timed-out child has between SIGTERM and SIGKILL
/// (docs/spec/core-library.md#processes).
pub const TERMINATION_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// Which provider serves a Proc: the host, or a scripted world.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProcessWorld {
    #[default]
    Host,
    Script(u64),
}

/// A child's environment after the checks passed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChildEnv {
    /// The granting rule's names, with the values the launcher's environment has.
    Inherit(Vec<String>),
    /// Exactly these variables, each name granted by the rule.
    Exact(Vec<(String, String)>),
}
impl ChildEnv {
    /// The environment `command` gets under its granting `rule`.
    pub fn of(command: &Command, rule: &ProcessRule) -> Self {
        match &command.env {
            Some(vars) => Self::Exact(vars.clone()),
            None => Self::Inherit(rule.env.clone()),
        }
    }
}

/// One child after its checks passed.
pub struct ProcessSpec<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub env: ChildEnv,
    /// The working directory's Dir handle, always host-backed here; `None` is `/`.
    pub directory: Option<u64>,
    /// Whether the child (every stage of a pipeline, which agree) runs in a process group
    /// of its own, which every signal the provider sends it reaches.
    pub own_group: bool,
}
impl<'a> ProcessSpec<'a> {
    /// The checked `command` under its granting `rule`.
    pub fn of(command: &'a Command, rule: &ProcessRule) -> Self {
        Self {
            program: &command.program,
            args: &command.args,
            env: ChildEnv::of(command, rule),
            directory: command.directory,
            own_group: command.own_group,
        }
    }
}

/// A checked one-shot run or pipeline: the host provider's whole input. Stage `i`'s
/// standard output feeds stage `i + 1`; `stdin` feeds the first stage.
pub struct HostRun<'a> {
    pub stages: &'a [ProcessSpec<'a>],
    pub stdin: &'a [u8],
    /// The bound on the captured standard output and the shared standard error.
    pub max_output: u64,
    /// CLOCK_MONOTONIC nanoseconds by which the run must end.
    pub deadline: Option<u64>,
}

/// The host process provider: every host process effect and a run's streaming children.
/// Only the entries that start a process (`proc.run`, `proc.pipeline`, `proc.spawn`,
/// `proc.spawn_pipeline`) hand its constructor, [`host`], to the adapter
/// (`Adapter::process`), so a program that starts none links no process spawning.
pub trait ProcessHost {
    /// Perform one checked call. `cwd` resolves a Dir handle to its pinned directory
    /// descriptor; `readiness` is the cooperative scheduler's, if any.
    fn call(
        &mut self,
        call: HostCall<'_>,
        cwd: &dyn Fn(u64) -> Option<std::os::fd::RawFd>,
        readiness: Option<&crate::readiness::Readiness>,
    ) -> Result<Completion, crate::io::AdapterError>;
}

/// A new host process provider, with no children.
pub fn host() -> Box<dyn ProcessHost> {
    Box::new(crate::host_child::HostChildren::default())
}

/// What a host provider performs after every check passed. Its answer is the completion
/// the request returns, which the policy validates: in replay, the recorded one.
pub enum HostCall<'a> {
    /// `proc.run`, one stage: [`Completion::Process`].
    Run(&'a HostRun<'a>),
    /// `proc.pipeline`, two or more stages: [`Completion::Pipeline`].
    Pipeline(&'a HostRun<'a>),
    /// `proc.spawn` as Child handle `child`: [`Completion::Spawned`] with that handle.
    Spawn {
        child: u64,
        spec: &'a ProcessSpec<'a>,
    },
    /// `proc.spawn_pipeline` of two or more checked stages as ChildGroup handle `group`:
    /// [`Completion::Spawned`] with that handle.
    SpawnPipeline {
        group: u64,
        stages: &'a [ProcessSpec<'a>],
        max_stderr: u64,
    },
    /// The operations below take a Child or a ChildGroup, whose input is its first stage's
    /// and whose output its last stage's; a ChildGroup is never read for standard error.
    /// Nonempty `data` to open input: [`Completion::ChildUnit`].
    Write { child: u64, data: &'a [u8] },
    /// [`Completion::ChildUnit`] `Ok`.
    CloseStdin { child: u64 },
    /// A positive `max`: [`Completion::ChildBytes`].
    Read {
        child: u64,
        stream: ChildStream,
        max: u64,
    },
    /// [`Completion::ChildExit`]; `TimedOut` only with a deadline, in CLOCK_MONOTONIC
    /// nanoseconds.
    Wait { child: u64, deadline: Option<u64> },
    /// `proc.wait_group`: [`Completion::GroupExit`], `TimedOut` only with a deadline.
    WaitGroup { group: u64, deadline: Option<u64> },
    /// [`Completion::ChildUnit`] `Ok`: `signal` to every stage.
    Kill { child: u64, signal: Signal },
    /// The drop of a Child or ChildGroup never waited: [`Completion::Unit`] `Ok`.
    Close { child: u64 },
}

impl State {
    /// Whether `request` is the Process policy's: a process request, or the close of a Child.
    pub(crate) fn is_process(&self, request: &Request) -> bool {
        is_process_request(request)
            || matches!(request, Request::Close { handle }
                if matches!(self.handles.rows.get(handle), Some(Grant::Child { .. })))
    }
    /// The admitted `io.process` grant: its rules, and for process.script@1 the root
    /// scripted world. Scripted runs never widen the rules.
    pub(crate) fn set_process_grant(&mut self, grant: &hirsel_run_config::ProcessGrant) {
        self.process_rules = grant.rules.clone();
        self.process_world = match &grant.runs {
            Some(runs) => {
                self.process_scripts
                    .insert(0, runs.iter().cloned().map(Some).collect());
                self.child_scripts.insert(
                    0,
                    grant.children.iter().flatten().cloned().map(Some).collect(),
                );
                ProcessWorld::Script(0)
            }
            None => ProcessWorld::Host,
        };
    }
    fn process_grant(&self, id: u64) -> Result<(Vec<ProcessRule>, ProcessWorld), AdapterError> {
        match self.handles.rows.get(&id) {
            Some(Grant::Process { rules, world }) => Ok((rules.clone(), *world)),
            _ => Err(AdapterError::InvalidHandle(id)),
        }
    }
    /// A command's checks (docs/spec/core-library.md#processes): program and environment,
    /// then a working directory a host provider cannot start in.
    fn command_error(
        &self,
        command: &Command,
        rules: &[ProcessRule],
        world: ProcessWorld,
    ) -> Result<Option<ProcessFailure>, AdapterError> {
        if let Some(d) = command.directory
            && !matches!(self.handles.rows.get(&d), Some(Grant::Directory { .. }))
        {
            return Err(AdapterError::InvalidHandle(d));
        }
        Ok(command.check(rules).err().or_else(|| {
            (world == ProcessWorld::Host
                && command
                    .directory
                    .is_some_and(|d| self.fake_directories.contains_key(&d)))
            .then_some(ProcessFailure::InvalidInput)
        }))
    }
    /// A checked `proc.run` (one stage) or `proc.pipeline`: served by a scripted world, or
    /// by `host`, whose completion may fail, time out or exceed the bound but never
    /// contradict the checks; cancellation never reaches it.
    fn run_stages(
        &mut self,
        host: &mut dyn FnMut(HostCall<'_>) -> Result<Completion, AdapterError>,
        process: u64,
        stages: &[Command],
        stdin: &[u8],
        max_output: u64,
        deadline: Option<Deadline>,
    ) -> Result<PipelineOutcome, AdapterError> {
        let (rules, world) = self.process_grant(process)?;
        let outcome = match world {
            // A scripted run takes no time: only a deadline the clock's latest reading
            // already reached times it out, before any entry is taken.
            ProcessWorld::Script(_)
                if deadline
                    .is_some_and(|d| self.last_monotonic.is_some_and(|now| now >= d.ticks)) =>
            {
                PipelineOutcome::Error(ProcessFailure::TimedOut)
            }
            ProcessWorld::Script(id) => {
                let runs = self
                    .process_scripts
                    .get_mut(&id)
                    .ok_or(AdapterError::InvalidCompletion)?;
                hirsel_run_config::script_pipeline(runs, stages, max_output)
            }
            ProcessWorld::Host => {
                let mut specs = vec![];
                for stage in stages {
                    let rule = stage
                        .check(&rules)
                        .map_err(|_| AdapterError::InvalidCompletion)?;
                    specs.push(ProcessSpec::of(stage, rule));
                }
                let run = HostRun {
                    stages: &specs,
                    stdin,
                    max_output,
                    deadline: deadline.map(|d| d.ticks),
                };
                let outcome = if stages.len() == 1 {
                    match host(HostCall::Run(&run))? {
                        Completion::Process(ProcessOutcome::Output {
                            status,
                            stdout,
                            stderr,
                        }) => PipelineOutcome::Output {
                            statuses: vec![status],
                            stdout,
                            stderr,
                        },
                        Completion::Process(ProcessOutcome::Error(f)) => PipelineOutcome::Error(f),
                        _ => return Err(AdapterError::InvalidCompletion),
                    }
                } else {
                    match host(HostCall::Pipeline(&run))? {
                        Completion::Pipeline(outcome) => outcome,
                        _ => return Err(AdapterError::InvalidCompletion),
                    }
                };
                let valid = match &outcome {
                    PipelineOutcome::Error(
                        ProcessFailure::NotGranted
                        | ProcessFailure::Canceled(_)
                        | ProcessFailure::InputClosed,
                    ) => false,
                    PipelineOutcome::Error(ProcessFailure::InvalidInput) => {
                        stages.iter().any(|s| s.directory.is_some())
                    }
                    PipelineOutcome::Error(ProcessFailure::TimedOut) => deadline.is_some(),
                    PipelineOutcome::Error(_) => true,
                    PipelineOutcome::Output { statuses, .. } => {
                        statuses.len() == stages.len()
                            && outcome.clone().bounded(max_output) == outcome
                    }
                };
                if !valid {
                    return Err(AdapterError::InvalidCompletion);
                }
                outcome
            }
        };
        // A completed timeout proves the clock reached the deadline, as a completed timer
        // wait does.
        if let (PipelineOutcome::Error(ProcessFailure::TimedOut), Some(d)) = (&outcome, deadline) {
            self.last_monotonic = Some(self.last_monotonic.unwrap_or(0).max(d.ticks));
        }
        Ok(outcome)
    }
    pub(crate) fn process_error(
        &self,
        request: &Request,
    ) -> Result<Option<Completion>, AdapterError> {
        let failure = match request {
            Request::ProcessRun {
                process,
                command,
                deadline,
                ..
            } => {
                let (rules, world) = self.process_grant(*process)?;
                if let Some(d) = deadline {
                    self.expect(d.clock, Grant::MonotonicClock)?;
                }
                self.command_error(command, &rules, world)?
            }
            Request::ProcessSpawn { process, command } => {
                let (rules, world) = self.process_grant(*process)?;
                self.command_error(command, &rules, world)?
            }
            // Every stage passes its checks, in stage order, before anything is spawned.
            Request::ProcessPipeline {
                process, stages, ..
            }
            | Request::ProcessSpawnPipeline {
                process, stages, ..
            } => {
                let (rules, world) = self.process_grant(*process)?;
                if let Request::ProcessPipeline {
                    deadline: Some(d), ..
                } = request
                {
                    self.expect(d.clock, Grant::MonotonicClock)?;
                }
                if let Some(failure) = hirsel_run_config::stages_error(stages) {
                    Some(failure)
                } else {
                    let mut failure = None;
                    for stage in stages {
                        failure = self.command_error(stage, &rules, world)?;
                        if failure.is_some() {
                            break;
                        }
                    }
                    failure
                }
            }
            Request::ChildWrite { process, child, .. } => {
                let (_, open, _) = self.child_grant(*process, *child)?;
                (!open).then_some(ProcessFailure::InvalidInput)
            }
            // A ChildGroup has no standard error to read.
            Request::ChildRead {
                process,
                child,
                stream,
                max,
            } => {
                let (_, _, group) = self.child_grant(*process, *child)?;
                if group.is_some() && *stream == ChildStream::Stderr {
                    return Err(AdapterError::InvalidHandle(*child));
                }
                (*max == 0).then_some(ProcessFailure::InvalidInput)
            }
            Request::ChildWait {
                process,
                child,
                deadline,
            }
            | Request::GroupWait {
                process,
                group: child,
                deadline,
            } => {
                let (_, _, group) = self.child_grant(*process, *child)?;
                if group.is_some() != matches!(request, Request::GroupWait { .. }) {
                    return Err(AdapterError::InvalidHandle(*child));
                }
                if let Some(d) = deadline {
                    self.expect(d.clock, Grant::MonotonicClock)?;
                }
                None
            }
            _ => None,
        };
        Ok(failure.map(|f| match request {
            Request::ProcessRun { .. } => Completion::Process(ProcessOutcome::Error(f)),
            Request::ProcessPipeline { .. } => Completion::Pipeline(PipelineOutcome::Error(f)),
            Request::ProcessSpawn { .. } | Request::ProcessSpawnPipeline { .. } => {
                Completion::Spawned(Err(f))
            }
            Request::ChildRead { .. } => Completion::ChildBytes(Err(f)),
            _ => Completion::ChildUnit(Err(f)),
        }))
    }
    /// Check the grant and inputs before any effect, serve a scripted world itself, and
    /// ask `host` only for a checked host run. `host` is the recorded completion in replay.
    pub(crate) fn execute_process(
        &mut self,
        host: &mut dyn FnMut(HostCall<'_>) -> Result<Completion, AdapterError>,
        request: &Request,
    ) -> Result<Completion, AdapterError> {
        if let Some(error) = self.process_error(request)? {
            return Ok(error);
        }
        match request {
            Request::Process { io } => {
                self.expect(*io, Grant::Io)?;
                if !self.projections.contains("process") {
                    return Ok(Completion::Handle(Err(crate::io::IoError::NotGranted)));
                }
                let grant = Grant::Process {
                    rules: self.process_rules.clone(),
                    world: self.process_world,
                };
                Ok(Completion::Handle(Ok(self.handles.insert(grant))))
            }
            Request::ProcessNarrow { process, rules } => {
                let (parent, world) = self.process_grant(*process)?;
                let rules = narrowed_rules(rules);
                if !rules.iter().all(|r| {
                    parent
                        .iter()
                        .any(|p| hirsel_run_config::process_rule_within(r, p))
                }) {
                    return Ok(Completion::Spawned(Err(ProcessFailure::NotGranted)));
                }
                Ok(Completion::Spawned(Ok(self
                    .handles
                    .insert(Grant::Process { rules, world }))))
            }
            Request::TestProcess {
                test,
                runs,
                children,
            } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                if runs.iter().any(|run| {
                    !canonical_program(&run.program) || !hirsel_run_config::scriptable(&run.outcome)
                }) || children.iter().any(|c| !canonical_program(&c.program))
                {
                    return Err(AdapterError::InvalidCompletion);
                }
                let mut programs: Vec<_> = runs
                    .iter()
                    .map(|r| r.program.clone())
                    .chain(children.iter().map(|c| c.program.clone()))
                    .collect();
                programs.sort();
                programs.dedup();
                let id = self.handles.next();
                self.handles.insert(Grant::Process {
                    rules: programs
                        .into_iter()
                        .map(|program| ProcessRule {
                            program,
                            env: vec![],
                        })
                        .collect(),
                    world: ProcessWorld::Script(id),
                });
                self.process_scripts
                    .insert(id, runs.iter().cloned().map(Some).collect());
                self.child_scripts
                    .insert(id, children.iter().cloned().map(Some).collect());
                Ok(Completion::Handle(Ok(id)))
            }
            Request::ProcessRun {
                process,
                command,
                stdin,
                max_output,
                deadline,
            } => Ok(Completion::Process(
                self.run_stages(
                    host,
                    *process,
                    std::slice::from_ref(command),
                    stdin,
                    *max_output,
                    *deadline,
                )?
                .into_run(),
            )),
            Request::ProcessPipeline {
                process,
                stages,
                stdin,
                max_output,
                deadline,
            } => Ok(Completion::Pipeline(self.run_stages(
                host,
                *process,
                stages,
                stdin,
                *max_output,
                *deadline,
            )?)),
            Request::ProcessSpawn { process, command } => {
                let (rules, world) = self.process_grant(*process)?;
                let child = self.handles.next();
                let spawned = match world {
                    ProcessWorld::Script(id) => {
                        let children = self
                            .child_scripts
                            .get_mut(&id)
                            .ok_or(AdapterError::InvalidCompletion)?;
                        match children.iter_mut().find(|c| {
                            c.as_ref().is_some_and(|c| {
                                c.program == command.program && c.args == command.args
                            })
                        }) {
                            Some(entry) => {
                                let script = ScriptChild::new(entry.take().unwrap());
                                self.script_children.insert(child, script);
                                Ok(child)
                            }
                            None => Err(ProcessFailure::NotFound),
                        }
                    }
                    ProcessWorld::Host => {
                        let rule = command
                            .check(&rules)
                            .map_err(|_| AdapterError::InvalidCompletion)?;
                        let spec = ProcessSpec::of(command, rule);
                        match host(HostCall::Spawn { child, spec: &spec })? {
                            Completion::Spawned(Ok(id)) if id == child => Ok(child),
                            Completion::Spawned(Err(
                                failure @ (ProcessFailure::NotFound
                                | ProcessFailure::PermissionDenied
                                | ProcessFailure::Fault(_)),
                            )) => Err(failure),
                            Completion::Spawned(Err(ProcessFailure::InvalidInput))
                                if command.directory.is_some() =>
                            {
                                Err(ProcessFailure::InvalidInput)
                            }
                            _ => return Err(AdapterError::InvalidCompletion),
                        }
                    }
                };
                if spawned.is_ok() {
                    self.handles.insert(Grant::Child {
                        process: *process,
                        world,
                        stdin_open: true,
                        group: None,
                    });
                }
                Ok(Completion::Spawned(spawned))
            }
            Request::ProcessSpawnPipeline {
                process,
                stages,
                max_stderr,
            } => {
                let (rules, world) = self.process_grant(*process)?;
                let group = self.handles.next();
                let spawned = match world {
                    ProcessWorld::Script(id) => {
                        let children = self
                            .child_scripts
                            .get_mut(&id)
                            .ok_or(AdapterError::InvalidCompletion)?;
                        match hirsel_run_config::take_children(children, stages) {
                            Ok(runs) => {
                                let (script, report) = ScriptGroup::new(runs, *max_stderr);
                                if let Some(report) = report {
                                    return Ok(self.child_failure(&report));
                                }
                                self.script_groups.insert(group, script);
                                Ok(group)
                            }
                            Err(failure) => Err(failure),
                        }
                    }
                    ProcessWorld::Host => {
                        let mut specs = vec![];
                        for stage in stages {
                            let rule = stage
                                .check(&rules)
                                .map_err(|_| AdapterError::InvalidCompletion)?;
                            specs.push(ProcessSpec::of(stage, rule));
                        }
                        match host(HostCall::SpawnPipeline {
                            group,
                            stages: &specs,
                            max_stderr: *max_stderr,
                        })? {
                            Completion::Spawned(Ok(id)) if id == group => Ok(group),
                            Completion::Spawned(Err(
                                failure @ (ProcessFailure::NotFound
                                | ProcessFailure::PermissionDenied
                                | ProcessFailure::Fault(_)),
                            )) => Err(failure),
                            Completion::Spawned(Err(ProcessFailure::InvalidInput))
                                if stages.iter().any(|s| s.directory.is_some()) =>
                            {
                                Err(ProcessFailure::InvalidInput)
                            }
                            _ => return Err(AdapterError::InvalidCompletion),
                        }
                    }
                };
                if spawned.is_ok() {
                    self.handles.insert(Grant::Child {
                        process: *process,
                        world,
                        stdin_open: true,
                        group: Some((stages.len(), *max_stderr)),
                    });
                }
                Ok(Completion::Spawned(spawned))
            }
            Request::ChildWrite {
                process,
                child,
                bytes,
            } => {
                let (world, _, group) = self.child_grant(*process, *child)?;
                if bytes.is_empty() {
                    return Ok(Completion::ChildUnit(Ok(())));
                }
                match world {
                    ProcessWorld::Script(_) if group.is_some() => {
                        match self.script_group(*child)?.write(bytes) {
                            Ok(written) => Ok(Completion::ChildUnit(written)),
                            Err(report) => Ok(self.child_failure(&report)),
                        }
                    }
                    ProcessWorld::Script(_) => match self.script_child(*child)?.write(bytes) {
                        Ok(written) => Ok(Completion::ChildUnit(written)),
                        Err(report) => Ok(self.child_failure(&report)),
                    },
                    ProcessWorld::Host => match host(HostCall::Write {
                        child: *child,
                        data: bytes,
                    })? {
                        completion @ Completion::ChildUnit(
                            Ok(()) | Err(ProcessFailure::InputClosed | ProcessFailure::Fault(_)),
                        ) => Ok(completion),
                        _ => Err(AdapterError::InvalidCompletion),
                    },
                }
            }
            Request::ChildCloseStdin { process, child } => {
                let (world, stdin_open, group) = self.child_grant(*process, *child)?;
                if stdin_open {
                    self.close_child_input(*child);
                    match world {
                        ProcessWorld::Script(_) => {
                            let report = match group {
                                Some(_) => self.script_group(*child)?.close_stdin(),
                                None => self.script_child(*child)?.close_stdin(),
                            };
                            if let Some(report) = report {
                                return Ok(self.child_failure(&report));
                            }
                        }
                        ProcessWorld::Host => {
                            if host(HostCall::CloseStdin { child: *child })?
                                != Completion::ChildUnit(Ok(()))
                            {
                                return Err(AdapterError::InvalidCompletion);
                            }
                        }
                    }
                }
                Ok(Completion::ChildUnit(Ok(())))
            }
            Request::ChildRead {
                process,
                child,
                stream,
                max,
            } => {
                let (world, _, group) = self.child_grant(*process, *child)?;
                let script = match (world, group) {
                    (ProcessWorld::Host, _) => None,
                    (ProcessWorld::Script(_), Some(_)) => {
                        Some(self.script_group(*child)?.read(*max))
                    }
                    (ProcessWorld::Script(_), None) => {
                        Some(self.script_child(*child)?.read(*stream, *max))
                    }
                };
                match script {
                    // At an Expect only the program's own writes can make bytes available.
                    Some(script) => match script {
                        ScriptRead::Bytes(bytes) => Ok(Completion::ChildBytes(Ok(bytes))),
                        ScriptRead::Waiting => Err(AdapterError::Blocked),
                    },
                    None => match host(HostCall::Read {
                        child: *child,
                        stream: *stream,
                        max: *max,
                    })? {
                        Completion::ChildBytes(Ok(bytes)) if bytes.len() as u64 <= *max => {
                            Ok(Completion::ChildBytes(Ok(bytes)))
                        }
                        completion @ Completion::ChildBytes(Err(ProcessFailure::Fault(_))) => {
                            Ok(completion)
                        }
                        _ => Err(AdapterError::InvalidCompletion),
                    },
                }
            }
            Request::ChildWait {
                process,
                child,
                deadline,
            } => {
                let (world, stdin_open, _) = self.child_grant(*process, *child)?;
                let status = match world {
                    // A scripted child takes no time: only a deadline the clock's latest
                    // reading already reached ends it, at once and without checking an
                    // Expect, as a kill does.
                    ProcessWorld::Script(_)
                        if deadline.is_some_and(|d| {
                            self.last_monotonic.is_some_and(|now| now >= d.ticks)
                        }) =>
                    {
                        Err(ProcessFailure::TimedOut)
                    }
                    ProcessWorld::Script(_) => {
                        if stdin_open {
                            self.close_child_input(*child);
                            if let Some(report) = self.script_child(*child)?.close_stdin() {
                                self.end_child(*child);
                                return Ok(self.child_failure(&report));
                            }
                        }
                        let status = self.script_child(*child)?.status();
                        Ok(status.ok_or(AdapterError::InvalidCompletion)?)
                    }
                    ProcessWorld::Host => match host(HostCall::Wait {
                        child: *child,
                        deadline: deadline.map(|d| d.ticks),
                    })? {
                        Completion::ChildExit(Ok(status)) => Ok(status),
                        Completion::ChildExit(Err(failure @ ProcessFailure::Fault(_))) => {
                            Err(failure)
                        }
                        Completion::ChildExit(Err(ProcessFailure::TimedOut))
                            if deadline.is_some() =>
                        {
                            Err(ProcessFailure::TimedOut)
                        }
                        _ => return Err(AdapterError::InvalidCompletion),
                    },
                };
                // A completed timeout proves the clock reached the deadline.
                if let (Err(ProcessFailure::TimedOut), Some(d)) = (&status, deadline) {
                    self.last_monotonic = Some(self.last_monotonic.unwrap_or(0).max(d.ticks));
                }
                // The wait consumes the Child on every outcome.
                self.end_child(*child);
                Ok(Completion::ChildExit(status))
            }
            Request::GroupWait {
                process,
                group,
                deadline,
            } => {
                let (world, _, shape) = self.child_grant(*process, *group)?;
                let (stages, max_stderr) = shape.ok_or(AdapterError::InvalidHandle(*group))?;
                let output = match world {
                    // A scripted group takes no time: only a deadline the clock's latest
                    // reading already reached ends it, at once and without checking an
                    // Expect, as a kill does.
                    ProcessWorld::Script(_)
                        if deadline.is_some_and(|d| {
                            self.last_monotonic.is_some_and(|now| now >= d.ticks)
                        }) =>
                    {
                        Err(ProcessFailure::TimedOut)
                    }
                    ProcessWorld::Script(_) => {
                        self.close_child_input(*group);
                        if let Some(report) = self.script_group(*group)?.close_stdin() {
                            self.end_child(*group);
                            return Ok(self.child_failure(&report));
                        }
                        self.script_group(*group)?.output()
                    }
                    ProcessWorld::Host => match host(HostCall::WaitGroup {
                        group: *group,
                        deadline: deadline.map(|d| d.ticks),
                    })? {
                        Completion::GroupExit(Ok(output))
                            if output.statuses.len() == stages
                                && output.stderr.len() == stages
                                && output.stderr.iter().all(|e| e.len() as u64 <= max_stderr) =>
                        {
                            Ok(output)
                        }
                        Completion::GroupExit(Err(
                            failure @ (ProcessFailure::Fault(_) | ProcessFailure::OutputLimit),
                        )) => Err(failure),
                        Completion::GroupExit(Err(ProcessFailure::TimedOut))
                            if deadline.is_some() =>
                        {
                            Err(ProcessFailure::TimedOut)
                        }
                        _ => return Err(AdapterError::InvalidCompletion),
                    },
                };
                // A completed timeout proves the clock reached the deadline.
                if let (Err(ProcessFailure::TimedOut), Some(d)) = (&output, deadline) {
                    self.last_monotonic = Some(self.last_monotonic.unwrap_or(0).max(d.ticks));
                }
                // The wait consumes the ChildGroup on every outcome.
                self.end_child(*group);
                Ok(Completion::GroupExit(output))
            }
            Request::ChildKill {
                process,
                child,
                signal,
            } => {
                let (world, _, group) = self.child_grant(*process, *child)?;
                match world {
                    ProcessWorld::Script(_) if group.is_some() => {
                        self.script_group(*child)?.kill(*signal)
                    }
                    ProcessWorld::Script(_) => self.script_child(*child)?.kill(*signal),
                    ProcessWorld::Host => {
                        if host(HostCall::Kill {
                            child: *child,
                            signal: *signal,
                        })? != Completion::ChildUnit(Ok(()))
                        {
                            return Err(AdapterError::InvalidCompletion);
                        }
                    }
                }
                Ok(Completion::ChildUnit(Ok(())))
            }
            // A dropped Child: its input closes first, so an unmet Expect still fails.
            Request::Close { handle } => {
                let Some(Grant::Child {
                    world,
                    stdin_open,
                    group,
                    ..
                }) = self.handles.rows.get(handle).cloned()
                else {
                    return Err(AdapterError::InvalidHandle(*handle));
                };
                let report = match world {
                    ProcessWorld::Script(_) if stdin_open && group.is_some() => {
                        self.script_group(*handle)?.close_stdin()
                    }
                    ProcessWorld::Script(_) if stdin_open => {
                        self.script_child(*handle)?.close_stdin()
                    }
                    ProcessWorld::Script(_) => None,
                    ProcessWorld::Host => {
                        if host(HostCall::Close { child: *handle })? != Completion::Unit(Ok(())) {
                            return Err(AdapterError::InvalidCompletion);
                        }
                        None
                    }
                };
                self.end_child(*handle);
                Ok(match report {
                    Some(report) => self.child_failure(&report),
                    None => Completion::Unit(Ok(())),
                })
            }
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    /// The world, input state and group shape of `child`, which must be a Child or
    /// ChildGroup of `process` (CAP-2).
    #[allow(clippy::type_complexity)]
    fn child_grant(
        &self,
        process: u64,
        child: u64,
    ) -> Result<(ProcessWorld, bool, Option<(usize, u64)>), AdapterError> {
        self.process_grant(process)?;
        match self.handles.rows.get(&child) {
            Some(Grant::Child {
                process: owner,
                world,
                stdin_open,
                group,
            }) if *owner == process => Ok((*world, *stdin_open, *group)),
            _ => Err(AdapterError::InvalidHandle(child)),
        }
    }
    fn script_group(&mut self, group: u64) -> Result<&mut ScriptGroup, AdapterError> {
        self.script_groups
            .get_mut(&group)
            .ok_or(AdapterError::InvalidCompletion)
    }
    fn script_child(&mut self, child: u64) -> Result<&mut ScriptChild, AdapterError> {
        self.script_children
            .get_mut(&child)
            .ok_or(AdapterError::InvalidCompletion)
    }
    fn close_child_input(&mut self, child: u64) {
        if let Some(Grant::Child { stdin_open, .. }) = self.handles.rows.get_mut(&child) {
            *stdin_open = false;
        }
    }
    fn end_child(&mut self, child: u64) {
        self.handles.rows.remove(&child);
        self.script_children.remove(&child);
        self.script_groups.remove(&child);
    }
    /// A scripted child's LANG-9 failure, after its Test report.
    fn child_failure(&mut self, report: &str) -> Completion {
        self.stdout.extend_from_slice(report.as_bytes());
        Completion::Checked(false)
    }
}

/// Requested `proc.narrow` rules as a Proc holds them: by program, one rule per program
/// passing through the union of its sorted names.
fn narrowed_rules(rules: &[ProcessRule]) -> Vec<ProcessRule> {
    let mut merged = std::collections::BTreeMap::<&str, Vec<String>>::new();
    for rule in rules {
        merged
            .entry(&rule.program)
            .or_default()
            .extend(rule.env.iter().cloned());
    }
    merged
        .into_iter()
        .map(|(program, mut env)| {
            env.sort();
            env.dedup();
            ProcessRule {
                program: program.into(),
                env,
            }
        })
        .collect()
}

pub(crate) fn is_process_request(request: &Request) -> bool {
    matches!(
        request,
        Request::Process { .. }
            | Request::TestProcess { .. }
            | Request::ProcessNarrow { .. }
            | Request::ProcessRun { .. }
            | Request::ProcessPipeline { .. }
            | Request::ProcessSpawn { .. }
            | Request::ProcessSpawnPipeline { .. }
            | Request::GroupWait { .. }
            | Request::ChildWrite { .. }
            | Request::ChildCloseStdin { .. }
            | Request::ChildRead { .. }
            | Request::ChildWait { .. }
            | Request::ChildKill { .. }
    )
}

pub(crate) fn spawn_failure(error: &std::io::Error) -> ProcessFailure {
    match error.kind() {
        std::io::ErrorKind::NotFound => ProcessFailure::NotFound,
        std::io::ErrorKind::PermissionDenied => ProcessFailure::PermissionDenied,
        _ => ProcessFailure::Fault(1),
    }
}

/// CLOCK_MONOTONIC nanoseconds, the clock `clock.monotonic.ns@1` ticks read.
pub(crate) fn monotonic_now() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: now is writable; CLOCK_MONOTONIC always exists on Linux.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    (now.tv_sec as u64) * 1_000_000_000 + now.tv_nsec as u64
}

/// Milliseconds a poll may sleep before `deadline`, rounded up; -1 without one.
pub(crate) fn poll_timeout(deadline: Option<u64>) -> libc::c_int {
    deadline.map_or(-1, |d| {
        let left = d.saturating_sub(monotonic_now());
        left.div_ceil(1_000_000).min(libc::c_int::MAX as u64) as libc::c_int
    })
}

/// Whether `fd` (a cancellation descriptor) is readable now.
pub(crate) fn signaled(fd: Option<RawFd>) -> bool {
    let Some(fd) = fd else { return false };
    let mut row = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: row holds a live descriptor owned by the caller.
    unsafe { libc::poll(&mut row, 1, 0) > 0 }
}

/// The host command for one checked spec: its program and arguments, its exact or
/// inherited environment and nothing else, starting in `cwd` (a pinned directory
/// descriptor) or `/`, with all three standard streams piped.
pub(crate) fn host_command(spec: &ProcessSpec<'_>, cwd: Option<RawFd>) -> std::process::Command {
    use std::process::Stdio;
    let mut command = std::process::Command::new(spec.program);
    command
        .args(spec.args)
        .env_clear()
        .current_dir(cwd.map_or_else(|| "/".to_owned(), |fd| format!("/proc/self/fd/{fd}")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A launcher that ignores SIGINT or SIGTERM must not make its children ignore the
    // signals `child.kill` sends: each starts with their default dispositions.
    // SAFETY: the hook only calls signal(2), which is async-signal-safe after fork.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            for signal in [libc::SIGINT, libc::SIGTERM] {
                libc::signal(signal, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    match &spec.env {
        ChildEnv::Inherit(names) => {
            for name in names {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
        }
        ChildEnv::Exact(vars) => {
            command.envs(vars.iter().map(|(name, value)| (name, value)));
        }
    }
    command
}

/// A pipe whose ends close on exec: (read, write).
fn pipe() -> std::io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    // SAFETY: fds is a writable pair; pipe2 fills it on success.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors are fresh and owned here.
    Ok(unsafe {
        (
            std::os::fd::OwnedFd::from_raw_fd(fds[0]),
            std::os::fd::OwnedFd::from_raw_fd(fds[1]),
        )
    })
}

/// Run a checked run or pipeline on the host to completion, stage `i` starting in
/// `cwds[i]` (a pinned directory descriptor) or `/`. Each stage's standard output feeds
/// the next through a pipe the program never sees, and every stage writes standard error
/// to one shared pipe. The run ends `TimedOut()` at its deadline, and kills every child at
/// once when `cancel` becomes readable (its outcome is then discarded). A stage that
/// cannot start kills and reaps the stages before it.
pub fn run_host(
    run: &HostRun<'_>,
    cwds: &[Option<RawFd>],
    cancel: Option<RawFd>,
) -> PipelineOutcome {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;
    if run.deadline.is_some_and(|d| monotonic_now() >= d) {
        return PipelineOutcome::Error(ProcessFailure::TimedOut);
    }
    let Ok((errors, error_input)) = pipe() else {
        return PipelineOutcome::Error(ProcessFailure::Fault(1));
    };
    let mut children = Stages::new(run.stages[0].own_group);
    let mut input = None;
    let mut previous: Option<std::process::ChildStdout> = None;
    // Every early return drops `children`, which kills and reaps every stage.
    for (spec, cwd) in run.stages.iter().zip(cwds) {
        let mut command = host_command(spec, *cwd);
        if let Some(output) = previous.take() {
            command.stdin(Stdio::from(output));
        }
        let Ok(stderr) = error_input.try_clone() else {
            return PipelineOutcome::Error(ProcessFailure::Fault(1));
        };
        command.stderr(Stdio::from(stderr));
        match children.spawn(&mut command) {
            Ok(child) => {
                if input.is_none() {
                    input = child.stdin.take();
                }
                previous = child.stdout.take();
            }
            Err(error) => return PipelineOutcome::Error(spawn_failure(&error)),
        }
    }
    // Only the children hold the shared error pipe's write end now.
    drop(error_input);
    let streams = without_sigpipe(|| {
        drain(
            input.expect("piped stdin"),
            run.stdin,
            std::fs::File::from(std::os::fd::OwnedFd::from(previous.expect("piped stdout"))),
            std::fs::File::from(errors),
            run.max_output,
            run.deadline,
            cancel,
        )
    });
    let (stdout, stderr) = match streams {
        Ok(Drained::Done(stdout, stderr)) => (stdout, stderr),
        // Over the bound: the children are not trusted to stop writing.
        Ok(Drained::OverLimit) => return PipelineOutcome::Error(ProcessFailure::OutputLimit),
        Ok(Drained::Expired) => {
            children.terminate(cancel);
            return PipelineOutcome::Error(ProcessFailure::TimedOut);
        }
        Ok(Drained::Canceled) => return PipelineOutcome::Error(ProcessFailure::Canceled(0)),
        Err(_) => return PipelineOutcome::Error(ProcessFailure::Fault(1)),
    };
    let statuses = match children.wait(run.deadline, cancel) {
        Ok(Ended::Exited(statuses)) => statuses,
        Ok(Ended::Expired) => {
            children.terminate(cancel);
            return PipelineOutcome::Error(ProcessFailure::TimedOut);
        }
        Ok(Ended::Canceled) => return PipelineOutcome::Error(ProcessFailure::Canceled(0)),
        Err(_) => return PipelineOutcome::Error(ProcessFailure::Fault(1)),
    };
    let mut reported = vec![];
    for status in statuses {
        reported.push(match (status.code(), status.signal()) {
            (Some(code), _) => ExitStatus::Exited(code),
            (None, Some(signal)) => ExitStatus::Signaled(signal),
            (None, None) => return PipelineOutcome::Error(ProcessFailure::Fault(1)),
        });
    }
    PipelineOutcome::Output {
        statuses: reported,
        stdout,
        stderr,
    }
}

/// What a [`HostJob`] ends with: a run's, a pipeline's or a child wait's outcome, or a
/// group wait's.
pub(crate) enum JobOutcome {
    Run(PipelineOutcome),
    Group(Result<GroupOutput, ProcessFailure>),
}

/// A host operation under a cooperative scheduler, on a helper thread. Its completion is
/// sent, then a byte makes `wake` readable, which is the parked request's readiness
/// interest. Dropping the job (a finished or cancelled wait) makes `cancel` readable, so a
/// running child is killed, and joins the thread, so the child is reaped before the drop
/// returns.
pub(crate) struct HostJob {
    wake: std::os::unix::net::UnixStream,
    cancel: std::os::unix::net::UnixStream,
    result: std::sync::mpsc::Receiver<JobOutcome>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl HostJob {
    fn start(body: impl FnOnce(RawFd) -> JobOutcome + Send + 'static) -> std::io::Result<Self> {
        use std::os::unix::net::UnixStream;
        let (wake, signal) = UnixStream::pair()?;
        let (cancel, canceled) = UnixStream::pair()?;
        let (send, result) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("hirsel-process".into())
            .spawn(move || {
                let outcome = body(canceled.as_raw_fd());
                let _ = send.send(outcome);
                let _ = (&signal).write_all(&[1]);
            })?;
        Ok(Self {
            wake,
            cancel,
            result,
            thread: Some(thread),
        })
    }
}
impl Drop for HostJob {
    fn drop(&mut self) {
        let _ = (&self.cancel).write_all(&[1]);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Deliver the entered cooperative token's finished job, or park while it runs: `start`
/// begins it the first time. A job that cannot start is a host fault.
pub(crate) fn cooperative_job(
    readiness: &crate::readiness::Readiness,
    start: impl FnOnce() -> Option<Box<dyn FnOnce(RawFd) -> JobOutcome + Send>>,
) -> Result<JobOutcome, AdapterError> {
    let mut pending = readiness.pending();
    if let Some(job) = &pending.process {
        return match job.result.try_recv() {
            Ok(outcome) => {
                pending.process = None;
                pending.interest = None;
                Ok(outcome)
            }
            Err(_) => Err(AdapterError::Blocked),
        };
    }
    let fault = || JobOutcome::Run(PipelineOutcome::Error(ProcessFailure::Fault(1)));
    let Some(body) = start() else {
        return Ok(fault());
    };
    match HostJob::start(body) {
        Ok(job) => {
            pending.interest = Some((job.wake.as_raw_fd(), libc::POLLIN));
            pending.process = Some(job);
            Err(AdapterError::Blocked)
        }
        Err(_) => Ok(fault()),
    }
}

/// `run_host` for the entered cooperative token, as a [`HostJob`].
pub(crate) fn run_cooperative(
    readiness: &crate::readiness::Readiness,
    run: &HostRun<'_>,
    cwds: &[Option<RawFd>],
) -> Result<PipelineOutcome, AdapterError> {
    cooperative_job(readiness, || {
        let stages: Vec<_> = run
            .stages
            .iter()
            .map(|s| {
                (
                    s.program.to_owned(),
                    s.args.to_vec(),
                    s.env.clone(),
                    s.directory,
                    s.own_group,
                )
            })
            .collect();
        let stdin = run.stdin.to_vec();
        let (max_output, deadline) = (run.max_output, run.deadline);
        // The thread keeps its own descriptors of the working directories.
        let cwds = cwds
            .iter()
            .map(|cwd| match cwd {
                // SAFETY: the pinned directory descriptor is live for this call.
                Some(fd) => unsafe { std::os::fd::BorrowedFd::borrow_raw(*fd) }
                    .try_clone_to_owned()
                    .ok()
                    .map(Some),
                None => Some(None),
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Box::new(move |cancel| {
            let specs: Vec<ProcessSpec<'_>> = stages
                .iter()
                .map(|(program, args, env, directory, own_group)| ProcessSpec {
                    program,
                    args,
                    env: env.clone(),
                    directory: *directory,
                    own_group: *own_group,
                })
                .collect();
            let run = HostRun {
                stages: &specs,
                stdin: &stdin,
                max_output,
                deadline,
            };
            let cwds: Vec<_> = cwds
                .iter()
                .map(|c| c.as_ref().map(AsRawFd::as_raw_fd))
                .collect();
            JobOutcome::Run(run_host(&run, &cwds, Some(cancel)))
        }))
    })
    .map(|outcome| match outcome {
        JobOutcome::Run(outcome) => outcome,
        JobOutcome::Group(_) => PipelineOutcome::Error(ProcessFailure::Fault(1)),
    })
}

/// Run `body` with SIGPIPE blocked on this thread, then discard a SIGPIPE it raised:
/// a child that closes its stdin early must not end the program, whose disposition of
/// SIGPIPE is not the runtime's to change.
pub(crate) fn without_sigpipe<T>(body: impl FnOnce() -> T) -> T {
    // SAFETY: sigset_t values are initialized by sigemptyset before use, and the
    // thread mask is restored before returning.
    unsafe {
        let mut block: libc::sigset_t = std::mem::zeroed();
        let mut previous: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGPIPE);
        let mut pending: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut pending);
        let already = libc::sigismember(&pending, libc::SIGPIPE) == 1;
        libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut previous);
        let result = body();
        if !already {
            let zero = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            while libc::sigtimedwait(&block, std::ptr::null_mut(), &zero) == libc::SIGPIPE {}
        }
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        result
    }
}

/// How draining a child's streams ended.
enum Drained {
    Done(Vec<u8>, Vec<u8>),
    OverLimit,
    Expired,
    Canceled,
}

/// Write all of `input` and read both streams to end of file, concurrently, until a
/// stream passes `max` bytes, `deadline` passes or `cancel` is readable. A child that
/// closes its stdin early only ends the writing.
fn drain(
    stdin: std::process::ChildStdin,
    input: &[u8],
    mut stdout: std::fs::File,
    mut stderr: std::fs::File,
    max: u64,
    deadline: Option<u64>,
    cancel: Option<RawFd>,
) -> std::io::Result<Drained> {
    fn nonblocking(fd: std::os::fd::RawFd) -> std::io::Result<()> {
        // SAFETY: fd is a live pipe descriptor owned by the caller.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    let mut stdin = Some(stdin);
    let mut written = 0;
    if input.is_empty() {
        stdin = None;
    }
    for fd in [
        stdout.as_raw_fd(),
        stderr.as_raw_fd(),
        stdin.as_ref().map_or(-1, AsRawFd::as_raw_fd),
    ] {
        if fd >= 0 {
            nonblocking(fd)?;
        }
    }
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_open, mut err_open) = (true, true);
    let mut chunk = vec![0u8; 64 * 1024];
    while out_open || err_open {
        let mut fds = [
            libc::pollfd {
                fd: if out_open { stdout.as_raw_fd() } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if err_open { stderr.as_raw_fd() } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stdin.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLOUT,
                revents: 0,
            },
            libc::pollfd {
                fd: cancel.unwrap_or(-1),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is a live array of four pollfd records.
        if unsafe { libc::poll(fds.as_mut_ptr(), 4, poll_timeout(deadline)) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fds[3].revents != 0 {
            return Ok(Drained::Canceled);
        }
        if deadline.is_some_and(|d| monotonic_now() >= d) {
            return Ok(Drained::Expired);
        }
        if fds[2].revents != 0
            && let Some(pipe) = &mut stdin
        {
            match pipe.write(&input[written..]) {
                Ok(n) => written += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                // The child closed its input; it simply did not read the rest.
                Err(_) => written = input.len(),
            }
            if written == input.len() {
                stdin = None;
            }
        }
        for (revents, open, pipe, buffer) in [
            (
                fds[0].revents,
                &mut out_open,
                &mut stdout as &mut dyn Read,
                &mut out,
            ),
            (
                fds[1].revents,
                &mut err_open,
                &mut stderr as &mut dyn Read,
                &mut err,
            ),
        ] {
            if revents == 0 {
                continue;
            }
            match pipe.read(&mut chunk) {
                Ok(0) => *open = false,
                Ok(n) => {
                    buffer.extend_from_slice(&chunk[..n]);
                    if buffer.len() as u64 > max {
                        return Ok(Drained::OverLimit);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(Drained::Done(out, err))
}

/// `Result[ProcessOutput, ProcessError]`. Ok: ExitStatus (u32 tag, i32) at 8, stdout at
/// 16, stderr at 40. Error: ProcessError (u32 tag, Fault's u32) at 8.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ProcessResult {
    pub tag: u32,
    pub padding: u32,
    pub kind: u32,
    pub code: u32,
    pub stdout: crate::views::Slice,
    pub stderr: crate::views::Slice,
}

pub(crate) unsafe fn list<T>(
    owner: *mut crate::Header,
    stride: usize,
    mut item: impl FnMut(*const u8) -> T,
) -> Vec<T> {
    let payload = crate::alloc::payload(owner);
    let count = usize::try_from(unsafe { payload.cast::<u64>().read() })
        .unwrap_or_else(|_| crate::fault("process list overflow"));
    let first = crate::alloc::sequence_elements_offset(8) as usize;
    (0..count)
        .map(|index| {
            let offset = index
                .checked_mul(stride)
                .and_then(|n| n.checked_add(first))
                .filter(|n| *n <= isize::MAX as usize)
                .unwrap_or_else(|| crate::fault("process list element overflow"));
            item(unsafe { payload.add(offset) })
        })
        .collect()
}
unsafe fn slice(at: *const u8) -> Vec<u8> {
    let slice = unsafe { at.cast::<crate::views::Slice>().read() };
    unsafe { crate::io::slice_bytes(slice.owner, slice.start, slice.length) }.to_vec()
}
pub(crate) unsafe fn text(at: *const u8) -> String {
    String::from_utf8(unsafe { slice(at) })
        .unwrap_or_else(|_| crate::fault("process String is not UTF-8"))
}
fn failure_parts(failure: ProcessFailure) -> (u32, u32) {
    match failure {
        ProcessFailure::NotGranted => (0, 0),
        ProcessFailure::InvalidInput => (1, 0),
        ProcessFailure::NotFound => (2, 0),
        ProcessFailure::PermissionDenied => (3, 0),
        ProcessFailure::OutputLimit => (4, 0),
        ProcessFailure::Fault(code) => (5, code),
        ProcessFailure::TimedOut => (6, 0),
        ProcessFailure::Canceled(cause) => (7, cause),
        ProcessFailure::InputClosed => (8, 0),
    }
}
/// `ProcessError` from its variant ordinal and payload word.
fn failure_of(kind: u32, code: u32) -> ProcessFailure {
    match (kind, code) {
        (0, _) => ProcessFailure::NotGranted,
        (1, _) => ProcessFailure::InvalidInput,
        (2, _) => ProcessFailure::NotFound,
        (3, _) => ProcessFailure::PermissionDenied,
        (4, _) => ProcessFailure::OutputLimit,
        (5, code) => ProcessFailure::Fault(code),
        (6, _) => ProcessFailure::TimedOut,
        (7, cause) => ProcessFailure::Canceled(cause),
        (8, _) => ProcessFailure::InputClosed,
        _ => crate::fault("ProcessError tag"),
    }
}
fn status_of(kind: u32, code: u32) -> ExitStatus {
    match kind {
        0 => ExitStatus::Exited(code as i32),
        1 => ExitStatus::Signaled(code as i32),
        _ => crate::fault("ExitStatus tag"),
    }
}
/// One `ProcessScript` (96 bytes): program String at 0, args List at 24, outcome
/// `Result[ProcessOutput, ProcessError]` at 32.
unsafe fn script(at: *const u8) -> ProcessRun {
    let outcome = unsafe { at.add(32) };
    let kind = unsafe { outcome.add(8).cast::<u32>().read() };
    let code = unsafe { outcome.add(12).cast::<u32>().read() };
    ProcessRun {
        program: unsafe { text(at) },
        args: unsafe {
            list(at.add(24).cast::<*mut crate::Header>().read(), 24, |a| {
                text(a)
            })
        },
        outcome: match unsafe { outcome.cast::<u32>().read() } {
            0 => ProcessOutcome::Output {
                status: status_of(kind, code),
                stdout: unsafe { slice(outcome.add(16)) },
                stderr: unsafe { slice(outcome.add(40)) },
            },
            1 => ProcessOutcome::Error(failure_of(kind, code)),
            _ => crate::fault("ProcessScript outcome tag"),
        },
    }
}
/// One `ChildScript` (40 bytes): program String at 0, args List at 24, steps List at 32,
/// each `ChildStep` 32 bytes: its tag, then Bytes or `ExitStatus` at 8.
unsafe fn child_script(at: *const u8) -> ChildRun {
    ChildRun {
        program: unsafe { text(at) },
        args: unsafe {
            list(at.add(24).cast::<*mut crate::Header>().read(), 24, |a| {
                text(a)
            })
        },
        steps: unsafe {
            list(at.add(32).cast::<*mut crate::Header>().read(), 32, |step| {
                let payload = step.add(8);
                match step.cast::<u32>().read() {
                    0 => ChildStep::Print(slice(payload)),
                    1 => ChildStep::PrintErr(slice(payload)),
                    2 => ChildStep::Expect(slice(payload)),
                    3 => ChildStep::Exit(status_of(
                        payload.cast::<u32>().read(),
                        payload.add(4).cast::<u32>().read(),
                    )),
                    _ => crate::fault("ChildStep tag"),
                }
            })
        },
    }
}

/// The size of a `Command`, a `List[Command]`'s element stride.
const COMMAND_SIZE: usize = 48;
/// One borrowed `Command` ([`COMMAND_SIZE`] bytes): program Path at 0, args List at 8,
/// `Option[Dir]` (u32 tag, Dir at 24) at 16, `Option[List[(String, String)]]` at 32, a
/// null-niche block pointer whose elements are 48-byte name and value Strings, and the
/// `own_group` bool at 40.
unsafe fn command(ctx: *mut crate::RunCtx, at: *const u8) -> Command {
    let env = unsafe { at.add(32).cast::<*mut crate::Header>().read() };
    Command {
        program: unsafe { crate::io::Binding::get(ctx) }.path(unsafe { at.cast::<u64>().read() }),
        args: unsafe {
            list(at.add(8).cast::<*mut crate::Header>().read(), 24, |a| {
                text(a)
            })
        },
        directory: match unsafe { at.add(16).cast::<u32>().read() } {
            0 => None,
            _ => Some(unsafe { at.add(24).cast::<u64>().read() }),
        },
        env: (!env.is_null())
            .then(|| unsafe { list(env, 48, |var| (text(var), text(var.add(24)))) }),
        own_group: unsafe { at.add(40).read() } != 0,
    }
}

/// One borrowed `Option[Deadline]`: u32 tag, the clock at 8 and the Instant's ticks at 16.
unsafe fn deadline_of(at: *const u8) -> Option<Deadline> {
    match unsafe { at.cast::<u32>().read() } {
        0 => None,
        _ => Some(Deadline {
            clock: unsafe { at.add(8).cast::<u64>().read() },
            ticks: unsafe { at.add(16).cast::<u64>().read() },
        }),
    }
}

/// # Safety
/// Context is bound; Proc, the Command, the Bytes slice and the `Option[Deadline]` (u32
/// tag, the clock at 8 and the Instant's ticks at 16) are borrowed live values; out is
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_proc_run(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ProcessResult,
    process: *const u64,
    cmd: *const u8,
    stdin_owner: *mut crate::Header,
    stdin_start: u64,
    stdin_length: u64,
    max_output: u64,
    deadline: *const u8,
) -> u8 {
    unsafe { crate::io::Binding::get(ctx) }.process(crate::process::host);
    let request = Request::ProcessRun {
        process: unsafe { process.read() },
        command: unsafe { command(ctx, cmd) },
        stdin: unsafe { crate::io::slice_bytes(stdin_owner, stdin_start, stdin_length) }.to_vec(),
        max_output,
        deadline: unsafe { deadline_of(deadline) },
    };
    let Ok(completion) = (unsafe { crate::io::capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Process(outcome) = completion else {
        crate::fault("process completion mismatch")
    };
    // Zero padding and the inactive arm; no pointer is made from error bits.
    let mut result: ProcessResult = unsafe { std::mem::zeroed() };
    match outcome {
        ProcessOutcome::Output {
            status,
            stdout,
            stderr,
        } => {
            (result.kind, result.code) = match status {
                ExitStatus::Exited(code) => (0, code as u32),
                ExitStatus::Signaled(signal) => (1, signal as u32),
            };
            result.stdout = unsafe { crate::text::allocate_bytes(ctx, &stdout) };
            result.stderr = unsafe { crate::text::allocate_bytes(ctx, &stderr) };
        }
        ProcessOutcome::Error(failure) => {
            result.tag = 1;
            (result.kind, result.code) = failure_parts(failure);
        }
    }
    unsafe { out.write(result) };
    crate::STATUS_OK
}

/// `Result[PipelineOutput, ProcessError]`. Ok: the statuses List block at 8, stdout at 16,
/// stderr at 40. Error: ProcessError (u32 tag, Fault's u32) at 8.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PipelineResult {
    pub tag: u32,
    pub padding: u32,
    pub statuses: u64,
    pub stdout: crate::views::Slice,
    pub stderr: crate::views::Slice,
}

/// `proc.pipeline(p, stages, stdin, max_output, deadline)`.
/// # Safety
/// Context is bound; Proc, the List[Command] block, the Bytes slice and the
/// `Option[Deadline]` are borrowed live values; out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_proc_pipeline(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut PipelineResult,
    process: *const u64,
    stages: *mut crate::Header,
    stdin_owner: *mut crate::Header,
    stdin_start: u64,
    stdin_length: u64,
    max_output: u64,
    deadline: *const u8,
) -> u8 {
    unsafe { crate::io::Binding::get(ctx) }.process(crate::process::host);
    let request = Request::ProcessPipeline {
        process: unsafe { process.read() },
        stages: unsafe { list(stages, COMMAND_SIZE, |at| command(ctx, at)) },
        stdin: unsafe { crate::io::slice_bytes(stdin_owner, stdin_start, stdin_length) }.to_vec(),
        max_output,
        deadline: unsafe { deadline_of(deadline) },
    };
    let Ok(completion) = (unsafe { crate::io::capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Pipeline(outcome) = completion else {
        crate::fault("pipeline completion mismatch")
    };
    // Zero padding and the inactive arm; no pointer is made from error bits.
    let mut result: PipelineResult = unsafe { std::mem::zeroed() };
    match outcome {
        PipelineOutcome::Output {
            statuses,
            stdout,
            stderr,
        } => {
            result.statuses = unsafe { status_list(ctx, &statuses) } as u64;
            result.stdout = unsafe { crate::text::allocate_bytes(ctx, &stdout) };
            result.stderr = unsafe { crate::text::allocate_bytes(ctx, &stderr) };
        }
        PipelineOutcome::Error(failure) => {
            result.tag = 1;
            let (kind, code) = failure_parts(failure);
            result.statuses = u64::from(kind) | u64::from(code) << 32;
        }
    }
    unsafe { out.write(result) };
    crate::STATUS_OK
}

/// # Safety
/// Context is bound; Test and the List[ProcessScript] and List[ChildScript] blocks are
/// borrowed live values; out is writable. No borrowed input is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_process(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
    runs: *mut crate::Header,
    children: *mut crate::Header,
) -> u8 {
    let request = Request::TestProcess {
        test: unsafe { test.read() },
        runs: unsafe { list(runs, 96, |at| script(at)) },
        children: unsafe { list(children, 40, |at| child_script(at)) },
    };
    let Ok(completion) = (unsafe { crate::io::capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Handle(Ok(process)) = completion else {
        crate::fault("Test Proc completion mismatch")
    };
    unsafe { out.write(process) };
    crate::STATUS_OK
}

/// `Result[T, ProcessError]` for an eight-byte `T` (a Child or u64): the tag, then the value
/// or the ProcessError (u32 tag, payload u32) at 8; 16 bytes, align 8.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChildWordResult {
    pub tag: u32,
    pub padding: u32,
    pub value: u64,
}
impl ChildWordResult {
    pub fn of(value: Result<u64, ProcessFailure>) -> Self {
        match value {
            Ok(value) => Self {
                value,
                ..Self::default()
            },
            Err(failure) => {
                let (kind, code) = failure_parts(failure);
                Self {
                    tag: 1,
                    padding: 0,
                    value: u64::from(kind) | u64::from(code) << 32,
                }
            }
        }
    }
}
/// `Result[T, ProcessError]` for `T` = `()` or `ExitStatus` (u32 tag, i32): the tag, then
/// the value or the ProcessError at 4; 12 bytes, align 4.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChildPairResult {
    pub tag: u32,
    pub kind: u32,
    pub code: u32,
}
impl ChildPairResult {
    fn unit(value: Result<(), ProcessFailure>) -> Self {
        value.map_or_else(Self::failure, |()| Self::default())
    }
    fn status(value: Result<ExitStatus, ProcessFailure>) -> Self {
        match value {
            Ok(ExitStatus::Exited(code)) => Self {
                tag: 0,
                kind: 0,
                code: code as u32,
            },
            Ok(ExitStatus::Signaled(signal)) => Self {
                tag: 0,
                kind: 1,
                code: signal as u32,
            },
            Err(failure) => Self::failure(failure),
        }
    }
    fn failure(failure: ProcessFailure) -> Self {
        let (kind, code) = failure_parts(failure);
        Self { tag: 1, kind, code }
    }
}

/// `proc.narrow(p: Proc, rules: List[ProcessRule]) -> Result[Proc, ProcessError]`. Each
/// `ProcessRule` is 32 bytes: the program String at 0 and its env List[String] at 24.
/// # Safety
/// Context is bound; the Proc is readable, rules a live List block; out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_proc_narrow(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ChildWordResult,
    process: *const u64,
    rules: *mut crate::Header,
) -> u8 {
    let request = Request::ProcessNarrow {
        process: unsafe { process.read() },
        rules: unsafe {
            list(rules, 32, |at| ProcessRule {
                program: text(at),
                env: list(at.add(24).cast::<*mut crate::Header>().read(), 24, |name| {
                    text(name)
                }),
            })
        },
    };
    let Ok(Completion::Spawned(value)) = (unsafe { crate::io::capability_call(ctx, request) })
    else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(ChildWordResult::of(value)) };
    crate::STATUS_OK
}
/// `proc.spawn(p, cmd) -> Result[Child of p, ProcessError]`.
/// # Safety
/// Context is bound; Proc and the Command are borrowed live values; out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_proc_spawn(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ChildWordResult,
    process: *const u64,
    cmd: *const u8,
) -> u8 {
    unsafe { crate::io::Binding::get(ctx) }.process(crate::process::host);
    let request = Request::ProcessSpawn {
        process: unsafe { process.read() },
        command: unsafe { command(ctx, cmd) },
    };
    let Ok(Completion::Spawned(value)) = (unsafe { crate::io::capability_call(ctx, request) })
    else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(ChildWordResult::of(value)) };
    crate::STATUS_OK
}

/// `child.write(p, mut c, data: Bytes) -> Result[(), ProcessError]`; a scripted child's
/// contradicted Expect fails LANG-9.
/// # Safety
/// Context is bound; p readable; c the exclusive address of a live Child; the data slice
/// borrowed; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_child_write(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ChildPairResult,
    p: *const u64,
    c: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    length: u64,
) -> u8 {
    let request = Request::ChildWrite {
        process: unsafe { p.read() },
        child: unsafe { c.read() },
        bytes: unsafe { crate::io::slice_bytes(owner, start, length) }.to_vec(),
    };
    let Some(Completion::ChildUnit(value)) =
        (unsafe { crate::net_abi::call_recording(ctx, request) })
    else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(ChildPairResult::unit(value)) };
    crate::STATUS_OK
}

/// `child.close_stdin(p, mut c)`; closing at a scripted child's unmet Expect fails LANG-9.
/// # Safety
/// Context is bound; p readable; c the exclusive address of a live Child.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_child_close_stdin(
    ctx: *mut crate::RunCtx,
    p: *const u64,
    c: *mut u64,
) -> u8 {
    let request = Request::ChildCloseStdin {
        process: unsafe { p.read() },
        child: unsafe { c.read() },
    };
    match unsafe { crate::net_abi::call_recording(ctx, request) } {
        Some(Completion::ChildUnit(Ok(()))) => crate::STATUS_OK,
        Some(_) => crate::fault("child completion mismatch"),
        None => crate::STATUS_FAILURE,
    }
}

/// A child read appending to the caller's list, growing it through `hirsel_rt_v2_seq_grow`.
unsafe fn child_read(
    ctx: *mut crate::RunCtx,
    out: *mut ChildWordResult,
    p: *const u64,
    c: *mut u64,
    buffer: *mut *mut crate::Header,
    max: u64,
    stream: ChildStream,
) -> u8 {
    let request = Request::ChildRead {
        process: unsafe { p.read() },
        child: unsafe { c.read() },
        stream,
        max,
    };
    let Ok(Completion::ChildBytes(value)) = (unsafe { crate::io::capability_call(ctx, request) })
    else {
        return crate::STATUS_FAILURE;
    };
    let value = value.map(|bytes| {
        unsafe { crate::io::append_byte_list(ctx, buffer, &bytes) };
        bytes.len() as u64
    });
    unsafe { out.write(ChildWordResult::of(value)) };
    crate::STATUS_OK
}

/// `child.read_stdout(p, mut c, mut buffer: List[u8], max) -> Result[u64, ProcessError]`.
/// # Safety
/// Context is bound; p readable; c and buffer exclusive addresses of live values; out
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_child_read_stdout(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ChildWordResult,
    p: *const u64,
    c: *mut u64,
    buffer: *mut *mut crate::Header,
    max: u64,
) -> u8 {
    unsafe { child_read(ctx, out, p, c, buffer, max, ChildStream::Stdout) }
}

/// `child.read_stderr`, as [`hirsel_rt_v2_child_read_stdout`].
/// # Safety
/// As [`hirsel_rt_v2_child_read_stdout`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_child_read_stderr(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ChildWordResult,
    p: *const u64,
    c: *mut u64,
    buffer: *mut *mut crate::Header,
    max: u64,
) -> u8 {
    unsafe { child_read(ctx, out, p, c, buffer, max, ChildStream::Stderr) }
}

/// `child.wait(p, take c, deadline) -> Result[ExitStatus, ProcessError]`; the Child is
/// consumed on every outcome, and closing at a scripted child's unmet Expect fails LANG-9.
/// # Safety
/// Context is bound; p readable; c the address of the transferred Child word; the
/// `Option[Deadline]` (u32 tag, the clock at 8 and the Instant's ticks at 16) borrowed;
/// out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_child_wait(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ChildPairResult,
    p: *const u64,
    c: *mut u64,
    deadline: *const u8,
) -> u8 {
    let request = Request::ChildWait {
        process: unsafe { p.read() },
        child: unsafe { c.read() },
        deadline: unsafe { deadline_of(deadline) },
    };
    // Erasing the transferred word makes accidental reuse fail closed.
    unsafe { c.write(0) };
    let Some(Completion::ChildExit(value)) =
        (unsafe { crate::net_abi::call_recording(ctx, request) })
    else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(ChildPairResult::status(value)) };
    crate::STATUS_OK
}

/// `child.kill(p, mut c, signal)`.
/// # Safety
/// Context is bound; p readable; c the exclusive address of a live Child; signal the
/// borrowed `Signal` ordinal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_child_kill(
    ctx: *mut crate::RunCtx,
    p: *const u64,
    c: *mut u64,
    signal: *const u32,
) -> u8 {
    let request = Request::ChildKill {
        process: unsafe { p.read() },
        child: unsafe { c.read() },
        signal: Signal::of_ordinal(unsafe { signal.read() })
            .unwrap_or_else(|| crate::fault("Signal tag")),
    };
    match unsafe { crate::io::capability_call(ctx, request) } {
        Ok(Completion::ChildUnit(Ok(()))) => crate::STATUS_OK,
        Ok(_) => crate::fault("child completion mismatch"),
        Err(()) => crate::STATUS_FAILURE,
    }
}

/// `proc.spawn_pipeline(p, stages, max_stderr) -> Result[ChildGroup of p, ProcessError]`;
/// a scripted stage whose first output contradicts the next stage's Expect fails LANG-9.
/// # Safety
/// Context is bound; Proc and the List[Command] block are borrowed live values; out is
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_proc_spawn_pipeline(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ChildWordResult,
    process: *const u64,
    stages: *mut crate::Header,
    max_stderr: u64,
) -> u8 {
    unsafe { crate::io::Binding::get(ctx) }.process(crate::process::host);
    let request = Request::ProcessSpawnPipeline {
        process: unsafe { process.read() },
        stages: unsafe { list(stages, COMMAND_SIZE, |at| command(ctx, at)) },
        max_stderr,
    };
    let Some(Completion::Spawned(value)) =
        (unsafe { crate::net_abi::call_recording(ctx, request) })
    else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(ChildWordResult::of(value)) };
    crate::STATUS_OK
}

/// `Result[GroupOutput, ProcessError]`. Ok: the statuses List block at 8 and the per-stage
/// stderr `List[Bytes]` block at 16. Error: ProcessError (u32 tag, Fault's u32) at 8.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GroupResult {
    pub tag: u32,
    pub padding: u32,
    pub statuses: u64,
    pub stderr: u64,
}

/// A fresh `List[ExitStatus]`: pointer-free (u32 tag, i32 at 4), like the runtime address
/// list.
unsafe fn status_list(ctx: *mut crate::RunCtx, statuses: &[ExitStatus]) -> *mut crate::Header {
    let block = unsafe { crate::alloc::allocate_sequence(ctx, 0, statuses.len() as u64, 8, 4) };
    let first = crate::alloc::sequence_elements_offset(4) as usize;
    for (index, status) in statuses.iter().enumerate() {
        let (kind, code) = match *status {
            ExitStatus::Exited(code) => (0u32, code as u32),
            ExitStatus::Signaled(signal) => (1, signal as u32),
        };
        unsafe {
            let slot = crate::alloc::payload(block).add(first + index * 8);
            slot.cast::<u32>().write(kind);
            slot.add(4).cast::<u32>().write(code);
        }
    }
    block
}

/// `proc.wait_group(p, take g, deadline) -> Result[GroupOutput, ProcessError]`; the
/// ChildGroup is consumed on every outcome, and closing at a scripted stage's unmet Expect
/// fails LANG-9.
/// # Safety
/// Context is bound; p readable; g the address of the transferred ChildGroup word; the
/// `Option[Deadline]` borrowed; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_proc_wait_group(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut GroupResult,
    p: *const u64,
    g: *mut u64,
    deadline: *const u8,
) -> u8 {
    let request = Request::GroupWait {
        process: unsafe { p.read() },
        group: unsafe { g.read() },
        deadline: unsafe { deadline_of(deadline) },
    };
    // Erasing the transferred word makes accidental reuse fail closed.
    unsafe { g.write(0) };
    let Some(Completion::GroupExit(value)) =
        (unsafe { crate::net_abi::call_recording(ctx, request) })
    else {
        return crate::STATUS_FAILURE;
    };
    // Zero padding and the inactive arm; no pointer is made from error bits.
    let mut result: GroupResult = unsafe { std::mem::zeroed() };
    match value {
        Ok(output) => {
            result.statuses = unsafe { status_list(ctx, &output.statuses) } as u64;
            result.stderr = unsafe { crate::text::allocate_bytes_list(ctx, &output.stderr) } as u64;
        }
        Err(failure) => {
            result.tag = 1;
            let (kind, code) = failure_parts(failure);
            result.statuses = u64::from(kind) | u64::from(code) << 32;
        }
    }
    unsafe { out.write(result) };
    crate::STATUS_OK
}
