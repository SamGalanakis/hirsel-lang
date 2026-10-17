//! Additional typed tape forms for host streams and exact environment reads.
use crate::io::{Completion, EnvError, InputError, Request};
use hirsel_run_config::hex;

pub(crate) fn render_request(request: &Request) -> Option<String> {
    Some(match request {
        Request::Stdin { io } => format!("stdin {io}"),
        Request::Stderr { io } => format!("stderr {io}"),
        Request::ReadStdin { stdin, max } => format!("stdin_read {stdin} {max}"),
        Request::PrintStderr { stderr, bytes } => format!("stderr_print {stderr} {}", hex(bytes)),
        Request::EnvGet { env, name } => format!("env_get {env} {}", hex(name.as_bytes())),
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
        Completion::Env(Err(EnvError::NotGranted)) => "env not_granted".into(),
        Completion::Env(Err(EnvError::InvalidUtf8)) => "env invalid_utf8".into(),
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
        ["env", "not_granted"] => Completion::Env(Err(EnvError::NotGranted)),
        ["env", "invalid_utf8"] => Completion::Env(Err(EnvError::InvalidUtf8)),
        _ => return Ok(None),
    }))
}
