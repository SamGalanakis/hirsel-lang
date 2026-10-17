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
    ExitStatus, ProcessFailure, ProcessGrant, ProcessOutcome, ProcessRule, ProcessRun,
    ProviderContract, bounded, canonical_program, check,
};
use hirsel_types::{BuiltinType, Primitive};
use std::collections::BTreeMap;

/// The unused script entries of one scripted world, in order.
pub(super) type World = Vec<Option<ProcessRun>>;

pub(super) fn configured_world(
    grant: Option<&ProcessGrant>,
) -> Result<BTreeMap<u64, World>, Error> {
    let runs = match grant {
        None => vec![],
        Some(grant) => {
            if grant.rules.iter().any(|r| !canonical_program(&r.program)) {
                return Err(invalid("invalid simulation process rules"));
            }
            match (ProviderContract::parse(&grant.provider), &grant.runs) {
                (Some(ProviderContract::ProcessScript), Some(runs)) => {
                    if runs.iter().any(|run| !canonical_program(&run.program)) {
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
    Ok(BTreeMap::from([(0, runs.into_iter().map(Some).collect())]))
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
        _ => return None,
    })
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
        ProcessOutcome::Error(failure) => {
            let (ordinal, code) = failure_parts(*failure);
            error(variant(
                ordinal,
                if ordinal == 5 {
                    vec![super::dispatch::u32_value(code)]
                } else {
                    vec![]
                },
            ))
        }
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
                _ => return Err(invalid("ProcessError representation")),
            };
            ProcessOutcome::Error(
                failure_of(ordinal, code).ok_or_else(|| invalid("ProcessError ordinal"))?,
            )
        }
        _ => return Err(invalid("ProcessScript outcome representation")),
    };
    Ok(ProcessRun {
        program,
        args: strings(f[1])?,
        outcome,
    })
}

impl Simulation {
    /// Give the next granted `proc.run` a host completion; authority and replay are unchanged.
    pub fn supply_process(&mut self, outcome: ProcessOutcome) {
        self.supplied_process = Some(outcome);
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
                let mut programs: Vec<_> = runs.iter().map(|r| r.program.clone()).collect();
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
                let value = self.handle("Proc", Grant::Proc { world, rules });
                self.record("Test", name, encode_arguments(a), encode_value(&value), cx)?;
                value
            }
            "proc.run" => {
                let (world, rules) = self.process_grant(arg(a, 0)?)?;
                let program = super::dispatch::path_text(arg(a, 1)?)?.to_owned();
                let args = strings(arg(a, 2)?)?;
                bytes(arg(a, 3)?)?;
                let with_dir = match variant_fields(arg(a, 4)?)? {
                    (0, d) if d.is_empty() => false,
                    (1, d) if d.len() == 1 => {
                        self.id(d[0], "Dir")?;
                        true
                    }
                    _ => return Err(invalid("Option[Dir] representation")),
                };
                let max_output = match arg(a, 5)? {
                    Value::Integer(n) if n.kind() == Primitive::U64 => n.bits() as u64,
                    _ => return Err(invalid("max_output requires u64")),
                };
                let checked = check(&program, &args, &rules);
                let request = encode_arguments(a);
                let outcome = if let Some(recorded) = self.replay_output("Proc", name, &request)? {
                    // Replay re-derives the program check; a contradicting completion refuses.
                    let outcome = decode_completion(recorded)
                        .ok_or_else(|| invalid("invalid process tape completion"))?;
                    let consistent = match (&checked, &outcome) {
                        (Err(expected), ProcessOutcome::Error(found)) => expected == found,
                        (Err(_), _) => false,
                        (Ok(()), ProcessOutcome::Error(ProcessFailure::NotGranted)) => false,
                        (Ok(()), ProcessOutcome::Error(ProcessFailure::InvalidInput)) => with_dir,
                        (Ok(()), outcome) => bounded(outcome.clone(), max_output) == *outcome,
                    };
                    if !consistent {
                        return Err(invalid("process tape completion contradicts its grant"));
                    }
                    outcome
                } else if let Err(failure) = checked {
                    ProcessOutcome::Error(failure)
                } else if let Some(supplied) = self.supplied_process.take() {
                    bounded(supplied, max_output)
                } else if world == 0
                    && self.config.process.as_ref().is_some_and(|p| {
                        ProviderContract::parse(&p.provider) == Some(ProviderContract::ProcessSpawn)
                    })
                {
                    return Err(Error::Unavailable(format!(
                        "the reference runs host programs only through a host adapter; simulation requires {}",
                        ProviderContract::ProcessScript.name()
                    )));
                } else {
                    let runs = self
                        .process_worlds
                        .get_mut(&world)
                        .ok_or_else(|| invalid("missing process world"))?;
                    match runs.iter_mut().find(|r| {
                        r.as_ref()
                            .is_some_and(|r| r.program == program && r.args == args)
                    }) {
                        Some(entry) => bounded(entry.take().unwrap().outcome, max_output),
                        None => ProcessOutcome::Error(ProcessFailure::NotFound),
                    }
                };
                self.record("Proc", name, request, encode_completion(&outcome), cx)?;
                outcome_value(&outcome)
            }
            _ => return Err(invalid("unknown process operation")),
        };
        Ok(OperationResult::returned(value))
    }
}
