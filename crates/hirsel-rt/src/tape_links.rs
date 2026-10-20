//! The joined check over a set of linked tapes (RUN-11). A callee run's record names its
//! cause: the caller's run instance, the request ordinal and the request digest. For every
//! link whose caller and callee are both in the set, the callee's call must be the request
//! its caller recorded under that call key (form, path, digest and argument bytes), and a
//! caller completion that carries the callee's outcome must name the callee's run instance
//! and hold exactly the bytes the callee recorded as it ended (`call_return`). Any mismatch
//! refuses the set. A link with one end outside the set (a run recorded at `none`, or a tape
//! not given) is reported as unchecked. No program is needed: the tapes alone are compared.
use crate::io::{Completion, Request};
use crate::launch::Recording;
use hirsel_run_config::RunConfig;
use hirsel_run_schema::{
    CalleeOutcome, RunnerCompletion, RunnerRequest, call_key, callee_instance, hex,
};
use std::collections::{BTreeMap, BTreeSet};

/// What a joined check found: the links it checked and those it could not.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Links {
    pub checked: Vec<String>,
    pub unchecked: Vec<String>,
}

/// One tape's side of its links.
struct Tape<'a> {
    name: &'a str,
    instance: Option<[u8; 16]>,
    record: RunConfig,
    /// Its calls by key: the request and, once completed, the completion.
    requests: BTreeMap<[u8; 32], (RunnerRequest, Option<RunnerCompletion>)>,
    /// Its own outcome, for a callee run.
    returned: Option<CalleeOutcome>,
}
impl<'a> Tape<'a> {
    fn new(name: &'a str, recording: &Recording) -> Result<Self, String> {
        let binding = recording
            .binding
            .as_ref()
            .ok_or_else(|| format!("{name}: the recording binds no run record"))?;
        let run = recording.bound_run().map_err(|e| format!("{name}: {e}"))?;
        let mut requests: BTreeMap<_, (RunnerRequest, Option<RunnerCompletion>)> = BTreeMap::new();
        let mut returned = None;
        for event in &recording.events {
            match (&event.request, &event.completion) {
                (Request::RunnerCall(request), completion) => {
                    let entry = requests
                        .entry(request.key)
                        .or_insert_with(|| ((**request).clone(), None));
                    if entry.0 != **request {
                        return Err(format!(
                            "{name}: call {} was issued with different requests",
                            hex(&request.key)
                        ));
                    }
                    if let Completion::Runner(completion) = completion {
                        entry.1 = Some(completion.clone());
                    }
                }
                (Request::CallReturn(outcome), _) => returned = Some(outcome.clone()),
                _ => {}
            }
        }
        Ok(Self {
            name,
            instance: binding.instance,
            record: run.record().clone(),
            requests,
            returned,
        })
    }
}

/// Check the links among `tapes`, each named for the report; a mismatch refuses the set.
pub fn check(tapes: &[(String, Recording)]) -> Result<Links, Vec<String>> {
    let mut parsed = vec![];
    let mut refused = vec![];
    for (name, recording) in tapes {
        match Tape::new(name, recording) {
            Ok(tape) => parsed.push(tape),
            Err(error) => refused.push(error),
        }
    }
    let mut links = Links::default();
    let mut linked = BTreeSet::new();
    for callee in &parsed {
        let Some(call) = &callee.record.call else {
            continue;
        };
        let cause = match call.cause() {
            Ok(cause) => cause,
            Err(error) => {
                refused.push(format!("{}: {error}", callee.name));
                continue;
            }
        };
        let key = call_key(&cause.instance, cause.ordinal);
        let link = format!(
            "{} answers request {} of run {}",
            callee.name,
            cause.ordinal,
            hex(&cause.instance)
        );
        let Some(caller) = parsed
            .iter()
            .find(|tape| tape.instance == Some(cause.instance))
        else {
            links
                .unchecked
                .push(format!("{link}: the caller's tape is not in the set"));
            continue;
        };
        linked.insert((caller.name, key));
        let mismatch = |what: &str| format!("{link} in {}: {what}", caller.name);
        let Some((request, completion)) = caller.requests.get(&key) else {
            refused.push(mismatch("the caller recorded no request with its call key"));
            continue;
        };
        let instance = callee_instance(&key);
        let mut problems = vec![];
        if request.digest != cause.digest {
            problems.push("the request digests differ");
        }
        if call.argument().ok().as_ref() != Some(&request.argument) {
            problems.push("the argument bytes differ");
        }
        if request.form != call.form() || request.path != callee.record.entry {
            problems.push("the call's form or callee differ");
        }
        if callee.instance != Some(instance) {
            problems.push("the callee's run instance is not its call key's");
        }
        if let Some(completion) = completion {
            if completion.callee.is_some_and(|c| c != instance) {
                problems.push("the completion names another callee run");
            }
            let carried = callee
                .returned
                .as_ref()
                .and_then(|returned| completion.outcome.carries(returned));
            match (carried, &callee.returned) {
                (Some(false), _) => problems.push("the outcome bytes differ"),
                (None, None) if carries_bytes(completion) => {
                    problems.push("the callee recorded no outcome")
                }
                _ => {}
            }
        }
        if problems.is_empty() {
            links.checked.push(link);
        } else {
            refused.extend(problems.into_iter().map(mismatch));
        }
    }
    // A completed call whose callee ran, with no callee tape in the set.
    for caller in &parsed {
        for (key, (request, completion)) in &caller.requests {
            if completion.as_ref().is_some_and(|c| c.callee.is_some())
                && !linked.contains(&(caller.name, *key))
            {
                links.unchecked.push(format!(
                    "{} called {} through {}: the callee's tape is not in the set",
                    caller.name, request.path, request.runner
                ));
            }
        }
    }
    if refused.is_empty() {
        Ok(links)
    } else {
        Err(refused)
    }
}

fn carries_bytes(completion: &RunnerCompletion) -> bool {
    use hirsel_run_schema::CallOutcome::*;
    matches!(completion.outcome, Ok(_) | Failed(_) | Faulted(_))
}
