//! The reference process provider: scripted runs (`test.process`, `process.script@1`)
//! and host completions supplied by an adapter. Nothing here spawns a program.
//! The exit-status, argument-vector and output conventions follow Rust
//! library/std/src/process.rs (`Command`, `Output`, `ExitStatus`).
use super::{
    collection,
    dispatch::{arg, bytes, encode_arguments, encode_value, error, i32_value, ok, text, variant},
    simulation::{Grant, Simulation},
};
use crate::{Error, OperationCall, OperationContext, OperationResult, Value, invalid};
use hirsel_run_config::{
    ChildRun, ChildStep, ChildStream, Command, ExitStatus, GroupOutput, PipelineOutcome,
    ProcessFailure, ProcessGrant, ProcessOutcome, ProcessRule, ProcessRun, ProviderContract,
    ScriptChild, ScriptGroup, ScriptRead, Signal, canonical_program, script_pipeline, scriptable,
    take_children,
};
use hirsel_types::{BuiltinType, Primitive};
use std::collections::BTreeMap;

/// The unused script entries of one scripted world, in order.
pub(super) type World = Vec<Option<ProcessRun>>;
/// The unused streaming-child entries of one scripted world, in order.
pub(super) type Children = Vec<Option<ChildRun>>;

pub(super) fn configured_world(
    grant: Option<&ProcessGrant>,
) -> Result<(BTreeMap<u64, World>, BTreeMap<u64, Children>), Error> {
    let children = grant
        .and_then(|g| g.children.clone())
        .unwrap_or_default()
        .into_iter()
        .map(Some)
        .collect();
    let runs = match grant {
        None => vec![],
        Some(grant) => {
            if grant.rules.iter().any(|r| !canonical_program(&r.program)) {
                return Err(invalid("invalid simulation process rules"));
            }
            match (ProviderContract::parse(&grant.provider), &grant.runs) {
                (Some(ProviderContract::ProcessScript), Some(runs)) => {
                    if runs
                        .iter()
                        .any(|run| !canonical_program(&run.program) || !scriptable(&run.outcome))
                        || grant
                            .children
                            .iter()
                            .flatten()
                            .any(|child| !canonical_program(&child.program))
                    {
                        return Err(invalid("invalid simulation process script"));
                    }
                    runs.clone()
                }
                // A host adapter supplies every granted completion of process.spawn@1.
                (Some(ProviderContract::ProcessSpawn), None) => vec![],
                _ => return Err(invalid("invalid simulation process provider")),
            }
        }
    };
    Ok((
        BTreeMap::from([(0, runs.into_iter().map(Some).collect())]),
        BTreeMap::from([(0, children)]),
    ))
}

/// `ProcessError`'s variant ordinal and Fault payload.
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
fn failure_of(ordinal: u32, code: u32) -> Option<ProcessFailure> {
    Some(match (ordinal, code) {
        (0, 0) => ProcessFailure::NotGranted,
        (1, 0) => ProcessFailure::InvalidInput,
        (2, 0) => ProcessFailure::NotFound,
        (3, 0) => ProcessFailure::PermissionDenied,
        (4, 0) => ProcessFailure::OutputLimit,
        (5, code) => ProcessFailure::Fault(code),
        (6, 0) => ProcessFailure::TimedOut,
        (7, cause) if cause <= super::tasks::CANCEL_DEADLINE => ProcessFailure::Canceled(cause),
        (8, 0) => ProcessFailure::InputClosed,
        _ => return None,
    })
}
/// `ProcessError` as a value.
pub fn failure_value(failure: ProcessFailure) -> Value {
    let (ordinal, code) = failure_parts(failure);
    variant(
        ordinal,
        match failure {
            ProcessFailure::Fault(_) => vec![super::dispatch::u32_value(code)],
            ProcessFailure::Canceled(_) => vec![variant(code, vec![])],
            _ => vec![],
        },
    )
}

/// The recorded completion: `0, status tag, i32, stdout, stderr` with u64-framed
/// streams, or `1, ordinal, code`. Integers are little endian.
pub fn encode_completion(outcome: &ProcessOutcome) -> Vec<u8> {
    match outcome {
        ProcessOutcome::Output {
            status,
            stdout,
            stderr,
        } => {
            let (tag, code) = match status {
                ExitStatus::Exited(code) => (0, code),
                ExitStatus::Signaled(signal) => (1, signal),
            };
            let mut out = vec![0, tag];
            out.extend_from_slice(&code.to_le_bytes());
            for stream in [stdout, stderr] {
                out.extend_from_slice(&(stream.len() as u64).to_le_bytes());
                out.extend_from_slice(stream);
            }
            out
        }
        ProcessOutcome::Error(failure) => {
            let (ordinal, code) = failure_parts(*failure);
            let mut out = vec![1];
            out.extend_from_slice(&ordinal.to_le_bytes());
            out.extend_from_slice(&code.to_le_bytes());
            out
        }
    }
}
pub fn decode_completion(bytes: &[u8]) -> Option<ProcessOutcome> {
    fn word<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
        bytes.get(at..at + N)?.try_into().ok()
    }
    match *bytes.first()? {
        0 => {
            let code = i32::from_le_bytes(word(bytes, 2)?);
            let status = match *bytes.get(1)? {
                0 => ExitStatus::Exited(code),
                1 => ExitStatus::Signaled(code),
                _ => return None,
            };
            let mut at = 6;
            let mut streams = vec![];
            for _ in 0..2 {
                let len = usize::try_from(u64::from_le_bytes(word(bytes, at)?)).ok()?;
                at += 8;
                streams.push(bytes.get(at..at.checked_add(len)?)?.to_vec());
                at += len;
            }
            let stderr = streams.pop()?;
            let stdout = streams.pop()?;
            (at == bytes.len()).then_some(ProcessOutcome::Output {
                status,
                stdout,
                stderr,
            })
        }
        1 if bytes.len() == 9 => failure_of(
            u32::from_le_bytes(word(bytes, 1)?),
            u32::from_le_bytes(word(bytes, 5)?),
        )
        .map(ProcessOutcome::Error),
        _ => None,
    }
}

/// The recorded pipeline completion: `0`, the u64 status count, each status as a tag byte
/// and its i32, then u64-framed stdout and stderr; or `1, ordinal, code`. Integers are
/// little endian.
pub fn encode_pipeline(outcome: &PipelineOutcome) -> Vec<u8> {
    match outcome {
        PipelineOutcome::Output {
            statuses,
            stdout,
            stderr,
        } => {
            let mut out = vec![0];
            out.extend_from_slice(&(statuses.len() as u64).to_le_bytes());
            for status in statuses {
                let (tag, code) = match status {
                    ExitStatus::Exited(code) => (0, code),
                    ExitStatus::Signaled(signal) => (1, signal),
                };
                out.push(tag);
                out.extend_from_slice(&code.to_le_bytes());
            }
            for stream in [stdout, stderr] {
                out.extend_from_slice(&(stream.len() as u64).to_le_bytes());
                out.extend_from_slice(stream);
            }
            out
        }
        PipelineOutcome::Error(failure) => encode_completion(&ProcessOutcome::Error(*failure)),
    }
}
pub fn decode_pipeline(bytes: &[u8]) -> Option<PipelineOutcome> {
    fn word<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
        bytes.get(at..at.checked_add(N)?)?.try_into().ok()
    }
    if *bytes.first()? != 0 {
        return decode_completion(bytes).map(PipelineOutcome::from);
    }
    let count = usize::try_from(u64::from_le_bytes(word(bytes, 1)?)).ok()?;
    let mut at = 9;
    let mut statuses = vec![];
    for _ in 0..count {
        let code = i32::from_le_bytes(word(bytes, at + 1)?);
        statuses.push(match *bytes.get(at)? {
            0 => ExitStatus::Exited(code),
            1 => ExitStatus::Signaled(code),
            _ => return None,
        });
        at += 5;
    }
    let mut streams = vec![];
    for _ in 0..2 {
        let len = usize::try_from(u64::from_le_bytes(word(bytes, at)?)).ok()?;
        at += 8;
        streams.push(bytes.get(at..at.checked_add(len)?)?.to_vec());
        at += len;
    }
    let stderr = streams.pop()?;
    let stdout = streams.pop()?;
    (at == bytes.len()).then_some(PipelineOutcome::Output {
        statuses,
        stdout,
        stderr,
    })
}

/// `Result[GroupOutput, ProcessError]` for one completion.
pub fn group_value(outcome: &Result<GroupOutput, ProcessFailure>) -> Value {
    match outcome {
        Ok(GroupOutput { statuses, stderr }) => ok(Value::Record {
            definition: hirsel_syntax::DefPath::core("GroupOutput"),
            fields: vec![
                Some(collection::sequence(
                    BuiltinType::List,
                    statuses.iter().map(|s| status_value(*s)),
                )),
                Some(collection::sequence(
                    BuiltinType::List,
                    stderr.iter().map(|e| Value::Bytes(e.clone())),
                )),
            ],
        }),
        Err(failure) => error(failure_value(*failure)),
    }
}

/// `Result[PipelineOutput, ProcessError]` for one completion.
pub fn pipeline_value(outcome: &PipelineOutcome) -> Value {
    match outcome {
        PipelineOutcome::Output {
            statuses,
            stdout,
            stderr,
        } => ok(Value::Record {
            definition: hirsel_syntax::DefPath::core("PipelineOutput"),
            fields: vec![
                Some(collection::sequence(
                    BuiltinType::List,
                    statuses.iter().map(|s| status_value(*s)),
                )),
                Some(Value::Bytes(stdout.clone())),
                Some(Value::Bytes(stderr.clone())),
            ],
        }),
        PipelineOutcome::Error(failure) => error(failure_value(*failure)),
    }
}

/// `Result[ProcessOutput, ProcessError]` for one completion.
pub fn outcome_value(outcome: &ProcessOutcome) -> Value {
    match outcome {
        ProcessOutcome::Output {
            status,
            stdout,
            stderr,
        } => {
            let status = match status {
                ExitStatus::Exited(code) => variant(0, vec![i32_value(*code)]),
                ExitStatus::Signaled(signal) => variant(1, vec![i32_value(*signal)]),
            };
            ok(Value::Record {
                definition: hirsel_syntax::DefPath::core("ProcessOutput"),
                fields: vec![
                    Some(status),
                    Some(Value::Bytes(stdout.clone())),
                    Some(Value::Bytes(stderr.clone())),
                ],
            })
        }
        ProcessOutcome::Error(failure) => error(failure_value(*failure)),
    }
}

fn record_fields<'a>(v: &'a Value, name: &str, len: usize) -> Result<Vec<&'a Value>, Error> {
    if let Value::Record { definition, fields } = v
        && *definition == hirsel_syntax::DefPath::core(name)
        && fields.len() == len
    {
        return fields
            .iter()
            .map(|f| f.as_ref().ok_or_else(|| invalid("dead process field")))
            .collect();
    }
    Err(invalid(format!("{name} representation")))
}
fn variant_fields(v: &Value) -> Result<(u32, Vec<&Value>), Error> {
    match v {
        Value::Variant { variant, fields } => Ok((
            *variant,
            fields
                .iter()
                .map(|f| f.as_ref().ok_or_else(|| invalid("dead process payload")))
                .collect::<Result<_, _>>()?,
        )),
        _ => Err(invalid("process variant representation")),
    }
}
fn strings(v: &Value) -> Result<Vec<String>, Error> {
    collection::elements(v, BuiltinType::List)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
fn int32(v: &Value) -> Result<i32, Error> {
    match v {
        Value::Integer(n) if n.kind() == Primitive::I32 => Ok(n.bits() as u32 as i32),
        _ => Err(invalid("expected i32")),
    }
}
fn script(v: &Value) -> Result<ProcessRun, Error> {
    let f = record_fields(v, "ProcessScript", 3)?;
    let program = text(f[0])?.to_owned();
    if !canonical_program(&program) {
        return Err(invalid("test.process noncanonical program"));
    }
    let outcome = match variant_fields(f[2])? {
        (0, payload) if payload.len() == 1 => {
            let out = record_fields(payload[0], "ProcessOutput", 3)?;
            let status = match variant_fields(out[0])? {
                (0, code) if code.len() == 1 => ExitStatus::Exited(int32(code[0])?),
                (1, signal) if signal.len() == 1 => ExitStatus::Signaled(int32(signal[0])?),
                _ => return Err(invalid("ExitStatus representation")),
            };
            ProcessOutcome::Output {
                status,
                stdout: bytes(out[1])?.to_vec(),
                stderr: bytes(out[2])?.to_vec(),
            }
        }
        (1, payload) if payload.len() == 1 => {
            let (ordinal, code) = variant_fields(payload[0])?;
            let code = match code.as_slice() {
                [] => 0,
                [Value::Integer(n)] if n.kind() == Primitive::U32 => n.bits() as u32,
                [cause] => variant_fields(cause)?.0,
                _ => return Err(invalid("ProcessError representation")),
            };
            ProcessOutcome::Error(
                failure_of(ordinal, code).ok_or_else(|| invalid("ProcessError ordinal"))?,
            )
        }
        _ => return Err(invalid("ProcessScript outcome representation")),
    };
    if !scriptable(&outcome) {
        return Err(invalid("test.process canceled script outcome"));
    }
    Ok(ProcessRun {
        program,
        args: strings(f[1])?,
        outcome,
    })
}
fn exit_status(v: &Value) -> Result<ExitStatus, Error> {
    match variant_fields(v)? {
        (0, code) if code.len() == 1 => Ok(ExitStatus::Exited(int32(code[0])?)),
        (1, signal) if signal.len() == 1 => Ok(ExitStatus::Signaled(int32(signal[0])?)),
        _ => Err(invalid("ExitStatus representation")),
    }
}
fn child_script(v: &Value) -> Result<ChildRun, Error> {
    let f = record_fields(v, "ChildScript", 3)?;
    let program = text(f[0])?.to_owned();
    if !canonical_program(&program) {
        return Err(invalid("test.process noncanonical program"));
    }
    let steps = collection::elements(f[2], BuiltinType::List)?
        .iter()
        .map(|step| {
            Ok(match variant_fields(step)? {
                (0, b) if b.len() == 1 => ChildStep::Print(bytes(b[0])?.to_vec()),
                (1, b) if b.len() == 1 => ChildStep::PrintErr(bytes(b[0])?.to_vec()),
                (2, b) if b.len() == 1 => ChildStep::Expect(bytes(b[0])?.to_vec()),
                (3, s) if s.len() == 1 => ChildStep::Exit(exit_status(s[0])?),
                _ => return Err(invalid("ChildStep representation")),
            })
        })
        .collect::<Result<_, Error>>()?;
    Ok(ChildRun {
        program,
        args: strings(f[1])?,
        steps,
    })
}
/// `Option[Deadline]`: the clock handle's id and the Instant's ticks.
fn deadline(v: &Value) -> Result<Option<(&Value, u64)>, Error> {
    match variant_fields(v)? {
        (0, d) if d.is_empty() => Ok(None),
        (1, d) if d.len() == 1 => {
            let f = record_fields(d[0], "Deadline", 2)?;
            match f[1] {
                Value::Handle { kind, id } if kind == "Instant" => Ok(Some((f[0], *id))),
                _ => Err(invalid("invalid Instant representation")),
            }
        }
        _ => Err(invalid("Option[Deadline] representation")),
    }
}

impl Simulation {
    /// A `Command` value, its Dir checked to be one.
    fn command(&self, v: &Value) -> Result<Command, Error> {
        let f = record_fields(v, "Command", 4)?;
        let directory = match variant_fields(f[2])? {
            (0, d) if d.is_empty() => None,
            (1, d) if d.len() == 1 => Some(self.id(d[0], "Dir")?),
            _ => return Err(invalid("Option[Dir] representation")),
        };
        let env = match variant_fields(f[3])? {
            (0, e) if e.is_empty() => None,
            (1, e) if e.len() == 1 => Some(
                collection::elements(e[0], BuiltinType::List)?
                    .iter()
                    .map(|var| match var {
                        Value::Tuple(pair) => match pair.as_slice() {
                            [Some(name), Some(value)] => {
                                Ok((text(name)?.to_owned(), text(value)?.to_owned()))
                            }
                            _ => Err(invalid("environment variable representation")),
                        },
                        _ => Err(invalid("environment variable representation")),
                    })
                    .collect::<Result<_, Error>>()?,
            ),
            _ => return Err(invalid("Option[List[(String, String)]] representation")),
        };
        Ok(Command {
            program: super::dispatch::path_text(f[0])?.to_owned(),
            args: strings(f[1])?,
            directory,
            env,
        })
    }
    fn process_grant(&self, v: &Value) -> Result<(u64, Vec<ProcessRule>), Error> {
        let id = self.id(v, "Proc")?;
        let Some(Grant::Proc { world, rules }) = self.grants.get(&id) else {
            return Err(invalid("Proc grant"));
        };
        Ok((*world, rules.clone()))
    }
    pub(super) fn process_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        cx: &mut OperationContext<'_>,
        canceled: Option<u32>,
    ) -> Result<OperationResult, Error> {
        let a = call.arguments;
        let value = match name {
            "io.process" => {
                self.id(arg(a, 0)?, "Io")?;
                let value = if self.config.projections.contains("process") {
                    let rules = self
                        .config
                        .process
                        .as_ref()
                        .map(|p| p.rules.clone())
                        .unwrap_or_default();
                    ok(self.handle("Proc", Grant::Proc { world: 0, rules }))
                } else {
                    error(variant(0, vec![]))
                };
                self.record("Io", name, encode_arguments(a), encode_value(&value), cx)?;
                value
            }
            "test.process" => {
                self.id(arg(a, 0)?, "Test")?;
                let runs = collection::elements(arg(a, 1)?, BuiltinType::List)?
                    .iter()
                    .map(script)
                    .collect::<Result<Vec<_>, _>>()?;
                let children = collection::elements(arg(a, 2)?, BuiltinType::List)?
                    .iter()
                    .map(child_script)
                    .collect::<Result<Vec<_>, _>>()?;
                let mut programs: Vec<_> = runs
                    .iter()
                    .map(|r| r.program.clone())
                    .chain(children.iter().map(|c| c.program.clone()))
                    .collect();
                programs.sort();
                programs.dedup();
                let rules = programs
                    .into_iter()
                    .map(|program| ProcessRule {
                        program,
                        env: vec![],
                    })
                    .collect();
                let world = self.next_id;
                self.process_worlds
                    .insert(world, runs.into_iter().map(Some).collect());
                self.process_children
                    .insert(world, children.into_iter().map(Some).collect());
                let value = self.handle("Proc", Grant::Proc { world, rules });
                self.record("Test", name, encode_arguments(a), encode_value(&value), cx)?;
                value
            }
            "proc.run" | "proc.pipeline" => {
                let (world, rules) = self.process_grant(arg(a, 0)?)?;
                let stages = if name == "proc.run" {
                    vec![self.command(arg(a, 1)?)?]
                } else {
                    collection::elements(arg(a, 1)?, BuiltinType::List)?
                        .iter()
                        .map(|stage| self.command(stage))
                        .collect::<Result<Vec<_>, _>>()?
                };
                bytes(arg(a, 2)?)?;
                let with_dir = stages.iter().any(|s| s.directory.is_some());
                let max_output = match arg(a, 3)? {
                    Value::Integer(n) if n.kind() == Primitive::U64 => n.bits() as u64,
                    _ => return Err(invalid("max_output requires u64")),
                };
                // The clock is only the deadline's owner: a host adapter's handle serves too.
                let deadline = deadline(arg(a, 4)?)?;
                if deadline.is_some_and(
                    |(clock, _)| !matches!(clock, Value::Handle { kind, .. } if kind == "MonotonicClock"),
                ) {
                    return Err(invalid("expected MonotonicClock"));
                }
                let deadline = deadline.map(|(_, ticks)| ticks);
                // Every stage's checks, in stage order, precede cancellation.
                let checked = if stages.is_empty() {
                    Err(ProcessFailure::InvalidInput)
                } else {
                    stages
                        .iter()
                        .try_for_each(|stage| stage.check(&rules).map(|_| ()))
                }
                .and_then(|()| {
                    canceled.map_or(Ok(()), |cause| Err(ProcessFailure::Canceled(cause)))
                });
                let request = encode_arguments(a);
                let outcome = if let Some(recorded) = self.replay_output("Proc", name, &request)? {
                    // Replay re-derives the program checks; a contradicting completion refuses.
                    let outcome = if name == "proc.run" {
                        decode_completion(recorded).map(PipelineOutcome::from)
                    } else {
                        decode_pipeline(recorded)
                    }
                    .ok_or_else(|| invalid("invalid process tape completion"))?;
                    let consistent = match (&checked, &outcome) {
                        (Err(expected), PipelineOutcome::Error(found)) => expected == found,
                        (Err(_), _) => false,
                        (
                            Ok(()),
                            PipelineOutcome::Error(
                                ProcessFailure::NotGranted
                                | ProcessFailure::Canceled(_)
                                | ProcessFailure::InputClosed,
                            ),
                        ) => false,
                        (Ok(()), PipelineOutcome::Error(ProcessFailure::InvalidInput)) => with_dir,
                        (Ok(()), PipelineOutcome::Error(ProcessFailure::TimedOut)) => {
                            deadline.is_some()
                        }
                        (Ok(()), PipelineOutcome::Error(_)) => true,
                        (Ok(()), PipelineOutcome::Output { statuses, .. }) => {
                            statuses.len() == stages.len()
                                && outcome.clone().bounded(max_output) == outcome
                        }
                    };
                    if !consistent {
                        return Err(invalid("process tape completion contradicts its grant"));
                    }
                    outcome
                } else if let Err(failure) = checked {
                    PipelineOutcome::Error(failure)
                } else if world == 0
                    && self.config.process.as_ref().is_some_and(|p| {
                        ProviderContract::parse(&p.provider) == Some(ProviderContract::ProcessSpawn)
                    })
                {
                    return Err(Error::Unavailable(format!(
                        "the reference runs host programs only through a host adapter; simulation requires {}",
                        ProviderContract::ProcessScript.name()
                    )));
                } else if deadline.is_some_and(|ticks| ticks <= self.monotonic_now()) {
                    // A scripted run takes no time: only a reached deadline times it out.
                    PipelineOutcome::Error(ProcessFailure::TimedOut)
                } else {
                    let runs = self
                        .process_worlds
                        .get_mut(&world)
                        .ok_or_else(|| invalid("missing process world"))?;
                    script_pipeline(runs, &stages, max_output)
                };
                if name == "proc.run" {
                    let outcome = outcome.into_run();
                    self.record("Proc", name, request, encode_completion(&outcome), cx)?;
                    outcome_value(&outcome)
                } else {
                    self.record("Proc", name, request, encode_pipeline(&outcome), cx)?;
                    pipeline_value(&outcome)
                }
            }
            "proc.spawn"
            | "child.write"
            | "child.close_stdin"
            | "child.read_stdout"
            | "child.read_stderr"
            | "child.wait"
            | "child.kill"
            | "proc.spawn_pipeline"
            | "group.write"
            | "group.close_stdin"
            | "group.read_stdout"
            | "proc.wait_group"
            | "group.kill" => {
                return self.child_call(name, call, cx, canceled);
            }
            _ => return Err(invalid("unknown process operation")),
        };
        Ok(OperationResult::returned(value))
    }
    /// A network or process operation: either can wait at a scripted peer or child.
    pub(super) fn waiting_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        cx: &mut OperationContext<'_>,
        canceled: Option<u32>,
    ) -> Result<OperationResult, Error> {
        if name.starts_with("proc.")
            || name.starts_with("child.")
            || name.starts_with("group.")
            || matches!(name, "io.process" | "test.process")
        {
            self.process_call(name, call, cx, canceled)
        } else {
            self.net_call(name, call, cx, canceled)
        }
    }
    /// The Child (or with `kind` "ChildGroup" the ChildGroup) `c` of Proc `p` and whether
    /// its input is open.
    fn child_grant(&self, p: &Value, c: &Value, kind: &str) -> Result<(u64, bool), Error> {
        let process = self.id(p, "Proc")?;
        let child = self.id(c, kind)?;
        match self.grants.get(&child) {
            Some(Grant::Child {
                process: owner,
                stdin_open,
                ..
            }) if *owner == process => Ok((child, *stdin_open)),
            _ => Err(invalid("Child of another Proc")),
        }
    }
    fn script_group(&mut self, group: u64) -> Result<&mut ScriptGroup, Error> {
        self.script_groups
            .get_mut(&group)
            .ok_or_else(|| invalid("missing scripted group"))
    }
    fn script_child(&mut self, child: u64) -> Result<&mut ScriptChild, Error> {
        self.script_children
            .get_mut(&child)
            .ok_or_else(|| invalid("missing scripted child"))
    }
    fn close_child_input(&mut self, child: u64) {
        if let Some(Grant::Child { stdin_open, .. }) = self.grants.get_mut(&child) {
            *stdin_open = false;
        }
    }
    /// Drop a Child or ChildGroup: its input closes first, so an unmet Expect reports.
    pub(super) fn close_child(&mut self, child: u64) -> Option<String> {
        let open = matches!(
            self.grants.remove(&child),
            Some(Grant::Child {
                stdin_open: true,
                ..
            })
        );
        if let Some(mut group) = self.script_groups.remove(&child) {
            return if open { group.close_stdin() } else { None };
        }
        let script = self.script_children.remove(&child)?;
        if open { script.close_stdin() } else { None }
    }
    /// `proc.spawn` and the streaming child operations of scripted worlds
    /// (docs/spec/core-library.md#processes); a host child needs a host adapter.
    fn child_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        cx: &mut OperationContext<'_>,
        canceled: Option<u32>,
    ) -> Result<OperationResult, Error> {
        let a = call.arguments;
        let mut updates = vec![];
        let mut consumed = vec![];
        let mut input = encode_arguments(a);
        let mut recorded = None;
        // A scripted child's contradicted or unmet Expect fails LANG-9 after its report.
        let mut report = None;
        let group = name.starts_with("group.") || name == "proc.wait_group";
        let kind = if group { "ChildGroup" } else { "Child" };
        let value = match name {
            "proc.spawn" => {
                let (world, rules) = self.process_grant(arg(a, 0)?)?;
                let process = self.id(arg(a, 0)?, "Proc")?;
                let command = self.command(arg(a, 1)?)?;
                let checked = command.check(&rules).map(|_| ()).and_then(|()| {
                    canceled.map_or(Ok(()), |cause| Err(ProcessFailure::Canceled(cause)))
                });
                match checked {
                    Err(failure) => error(failure_value(failure)),
                    Ok(())
                        if world == 0
                            && self.config.process.as_ref().is_some_and(|p| {
                                ProviderContract::parse(&p.provider)
                                    == Some(ProviderContract::ProcessSpawn)
                            }) =>
                    {
                        return Err(Error::Unavailable(format!(
                            "the reference spawns host children only through a host adapter; simulation requires {}",
                            ProviderContract::ProcessScript.name()
                        )));
                    }
                    Ok(()) => {
                        let children = self
                            .process_children
                            .get_mut(&world)
                            .ok_or_else(|| invalid("missing process world"))?;
                        match children.iter_mut().find(|c| {
                            c.as_ref().is_some_and(|c| {
                                c.program == command.program && c.args == command.args
                            })
                        }) {
                            None => error(failure_value(ProcessFailure::NotFound)),
                            Some(entry) => {
                                let script = ScriptChild::new(entry.take().unwrap());
                                let child = self.handle(
                                    "Child",
                                    Grant::Child {
                                        process,
                                        stdin_open: true,
                                        group: false,
                                    },
                                );
                                self.script_children
                                    .insert(self.id(&child, "Child")?, script);
                                ok(child)
                            }
                        }
                    }
                }
            }
            "proc.spawn_pipeline" => {
                let (world, rules) = self.process_grant(arg(a, 0)?)?;
                let process = self.id(arg(a, 0)?, "Proc")?;
                let stages = collection::elements(arg(a, 1)?, BuiltinType::List)?
                    .iter()
                    .map(|stage| self.command(stage))
                    .collect::<Result<Vec<_>, _>>()?;
                let max_stderr = match arg(a, 2)? {
                    Value::Integer(n) if n.kind() == Primitive::U64 => n.bits() as u64,
                    _ => return Err(invalid("max_stderr requires u64")),
                };
                // Every stage's checks, in stage order, precede cancellation.
                let checked = if stages.is_empty() {
                    Err(ProcessFailure::InvalidInput)
                } else {
                    stages
                        .iter()
                        .try_for_each(|stage| stage.check(&rules).map(|_| ()))
                }
                .and_then(|()| {
                    canceled.map_or(Ok(()), |cause| Err(ProcessFailure::Canceled(cause)))
                });
                match checked {
                    Err(failure) => error(failure_value(failure)),
                    Ok(())
                        if world == 0
                            && self.config.process.as_ref().is_some_and(|p| {
                                ProviderContract::parse(&p.provider)
                                    == Some(ProviderContract::ProcessSpawn)
                            }) =>
                    {
                        return Err(Error::Unavailable(format!(
                            "the reference spawns host children only through a host adapter; simulation requires {}",
                            ProviderContract::ProcessScript.name()
                        )));
                    }
                    Ok(()) => {
                        let children = self
                            .process_children
                            .get_mut(&world)
                            .ok_or_else(|| invalid("missing process world"))?;
                        match take_children(children, &stages) {
                            Err(failure) => error(failure_value(failure)),
                            Ok(runs) => {
                                let (script, first) = ScriptGroup::new(runs, max_stderr);
                                report = first;
                                let handle = self.handle(
                                    "ChildGroup",
                                    Grant::Child {
                                        process,
                                        stdin_open: true,
                                        group: true,
                                    },
                                );
                                self.script_groups
                                    .insert(self.id(&handle, "ChildGroup")?, script);
                                ok(handle)
                            }
                        }
                    }
                }
            }
            "proc.wait_group" => {
                // The wait consumes its ChildGroup on every outcome.
                consumed.push(1);
                let (group, _) = self.child_grant(arg(a, 0)?, arg(a, 1)?, kind)?;
                let deadline = deadline(arg(a, 2)?)?;
                if deadline.is_some_and(
                    |(clock, _)| !matches!(clock, Value::Handle { kind, .. } if kind == "MonotonicClock"),
                ) {
                    return Err(invalid("expected MonotonicClock"));
                }
                match canceled {
                    // A canceled wait drops the ChildGroup it took.
                    Some(cause) => {
                        report = self.close_child(group);
                        error(failure_value(ProcessFailure::Canceled(cause)))
                    }
                    // A scripted group takes no time: a reached deadline ends every stage at
                    // once, without checking an Expect, as a kill does.
                    None if deadline.is_some_and(|(_, ticks)| ticks <= self.monotonic_now()) => {
                        self.grants.remove(&group);
                        self.script_groups.remove(&group);
                        error(failure_value(ProcessFailure::TimedOut))
                    }
                    None => {
                        report = self.script_group(group)?.close_stdin();
                        let output = self.script_group(group)?.output();
                        self.grants.remove(&group);
                        self.script_groups.remove(&group);
                        match report {
                            Some(_) => Value::Unit,
                            None => group_value(&output),
                        }
                    }
                }
            }
            "child.write" | "group.write" => {
                let data = bytes(arg(a, 2)?)?.to_vec();
                let (child, open) = self.child_grant(arg(a, 0)?, arg(a, 1)?, kind)?;
                match canceled.filter(|_| open) {
                    Some(cause) => error(failure_value(ProcessFailure::Canceled(cause))),
                    None => {
                        if !open {
                            error(failure_value(ProcessFailure::InvalidInput))
                        } else if data.is_empty() {
                            ok(Value::Unit)
                        } else {
                            let written = if group {
                                self.script_group(child)?.write(&data)
                            } else {
                                self.script_child(child)?.write(&data)
                            };
                            match written {
                                Ok(Ok(())) => ok(Value::Unit),
                                Ok(Err(failure)) => error(failure_value(failure)),
                                Err(text) => {
                                    report = Some(text);
                                    Value::Unit
                                }
                            }
                        }
                    }
                }
            }
            "child.close_stdin" | "group.close_stdin" => {
                let (child, open) = self.child_grant(arg(a, 0)?, arg(a, 1)?, kind)?;
                if open {
                    self.close_child_input(child);
                    report = if group {
                        self.script_group(child)?.close_stdin()
                    } else {
                        self.script_child(child)?.close_stdin()
                    };
                }
                Value::Unit
            }
            "child.read_stdout" | "child.read_stderr" | "group.read_stdout" => {
                let max = match arg(a, 3)? {
                    Value::Integer(n) if n.kind() == Primitive::U64 => n.bits() as u64,
                    _ => return Err(invalid("max requires u64")),
                };
                input = encode_arguments(&[a[0].clone(), a[1].clone(), a[3].clone()]);
                // A zero-byte read is invalid before it is a cancellation point.
                if max == 0 {
                    error(failure_value(ProcessFailure::InvalidInput))
                } else if let Some(cause) = canceled {
                    error(failure_value(ProcessFailure::Canceled(cause)))
                } else {
                    let (child, _) = self.child_grant(arg(a, 0)?, arg(a, 1)?, kind)?;
                    let stream = if name == "child.read_stderr" {
                        ChildStream::Stderr
                    } else {
                        ChildStream::Stdout
                    };
                    let read = if group {
                        self.script_group(child)?.read(max)
                    } else {
                        self.script_child(child)?.read(stream, max)
                    };
                    match read {
                        ScriptRead::Waiting => {
                            let anchor =
                                cx.anchor().cloned().unwrap_or(crate::observation::Anchor {
                                    definition: call.path.clone(),
                                    body: hirsel_tir::BodyId(0),
                                    cleanup: None,
                                    site: call.site,
                                });
                            return Err(Error::Blocked(vec![crate::BlockedWait {
                                spawn_path: vec![],
                                anchor,
                                kind: name.into(),
                                targets: vec![child],
                            }]));
                        }
                        ScriptRead::Bytes(read) => {
                            let mut buffer = arg(a, 2)?.clone();
                            let Value::Sequence { elements, .. } = &mut buffer else {
                                return Err(invalid("read buffer sequence"));
                            };
                            elements.extend(read.iter().map(|b| {
                                Some(Value::Integer(
                                    crate::Integer::new(Primitive::U8, (*b).into()).unwrap(),
                                ))
                            }));
                            updates.push((2, buffer));
                            recorded = Some(encode_value(&ok(Value::Bytes(read.clone()))));
                            ok(super::dispatch::u64_value(read.len() as u64))
                        }
                    }
                }
            }
            "child.wait" => {
                // The wait consumes its Child on every outcome.
                consumed.push(1);
                let (child, open) = self.child_grant(arg(a, 0)?, arg(a, 1)?, kind)?;
                let deadline = deadline(arg(a, 2)?)?;
                if deadline.is_some_and(
                    |(clock, _)| !matches!(clock, Value::Handle { kind, .. } if kind == "MonotonicClock"),
                ) {
                    return Err(invalid("expected MonotonicClock"));
                }
                match canceled {
                    // A canceled wait drops the Child it took.
                    Some(cause) => {
                        report = self.close_child(child);
                        error(failure_value(ProcessFailure::Canceled(cause)))
                    }
                    // A scripted child takes no time: a reached deadline ends it at once,
                    // without checking an Expect, as a kill does.
                    None if deadline.is_some_and(|(_, ticks)| ticks <= self.monotonic_now()) => {
                        self.grants.remove(&child);
                        self.script_children.remove(&child);
                        error(failure_value(ProcessFailure::TimedOut))
                    }
                    None => {
                        if open {
                            report = self.script_child(child)?.close_stdin();
                        }
                        let status = self.script_child(child)?.status();
                        self.grants.remove(&child);
                        self.script_children.remove(&child);
                        match (report.is_some(), status) {
                            (true, _) => Value::Unit,
                            (false, Some(status)) => ok(status_value(status)),
                            (false, None) => return Err(invalid("scripted child never ended")),
                        }
                    }
                }
            }
            "child.kill" | "group.kill" => {
                let (child, _) = self.child_grant(arg(a, 0)?, arg(a, 1)?, kind)?;
                let signal = match arg(a, 2)? {
                    Value::Variant { variant, .. } => Signal::of_ordinal(*variant),
                    _ => None,
                }
                .ok_or_else(|| invalid("Signal representation"))?;
                if group {
                    self.script_group(child)?.kill(signal);
                } else {
                    self.script_child(child)?.kill(signal);
                }
                Value::Unit
            }
            _ => return Err(invalid("unknown child operation")),
        };
        if let Some(report) = report {
            cx.stdout.extend_from_slice(report.as_bytes());
            let failure = super::dispatch::fail(call, cx, "LANG-9", "scripted child unmet");
            self.record("Proc", name, input, b"failure:LANG-9".to_vec(), cx)?;
            return Ok(OperationResult::Failed(failure));
        }
        self.record(
            "Proc",
            name,
            input,
            recorded.unwrap_or_else(|| encode_value(&value)),
            cx,
        )?;
        let mut result = OperationResult::returned(value).transferring(consumed);
        if let OperationResult::Complete { arguments, .. } = &mut result {
            *arguments = updates;
        }
        Ok(result)
    }
}

/// The LANG-9 failure of a scripted child's contradicted or unmet Expect at the current
/// operation, whose Test report was already written.
pub fn script_failure(cx: &OperationContext<'_>, kind: &str) -> Result<OperationResult, Error> {
    let site = cx
        .anchor()
        .ok_or_else(|| invalid("operation has no execution anchor"))?
        .site;
    Ok(OperationResult::Failed(crate::Failure {
        rule: hirsel_diag::RuleId::new("LANG-9").unwrap(),
        line: cx.sources.location(site).map_or(0, |p| p.line),
        site,
        kind: kind.into(),
    }))
}

/// An `ExitStatus` value.
pub fn status_value(status: ExitStatus) -> Value {
    match status {
        ExitStatus::Exited(code) => variant(0, vec![i32_value(code)]),
        ExitStatus::Signaled(signal) => variant(1, vec![i32_value(signal)]),
    }
}
