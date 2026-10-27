//! Additional typed tape forms for host streams, exact environment reads, processes,
//! random draws and timer waits.
use crate::io::{
    ClockError, Completion, EnvError, InputError, RandomError, Request, SignalError, WaitError,
};
use hirsel_run_schema::{
    ChildStream, ExitStatus, PipelineOutcome, ProcessFailure, ProcessOutcome, Signal, hex,
};

fn failure_word(failure: ProcessFailure) -> String {
    match failure {
        ProcessFailure::NotGranted => "not_granted".into(),
        ProcessFailure::InvalidInput => "invalid_input".into(),
        ProcessFailure::NotFound => "not_found".into(),
        ProcessFailure::PermissionDenied => "permission_denied".into(),
        ProcessFailure::OutputLimit => "output_limit".into(),
        ProcessFailure::Fault(code) => format!("fault {code}"),
        ProcessFailure::TimedOut => "timed_out".into(),
        ProcessFailure::Canceled(cause) => format!("canceled {cause}"),
        ProcessFailure::InputClosed => "input_closed".into(),
    }
}

fn deadline_word(deadline: Option<crate::process::Deadline>) -> String {
    deadline.map_or_else(|| "none".into(), |d| format!("{}:{}", d.clock, d.ticks))
}
/// A Command's words: program, directory, the argument count and each argument, then
/// `inherit` or the variable count and each name and value, then `own_group` only when it
/// is set, so a recording made before the field keeps its bytes. Each count leads the
/// words it counts.
fn command_words(command: &hirsel_run_schema::Command) -> String {
    let mut text = format!(
        "{} {} {}",
        hex(command.program.as_bytes()),
        command
            .directory
            .map_or_else(|| "none".into(), |d| d.to_string()),
        command.args.len()
    );
    for arg in &command.args {
        text.push(' ');
        text.push_str(&hex(arg.as_bytes()));
    }
    match &command.env {
        None => text.push_str(" inherit"),
        Some(vars) => {
            text.push_str(&format!(" {}", vars.len()));
            for (name, value) in vars {
                text.push_str(&format!(
                    " {} {}",
                    hex(name.as_bytes()),
                    hex(value.as_bytes())
                ));
            }
        }
    }
    if command.own_group {
        text.push_str(" own_group");
    }
    text
}

pub(crate) fn render_request(request: &Request) -> Option<String> {
    Some(match request {
        Request::Stdin { io } => format!("stdin {io}"),
        Request::Stderr { io } => format!("stderr {io}"),
        Request::ReadStdin { stdin, max } => format!("stdin_read {stdin} {max}"),
        Request::PrintStderr { stderr, bytes } => format!("stderr_print {stderr} {}", hex(bytes)),
        Request::EnvGet { env, name } => format!("env_get {env} {}", hex(name.as_bytes())),
        Request::EnvNarrow { env, names } => {
            let mut text = format!("env_narrow {env} {}", names.len());
            for name in names {
                text.push_str(&format!(" {}", hex(name.as_bytes())));
            }
            text
        }
        // Each rule is its program and its counted names; the rule count leads.
        Request::ProcessNarrow { process, rules } => {
            let mut text = format!("process_narrow {process} {}", rules.len());
            for rule in rules {
                text.push_str(&format!(
                    " {} {}",
                    hex(rule.program.as_bytes()),
                    rule.env.len()
                ));
                for name in &rule.env {
                    text.push_str(&format!(" {}", hex(name.as_bytes())));
                }
            }
            text
        }
        Request::Process { io } => format!("process {io}"),
        Request::Signals { io } => format!("signals {io}"),
        // The kinds are their bits (`SignalKind::bit`).
        Request::SignalWait { signals, kinds } => format!("signal_wait {signals} {kinds}"),
        Request::Ready { wait, points } => format!("ready {wait} {points}"),
        // Every member is one word, so the request needs no counts.
        Request::RunnerCall(call) => format!(
            "runner_call {} {} {} {} {} {} {} {}",
            hex(call.runner.as_bytes()),
            call.form.name(),
            hex(call.path.as_bytes()),
            hex(&call.grants),
            hex(&call.definitions),
            hex(&call.key),
            hex(&call.digest),
            hex(&call.argument)
        ),
        Request::RunnerPoll { key, cancel } => format!(
            "runner_poll {} {}",
            hex(key),
            cancel.map_or_else(|| "none".into(), |cause| cause.to_string())
        ),
        Request::RunnerCancel { key, cause } => format!("runner_cancel {} {cause}", hex(key)),
        Request::CallReturn(outcome) => match outcome {
            hirsel_run_schema::CalleeOutcome::Ok(bytes) => format!("call_return ok {}", hex(bytes)),
            hirsel_run_schema::CalleeOutcome::Err(bytes) => {
                format!("call_return err {}", hex(bytes))
            }
            hirsel_run_schema::CalleeOutcome::Failed(bytes) => {
                format!("call_return failed {}", hex(bytes))
            }
        },
        Request::Stopped => "stopped".into(),
        Request::Schedule { count, choice } => format!("schedule {count} {choice}"),
        Request::CpuControl {
            width,
            action,
            count,
            outcome,
        } => format!("cpu_control {width} {action} {count} {outcome}"),
        Request::CpuSummary(summary) => {
            use crate::tape_wire::Wire;
            let mut bytes = Vec::new();
            summary.encode(&mut bytes);
            format!("cpu_summary {}", hex(&bytes))
        }
        Request::ProcessTime { from, to } => format!("process_time {from} {to}"),
        Request::ProcessSettle { canceled } => format!(
            "process_settle {}",
            hex(&serde_json::to_vec(canceled).unwrap())
        ),
        Request::RandomBytes { random, count } => format!("random_bytes {random} {count}"),
        Request::Telemetry { io } => format!("telemetry {io}"),
        Request::TelemetryBegin {
            telemetry,
            parent,
            spec,
        } => format!("telemetry_begin {telemetry} {} {}", hex(parent), hex(spec)),
        Request::TelemetryEvent {
            telemetry,
            span,
            payload,
        } => format!("telemetry_event {telemetry} {span} {}", hex(payload)),
        Request::TelemetryFinish {
            telemetry,
            span,
            status,
        } => format!("telemetry_finish {telemetry} {span} {}", hex(status)),
        Request::TelemetryLog { telemetry, payload } => {
            format!("telemetry_log {telemetry} {}", hex(payload))
        }
        Request::TelemetryMeasure { telemetry, payload } => {
            format!("telemetry_measure {telemetry} {}", hex(payload))
        }
        Request::SleepUntil { clock, deadline } => format!("sleep_until {clock} {deadline}"),
        Request::TestProcess {
            test,
            runs,
            children,
        } => format!(
            "test_process {test} {} {}",
            hex(&hirsel_run_schema::encode_process_runs(runs)),
            hex(&hirsel_run_schema::encode_child_runs(children))
        ),
        Request::ProcessRun {
            process,
            command,
            stdin,
            max_output,
            deadline,
        } => format!(
            "process_run {process} {} {max_output} {} {}",
            hex(stdin),
            deadline_word(*deadline),
            command_words(command)
        ),
        Request::ProcessSpawn { process, command } => {
            format!("process_spawn {process} {}", command_words(command))
        }
        // The stage count leads, so the words after it are that many commands.
        Request::ProcessPipeline {
            process,
            stages,
            stdin,
            max_output,
            deadline,
        } => {
            let mut text = format!(
                "process_pipeline {process} {} {max_output} {} {}",
                hex(stdin),
                deadline_word(*deadline),
                stages.len()
            );
            for stage in stages {
                text.push(' ');
                text.push_str(&command_words(stage));
            }
            text
        }
        // The stage count leads, so the words after it are that many commands.
        Request::ProcessSpawnPipeline {
            process,
            stages,
            max_stderr,
        } => {
            let mut text = format!(
                "process_spawn_pipeline {process} {max_stderr} {}",
                stages.len()
            );
            for stage in stages {
                text.push(' ');
                text.push_str(&command_words(stage));
            }
            text
        }
        Request::GroupWait {
            process,
            group,
            deadline,
        } => format!("group_wait {process} {group} {}", deadline_word(*deadline)),
        Request::ChildWrite {
            process,
            child,
            bytes,
        } => format!("child_write {process} {child} {}", hex(bytes)),
        Request::ChildCloseStdin { process, child } => {
            format!("child_close_stdin {process} {child}")
        }
        Request::ChildRead {
            process,
            child,
            stream,
            max,
        } => format!(
            "child_read {process} {child} {} {max}",
            match stream {
                ChildStream::Stdout => "stdout",
                ChildStream::Stderr => "stderr",
            }
        ),
        Request::ChildWait {
            process,
            child,
            deadline,
        } => format!("child_wait {process} {child} {}", deadline_word(*deadline)),
        Request::ChildKill {
            process,
            child,
            signal,
        } => format!(
            "child_kill {process} {child} {}",
            match signal {
                Signal::Interrupt => "interrupt",
                Signal::Terminate => "terminate",
                Signal::Kill => "kill",
            }
        ),
        _ => return None,
    })
}
fn status_words(status: ExitStatus) -> String {
    match status {
        ExitStatus::Exited(code) => format!("exited {code}"),
        ExitStatus::Signaled(signal) => format!("signaled {signal}"),
    }
}
pub(crate) fn render_completion(completion: &Completion) -> Option<String> {
    Some(match completion {
        Completion::Input(Ok(bytes)) => format!("input ok {}", hex(bytes)),
        Completion::Input(Err(InputError::Fault(code))) => format!("input fault {code}"),
        Completion::Input(Err(InputError::Canceled(cause))) => format!("input canceled {cause}"),
        Completion::Input(Err(InputError::InvalidInput)) => "input invalid_input".into(),
        Completion::Input(Err(InputError::InvalidUtf8)) => "input invalid_utf8".into(),
        Completion::Env(Ok(Some(value))) => format!("env some {}", hex(value.as_bytes())),
        Completion::Env(Ok(None)) => "env none".into(),
        Completion::Span {
            handle,
            trace,
            span,
        } => format!("span {handle} {} {}", hex(trace), hex(span)),
        Completion::Held => "held".into(),
        Completion::Blocked { committed } => format!("blocked {}", u8::from(*committed)),
        Completion::Runner(completion) => {
            use hirsel_run_schema::CallOutcome::*;
            let outcome = match &completion.outcome {
                Ok(bytes) => format!("ok {}", hex(bytes)),
                Failed(bytes) => format!("failed {}", hex(bytes)),
                Faulted(bytes) => format!("faulted {}", hex(bytes)),
                NotRun(reason) => format!("not_run {reason}"),
                InDoubt(reason) => format!("in_doubt {reason}"),
                Canceled(cause) => format!("canceled {cause}"),
            };
            let callee = completion.callee.map_or_else(|| "none".into(), |c| hex(&c));
            format!("runner {outcome} {callee}")
        }
        Completion::Ready => "ready".into(),
        Completion::Env(Err(EnvError::NotGranted)) => "env not_granted".into(),
        Completion::Env(Err(EnvError::InvalidUtf8)) => "env invalid_utf8".into(),
        Completion::Random(Ok(bytes)) => format!("random ok {}", hex(bytes)),
        Completion::Random(Err(RandomError::Unavailable)) => "random unavailable".into(),
        Completion::Wait(Ok(())) => "wait ok".into(),
        Completion::Wait(Err(WaitError::Clock(ClockError::Unavailable))) => {
            "wait unavailable".into()
        }
        Completion::Wait(Err(WaitError::Clock(ClockError::OutOfRange))) => {
            "wait out_of_range".into()
        }
        Completion::Wait(Err(WaitError::Canceled(cause))) => format!("wait canceled {cause}"),
        Completion::Signal(Ok(kind)) => format!("signal {}", kind.name()),
        Completion::Signal(Err(SignalError::Canceled(cause))) => {
            format!("signal canceled {cause}")
        }
        Completion::Signal(Err(SignalError::NotGranted)) => "signal not_granted".into(),
        Completion::Signal(Err(SignalError::InvalidInput)) => "signal invalid_input".into(),
        Completion::Process(ProcessOutcome::Output {
            status,
            stdout,
            stderr,
        }) => {
            let (kind, code) = match status {
                ExitStatus::Exited(code) => ("exited", code),
                ExitStatus::Signaled(signal) => ("signaled", signal),
            };
            format!("process {kind} {code} {} {}", hex(stdout), hex(stderr))
        }
        Completion::Process(ProcessOutcome::Error(failure)) => {
            format!("process error {}", failure_word(*failure))
        }
        Completion::Pipeline(PipelineOutcome::Output {
            statuses,
            stdout,
            stderr,
        }) => {
            let mut text = format!(
                "pipeline ok {} {} {}",
                hex(stdout),
                hex(stderr),
                statuses.len()
            );
            for status in statuses {
                text.push(' ');
                text.push_str(&status_words(*status));
            }
            text
        }
        Completion::Pipeline(PipelineOutcome::Error(failure)) => {
            format!("pipeline error {}", failure_word(*failure))
        }
        Completion::Spawned(Ok(child)) => format!("spawned ok {child}"),
        Completion::ChildUnit(Ok(())) => "child_unit ok".into(),
        Completion::ChildBytes(Ok(bytes)) => format!("child_bytes ok {}", hex(bytes)),
        Completion::ChildExit(Ok(status)) => format!("child_exit ok {}", status_words(*status)),
        // Every status, then every stage's standard error, in stage order.
        Completion::GroupExit(Ok(output)) => {
            let mut text = format!("group_exit ok {}", output.statuses.len());
            for status in &output.statuses {
                text.push(' ');
                text.push_str(&status_words(*status));
            }
            for stderr in &output.stderr {
                text.push(' ');
                text.push_str(&hex(stderr));
            }
            text
        }
        Completion::GroupExit(Err(failure)) => {
            format!("group_exit error {}", failure_word(*failure))
        }
        Completion::Spawned(Err(failure))
        | Completion::ChildUnit(Err(failure))
        | Completion::ChildBytes(Err(failure))
        | Completion::ChildExit(Err(failure)) => {
            let head = match completion {
                Completion::Spawned(_) => "spawned",
                Completion::ChildUnit(_) => "child_unit",
                Completion::ChildBytes(_) => "child_bytes",
                _ => "child_exit",
            };
            format!("{head} error {}", failure_word(*failure))
        }
        _ => return None,
    })
}
fn number<T: std::str::FromStr + ToString>(word: &str) -> Result<T, String> {
    let n: T = word.parse().map_err(|_| "bad host input tape number")?;
    if n.to_string() != word {
        return Err("noncanonical host input tape number".into());
    }
    Ok(n)
}
fn bytes(word: &str) -> Result<Vec<u8>, String> {
    if !word.is_ascii() || word.len() % 2 != 0 {
        return Err("bad host input tape bytes".into());
    }
    let bytes = (0..word.len() / 2)
        .map(|i| {
            u8::from_str_radix(&word[2 * i..2 * i + 2], 16)
                .map_err(|_| "bad host input tape bytes".into())
        })
        .collect::<Result<Vec<_>, String>>()?;
    if hex(&bytes) != word {
        return Err("noncanonical host input tape bytes".into());
    }
    Ok(bytes)
}
/// Exactly `N` bytes in lowercase hex.
fn fixed<const N: usize>(word: &str) -> Result<[u8; N], String> {
    bytes(word)?
        .try_into()
        .map_err(|_| "bad host input tape digest".into())
}
fn text(word: &str) -> Result<String, String> {
    String::from_utf8(bytes(word)?).map_err(|_| "bad host input tape UTF-8".into())
}
fn parse_deadline(word: &str) -> Result<Option<crate::process::Deadline>, String> {
    Ok(match word {
        "none" => None,
        d => {
            let (clock, ticks) = d.split_once(':').ok_or("process tape deadline")?;
            Some(crate::process::Deadline {
                clock: number(clock)?,
                ticks: number(ticks)?,
            })
        }
    })
}
/// One Command from the front of `words`, as [`command_words`] writes it.
fn parse_command<'a>(
    words: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Result<(hirsel_run_schema::Command, usize), String> {
    let mut used = 0;
    let mut next = || {
        used += 1;
        words
            .next()
            .ok_or_else(|| "truncated process tape command".to_string())
    };
    let program = text(next()?)?;
    let directory = match next()? {
        "none" => None,
        d => Some(number(d)?),
    };
    let args = (0..number::<usize>(next()?)?)
        .map(|_| text(next()?))
        .collect::<Result<_, _>>()?;
    let env = match next()? {
        "inherit" => None,
        count => Some(
            (0..number::<usize>(count)?)
                .map(|_| Ok((text(next()?)?, text(next()?)?)))
                .collect::<Result<_, String>>()?,
        ),
    };
    // A program word is hexadecimal, so it is never `own_group`.
    let own_group = words.next_if_eq(&"own_group").is_some();
    Ok((
        hirsel_run_schema::Command {
            program,
            args,
            directory,
            env,
            own_group,
        },
        used + usize::from(own_group),
    ))
}
/// A command that used every remaining word.
fn ended(
    rest: &[&str],
) -> impl FnOnce((hirsel_run_schema::Command, usize)) -> Result<hirsel_run_schema::Command, String>
{
    let len = rest.len();
    move |(command, used)| {
        (used == len)
            .then_some(command)
            .ok_or_else(|| "trailing process tape words".into())
    }
}

pub(crate) fn parse_request(words: &[&str]) -> Result<Option<Request>, String> {
    Ok(Some(match words {
        ["stdin", io] => Request::Stdin { io: number(io)? },
        ["stderr", io] => Request::Stderr { io: number(io)? },
        ["signals", io] => Request::Signals { io: number(io)? },
        ["signal_wait", signals, kinds] => Request::SignalWait {
            signals: number(signals)?,
            kinds: number(kinds)?,
        },
        ["stdin_read", stdin, max] => Request::ReadStdin {
            stdin: number(stdin)?,
            max: number(max)?,
        },
        ["stderr_print", stderr, b] => Request::PrintStderr {
            stderr: number(stderr)?,
            bytes: bytes(b)?,
        },
        ["env_get", env, name] => Request::EnvGet {
            env: number(env)?,
            name: text(name)?,
        },
        ["env_narrow", env, count, names @ ..] if number::<usize>(count)? == names.len() => {
            Request::EnvNarrow {
                env: number(env)?,
                names: names.iter().map(|n| text(n)).collect::<Result<_, _>>()?,
            }
        }
        ["process_narrow", process, count, rest @ ..] => {
            let mut words = rest.iter().copied();
            let mut next = || words.next().ok_or("truncated process tape rule");
            let mut rules = vec![];
            for _ in 0..number::<usize>(count)? {
                let program = text(next()?)?;
                let env = (0..number::<usize>(next()?)?)
                    .map(|_| text(next()?))
                    .collect::<Result<_, _>>()?;
                rules.push(hirsel_run_schema::ProcessRule { program, env });
            }
            if words.next().is_some() {
                return Err("trailing process tape words".into());
            }
            Request::ProcessNarrow {
                process: number(process)?,
                rules,
            }
        }
        ["process", io] => Request::Process { io: number(io)? },
        ["ready", wait, points] => Request::Ready {
            wait: number(wait)?,
            points: number(points)?,
        },
        [
            "runner_call",
            runner,
            form,
            path,
            grants,
            definitions,
            key,
            digest,
            argument,
        ] => Request::RunnerCall(Box::new(hirsel_run_schema::RunnerRequest {
            runner: text(runner)?,
            form: hirsel_run_schema::CallForm::parse(form).ok_or("bad runner call form")?,
            path: text(path)?,
            grants: bytes(grants)?,
            definitions: bytes(definitions)?,
            key: fixed(key)?,
            digest: fixed(digest)?,
            argument: bytes(argument)?,
        })),
        ["runner_poll", key, cancel] => Request::RunnerPoll {
            key: fixed(key)?,
            cancel: if *cancel == "none" {
                None
            } else {
                Some(number(cancel)?)
            },
        },
        ["runner_cancel", key, cause] => Request::RunnerCancel {
            key: fixed(key)?,
            cause: number(cause)?,
        },
        ["call_return", kind, value] => Request::CallReturn(match *kind {
            "ok" => hirsel_run_schema::CalleeOutcome::Ok(bytes(value)?),
            "err" => hirsel_run_schema::CalleeOutcome::Err(bytes(value)?),
            "failed" => hirsel_run_schema::CalleeOutcome::Failed(bytes(value)?),
            _ => return Err("bad call return".into()),
        }),
        ["stopped"] => Request::Stopped,
        ["process_settle", encoded] => Request::ProcessSettle {
            canceled: serde_json::from_slice(&bytes(encoded)?).map_err(|_| "bad process settle")?,
        },
        ["process_time", from, to] => Request::ProcessTime {
            from: number(from)?,
            to: number(to)?,
        },
        ["cpu_summary", record] => Request::CpuSummary(crate::tape_wire::decode(&bytes(record)?)?),
        ["cpu_control", width, action, count, outcome] => Request::CpuControl {
            width: width.parse().map_err(|_| "invalid CPU width")?,
            action: action.parse().map_err(|_| "invalid CPU action")?,
            count: count.parse().map_err(|_| "invalid CPU count")?,
            outcome: outcome.parse().map_err(|_| "invalid CPU outcome")?,
        },
        ["schedule", count, choice] => Request::Schedule {
            count: number(count)?,
            choice: number(choice)?,
        },
        ["random_bytes", random, count] => Request::RandomBytes {
            random: number(random)?,
            count: number(count)?,
        },
        ["telemetry", io] => Request::Telemetry { io: number(io)? },
        ["telemetry_begin", telemetry, parent, spec] => Request::TelemetryBegin {
            telemetry: number(telemetry)?,
            parent: bytes(parent)?,
            spec: bytes(spec)?,
        },
        ["telemetry_event", telemetry, span, payload] => Request::TelemetryEvent {
            telemetry: number(telemetry)?,
            span: number(span)?,
            payload: bytes(payload)?,
        },
        ["telemetry_finish", telemetry, span, status] => Request::TelemetryFinish {
            telemetry: number(telemetry)?,
            span: number(span)?,
            status: bytes(status)?,
        },
        ["telemetry_log", telemetry, payload] => Request::TelemetryLog {
            telemetry: number(telemetry)?,
            payload: bytes(payload)?,
        },
        ["telemetry_measure", telemetry, payload] => Request::TelemetryMeasure {
            telemetry: number(telemetry)?,
            payload: bytes(payload)?,
        },
        ["sleep_until", clock, deadline] => Request::SleepUntil {
            clock: number(clock)?,
            deadline: number(deadline)?,
        },
        ["test_process", test, runs, children] => Request::TestProcess {
            test: number(test)?,
            runs: hirsel_run_schema::decode_process_runs(&bytes(runs)?)
                .map_err(|e| e.to_string())?,
            children: hirsel_run_schema::decode_child_runs(&bytes(children)?)
                .map_err(|e| e.to_string())?,
        },
        [
            "process_run",
            process,
            stdin,
            max_output,
            deadline,
            rest @ ..,
        ] => Request::ProcessRun {
            process: number(process)?,
            stdin: bytes(stdin)?,
            max_output: number(max_output)?,
            deadline: parse_deadline(deadline)?,
            command: parse_command(&mut rest.iter().copied().peekable()).and_then(ended(rest))?,
        },
        [
            "process_pipeline",
            process,
            stdin,
            max_output,
            deadline,
            count,
            rest @ ..,
        ] => {
            let mut words = rest.iter().copied().peekable();
            let mut used = 0;
            let mut stages = vec![];
            for _ in 0..number::<usize>(count)? {
                let (stage, n) = parse_command(&mut words)?;
                used += n;
                stages.push(stage);
            }
            if used != rest.len() {
                return Err("trailing process tape words".into());
            }
            Request::ProcessPipeline {
                process: number(process)?,
                stages,
                stdin: bytes(stdin)?,
                max_output: number(max_output)?,
                deadline: parse_deadline(deadline)?,
            }
        }
        [
            "process_spawn_pipeline",
            process,
            max_stderr,
            count,
            rest @ ..,
        ] => {
            let mut words = rest.iter().copied().peekable();
            let mut used = 0;
            let mut stages = vec![];
            for _ in 0..number::<usize>(count)? {
                let (stage, n) = parse_command(&mut words)?;
                used += n;
                stages.push(stage);
            }
            if used != rest.len() {
                return Err("trailing process tape words".into());
            }
            Request::ProcessSpawnPipeline {
                process: number(process)?,
                stages,
                max_stderr: number(max_stderr)?,
            }
        }
        ["group_wait", process, group, deadline] => Request::GroupWait {
            process: number(process)?,
            group: number(group)?,
            deadline: parse_deadline(deadline)?,
        },
        ["process_spawn", process, rest @ ..] => Request::ProcessSpawn {
            process: number(process)?,
            command: parse_command(&mut rest.iter().copied().peekable()).and_then(ended(rest))?,
        },
        ["child_write", process, child, b] => Request::ChildWrite {
            process: number(process)?,
            child: number(child)?,
            bytes: bytes(b)?,
        },
        ["child_close_stdin", process, child] => Request::ChildCloseStdin {
            process: number(process)?,
            child: number(child)?,
        },
        ["child_read", process, child, stream, max] => Request::ChildRead {
            process: number(process)?,
            child: number(child)?,
            stream: match *stream {
                "stdout" => ChildStream::Stdout,
                "stderr" => ChildStream::Stderr,
                _ => return Err("bad child tape stream".into()),
            },
            max: number(max)?,
        },
        ["child_wait", process, child, deadline] => Request::ChildWait {
            process: number(process)?,
            child: number(child)?,
            deadline: parse_deadline(deadline)?,
        },
        ["child_kill", process, child, signal] => Request::ChildKill {
            process: number(process)?,
            child: number(child)?,
            signal: match *signal {
                "interrupt" => Signal::Interrupt,
                "terminate" => Signal::Terminate,
                "kill" => Signal::Kill,
                _ => return Err("bad process tape signal".into()),
            },
        },
        _ => return Ok(None),
    }))
}
fn parse_failure(words: &[&str]) -> Result<ProcessFailure, String> {
    Ok(match words {
        ["not_granted"] => ProcessFailure::NotGranted,
        ["invalid_input"] => ProcessFailure::InvalidInput,
        ["not_found"] => ProcessFailure::NotFound,
        ["permission_denied"] => ProcessFailure::PermissionDenied,
        ["output_limit"] => ProcessFailure::OutputLimit,
        ["fault", code] => ProcessFailure::Fault(number(code)?),
        ["timed_out"] => ProcessFailure::TimedOut,
        ["canceled", cause] => ProcessFailure::Canceled(number(cause)?),
        ["input_closed"] => ProcessFailure::InputClosed,
        _ => return Err("bad process tape error".into()),
    })
}
pub(crate) fn parse_completion(words: &[&str]) -> Result<Option<Completion>, String> {
    Ok(Some(match words {
        ["input", "ok", b] => Completion::Input(Ok(bytes(b)?)),
        ["input", "fault", n] => Completion::Input(Err(InputError::Fault(number(n)?))),
        ["input", "canceled", n] => Completion::Input(Err(InputError::Canceled(number(n)?))),
        ["input", "invalid_input"] => Completion::Input(Err(InputError::InvalidInput)),
        ["input", "invalid_utf8"] => Completion::Input(Err(InputError::InvalidUtf8)),
        ["env", "some", s] => Completion::Env(Ok(Some(text(s)?))),
        ["env", "none"] => Completion::Env(Ok(None)),
        ["span", handle, trace, span] => Completion::Span {
            handle: number(handle)?,
            trace: fixed(trace)?,
            span: fixed(span)?,
        },
        ["held"] => Completion::Held,
        ["blocked", "0"] => Completion::Blocked { committed: false },
        ["blocked", "1"] => Completion::Blocked { committed: true },
        ["ready"] => Completion::Ready,
        ["runner", kind, value, callee] => {
            use hirsel_run_schema::CallOutcome::*;
            Completion::Runner(hirsel_run_schema::RunnerCompletion {
                outcome: match *kind {
                    "ok" => Ok(bytes(value)?),
                    "failed" => Failed(bytes(value)?),
                    "faulted" => Faulted(bytes(value)?),
                    "not_run" => NotRun(number(value)?),
                    "in_doubt" => InDoubt(number(value)?),
                    "canceled" => Canceled(number(value)?),
                    _ => return Err("bad runner outcome".into()),
                },
                callee: match *callee {
                    "none" => None,
                    callee => Some(fixed(callee)?),
                },
            })
        }
        ["env", "not_granted"] => Completion::Env(Err(EnvError::NotGranted)),
        ["env", "invalid_utf8"] => Completion::Env(Err(EnvError::InvalidUtf8)),
        ["random", "ok", b] => Completion::Random(Ok(bytes(b)?)),
        ["random", "unavailable"] => Completion::Random(Err(RandomError::Unavailable)),
        ["wait", "ok"] => Completion::Wait(Ok(())),
        ["wait", "unavailable"] => Completion::Wait(Err(WaitError::Clock(ClockError::Unavailable))),
        ["wait", "out_of_range"] => Completion::Wait(Err(WaitError::Clock(ClockError::OutOfRange))),
        ["wait", "canceled", cause] => Completion::Wait(Err(WaitError::Canceled(number(cause)?))),
        ["signal", "canceled", cause] => {
            Completion::Signal(Err(SignalError::Canceled(number(cause)?)))
        }
        ["signal", "not_granted"] => Completion::Signal(Err(SignalError::NotGranted)),
        ["signal", "invalid_input"] => Completion::Signal(Err(SignalError::InvalidInput)),
        ["signal", kind] => Completion::Signal(Ok(
            hirsel_run_schema::SignalKind::parse(kind).ok_or("unknown signal kind")?
        )),
        [
            "process",
            kind @ ("exited" | "signaled"),
            code,
            stdout,
            stderr,
        ] => {
            let code = number(code)?;
            Completion::Process(ProcessOutcome::Output {
                status: if *kind == "exited" {
                    ExitStatus::Exited(code)
                } else {
                    ExitStatus::Signaled(code)
                },
                stdout: bytes(stdout)?,
                stderr: bytes(stderr)?,
            })
        }
        ["process", "error", failure @ ..] => {
            Completion::Process(ProcessOutcome::Error(parse_failure(failure)?))
        }
        ["pipeline", "ok", stdout, stderr, count, statuses @ ..] => {
            if statuses.len() != number::<usize>(count)?.saturating_mul(2) {
                return Err("process tape status count".into());
            }
            Completion::Pipeline(PipelineOutcome::Output {
                statuses: statuses
                    .chunks(2)
                    .map(|pair| {
                        let code = number(pair[1])?;
                        match pair[0] {
                            "exited" => Ok(ExitStatus::Exited(code)),
                            "signaled" => Ok(ExitStatus::Signaled(code)),
                            _ => Err("bad process tape status".to_string()),
                        }
                    })
                    .collect::<Result<_, _>>()?,
                stdout: bytes(stdout)?,
                stderr: bytes(stderr)?,
            })
        }
        ["pipeline", "error", failure @ ..] => {
            Completion::Pipeline(PipelineOutcome::Error(parse_failure(failure)?))
        }
        ["spawned", "ok", child] => Completion::Spawned(Ok(number(child)?)),
        ["child_unit", "ok"] => Completion::ChildUnit(Ok(())),
        ["child_bytes", "ok", b] => Completion::ChildBytes(Ok(bytes(b)?)),
        ["child_exit", "ok", kind @ ("exited" | "signaled"), code] => {
            let code = number(code)?;
            Completion::ChildExit(Ok(if *kind == "exited" {
                ExitStatus::Exited(code)
            } else {
                ExitStatus::Signaled(code)
            }))
        }
        ["group_exit", "ok", count, rest @ ..] => {
            let count = number::<usize>(count)?;
            if rest.len() != count.saturating_mul(3) {
                return Err("process tape stage count".into());
            }
            let (statuses, stderr) = rest.split_at(count * 2);
            Completion::GroupExit(Ok(hirsel_run_schema::GroupOutput {
                statuses: statuses
                    .chunks(2)
                    .map(|pair| {
                        let code = number(pair[1])?;
                        match pair[0] {
                            "exited" => Ok(ExitStatus::Exited(code)),
                            "signaled" => Ok(ExitStatus::Signaled(code)),
                            _ => Err("bad process tape status".to_string()),
                        }
                    })
                    .collect::<Result<_, _>>()?,
                stderr: stderr.iter().map(|b| bytes(b)).collect::<Result<_, _>>()?,
            }))
        }
        ["group_exit", "error", failure @ ..] => {
            Completion::GroupExit(Err(parse_failure(failure)?))
        }
        ["spawned", "error", failure @ ..] => Completion::Spawned(Err(parse_failure(failure)?)),
        ["child_unit", "error", failure @ ..] => {
            Completion::ChildUnit(Err(parse_failure(failure)?))
        }
        ["child_bytes", "error", failure @ ..] => {
            Completion::ChildBytes(Err(parse_failure(failure)?))
        }
        ["child_exit", "error", failure @ ..] => {
            Completion::ChildExit(Err(parse_failure(failure)?))
        }
        _ => return Ok(None),
    }))
}
