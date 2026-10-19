//! Runner calls of a host run (docs/spec/runners.md). The library decides each call; this
//! records it on the run's tape as one request and one completion (RUN-11): a call refused
//! before it is sent completes at once, and a sent call parks until its callee run ends,
//! when the execution delivers its outcome and the caller's next scheduling point re-issues
//! the request to complete it (`ready`, then the completion). A `runner.spawn` call's child
//! is completed at that scheduling point, since no task polls it. A forwarded cancellation
//! is recorded with its acknowledgement.
//!
//! Live, each sent call's callee runs as its own admitted host run (RUN-10): a record with
//! launch `call`, the call's cause and argument bytes, the requested grants, seeds and run
//! instance derived from the call key, and the place's code policy. Its providers park on
//! the caller's readiness under their own tag (RUN-12), and when the caller records, the
//! callee records `<tape>.call-<ordinal>` and ends it with its own outcome's bytes
//! (`call_return`). Replay starts no callee: the recorded completion stands in for it.
use super::*;
use hirsel_interp::library::runners::{RunnerSend, callee_outcome};
use hirsel_interp::{CallSchemas, CalleeEnd, CalleeStart};
use hirsel_run_config::{CallForm, CalleeFacts, RunnerCompletion, RunnerRequest};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// A host run's runner calls and the callee runs it starts.
#[derive(Default)]
pub(crate) struct HostRunners {
    /// Parked calls by wait token: the request to re-issue, and whether `runner.spawn`'s
    /// child holds the outcome.
    waits: BTreeMap<u64, (RunnerRequest, bool)>,
    /// This run's tape: a callee records `<tape>.call-<ordinal>` beside it.
    tape: Option<PathBuf>,
    /// For a callee run, its call's schemas: it records its own outcome as it ends.
    role: Option<CallSchemas>,
    /// The execution's next readiness tag, shared by all its runs.
    tags: Rc<Cell<u64>>,
}

/// Where a host run's runner calls lead: what the launcher found about the program and
/// recorded about the run.
pub(crate) struct RunnerContext {
    pub instance: Option<[u8; 16]>,
    pub callees: Arc<BTreeMap<String, CalleeFacts>>,
    pub tape: Option<PathBuf>,
    /// A callee run's schemas (a `call` launch).
    pub role: Option<CallSchemas>,
}

impl HostLibrary {
    /// Configure the run's runners: its instance, the callee facts and its tape.
    pub(crate) fn set_runners(&mut self, context: RunnerContext) {
        let config = &mut self.library.simulation.config;
        if let Some(instance) = context.instance {
            config.instance = instance;
        }
        config.callees = context.callees;
        self.runners.tape = context.tape;
        self.runners.role = context.role;
    }
    fn adapter_error(&mut self, error: hirsel_rt::io::AdapterError) -> Error {
        let message = format!("{error:?}");
        self.refused.get_or_insert(message.clone());
        Error::Unavailable(format!("host Io: {message}"))
    }
    /// A recorded completion whose bytes the call's schemas do not decode diverges from
    /// the run that recorded it.
    fn undecodable(&mut self, error: Error) -> Error {
        match self.io {
            HostIo::Replay(_) => {
                let message = format!("a runner call's recorded completion: {error:?}");
                self.refused.get_or_insert(message.clone());
                Error::Unavailable(message)
            }
            HostIo::Live(_) => error,
        }
    }
    /// Issue a call's request under `token`: its completion, or None when it parked.
    fn runner_issue(
        &mut self,
        token: u64,
        request: RunnerRequest,
    ) -> Result<Option<RunnerCompletion>, Error> {
        self.io.adapter().enter(token);
        match self.io.call(R::RunnerCall(Box::new(request))) {
            Ok(C::Runner(completion)) => {
                self.io.adapter().finish(token);
                Ok(Some(completion))
            }
            Err(hirsel_rt::io::AdapterError::Blocked) => Ok(None),
            Ok(_) => Err(self.adapter_error(hirsel_rt::io::AdapterError::InvalidCompletion)),
            Err(error) => Err(self.adapter_error(error)),
        }
    }
    /// Record the calls the library made: a refused call with its completion at once, a sent
    /// one parked until its callee ends.
    pub(super) fn runner_sends(&mut self, cx: &mut OperationContext<'_>) -> Result<(), Error> {
        for RunnerSend {
            request,
            sent,
            child,
        } in self.library.take_runner_sends()
        {
            match sent {
                Err(completion) => {
                    let token = self.next_wait;
                    self.next_wait += 1;
                    if let HostIo::Live(io) = &mut self.io {
                        io.deliver(token, completion.clone());
                    }
                    // Replay checks the recorded completion against the library's decision.
                    if self.runner_issue(token, request)? != Some(completion) {
                        return Err(
                            self.adapter_error(hirsel_rt::io::AdapterError::InvalidCompletion)
                        );
                    }
                }
                Ok(token) => match self.runner_issue(token, request.clone())? {
                    None => {
                        self.runners.waits.insert(token, (request, child));
                        // No task parks for a spawned call: its child waits from now.
                        if child {
                            self.io.adapter().parked(token);
                        }
                    }
                    Some(completion) => self
                        .library
                        .runner_completed(token, completion, cx)
                        .map_err(|e| self.undecodable(e))?,
                },
            }
        }
        Ok(())
    }
    pub(super) fn runner_waits(&self) -> bool {
        !self.runners.waits.is_empty()
    }
    pub(super) fn waits_on_runner(&self, token: u64) -> bool {
        self.runners.waits.contains_key(&token)
    }
    /// Re-issue the call parked at `token` once ready: true when it completed.
    fn runner_retry(&mut self, token: u64, cx: &mut OperationContext<'_>) -> Result<bool, Error> {
        if !self.ready.contains(token) {
            return Ok(false);
        }
        let (request, _) = self.runners.waits[&token].clone();
        let Some(completion) = self.runner_issue(token, request)? else {
            return Ok(false);
        };
        self.runners.waits.remove(&token);
        self.library
            .runner_completed(token, completion, cx)
            .map_err(|e| self.undecodable(e))?;
        Ok(true)
    }
    /// The task parked on a sent call: its outcome once the call completed.
    pub(super) fn runner_poll(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if !self.runner_retry(token, cx)? {
            return Ok(None);
        }
        self.library
            .poll(token, cx)
            .map_err(|e| self.undecodable(e))
    }
    /// A scheduling point: complete the spawned calls whose outcome arrived.
    pub(super) fn runner_children(&mut self, cx: &mut OperationContext<'_>) -> Result<(), Error> {
        let children: Vec<u64> = self
            .runners
            .waits
            .iter()
            .filter(|(_, (_, child))| *child)
            .map(|(token, _)| *token)
            .collect();
        for token in children {
            self.runner_retry(token, cx)?;
        }
        Ok(())
    }
    /// The callee runs of the calls sent since the last take, each admitted as its own
    /// host run. Replay starts none.
    pub(super) fn runner_calls(&mut self) -> Vec<CalleeStart> {
        let starts = self.library.take_calls();
        if matches!(self.io, HostIo::Replay(_)) {
            return vec![];
        }
        let mut admitted = vec![];
        for start in starts {
            match self.host_callee(start) {
                Ok(start) => admitted.push(start),
                Err(error) => {
                    self.refused.get_or_insert(error);
                }
            }
        }
        admitted
    }
    /// RUN-9: record each forwarded cancellation and its acknowledgement; live, the
    /// execution forwards it to the callee.
    pub(super) fn runner_cancels(&mut self) -> Vec<u64> {
        let tokens = self.library.take_cancels();
        for &token in &tokens {
            let Some((key, cause)) = self.library.runner_cancel(token) else {
                continue;
            };
            match self.io.call(R::RunnerCancel { key, cause }) {
                Ok(C::Ready) => {}
                Ok(_) => {
                    self.adapter_error(hirsel_rt::io::AdapterError::InvalidCompletion);
                }
                Err(error) => {
                    self.adapter_error(error);
                }
            }
        }
        match self.io {
            HostIo::Live(_) => tokens,
            HostIo::Replay(_) => vec![],
        }
    }
    /// The callee of the call parked at `token` ended: its completion is delivered, and the
    /// caller's next scheduling point completes the call.
    pub(super) fn runner_ended(
        &mut self,
        token: u64,
        end: CalleeEnd,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let completion = self.library.runner_completion(token, end, cx)?;
        match &mut self.io {
            HostIo::Live(io) => io.deliver(token, completion),
            HostIo::Replay(_) => return Err(Error::Unavailable("a callee ran in replay".into())),
        }
        Ok(())
    }
    /// This run, a callee, ended: record its own outcome's bytes, then end its tape.
    pub(crate) fn callee_return(
        &mut self,
        end: &CalleeEnd,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let Some(schemas) = &self.runners.role else {
            return Ok(());
        };
        if let Some(outcome) = callee_outcome(end, schemas, cx)? {
            match self.io.call(R::CallReturn(outcome)) {
                Ok(C::Ready) => {}
                Ok(_) => {
                    return Err(self.adapter_error(hirsel_rt::io::AdapterError::InvalidCompletion));
                }
                Err(error) => return Err(self.adapter_error(error)),
            }
        }
        Ok(())
    }
    /// A live callee run's end: its own outcome, then its tape's trailer.
    pub(super) fn callee_finished(
        &mut self,
        end: &CalleeEnd,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        self.callee_return(end, cx)?;
        let outcome = match end {
            CalleeEnd::Returned(Value::Variant { variant: 0, .. }) => {
                hirsel_run_config::RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            }
            CalleeEnd::Returned(_) => {
                hirsel_run_config::RunOutcome::Returned(hirsel_run_config::Returned::Error)
            }
            CalleeEnd::Failed(chain) => hirsel_engine::project_failures(chain, cx.sources)
                .unwrap_or_else(|error| {
                    hirsel_run_config::RunOutcome::Tooling(format!("{error:?}"))
                }),
            CalleeEnd::BudgetExceeded(kind) => hirsel_run_config::RunOutcome::BudgetExceeded(*kind),
            CalleeEnd::Canceled => hirsel_run_config::RunOutcome::Tooling(
                "the call was cancelled before its callee started".into(),
            ),
            CalleeEnd::Abandoned => hirsel_run_config::RunOutcome::Tooling(
                "the place stopped the callee without an outcome".into(),
            ),
        };
        if let HostIo::Live(io) = &mut self.io {
            io.finish_recording(&outcome)
                .map_err(|e| Error::Unavailable(format!("the callee's Io tape: {e}")))?;
        }
        Ok(())
    }

    /// Admit a sent call's callee as its own host run beside its caller.
    fn host_callee(&mut self, mut start: CalleeStart) -> Result<CalleeStart, String> {
        let call = &start.call;
        let path = start.configuration.entry.to_string();
        let facts = self
            .library
            .simulation
            .config
            .callees
            .get(&path)
            .cloned()
            .ok_or_else(|| format!("no checked facts for the callee {path}"))?;
        let key = hirsel_run_config::call_key(&call.cause.instance, call.cause.ordinal);
        let seed = |name: &str| hirsel_run_config::callee_seed(&key, name);
        let mut record = hirsel_run_config::RunConfig::empty(
            facts.program,
            path.clone(),
            hirsel_run_config::LaunchKind::Call,
        );
        record.call = Some(hirsel_run_config::CallRecord::new(
            call.form,
            &call.cause,
            &call.argument,
        ));
        record.inputs.hash_seed = seed("hash");
        record.inputs.random_seed = seed("random");
        record.inputs.test_seed = seed("test");
        record.io = (call.form == CallForm::Call).then(|| call.grants.clone());
        record.admission =
            call.runner
                .admission
                .as_ref()
                .map(|place| hirsel_run_config::Admission {
                    not_before: None,
                    expires: None,
                    revocation: None,
                    code: place.code.clone(),
                });
        let refused = |e: hirsel_run_config::RunRefusal| format!("callee admission: {e}");
        let bound = record
            .validated()
            .map_err(refused)?
            .bind(&hirsel_run_config::ProgramFacts {
                program: facts.program,
                entry: path,
                launch: hirsel_run_config::LaunchKind::Call,
                roots: match call.form {
                    CallForm::Call => vec![hirsel_run_config::Root::Io],
                    CallForm::Compute => vec![],
                },
                code: facts.reach.clone(),
                fueled: true,
            })
            .map_err(refused)?;
        let HostIo::Live(io) = &self.io else {
            return Err("a callee run in replay".into());
        };
        let readiness = io
            .readiness()
            .ok_or("a runner call without cooperative tasks")?;
        let tag = self.runners.tags.get() + 1;
        self.runners.tags.set(tag);
        let mut host = hirsel_rt::host_launch::HostGrants::sharing(
            hirsel_rt::launch::RawStdout,
            readiness.tagged(tag),
        );
        let now = crate::admission_time().map_err(|e| e.to_string())?;
        let admitted = bound.admit(&mut host, now).map_err(refused)?;
        let mut io = host.into_io();
        // A runner entry recording `none` keeps its callees off tape.
        let tape = self
            .runners
            .tape
            .as_ref()
            .filter(|_| call.runner.recording.as_deref() != Some("none"))
            .map(|tape| {
                let mut name = tape.clone().into_os_string();
                name.push(format!(".call-{}", call.cause.ordinal));
                PathBuf::from(name)
            });
        hirsel_rt::host_launch::journal(&mut io, &admitted, tape.as_deref(), &[])
            .map_err(|e| format!("the callee's Io tape: {e}"))?;
        let mut callee = HostLibrary::new(&start.configuration, admitted.run(), HostIo::Live(io))
            .map_err(|e| format!("callee launch: {e:?}"))?;
        callee.set_runners(RunnerContext {
            instance: admitted.instance(),
            callees: self.library.simulation.config.callees.clone(),
            tape,
            role: Some(call.schemas.clone()),
        });
        callee.runners.tags = self.runners.tags.clone();
        // The callee receives its own root Io and the argument decoded from the request.
        let argument = start
            .arguments
            .pop()
            .ok_or("a runner call without its argument")?;
        start.arguments = match call.form {
            CallForm::Call => vec![callee.library.root_io(), argument],
            CallForm::Compute => vec![argument],
        };
        start.operations = Box::new(callee);
        Ok(start)
    }
}
