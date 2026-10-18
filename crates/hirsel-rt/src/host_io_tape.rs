//! Additional typed tape forms for host streams, exact environment reads, processes,
//! random draws and timer waits.
use crate::io::{ClockError, Completion, EnvError, InputError, RandomError, Request, WaitError};
use hirsel_run_config::{ExitStatus, ProcessFailure, ProcessOutcome, hex};

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

pub(crate) fn render_request(request: &Request) -> Option<String> {
    Some(match request {
        Request::Stdin { io } => format!("stdin {io}"),
        Request::Stderr { io } => format!("stderr {io}"),
        Request::ReadStdin { stdin, max } => format!("stdin_read {stdin} {max}"),
        Request::PrintStderr { stderr, bytes } => format!("stderr_print {stderr} {}", hex(bytes)),
        Request::EnvGet { env, name } => format!("env_get {env} {}", hex(name.as_bytes())),
        Request::Process { io } => format!("process {io}"),
        Request::Ready { wait, polls } => format!("ready {wait} {polls}"),
        Request::RandomBytes { random, count } => format!("random_bytes {random} {count}"),
        Request::SleepUntil { clock, deadline } => format!("sleep_until {clock} {deadline}"),
        Request::TestProcess {
            test,
            runs,
            children,
        } => format!(
            "test_process {test} {} {}",
            hex(&hirsel_run_config::encode_process_runs(runs)),
            hex(&hirsel_run_config::encode_child_runs(children))
        ),
        // The argument count leads, so every word after it is one argument.
        Request::ProcessRun {
            process,
            program,
            args,
            stdin,
            directory,
            max_output,
            deadline,
        } => {
            let mut text = format!(
                "process_run {process} {} {} {} {max_output} {} {}",
                hex(program.as_bytes()),
                hex(stdin),
                directory.map_or_else(|| "none".into(), |d| d.to_string()),
                deadline.map_or_else(|| "none".into(), |d| format!("{}:{}", d.clock, d.ticks)),
                args.len()
            );
            for arg in args {
                text.push(' ');
                text.push_str(&hex(arg.as_bytes()));
            }
            text
        }
        _ => return None,
    })
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
        Completion::Blocked { committed } => format!("blocked {}", u8::from(*committed)),
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
fn text(word: &str) -> Result<String, String> {
    String::from_utf8(bytes(word)?).map_err(|_| "bad host input tape UTF-8".into())
}
pub(crate) fn parse_request(words: &[&str]) -> Result<Option<Request>, String> {
    Ok(Some(match words {
        ["stdin", io] => Request::Stdin { io: number(io)? },
        ["stderr", io] => Request::Stderr { io: number(io)? },
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
        ["process", io] => Request::Process { io: number(io)? },
        ["ready", wait, polls] => Request::Ready {
            wait: number(wait)?,
            polls: number(polls)?,
        },
        ["random_bytes", random, count] => Request::RandomBytes {
            random: number(random)?,
            count: number(count)?,
        },
        ["sleep_until", clock, deadline] => Request::SleepUntil {
            clock: number(clock)?,
            deadline: number(deadline)?,
        },
        ["test_process", test, runs, children] => Request::TestProcess {
            test: number(test)?,
            runs: hirsel_run_config::decode_process_runs(&bytes(runs)?)
                .map_err(|e| e.to_string())?,
            children: hirsel_run_config::decode_child_runs(&bytes(children)?)
                .map_err(|e| e.to_string())?,
        },
        [
            "process_run",
            process,
            program,
            stdin,
            directory,
            max_output,
            deadline,
            count,
            args @ ..,
        ] => {
            if number::<usize>(count)? != args.len() {
                return Err("process tape argument count".into());
            }
            Request::ProcessRun {
                process: number(process)?,
                program: text(program)?,
                args: args.iter().map(|a| text(a)).collect::<Result<_, _>>()?,
                stdin: bytes(stdin)?,
                directory: match *directory {
                    "none" => None,
                    d => Some(number(d)?),
                },
                max_output: number(max_output)?,
                deadline: match *deadline {
                    "none" => None,
                    d => {
                        let (clock, ticks) = d.split_once(':').ok_or("process tape deadline")?;
                        Some(crate::process::Deadline {
                            clock: number(clock)?,
                            ticks: number(ticks)?,
                        })
                    }
                },
            }
        }
        _ => return Ok(None),
    }))
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
        ["blocked", "0"] => Completion::Blocked { committed: false },
        ["blocked", "1"] => Completion::Blocked { committed: true },
        ["ready"] => Completion::Ready,
        ["env", "not_granted"] => Completion::Env(Err(EnvError::NotGranted)),
        ["env", "invalid_utf8"] => Completion::Env(Err(EnvError::InvalidUtf8)),
        ["random", "ok", b] => Completion::Random(Ok(bytes(b)?)),
        ["random", "unavailable"] => Completion::Random(Err(RandomError::Unavailable)),
        ["wait", "ok"] => Completion::Wait(Ok(())),
        ["wait", "unavailable"] => Completion::Wait(Err(WaitError::Clock(ClockError::Unavailable))),
        ["wait", "out_of_range"] => Completion::Wait(Err(WaitError::Clock(ClockError::OutOfRange))),
        ["wait", "canceled", cause] => Completion::Wait(Err(WaitError::Canceled(number(cause)?))),
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
            Completion::Process(ProcessOutcome::Error(match failure {
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
            }))
        }
        _ => return Ok(None),
    }))
}
