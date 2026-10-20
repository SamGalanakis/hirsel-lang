//! Runner calls as data (RUN-10, RUN-11): a callee run's `call` member, the request a
//! caller records, the outcome its completion carries, and the hashes that link them. A
//! callee's seeds and run instance derive from the call key, so an in-process or simulated
//! callee is a pure function of its caller's run instance and the request ordinal.
use crate::{RunRefusal, hash_parts, hex, invalid};
use serde::{Deserialize, Serialize};

/// A runner call's form.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum CallForm {
    /// `runner.compute`: no capability crosses.
    Compute,
    /// `runner.call` and `runner.spawn`: the callee receives the place's `Io`.
    Call,
}
impl CallForm {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Compute => "compute",
            Self::Call => "call",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Compute, Self::Call]
            .into_iter()
            .find(|form| form.name() == name)
    }
}

/// A callee run's `call` member: the form, the cause naming the request it answers, and the
/// exact argument bytes. Hashes and bytes are lowercase hex.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallRecord {
    pub form: String,
    pub cause: CallCause,
    pub argument: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallCause {
    /// The caller's run instance, 32 hex digits.
    pub instance: String,
    pub ordinal: u64,
    /// The request digest, 64 hex digits.
    pub digest: String,
}
impl CallRecord {
    pub fn new(form: CallForm, cause: &Cause, argument: &[u8]) -> Self {
        Self {
            form: form.name().into(),
            cause: CallCause {
                instance: hex(&cause.instance),
                ordinal: cause.ordinal,
                digest: hex(&cause.digest),
            },
            argument: hex(argument),
        }
    }
    pub fn form(&self) -> CallForm {
        CallForm::parse(&self.form).expect("a validated call form")
    }
    pub fn cause(&self) -> Result<Cause, RunRefusal> {
        Ok(Cause {
            instance: fixed(&self.cause.instance).ok_or_else(|| {
                invalid("call.cause.instance", "expected 32 lowercase hex digits")
            })?,
            ordinal: self.cause.ordinal,
            digest: fixed(&self.cause.digest)
                .ok_or_else(|| invalid("call.cause.digest", "expected 64 lowercase hex digits"))?,
        })
    }
    pub fn argument(&self) -> Result<Vec<u8>, RunRefusal> {
        unhex(&self.argument)
            .ok_or_else(|| invalid("call.argument", "expected lowercase hex bytes"))
    }
}
/// A call's cause: the caller's run instance, its request ordinal and request digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cause {
    pub instance: [u8; 16],
    pub ordinal: u64,
    pub digest: [u8; 32],
}

/// What a caller's place knows of a callee entry, found by the launcher from checked code:
/// the semantic program revision its run binds, its definition hash (the code hash a request
/// names), its signature fingerprint, and what it reaches (CAP-6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalleeFacts {
    pub program: [u8; 32],
    pub code: [u8; 32],
    pub fingerprint: [u8; 32],
    pub reach: crate::CodeReach,
}

/// A request as the caller's tape records it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerRequest {
    pub runner: String,
    pub form: CallForm,
    pub path: String,
    /// The requested grant set and allowlist, in canonical bytes.
    pub grants: Vec<u8>,
    pub definitions: Vec<u8>,
    pub key: [u8; 32],
    pub digest: [u8; 32],
    pub argument: Vec<u8>,
}
/// How a call ended, as its completion records it: the exact result, error or failure
/// bytes when the callee produced them, else the reason or cause.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallOutcome {
    Ok(Vec<u8>),
    Failed(Vec<u8>),
    Faulted(Vec<u8>),
    NotRun(u32),
    InDoubt(u32),
    Canceled(u32),
}
/// A call's completion: its outcome and the callee run's instance when the callee ran.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerCompletion {
    pub outcome: CallOutcome,
    pub callee: Option<[u8; 16]>,
}
/// How a callee run ended, as its own tape's trailer records it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalleeOutcome {
    /// The entry returned `Ok(r)`: r's bytes.
    Ok(Vec<u8>),
    /// The entry returned `Err(e)`: e's bytes.
    Err(Vec<u8>),
    /// The run failed: its `Failure`'s bytes.
    Failed(Vec<u8>),
}
impl CallOutcome {
    /// Whether this outcome carries `callee`'s own outcome, which a joined tape check
    /// compares with the callee's trailer.
    pub fn carries(&self, callee: &CalleeOutcome) -> Option<bool> {
        match (self, callee) {
            (Self::Ok(a), CalleeOutcome::Ok(b))
            | (Self::Failed(a), CalleeOutcome::Err(b))
            | (Self::Faulted(a), CalleeOutcome::Failed(b)) => Some(a == b),
            (Self::Ok(_) | Self::Failed(_) | Self::Faulted(_), _) => Some(false),
            _ => None,
        }
    }
}

/// `H("hirsel.runner.request.v1", form, path, signature fingerprint, code hash, requested
/// grant set, allowlist, argument bytes)` (RUN-11).
#[allow(clippy::too_many_arguments)]
pub fn request_digest(
    form: CallForm,
    path: &str,
    fingerprint: &[u8; 32],
    code: &[u8; 32],
    grants: &[u8],
    definitions: &[u8],
    argument: &[u8],
) -> [u8; 32] {
    hash_parts(
        "hirsel.runner.request.v1",
        [
            form.name().as_bytes(),
            path.as_bytes(),
            fingerprint,
            code,
            grants,
            definitions,
            argument,
        ],
    )
}
/// `H("hirsel.runner.key.v1", run instance, request ordinal)`.
pub fn call_key(instance: &[u8; 16], ordinal: u64) -> [u8; 32] {
    hash_parts(
        "hirsel.runner.key.v1",
        [&instance[..], &ordinal.to_le_bytes()],
    )
}
/// A callee seed: the first eight bytes, little-endian, of `H("hirsel.runner.seed.v1",
/// call key, seed name)`.
pub fn callee_seed(key: &[u8; 32], name: &str) -> u64 {
    let hash = hash_parts("hirsel.runner.seed.v1", [&key[..], name.as_bytes()]);
    u64::from_le_bytes(hash[..8].try_into().unwrap())
}
/// An in-process or simulated callee run's instance, derived as its seeds are.
pub fn callee_instance(key: &[u8; 32]) -> [u8; 16] {
    let hash = hash_parts("hirsel.runner.instance.v1", [&key[..]]);
    hash[..16].try_into().unwrap()
}
fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok())
        .collect()
}
fn fixed<const N: usize>(text: &str) -> Option<[u8; N]> {
    unhex(text)?.try_into().ok()
}

/// Callee facts as a launcher hands them to a native run: a JSON object keyed by entry path,
/// each `{"program", "code", "fingerprint"}` in lowercase hex and `"reach"`, null for a safe
/// callee or the path to its first escape.
pub fn callees_json(facts: &std::collections::BTreeMap<String, CalleeFacts>) -> String {
    let object: serde_json::Map<String, serde_json::Value> = facts
        .iter()
        .map(|(path, f)| {
            let reach = match &f.reach {
                crate::CodeReach::Safe => serde_json::Value::Null,
                crate::CodeReach::Escapes(path) => serde_json::json!(path),
            };
            (
                path.clone(),
                serde_json::json!({
                    "program": crate::hex(&f.program),
                    "code": crate::hex(&f.code),
                    "fingerprint": crate::hex(&f.fingerprint),
                    "reach": reach,
                }),
            )
        })
        .collect();
    serde_json::Value::Object(object).to_string()
}
/// [`callees_json`]'s inverse.
pub fn parse_callees(
    text: &str,
) -> Result<std::collections::BTreeMap<String, CalleeFacts>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let object = value.as_object().ok_or("callee facts are not an object")?;
    let mut out = std::collections::BTreeMap::new();
    for (path, facts) in object {
        let word = |name: &str| -> Result<[u8; 32], String> {
            facts
                .get(name)
                .and_then(serde_json::Value::as_str)
                .and_then(fixed)
                .ok_or_else(|| format!("callee {path}: bad {name}"))
        };
        let reach = match facts.get("reach") {
            Some(serde_json::Value::Null) => crate::CodeReach::Safe,
            Some(serde_json::Value::Array(items)) => crate::CodeReach::Escapes(
                items
                    .iter()
                    .map(|i| i.as_str().map(str::to_owned))
                    .collect::<Option<_>>()
                    .ok_or_else(|| format!("callee {path}: bad reach"))?,
            ),
            _ => return Err(format!("callee {path}: bad reach")),
        };
        out.insert(
            path.clone(),
            CalleeFacts {
                program: word("program")?,
                code: word("code")?,
                fingerprint: word("fingerprint")?,
                reach,
            },
        );
    }
    Ok(out)
}
