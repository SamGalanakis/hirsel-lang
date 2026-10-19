//! Driver-only conversion between H semantic values and the runtime's process model.
//! All policy and scripted runs live in hirsel-rt; the model never spawns a program.
use super::{Io, IoGrant, io_ok};
use hirsel_heapcheck::{OpOutcome, OpValue, Operations};
use hirsel_lir::layout_types::ScalarKind;
use hirsel_rt::io::{Completion as C, Request as R};
use hirsel_rt::process::{
    ChildRun, ChildStep, Command, ExitStatus, PipelineOutcome, ProcessFailure, ProcessOutcome,
    ProcessRun,
};

fn bytes(v: &OpValue) -> Option<Vec<u8>> {
    match v {
        OpValue::Bytes(b) => Some(b.clone()),
        OpValue::Elements {
            scalar: ScalarKind::U8,
            bits,
        } => bits.iter().map(|&b| u8::try_from(b).ok()).collect(),
        _ => None,
    }
}
fn text(v: &OpValue) -> Option<String> {
    String::from_utf8(bytes(v)?).ok()
}
fn strings(v: &OpValue) -> Option<Vec<String>> {
    match v {
        OpValue::Sequence(items) => items.iter().map(text).collect(),
        _ => None,
    }
}
fn word(v: &OpValue) -> Option<u64> {
    match v {
        OpValue::Bits { bits, .. } => u64::try_from(*bits).ok(),
        _ => None,
    }
}
fn variant(v: &OpValue) -> Option<(u32, &OpValue)> {
    match v {
        OpValue::Variant { tag, value } => Some((*tag, value)),
        _ => None,
    }
}
fn v(tag: u32, value: OpValue) -> OpValue {
    OpValue::Variant {
        tag,
        value: Box::new(value),
    }
}
fn failure(f: ProcessFailure) -> OpValue {
    match f {
        ProcessFailure::NotGranted => v(0, OpValue::Unit),
        ProcessFailure::InvalidInput => v(1, OpValue::Unit),
        ProcessFailure::NotFound => v(2, OpValue::Unit),
        ProcessFailure::PermissionDenied => v(3, OpValue::Unit),
        ProcessFailure::OutputLimit => v(4, OpValue::Unit),
        ProcessFailure::Fault(code) => v(
            5,
            OpValue::Bits {
                scalar: ScalarKind::U32,
                bits: code.into(),
            },
        ),
        ProcessFailure::TimedOut => v(6, OpValue::Unit),
        ProcessFailure::Canceled(cause) => v(7, v(cause, OpValue::Unit)),
        ProcessFailure::InputClosed => v(8, OpValue::Unit),
    }
}
fn status_value(status: ExitStatus) -> OpValue {
    let (tag, code) = match status {
        ExitStatus::Exited(code) => (0, code),
        ExitStatus::Signaled(signal) => (1, signal),
    };
    v(
        tag,
        OpValue::Bits {
            scalar: ScalarKind::I32,
            bits: u128::from(code as u32),
        },
    )
}
fn status(v: &OpValue) -> Option<ExitStatus> {
    let (tag, code) = variant(v)?;
    let code = word(code)? as u32 as i32;
    match tag {
        0 => Some(ExitStatus::Exited(code)),
        1 => Some(ExitStatus::Signaled(code)),
        _ => None,
    }
}
fn child_script(v: &OpValue) -> Option<ChildRun> {
    let OpValue::Aggregate(fields) = v else {
        return None;
    };
    let [program, args, steps] = fields.as_slice() else {
        return None;
    };
    let OpValue::Sequence(steps) = steps else {
        return None;
    };
    Some(ChildRun {
        program: text(program)?,
        args: strings(args)?,
        steps: steps
            .iter()
            .map(|step| {
                Some(match variant(step)? {
                    (0, b) => ChildStep::Print(bytes(b)?),
                    (1, b) => ChildStep::PrintErr(bytes(b)?),
                    (2, b) => ChildStep::Expect(bytes(b)?),
                    (3, s) => ChildStep::Exit(status(s)?),
                    _ => return None,
                })
            })
            .collect::<Option<_>>()?,
    })
}
fn script(v: &OpValue) -> Option<ProcessRun> {
    let OpValue::Aggregate(fields) = v else {
        return None;
    };
    let [program, args, outcome] = fields.as_slice() else {
        return None;
    };
    let outcome = match variant(outcome)? {
        (0, OpValue::Aggregate(output)) => {
            let [status_value, stdout, stderr] = output.as_slice() else {
                return None;
            };
            ProcessOutcome::Output {
                status: status(status_value)?,
                stdout: bytes(stdout)?,
                stderr: bytes(stderr)?,
            }
        }
        (1, error) => ProcessOutcome::Error(match variant(error)? {
            (0, _) => ProcessFailure::NotGranted,
            (1, _) => ProcessFailure::InvalidInput,
            (2, _) => ProcessFailure::NotFound,
            (3, _) => ProcessFailure::PermissionDenied,
            (4, _) => ProcessFailure::OutputLimit,
            (5, code) => ProcessFailure::Fault(word(code)? as u32),
            (6, _) => ProcessFailure::TimedOut,
            // A script is never canceled: the runtime refuses it.
            (7, cause) => ProcessFailure::Canceled(variant(cause)?.0),
            (8, _) => ProcessFailure::InputClosed,
            _ => return None,
        }),
        _ => return None,
    };
    Some(ProcessRun {
        program: text(program)?,
        args: strings(args)?,
        outcome,
    })
}

impl Io {
    /// An `Option[Deadline]` value as H's clock handle, the Instant's ticks, the clock's
    /// reading now and the runtime model's clock handle; `Err` is the CAP-2 failure of an
    /// unreadable clock.
    #[allow(clippy::type_complexity)]
    fn deadline(&mut self, v: &OpValue) -> Option<Result<Option<(u64, u64, u64, u64)>, OpOutcome>> {
        Some(Ok(match variant(v)? {
            (0, _) => None,
            (1, OpValue::Aggregate(fields)) => match fields.as_slice() {
                [OpValue::Handle(clock), OpValue::Handle(at)] => {
                    let Some(now) = self.monotonic(*clock) else {
                        return Some(Err(OpOutcome::Fail("CAP-2".into())));
                    };
                    self.network.observe_monotonic(now);
                    let network = &mut self.network;
                    let script = *self
                        .script_clocks
                        .entry(*clock)
                        .or_insert_with(|| network.monotonic_clock());
                    Some((*clock, *at, now, script))
                }
                _ => return None,
            },
            _ => return None,
        }))
    }
    /// A `Command` value. The model's Dir handles are H's own, so its Dir is checked here
    /// and not passed on: a scripted world does not consult it, and the model has no host
    /// world to start one in. `Err` is the CAP-2 failure of a value that is not a Dir.
    fn command(&self, v: &OpValue) -> Option<Result<Command, OpOutcome>> {
        let OpValue::Aggregate(fields) = v else {
            return None;
        };
        let [
            OpValue::Handle(path),
            argv,
            dir,
            env,
            OpValue::Bits {
                scalar: ScalarKind::Bool,
                bits: own_group,
            },
        ] = fields.as_slice()
        else {
            return None;
        };
        match variant(dir)? {
            (0, _) => {}
            (1, OpValue::Handle(d)) if matches!(self.grants.get(d), Some(IoGrant::Dir { .. })) => {}
            _ => return Some(Err(OpOutcome::Fail("CAP-2".into()))),
        }
        let env = match variant(env)? {
            (0, _) => None,
            (1, OpValue::Sequence(vars)) => Some(
                vars.iter()
                    .map(|var| match var {
                        OpValue::Aggregate(pair) => match pair.as_slice() {
                            [name, value] => Some((text(name)?, text(value)?)),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect::<Option<_>>()?,
            ),
            _ => return None,
        };
        Some(Ok(Command {
            program: self.paths.get(path.checked_sub(1)? as usize)?.clone(),
            args: strings(argv)?,
            directory: None,
            env,
            own_group: *own_group != 0,
        }))
    }
    pub(super) fn process(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[OpValue],
        cancellation: Option<u32>,
    ) -> Option<OpOutcome> {
        let request = match (module, name, args) {
            (Some("test"), "process", [OpValue::Handle(test), runs, children])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                let (OpValue::Sequence(runs), OpValue::Sequence(children)) = (runs, children)
                else {
                    return None;
                };
                R::TestProcess {
                    test: 1,
                    runs: runs.iter().map(script).collect::<Option<_>>()?,
                    children: children.iter().map(child_script).collect::<Option<_>>()?,
                }
            }
            (Some("io"), "process", [OpValue::Handle(1)]) => R::Process { io: 1 },
            // A narrowing is no cancellation point; its Proc is a plain capability.
            (Some("proc"), "narrow", [p, OpValue::Sequence(rules)]) => {
                let request = R::ProcessNarrow {
                    process: self.network_id(p)?,
                    rules: rules
                        .iter()
                        .map(|rule| match rule {
                            OpValue::Aggregate(fields) => match fields.as_slice() {
                                [program, env] => Some(hirsel_rt::process::ProcessRule {
                                    program: text(program)?,
                                    env: strings(env)?,
                                }),
                                _ => None,
                            },
                            _ => None,
                        })
                        .collect::<Option<_>>()?,
                };
                return Some(match self.script(request, None) {
                    Ok(C::Spawned(Ok(id))) => {
                        OpOutcome::Value(io_ok(self.network_handle(id, false)))
                    }
                    Ok(C::Spawned(Err(f))) => OpOutcome::Value(v(1, failure(f))),
                    Ok(_) => OpOutcome::Unsupported,
                    Err(outcome) => outcome,
                });
            }
            (Some("proc"), name @ ("run" | "pipeline"), [p, cmd, stdin, max, deadline]) => {
                // The runtime's policy checks a deadline against the clock's reading, which
                // H owns; a run that starts before it is reached races it.
                let deadline = match self.deadline(deadline)? {
                    Ok(deadline) => deadline,
                    Err(fail) => return Some(fail),
                };
                let (process, stdin, max_output) = (self.network_id(p)?, bytes(stdin)?, word(max)?);
                let deadline_request = deadline
                    .map(|(_, at, _, clock)| hirsel_rt::process::Deadline { clock, ticks: at });
                let request = if name == "run" {
                    let command = match self.command(cmd)? {
                        Ok(command) => command,
                        Err(fail) => return Some(fail),
                    };
                    R::ProcessRun {
                        process,
                        command,
                        stdin,
                        max_output,
                        deadline: deadline_request,
                    }
                } else {
                    let OpValue::Sequence(commands) = cmd else {
                        return None;
                    };
                    let mut stages = vec![];
                    for command in commands {
                        match self.command(command)? {
                            Ok(command) => stages.push(command),
                            Err(fail) => return Some(fail),
                        }
                    }
                    R::ProcessPipeline {
                        process,
                        stages,
                        stdin,
                        max_output,
                        deadline: deadline_request,
                    }
                };
                return Some(
                    match self.process_completion(module, request, cancellation) {
                        // A run that started (its checks passed and it found its entry) before
                        // its deadline races it.
                        OpOutcome::Value(value)
                            if cancellation.is_none()
                                && deadline.is_some_and(|(_, at, now, _)| at > now)
                                && !matches!(
                                    variant(&value),
                                    Some((1, OpValue::Variant { tag: 0..=2, .. }))
                                ) =>
                        {
                            let (clock, at, ..) = deadline.unwrap();
                            OpOutcome::Deadline {
                                clock,
                                at,
                                completed: Box::new(OpOutcome::Value(value)),
                                timed_out: Box::new(OpOutcome::Value(v(
                                    1,
                                    failure(ProcessFailure::TimedOut),
                                ))),
                            }
                        }
                        outcome => outcome,
                    },
                );
            }
            (Some("proc"), "spawn", [p, cmd]) => match self.command(cmd)? {
                Ok(command) => R::ProcessSpawn {
                    process: self.network_id(p)?,
                    command,
                },
                Err(fail) => return Some(fail),
            },
            (Some("proc"), "spawn_pipeline", [p, OpValue::Sequence(commands), max]) => {
                let mut stages = vec![];
                for command in commands {
                    match self.command(command)? {
                        Ok(command) => stages.push(command),
                        Err(fail) => return Some(fail),
                    }
                }
                R::ProcessSpawnPipeline {
                    process: self.network_id(p)?,
                    stages,
                    max_stderr: word(max)?,
                }
            }
            // A ChildGroup's operations are a Child's on its handle; its wait is
            // `proc.wait_group`.
            (Some(module @ ("child" | "group" | "proc")), _, [p, c, rest @ ..])
                if module != "proc" || name == "wait_group" =>
            {
                let group = module != "child";
                let name = if name == "wait_group" { "wait" } else { name };
                let (process, child) = (self.network_id(p)?, self.network_id(c)?);
                let request = match (name, rest) {
                    ("write", [data]) => R::ChildWrite {
                        process,
                        child,
                        bytes: bytes(data)?,
                    },
                    ("close_stdin", []) => R::ChildCloseStdin { process, child },
                    ("read_stdout" | "read_stderr", [_, max]) => R::ChildRead {
                        process,
                        child,
                        stream: if name == "read_stdout" {
                            hirsel_rt::process::ChildStream::Stdout
                        } else {
                            hirsel_rt::process::ChildStream::Stderr
                        },
                        max: word(max)?,
                    },
                    ("wait", [deadline]) => {
                        let deadline = match self.deadline(deadline)? {
                            Ok(deadline) => deadline,
                            Err(fail) => return Some(fail),
                        };
                        let deadline_request = deadline.map(|(_, at, _, clock)| {
                            hirsel_rt::process::Deadline { clock, ticks: at }
                        });
                        let request = if group {
                            R::GroupWait {
                                process,
                                group: child,
                                deadline: deadline_request,
                            }
                        } else {
                            R::ChildWait {
                                process,
                                child,
                                deadline: deadline_request,
                            }
                        };
                        return Some(match self.child(name, args, request, cancellation) {
                            // A wait that started before its deadline and found its child's
                            // status races it, as a run does.
                            OpOutcome::Complete {
                                result: Ok(value),
                                consumed,
                            } if cancellation.is_none()
                                && deadline.is_some_and(|(_, at, now, _)| at > now)
                                && matches!(variant(&value), Some((0, _))) =>
                            {
                                let (clock, at, ..) = deadline.unwrap();
                                OpOutcome::Deadline {
                                    clock,
                                    at,
                                    completed: Box::new(OpOutcome::Complete {
                                        result: Ok(value),
                                        consumed: consumed.clone(),
                                    }),
                                    timed_out: Box::new(OpOutcome::Complete {
                                        result: Ok(v(1, failure(ProcessFailure::TimedOut))),
                                        consumed,
                                    }),
                                }
                            }
                            outcome => outcome,
                        });
                    }
                    ("kill", [signal]) => R::ChildKill {
                        process,
                        child,
                        signal: hirsel_rt::process::Signal::of_ordinal(variant(signal)?.0)?,
                    },
                    _ => return None,
                };
                return Some(self.child(name, args, request, cancellation));
            }
            _ => return None,
        };
        Some(self.process_completion(module, request, cancellation))
    }
    /// A process request's outcome from the runtime's process model.
    fn process_completion(
        &mut self,
        module: Option<&str>,
        request: R,
        cancellation: Option<u32>,
    ) -> OpOutcome {
        let completion = match self.script(request, cancellation) {
            Ok(c) => c,
            Err(outcome) => return outcome,
        };
        if let C::Spawned(spawned) = completion {
            return OpOutcome::Value(match spawned {
                Ok(child) => io_ok(self.network_handle(child, true)),
                Err(f) => v(1, failure(f)),
            });
        }
        OpOutcome::Value(match completion {
            C::Handle(Ok(id)) if module == Some("test") => self.network_handle(id, false),
            C::Handle(Ok(id)) => io_ok(self.network_handle(id, false)),
            C::Handle(Err(_)) => v(1, v(0, OpValue::Unit)),
            C::Process(ProcessOutcome::Output {
                status,
                stdout,
                stderr,
            }) => {
                let (tag, code) = match status {
                    ExitStatus::Exited(code) => (0, code),
                    ExitStatus::Signaled(signal) => (1, signal),
                };
                io_ok(OpValue::Aggregate(vec![
                    v(
                        tag,
                        OpValue::Bits {
                            scalar: ScalarKind::I32,
                            bits: u128::from(code as u32),
                        },
                    ),
                    OpValue::ByteSequence(stdout),
                    OpValue::ByteSequence(stderr),
                ]))
            }
            C::Process(ProcessOutcome::Error(f)) => v(1, failure(f)),
            C::Pipeline(PipelineOutcome::Output {
                statuses,
                stdout,
                stderr,
            }) => io_ok(OpValue::Aggregate(vec![
                OpValue::Sequence(statuses.into_iter().map(status_value).collect()),
                OpValue::ByteSequence(stdout),
                OpValue::ByteSequence(stderr),
            ])),
            C::Pipeline(PipelineOutcome::Error(f)) => v(1, failure(f)),
            _ => return OpOutcome::Unsupported,
        })
    }
    /// A child operation's outcome; `child.wait` consumes its Child on every outcome.
    fn child(
        &mut self,
        name: &str,
        args: &[OpValue],
        request: R,
        cancellation: Option<u32>,
    ) -> OpOutcome {
        let completion = match self.script(request, cancellation) {
            Ok(c) => c,
            Err(outcome) => return outcome,
        };
        let value = match completion {
            C::ChildUnit(Ok(())) if name == "write" => io_ok(OpValue::Unit),
            C::ChildUnit(Ok(())) => OpValue::Unit,
            C::ChildBytes(Ok(read)) => {
                let Some(mut buffer) = bytes(&args[2]) else {
                    return OpOutcome::Unsupported;
                };
                let count = read.len() as u64;
                buffer.extend_from_slice(&read);
                return OpOutcome::Updated {
                    value: io_ok(OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: count.into(),
                    }),
                    arguments: vec![(
                        2,
                        OpValue::Elements {
                            scalar: ScalarKind::U8,
                            bits: buffer.into_iter().map(u128::from).collect(),
                        },
                    )],
                };
            }
            C::ChildExit(Ok(status)) => io_ok(status_value(status)),
            C::GroupExit(Ok(output)) => io_ok(OpValue::Aggregate(vec![
                OpValue::Sequence(output.statuses.into_iter().map(status_value).collect()),
                OpValue::Sequence(
                    output
                        .stderr
                        .into_iter()
                        .map(OpValue::ByteSequence)
                        .collect(),
                ),
            ])),
            C::ChildUnit(Err(f))
            | C::ChildBytes(Err(f))
            | C::ChildExit(Err(f))
            | C::GroupExit(Err(f)) => v(1, failure(f)),
            // A scripted child's contradicted or unmet Expect.
            C::Checked(false) if name == "wait" => {
                self.forget(&args[1]);
                return OpOutcome::Complete {
                    result: Err("LANG-9".into()),
                    consumed: vec![1],
                };
            }
            C::Checked(false) => return OpOutcome::Fail("LANG-9".into()),
            _ => return OpOutcome::Unsupported,
        };
        if name == "wait" {
            self.forget(&args[1]);
            return OpOutcome::Complete {
                result: Ok(value),
                consumed: vec![1],
            };
        }
        OpOutcome::Value(value)
    }
    fn forget(&mut self, child: &OpValue) {
        if let OpValue::Handle(h) = child {
            self.network_handles.remove(h);
        }
    }
}
