//! `hirsel tapes trial`: an upgrade trial of recorded runner calls (docs/spec/runners.md,
//! Upgrade trials). Each call a caller's tape recorded is issued again, in order, to a
//! callee run of a candidate revision: the same entry, form, cause and argument bytes,
//! admitted live under the grants a grants template's runner of that name holds (a fork of
//! the place's store, say). The trial reports, for each call, whether the candidate's
//! callee is the code the request named (its request digest is the recorded one) and
//! whether its outcome bytes equal the recorded completion's. The trial never touches the
//! caller: it replays nothing and contacts no place but the ones the record names.
use super::*;
use hirsel_rt::io::{Completion, Request};
use hirsel_run_schema::{CallForm, CallOutcome, CalleeOutcome, RunnerCompletion, RunnerRequest};

/// One recorded call and how the candidate answered it.
#[derive(Debug)]
pub struct TrialCall {
    pub ordinal: u64,
    pub path: String,
    /// The candidate's callee has the recorded code hash and signature fingerprint.
    pub same_code: bool,
    pub recorded: String,
    pub trial: String,
    /// Whether the outcome bytes are equal; None when the recording holds no outcome of the
    /// callee's (it was in doubt, not run or canceled).
    pub same_outcome: Option<bool>,
}

/// Issue the runner calls `tape` recorded, from request ordinal `from` on, to callee runs of
/// the program at `candidate` under the runners the grants template `grants` configures.
pub fn trial_tapes(
    candidate: &Path,
    grants: &std::ffi::OsStr,
    tape: &Path,
    from: u64,
) -> Result<Vec<TrialCall>, ToolError> {
    let refused = |e: String| ToolError::Runtime(format!("trial: {e}"));
    let recording = hirsel_rt::host_launch::read_recording(tape)
        .map_err(|e| refused(format!("{}: {e}", tape.display())))?;
    let instance = recording
        .binding
        .as_ref()
        .and_then(|binding| binding.instance)
        .ok_or_else(|| refused(format!("{} records no runner calls", tape.display())))?;
    let mut session = session::Session::open(candidate, hirsel_engine::Verify::On);
    let initial = session.request(requests::CompileOperation::Tir { definition: None })?;
    let requests::CompileResult::Tir(input) = &initial.result else {
        return Err(refused("TIR request returned another operation".into()));
    };
    tir_execution::refuse_foreign_interpretation(input)?;
    let revision = &input.revision;
    let place = hirsel_run_config::RunConfig::grants_template(
        grants,
        revision.digest(),
        revision.entry().into(),
        hirsel_run_schema::LaunchKind::Main,
    )
    .map_err(|e| refused(e.to_string()))?;
    let runners = place.io.map(|io| io.runners).unwrap_or_default();
    let facts = &input.callees;
    let mut trials = vec![];
    for (ordinal, (request, completion)) in calls(&recording.events, &instance)
        .map_err(refused)?
        .into_iter()
        .enumerate()
    {
        let ordinal = ordinal as u64;
        if ordinal < from {
            continue;
        }
        let recorded = match &completion {
            Some(completion) => shown(completion),
            None => "no completion".into(),
        };
        let fact = facts.get(&request.path);
        let same_code = fact.is_some_and(|fact| {
            hirsel_run_schema::request_digest(
                request.form,
                &request.path,
                &fact.fingerprint,
                &fact.code,
                &request.grants,
                &request.definitions,
                &request.argument,
            ) == request.digest
        });
        let (Some(fact), Some(entry)) = (fact, runners.iter().find(|r| r.name == request.runner))
        else {
            trials.push(TrialCall {
                ordinal,
                path: request.path.clone(),
                same_code,
                recorded,
                trial: match fact {
                    None => "the candidate has no such entry".into(),
                    Some(_) => format!("the record names no runner {}", request.runner),
                },
                same_outcome: Some(false),
            });
            continue;
        };
        let cause = hirsel_run_schema::Cause {
            instance,
            ordinal,
            digest: request.digest,
        };
        let key = hirsel_run_schema::call_key(&instance, ordinal);
        let seed = |name: &str| hirsel_run_schema::callee_seed(&key, name);
        let mut record = hirsel_run_config::RunConfig::empty(
            fact.program,
            request.path.clone(),
            hirsel_run_schema::LaunchKind::Call,
        );
        record.call = Some(hirsel_run_schema::CallRecord::new(
            request.form,
            &cause,
            &request.argument,
        ));
        record.inputs.hash_seed = seed("hash");
        record.inputs.random_seed = seed("random");
        record.inputs.test_seed = seed("test");
        record.io = match request.form {
            CallForm::Call => Some(entry.grants.clone().unwrap_or_default()),
            CallForm::Compute => None,
        };
        record.admission = entry
            .admission
            .as_ref()
            .map(|place| hirsel_run_schema::Admission {
                not_before: None,
                expires: None,
                revocation: None,
                code: place.code.clone(),
            });
        let run = record
            .validated()
            .map_err(|e| refused(format!("the callee record: {e}")))?;
        let launch = HostLaunch::Callee {
            run: &run,
            record: None,
        };
        let selection = launch_binding::LaunchSelection::read(launch)?;
        let selected = session.request_applied(requests::CompileOperation::Tir {
            definition: selection.entry().cloned(),
        })?;
        let requests::CompileResult::Tir(callee) = &selected.result else {
            return Err(refused("callee request returned another operation".into()));
        };
        let (outcome, returned) =
            tir_execution::run_tir_on_host(callee, launch, selection, None, None, None, false)?;
        let trial = match &returned {
            Some(returned) => shown_return(returned),
            None => format!("{outcome:?}"),
        };
        let same_outcome = completion.as_ref().and_then(|completion| {
            let recorded = match &completion.outcome {
                CallOutcome::Ok(bytes) => CalleeOutcome::Ok(bytes.clone()),
                CallOutcome::Failed(bytes) => CalleeOutcome::Err(bytes.clone()),
                CallOutcome::Faulted(bytes) => CalleeOutcome::Failed(bytes.clone()),
                _ => return None,
            };
            Some(returned.as_ref() == Some(&recorded))
        });
        trials.push(TrialCall {
            ordinal,
            path: request.path.clone(),
            same_code,
            recorded,
            trial,
            same_outcome,
        });
    }
    Ok(trials)
}

/// The tape's runner requests in request-ordinal order, each with its final completion.
fn calls(
    events: &[hirsel_rt::io::Event],
    instance: &[u8; 16],
) -> Result<Vec<(RunnerRequest, Option<RunnerCompletion>)>, String> {
    let mut calls: Vec<(RunnerRequest, Option<RunnerCompletion>)> = vec![];
    for event in events {
        let Request::RunnerCall(request) = &event.request else {
            continue;
        };
        let index = match calls.iter().position(|(r, _)| r.key == request.key) {
            Some(index) => index,
            None => {
                let ordinal = calls.len() as u64;
                if hirsel_run_schema::call_key(instance, ordinal) != request.key {
                    return Err(format!("request {ordinal}'s call key is not its own"));
                }
                calls.push(((**request).clone(), None));
                calls.len() - 1
            }
        };
        if let Completion::Runner(completion) = &event.completion {
            calls[index].1 = Some(completion.clone());
        }
    }
    Ok(calls)
}

fn text(bytes: &[u8]) -> String {
    hirsel_rt::codec_text::render(bytes).unwrap_or_else(|e| format!("<{e}>"))
}

/// A recorded completion as text.
fn shown(completion: &RunnerCompletion) -> String {
    const NOT_RUN: [&str; 5] = [
        "Denied",
        "VersionMismatch",
        "Unreachable",
        "Overloaded",
        "Rejected",
    ];
    const IN_DOUBT: [&str; 3] = ["Lost", "TimedOut", "Abandoned"];
    let reason = |names: &[&str], n: u32| names.get(n as usize).copied().unwrap_or("?").to_owned();
    match &completion.outcome {
        CallOutcome::Ok(bytes) => format!("Ok({})", text(bytes)),
        CallOutcome::Failed(bytes) => format!("Failed({})", text(bytes)),
        CallOutcome::Faulted(bytes) => format!("Faulted({})", text(bytes)),
        CallOutcome::NotRun(n) => format!("NotRun({}())", reason(&NOT_RUN, *n)),
        CallOutcome::InDoubt(n) => format!("InDoubt({}())", reason(&IN_DOUBT, *n)),
        CallOutcome::Canceled(_) => "Canceled".into(),
    }
}

/// A trial callee's own outcome as text, spelled as its caller would receive it.
fn shown_return(returned: &CalleeOutcome) -> String {
    match returned {
        CalleeOutcome::Ok(bytes) => format!("Ok({})", text(bytes)),
        CalleeOutcome::Err(bytes) => format!("Failed({})", text(bytes)),
        CalleeOutcome::Failed(bytes) => format!("Faulted({})", text(bytes)),
    }
}
