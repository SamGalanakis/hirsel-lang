//! Runners (docs/spec/runners.md). `io.runner` projects a runner the run's configuration
//! names; narrowing mints one over a subset of its grant set or allowlist. A call
//! encodes its argument, checks authority (the allowlist, the place's code policy) and
//! cancellation, then meets its place's contract: `local.threads@1` admits it up to its
//! threads and queue, and a `sim@1` node draws its configured faults from the fault seed.
//! A sent call's callee runs as its own run in this execution, in a world projected from
//! the runner's grants (`take_calls`); the calling task waits until the execution reports
//! the callee's end (`call_ended`). Arguments, results, errors and failures cross as
//! canonical codec bytes: a call's completion holds them (RUN-11), and the caller receives
//! fresh storage decoded from it. A host provider records each call's request and
//! completion (`take_runner_sends`, `runner_end`, `runner_completed`); its replay supplies
//! the recorded completion instead of a callee.
use super::dispatch::{arg, text, variant};
use super::simulation::{Grant, SimulationConfig};
use super::{Library, collection, error, ok};
use crate::observation::FailureRecord;
use crate::{
    BlockedWait, CallSchemas, CallStart, CalleeEnd, CalleeStart, Error, OperationCall,
    OperationContext, OperationResult, Operations, RunBudgets, Value, invalid,
};
use hirsel_run_config::{
    Access, CallForm, CallOutcome, CalleeOutcome, Cause, CodeReach, IoGrants, Mount, NetRule,
    ProcessRule, RunnerCompletion, RunnerEntry, RunnerRequest,
};
use hirsel_types::serialization::SerializableSchema;
use hirsel_types::{BuiltinType, GenericArgument};
use std::collections::{BTreeMap, BTreeSet};

/// A runner handle's place and the authority its callees run under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunnerGrant {
    pub entry: RunnerEntry,
    /// The entry's grant set, or a narrowing of it (RUN-2).
    pub grants: IoGrants,
    /// The definitions it runs, as full versioned paths; None runs any.
    pub definitions: Option<Vec<String>>,
}

#[derive(Default)]
pub(super) struct Runners {
    /// Sent calls not yet answered, by their wait token.
    pending: BTreeMap<u64, Pending>,
    starts: Vec<CalleeStart>,
    sends: Vec<RunnerSend>,
    cancels: Vec<u64>,
    /// Requests made so far (RUN-11's request ordinals).
    requests: u64,
    /// Each place's state in this run, by runner name.
    places: BTreeMap<String, Place>,
}
impl Runners {
    /// Whether `token` is a sent call's wait.
    pub(super) fn waits_on(&self, token: u64) -> bool {
        self.pending.contains_key(&token)
    }
}
/// A call as a host provider records it: its request, and either its wait (the call was
/// sent) or the completion it ended in at once.
pub struct RunnerSend {
    pub request: RunnerRequest,
    pub sent: Result<u64, RunnerCompletion>,
    /// The call's task is `runner.spawn`'s child, which no task polls.
    pub child: bool,
}
struct Pending {
    /// The waiting task: the caller's own, or `runner.spawn`'s child.
    task: u64,
    child: bool,
    schemas: CallSchemas,
    /// The program's distinct nominals, which decoding the outcome reads.
    nominals: crate::aggregate::RuntimeTypes,
    place: String,
    key: [u8; 32],
    /// The callee run's instance.
    callee: [u8; 16],
    /// `sim@1`'s `lose_reply`: the outcome is lost after the callee completes.
    lose_reply: bool,
    /// The cause of the cancellation forwarded to the callee, once forwarded.
    forwarded: Option<u32>,
    completion: Option<RunnerCompletion>,
}
#[derive(Default)]
struct Place {
    /// The fault sites enabled for this run, decided at the place's first call.
    enabled: Option<BTreeSet<String>>,
    /// Callees sent and not yet answered.
    running: u64,
    partitioned: bool,
}

/// How a call ended before or instead of its callee's outcome.
enum Refused {
    NotRun(u32),
    InDoubt(u32),
    Canceled(u32),
}
const DENIED: u32 = 0;
const UNREACHABLE: u32 = 2;
const OVERLOADED: u32 = 3;
const REJECTED: u32 = 4;
const LOST: u32 = 0;
const ABANDONED: u32 = 2;

fn refused(outcome: Refused) -> RunnerCompletion {
    RunnerCompletion {
        outcome: match outcome {
            Refused::NotRun(reason) => CallOutcome::NotRun(reason),
            Refused::InDoubt(reason) => CallOutcome::InDoubt(reason),
            Refused::Canceled(cause) => CallOutcome::Canceled(cause),
        },
        callee: None,
    }
}

/// The first eight bytes, little-endian, of a domain-separated hash of `parts`.
fn draw(domain: &str, parts: &[&[u8]]) -> u64 {
    let hash = hirsel_run_config::hash_parts(domain, parts.iter().copied());
    u64::from_le_bytes(hash[..8].try_into().unwrap())
}
/// Whether a draw in parts per million falls under `rate`.
fn fires(rate: u32, domain: &str, parts: &[&[u8]]) -> bool {
    draw(domain, parts) % 1_000_000 < u64::from(rate)
}

/// The codec schema of core `Failure`, which a `Faulted` outcome's bytes hold.
pub fn failure_schema(
    runtime: &crate::aggregate::RuntimeTypes,
) -> Result<SerializableSchema, Error> {
    let store = hirsel_types::TypeStore::from_graph(1, |_| {
        vec![hirsel_types::Type::with_kind(
            hirsel_types::TypeKind::Named {
                definition: hirsel_syntax::DefPath::core("Failure"),
                arguments: vec![].into(),
            },
        )]
    });
    let root = store.iter().next().map(|(id, _)| id).unwrap();
    hirsel_types::serialization::serializable_schema(
        &store,
        root,
        &hirsel_types::GenericDeclarations::default(),
        &mut |path| Ok(runtime.serialization.get(path).cloned()),
    )
    .map_err(Error::Query)?
    .ok_or_else(|| invalid("core Failure has no codec schema"))
}

/// A callee's own outcome as canonical bytes: its entry's `Ok` or `Err` payload, or its
/// primary failure with its causes. None when it ended otherwise, or when its result
/// exceeds the codec's budgets.
pub fn callee_outcome(
    end: &CalleeEnd,
    schemas: &CallSchemas,
    context: &OperationContext<'_>,
) -> Result<Option<CalleeOutcome>, Error> {
    let bytes = |schema: &SerializableSchema, value: &Value| {
        super::codec::encode(schema, value).map(Result::ok)
    };
    Ok(match end {
        CalleeEnd::Returned(Value::Variant {
            variant: tag,
            fields,
        }) => {
            let [Some(value)] = &fields[..] else {
                return Err(invalid("a callee's Result without its payload"));
            };
            match tag {
                0 => bytes(&schemas.result, value)?.map(CalleeOutcome::Ok),
                _ => bytes(&schemas.error, value)?.map(CalleeOutcome::Err),
            }
        }
        CalleeEnd::Returned(_) => return Err(invalid("a callee entry returns a Result")),
        CalleeEnd::Failed(chain) => {
            bytes(&schemas.failure, &faulted(chain, context)?)?.map(CalleeOutcome::Failed)
        }
        CalleeEnd::BudgetExceeded(_) | CalleeEnd::Canceled | CalleeEnd::Abandoned => None,
    })
}

impl Library {
    fn runner(&self, value: &Value) -> Result<&RunnerGrant, Error> {
        let id = self.simulation.id(value, "Runner")?;
        match self.simulation.grants.get(&id) {
            Some(Grant::Runner(runner)) => Ok(runner),
            _ => Err(invalid("invalid Runner grant")),
        }
    }
    fn runner_handle(&mut self, runner: RunnerGrant) -> Value {
        self.simulation
            .handle("Runner", Grant::Runner(Box::new(runner)))
    }

    pub(super) fn runner_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.tasks.yield_now = true;
        let a = &*call.arguments;
        let value = match name {
            "io.runner" => {
                self.simulation.id(arg(a, 0)?, "Io")?;
                let name = text(arg(a, 1)?)?;
                match self
                    .simulation
                    .config
                    .runners
                    .iter()
                    .find(|r| r.name == name)
                    .cloned()
                {
                    Some(entry) => {
                        let runner = RunnerGrant {
                            grants: entry.grants.clone().unwrap_or_default(),
                            definitions: entry.definitions.clone(),
                            entry,
                        };
                        ok(self.runner_handle(runner))
                    }
                    None => error(variant(0, vec![])),
                }
            }
            "test.runner" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                let entry = RunnerEntry {
                    name: "test".into(),
                    provider: "sim@1".into(),
                    threads: None,
                    queue: None,
                    node: Some("test".into()),
                    models: Some("local.threads@1".into()),
                    faults: BTreeMap::new(),
                    recording: None,
                    retry: None,
                    cache: None,
                    code: None,
                    admission: None,
                    grants: None,
                    definitions: None,
                };
                self.runner_handle(RunnerGrant {
                    entry,
                    grants: IoGrants::default(),
                    definitions: None,
                })
            }
            "runner.grants" => grant_set(&self.runner(arg(a, 0)?)?.grants)?,
            "runner.narrow" => {
                let runner = self.runner(arg(a, 0)?)?.clone();
                match narrowed(&runner.grants, arg(a, 1)?)? {
                    Some(grants) => ok(self.runner_handle(RunnerGrant { grants, ..runner })),
                    None => error(variant(0, vec![])),
                }
            }
            "runner.only" => {
                let runner = self.runner(arg(a, 0)?)?.clone();
                let listed = collection::elements(arg(a, 1)?, BuiltinType::List)?
                    .iter()
                    .map(|v| text(v).map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()?;
                let sorted = listed.windows(2).all(|pair| pair[0] < pair[1]);
                let within = runner
                    .definitions
                    .as_ref()
                    .is_none_or(|allowed| listed.iter().all(|path| allowed.contains(path)));
                if sorted && within {
                    ok(self.runner_handle(RunnerGrant {
                        definitions: Some(listed),
                        ..runner
                    }))
                } else {
                    error(variant(0, vec![]))
                }
            }
            "runner.compute" => return self.send(CallForm::Compute, None, a, call, context),
            "runner.call" => return self.send(CallForm::Call, None, a, call, context),
            "runner.spawn" => {
                let scope = super::tasks::id(arg(a, 0)?, "Scope")?;
                return self.send(CallForm::Call, Some(scope), &a[1..], call, context);
            }
            _ => return Err(invalid("unknown runner operation")),
        };
        Ok(OperationResult::returned(value))
    }

    /// RUN-4: the checks in order, then the place's contract, then the send. `scope` is
    /// `runner.spawn`'s: its child task performs the call and holds the outcome.
    fn send(
        &mut self,
        form: CallForm,
        scope: Option<u64>,
        a: &[Value],
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let runner = self.runner(arg(a, 0)?)?.clone();
        let Value::FunctionReference(entry) = arg(a, 1)? else {
            return Err(invalid("a runner call names an Entry"));
        };
        let path = entry.definition.to_string();
        let [
            GenericArgument::Type(argument),
            GenericArgument::Type(result),
            GenericArgument::Type(failure),
        ] = *call.generics
        else {
            return Err(invalid("runner call type actuals"));
        };
        let argument_schema = super::codec::schema(call, argument)?;
        let schemas = CallSchemas {
            result: super::codec::schema(call, result)?,
            error: super::codec::schema(call, failure)?,
            failure: failure_schema(call.runtime)?,
        };
        let task = match scope {
            Some(scope) => self.tasks.native_child(scope)?,
            None => self.tasks.current,
        };
        let ordinal = self.runners.requests;
        self.runners.requests += 1;
        let instance = self.simulation.config.instance;
        let key = hirsel_run_config::call_key(&instance, ordinal);
        let facts = self.simulation.config.callees.get(&path).cloned();
        // 1. The argument encodes within the request budget.
        let bytes = super::codec::encode(&argument_schema, arg(a, 2)?)?;
        let grants = hirsel_run_config::request_grants(&runner.grants);
        let definitions = hirsel_run_config::request_definitions(runner.definitions.as_deref());
        let (fingerprint, code) = facts
            .as_ref()
            .map_or(([0; 32], [0; 32]), |f| (f.fingerprint, f.code));
        let request = |argument: &[u8]| RunnerRequest {
            runner: runner.entry.name.clone(),
            form,
            path: path.clone(),
            digest: hirsel_run_config::request_digest(
                form,
                &path,
                &fingerprint,
                &code,
                &grants,
                &definitions,
                argument,
            ),
            grants: grants.clone(),
            definitions: definitions.clone(),
            key,
            argument: argument.to_vec(),
        };
        let (request, outcome) = match bytes {
            Err(_) => (request(&[]), Err(Refused::NotRun(REJECTED))),
            Ok(bytes) => {
                let request = request(&bytes);
                // 2. Authority at the caller: the allowlist, then the place's code policy.
                let listed = runner
                    .definitions
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&path));
                let admitted = !runner.entry.admits_only_safe()
                    || facts.is_some_and(|f| f.reach == CodeReach::Safe);
                let outcome = if !listed || !admitted {
                    Err(Refused::NotRun(DENIED))
                } else if let Some(cause) = self.tasks.canceled_task(task) {
                    // 3. Cancellation: nothing is sent.
                    Err(Refused::Canceled(cause))
                } else {
                    let cause = Cause {
                        instance,
                        ordinal,
                        digest: request.digest,
                    };
                    self.place(
                        &runner,
                        form,
                        &entry.definition,
                        cause,
                        &bytes,
                        argument_schema,
                        &schemas,
                        call,
                    )?
                };
                (request, outcome)
            }
        };
        match outcome {
            Err(refusal) => {
                let completion = refused(refusal);
                self.runners.sends.push(RunnerSend {
                    request,
                    sent: Err(completion.clone()),
                    child: scope.is_some(),
                });
                let value = refused_value(&completion);
                if scope.is_some() {
                    self.task_finished(task, OperationResult::returned(value), context)?;
                    Ok(OperationResult::returned(task_handle(task)))
                } else {
                    Ok(OperationResult::returned(value))
                }
            }
            Ok((start, lose_reply)) => {
                let token = self.next_callback;
                self.next_callback += 1;
                self.runners.pending.insert(
                    token,
                    Pending {
                        task,
                        child: scope.is_some(),
                        schemas,
                        nominals: crate::aggregate::RuntimeTypes {
                            nominals: call
                                .runtime
                                .nominals
                                .iter()
                                .filter(|n| n.distinct)
                                .cloned()
                                .collect(),
                            ..Default::default()
                        },
                        place: runner.entry.name.clone(),
                        key,
                        callee: hirsel_run_config::callee_instance(&key),
                        lose_reply,
                        forwarded: None,
                        completion: None,
                    },
                );
                self.runners.sends.push(RunnerSend {
                    request,
                    sent: Ok(token),
                    child: scope.is_some(),
                });
                self.runners.starts.push(CalleeStart { token, ..start });
                if scope.is_some() {
                    return Ok(OperationResult::returned(task_handle(task)));
                }
                Ok(OperationResult::Park {
                    token,
                    wait: BlockedWait {
                        spawn_path: self.tasks.path(),
                        anchor: context
                            .anchor()
                            .cloned()
                            .ok_or_else(|| invalid("runner call without an anchor"))?,
                        kind: "runner.call".into(),
                        targets: vec![],
                    },
                })
            }
        }
    }

    /// The place's contract: capacity, `sim@1`'s faults and re-sends, then the callee's
    /// world. A sent call returns its callee and whether its reply is to be lost.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        runner: &RunnerGrant,
        form: CallForm,
        path: &hirsel_syntax::DefPath,
        cause: Cause,
        bytes: &[u8],
        argument: SerializableSchema,
        schemas: &CallSchemas,
        call: &OperationCall<'_>,
    ) -> Result<Result<(CalleeStart, bool), Refused>, Error> {
        let entry = &runner.entry;
        let seed = self.simulation.config.fault_seed.to_le_bytes();
        let ordinal = cause.ordinal;
        let place = self.runners.places.entry(entry.name.clone()).or_default();
        let enabled = place.enabled.get_or_insert_with(|| {
            entry
                .faults
                .iter()
                .filter(|(site, rate)| {
                    fires(
                        rate.enable,
                        "hirsel.runner.fault.enable.v1",
                        &[&seed, entry.name.as_bytes(), site.as_bytes()],
                    )
                })
                .map(|(site, _)| site.clone())
                .collect()
        });
        let fire = |site: &str, attempt: u32| {
            enabled.contains(site)
                && fires(
                    entry.faults[site].fire,
                    "hirsel.runner.fault.fire.v1",
                    &[
                        &seed,
                        entry.name.as_bytes(),
                        site.as_bytes(),
                        &ordinal.to_le_bytes(),
                        &attempt.to_le_bytes(),
                    ],
                )
        };
        let capacity = entry.threads() + entry.queue();
        // A request refused before its commit point may be re-sent (RUN-7).
        let mut attempt = 0;
        let refusal = loop {
            let refusal = if place.partitioned && !fire("partition", attempt) {
                Some(UNREACHABLE)
            } else if fire("partition", attempt) && !place.partitioned {
                place.partitioned = true;
                Some(UNREACHABLE)
            } else if fire("refuse", attempt) {
                Some(UNREACHABLE)
            } else if place.running >= capacity || fire("overload", attempt) {
                Some(OVERLOADED)
            } else {
                place.partitioned = false;
                None
            };
            match refusal {
                Some(_) if attempt < entry.attempts() => attempt += 1,
                refusal => break refusal,
            }
        };
        if let Some(reason) = refusal {
            return Ok(Err(Refused::NotRun(reason)));
        }
        // The commit point: the request is sent.
        if fire("lose_request", attempt) {
            return Ok(Err(Refused::InDoubt(LOST)));
        }
        place.running += 1;
        let crash_after = fire("crash", attempt).then(|| {
            draw(
                "hirsel.runner.fault.crash.v1",
                &[&seed, entry.name.as_bytes(), &ordinal.to_le_bytes()],
            ) % 4
        });
        let lose_reply = fire("lose_reply", attempt);
        let key = hirsel_run_config::call_key(&cause.instance, ordinal);
        let seed = |name: &str| hirsel_run_config::callee_seed(&key, name);
        let configuration = hirsel_tir::RunConfiguration {
            entry: path.clone(),
            hash_seed: seed("hash"),
            inputs: vec![],
        };
        let world = callee_world(
            &runner.grants,
            seed("random"),
            seed("test"),
            hirsel_run_config::callee_instance(&key),
            self.simulation.config.fault_seed,
            self.simulation.config.callees.clone(),
        );
        let mut callee = Library::new(&configuration, world)?;
        let mut arguments = vec![];
        if form == CallForm::Call {
            arguments.push(callee.simulation.handle("Io", Grant::Root("Io".into())));
        }
        // The callee receives fresh storage decoded from the request's bytes.
        arguments.push(super::codec::decode(&argument, bytes, call.runtime)?);
        Ok(Ok((
            CalleeStart {
                token: 0,
                configuration,
                arguments,
                operations: Box::new(callee),
                budgets: RunBudgets::default(),
                crash_after: crash_after.map(|k| k as u32),
                call: CallStart {
                    runner: entry.clone(),
                    grants: runner.grants.clone(),
                    form,
                    cause,
                    argument: bytes.to_vec(),
                    schemas: schemas.clone(),
                },
            },
            lose_reply,
        )))
    }

    /// A sent call's completion once its callee ended, and the place freed: the callee's
    /// own outcome bytes, unless the place lost its reply.
    fn runner_end(
        &mut self,
        token: u64,
        end: CalleeEnd,
        context: &mut OperationContext<'_>,
    ) -> Result<RunnerCompletion, Error> {
        let pending = self
            .runners
            .pending
            .get(&token)
            .ok_or_else(|| invalid("an outcome for an unknown runner call"))?;
        let callee = Some(pending.callee);
        if pending.lose_reply && !matches!(end, CalleeEnd::Canceled) {
            return Ok(RunnerCompletion {
                outcome: CallOutcome::InDoubt(LOST),
                callee,
            });
        }
        let outcome = match (&end, callee_outcome(&end, &pending.schemas, context)?) {
            (_, Some(CalleeOutcome::Ok(bytes))) => CallOutcome::Ok(bytes),
            (_, Some(CalleeOutcome::Err(bytes))) => CallOutcome::Failed(bytes),
            (_, Some(CalleeOutcome::Failed(bytes))) => CallOutcome::Faulted(bytes),
            // A result beyond the codec's budgets cannot cross.
            (CalleeEnd::Returned(_) | CalleeEnd::Failed(_), None) => CallOutcome::NotRun(REJECTED),
            (CalleeEnd::BudgetExceeded(_), None) => {
                return Err(Error::Unavailable(
                    "per-call budgets are not offered by any runner provider".into(),
                ));
            }
            (CalleeEnd::Canceled, None) => {
                return Ok(refused(Refused::Canceled(pending.forwarded.unwrap_or(0))));
            }
            (CalleeEnd::Abandoned, None) => CallOutcome::InDoubt(ABANDONED),
        };
        Ok(RunnerCompletion { outcome, callee })
    }

    /// The value a call's completion gives its caller: fresh storage decoded from the
    /// completion's bytes.
    fn completion_value(pending: &Pending, completion: &RunnerCompletion) -> Result<Value, Error> {
        let decode = |schema, bytes: &[u8]| super::codec::decode(schema, bytes, &pending.nominals);
        Ok(match &completion.outcome {
            CallOutcome::Ok(bytes) => ok(decode(&pending.schemas.result, bytes)?),
            CallOutcome::Failed(bytes) => {
                error(variant(2, vec![decode(&pending.schemas.error, bytes)?]))
            }
            CallOutcome::Faulted(bytes) => {
                error(variant(3, vec![decode(&pending.schemas.failure, bytes)?]))
            }
            _ => refused_value(completion),
        })
    }

    pub(super) fn poll_runner(
        &mut self,
        token: u64,
        _context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let pending = self
            .runners
            .pending
            .get(&token)
            .ok_or_else(|| invalid("unknown runner wait"))?;
        let Some(completion) = &pending.completion else {
            return Ok(None);
        };
        let value = Self::completion_value(pending, completion)?;
        self.runners.pending.remove(&token);
        Ok(Some(OperationResult::returned(value)))
    }

    /// RUN-9: each sent call whose task was cancelled has its cancellation forwarded once.
    pub(super) fn take_runner_cancels(&mut self) -> Vec<u64> {
        let mut cancels = std::mem::take(&mut self.runners.cancels);
        for (token, pending) in &mut self.runners.pending {
            if pending.forwarded.is_none()
                && pending.completion.is_none()
                && let Some(cause) = self.tasks.canceled_task(pending.task)
            {
                pending.forwarded = Some(cause);
                cancels.push(*token);
            }
        }
        cancels
    }

    /// The execution reports the callee's end: the call completes at once.
    pub(super) fn runner_ended(
        &mut self,
        token: u64,
        end: CalleeEnd,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let completion = self.runner_end(token, end, context)?;
        self.runner_completed(token, completion, context)
    }

    pub(super) fn take_runner_starts(&mut self) -> Vec<CalleeStart> {
        std::mem::take(&mut self.runners.starts)
    }

    /// The calls made since the last take, for a host provider to record.
    pub fn take_runner_sends(&mut self) -> Vec<RunnerSend> {
        std::mem::take(&mut self.runners.sends)
    }
    /// A host provider learns the callee of the call waiting at `token` ended: the
    /// completion it records and later delivers ([`Self::runner_completed`]).
    pub fn runner_completion(
        &mut self,
        token: u64,
        end: CalleeEnd,
        context: &mut OperationContext<'_>,
    ) -> Result<RunnerCompletion, Error> {
        self.runner_end(token, end, context)
    }
    /// The call waiting at `token` completed: its place is free, and its task (or its
    /// spawned child) receives the outcome.
    pub fn runner_completed(
        &mut self,
        token: u64,
        completion: RunnerCompletion,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let pending = self
            .runners
            .pending
            .get_mut(&token)
            .ok_or_else(|| invalid("an outcome for an unknown runner call"))?;
        if pending.completion.is_some() {
            return Err(invalid("a runner call completed twice"));
        }
        if let Some(place) = self.runners.places.get_mut(&pending.place) {
            place.running = place.running.saturating_sub(1);
        }
        pending.completion = Some(completion);
        if pending.child {
            let pending = self.runners.pending.remove(&token).unwrap();
            let value = Self::completion_value(&pending, pending.completion.as_ref().unwrap())?;
            self.task_finished(pending.task, OperationResult::returned(value), context)?;
        }
        Ok(())
    }
    /// Whether `token` is a sent call's wait not yet answered.
    pub fn runner_waits(&self, token: u64) -> bool {
        self.runners.pending.contains_key(&token)
    }
    /// The call key and forwarded cancellation cause of the call waiting at `token`.
    pub fn runner_cancel(&self, token: u64) -> Option<([u8; 32], u32)> {
        let pending = self.runners.pending.get(&token)?;
        Some((pending.key, pending.forwarded?))
    }
}

/// A completion that carries no callee bytes, as the caller's `RunError`.
fn refused_value(completion: &RunnerCompletion) -> Value {
    error(match completion.outcome {
        CallOutcome::NotRun(reason) => variant(0, vec![variant(reason, vec![])]),
        CallOutcome::InDoubt(reason) => variant(1, vec![variant(reason, vec![])]),
        CallOutcome::Canceled(cause) => variant(4, vec![variant(cause, vec![])]),
        CallOutcome::Ok(_) | CallOutcome::Failed(_) | CallOutcome::Faulted(_) => {
            unreachable!("a completion with callee bytes decodes")
        }
    })
}

fn task_handle(task: u64) -> Value {
    Value::Handle {
        kind: "Task".into(),
        id: task,
    }
}

/// The callee's world: the runner's grant set as its root grants, its seeds derived from
/// the call key (RUN-10), and none of the caller's state.
fn callee_world(
    grants: &IoGrants,
    random_seed: u64,
    test_seed: u64,
    instance: [u8; 16],
    fault_seed: u64,
    callees: std::sync::Arc<BTreeMap<String, hirsel_run_config::CalleeFacts>>,
) -> SimulationConfig {
    let mut world = SimulationConfig {
        random_seed,
        test_seed,
        instance,
        fault_seed,
        callees,
        net: grants.net.clone(),
        process: grants.process.clone(),
        ..Default::default()
    };
    world.grants = std::iter::once("Io")
        .chain(grants.monotonic_clock.as_ref().map(|_| "MonotonicClock"))
        .chain(grants.wall_clock.as_ref().map(|_| "WallClock"))
        .map(str::to_owned)
        .collect();
    world.projections = [
        ("stderr", grants.stderr.is_some()),
        ("stdout", grants.stdout.is_some()),
        ("tasks", grants.tasks.is_some()),
        ("net", grants.net.is_some()),
        ("process", grants.process.is_some()),
        ("random", grants.random.is_some()),
        ("dir", !grants.dirs.is_empty()),
    ]
    .into_iter()
    .filter(|(_, granted)| *granted)
    .map(|(name, _)| name.to_owned())
    .collect();
    world.mounts = grants
        .dirs
        .iter()
        .map(|mount| (mount.path.clone(), mount.access))
        .collect();
    world.directories = std::iter::once(String::new())
        .chain(grants.dirs.iter().map(|mount| mount.path.clone()))
        .collect();
    world
}

/// `Faulted(f)`'s failure: the callee's primary failure with its causes (RUN-6).
fn faulted(chain: &[FailureRecord], context: &OperationContext<'_>) -> Result<Value, Error> {
    super::tasks::failure_value(chain, context)
}

fn boolean(value: bool) -> Value {
    Value::Bool(value)
}
fn record(name: &str, fields: Vec<Value>) -> Value {
    Value::Record {
        definition: (&hirsel_syntax::DefPath::core(name)).into(),
        fields: fields.into_iter().map(Some).collect(),
    }
}
fn access_value(access: Access) -> Value {
    variant(u32::from(access == Access::ReadWrite), vec![])
}

/// RUN-2's `GrantSet` of a runner's grant set.
fn grant_set(grants: &IoGrants) -> Result<Value, Error> {
    let list = |values: Vec<Value>| collection::sequence(BuiltinType::List, values);
    Ok(record(
        "GrantSet",
        vec![
            boolean(grants.stdout.is_some()),
            boolean(grants.stderr.is_some()),
            boolean(grants.tasks.is_some()),
            boolean(grants.monotonic_clock.is_some()),
            boolean(grants.wall_clock.is_some()),
            boolean(grants.random.is_some()),
            list(
                grants
                    .dirs
                    .iter()
                    .map(|mount| {
                        record(
                            "DirGrant",
                            vec![
                                Value::String(mount.path.clone()),
                                access_value(mount.access),
                            ],
                        )
                    })
                    .collect(),
            ),
            list(
                grants
                    .net
                    .iter()
                    .flat_map(|net| &net.rules)
                    .map(super::net::rule_value)
                    .collect::<Result<_, _>>()?,
            ),
            list(
                grants
                    .process
                    .iter()
                    .flat_map(|process| &process.rules)
                    .map(|rule| {
                        record(
                            "ProcessRule",
                            vec![
                                Value::String(rule.program.clone()),
                                list(rule.env.iter().cloned().map(Value::String).collect()),
                            ],
                        )
                    })
                    .collect(),
            ),
        ],
    ))
}

/// The grant set `keep` asks for, when `grants` covers it (RUN-2); None otherwise.
fn narrowed(grants: &IoGrants, keep: &Value) -> Result<Option<IoGrants>, Error> {
    let Value::Record { definition, fields } = keep else {
        return Err(invalid("GrantSet representation"));
    };
    if !definition.is_core("GrantSet") || fields.len() != 9 {
        return Err(invalid("GrantSet representation"));
    }
    let field = |i: usize| {
        fields[i]
            .as_ref()
            .ok_or_else(|| invalid("dead GrantSet field"))
    };
    let flag = |i: usize| match field(i)? {
        Value::Bool(b) => Ok(*b),
        _ => Err(invalid("GrantSet flag")),
    };
    let kept = |granted: &Option<String>, i: usize| -> Result<Result<Option<String>, ()>, Error> {
        Ok(match (flag(i)?, granted) {
            (false, _) => Ok(None),
            (true, Some(provider)) => Ok(Some(provider.clone())),
            (true, None) => Err(()),
        })
    };
    let mut narrow = IoGrants::default();
    for (i, (granted, slot)) in [
        (&grants.stdout, &mut narrow.stdout),
        (&grants.stderr, &mut narrow.stderr),
        (&grants.tasks, &mut narrow.tasks),
        (&grants.monotonic_clock, &mut narrow.monotonic_clock),
        (&grants.wall_clock, &mut narrow.wall_clock),
        (&grants.random, &mut narrow.random),
    ]
    .into_iter()
    .enumerate()
    {
        match kept(granted, i)? {
            Ok(provider) => *slot = provider,
            Err(()) => return Ok(None),
        }
    }
    let mut paths = vec![];
    for dir in collection::elements(field(6)?, BuiltinType::List)? {
        let Value::Record { fields, .. } = &dir else {
            return Err(invalid("DirGrant representation"));
        };
        let (
            Some(Some(path)),
            Some(Some(Value::Variant {
                variant: access, ..
            })),
        ) = (fields.first(), fields.get(1))
        else {
            return Err(invalid("DirGrant representation"));
        };
        let path = text(path)?.to_owned();
        let access = if *access == 1 {
            Access::ReadWrite
        } else {
            Access::Read
        };
        // v1 narrows a mount's access, never to a subdirectory.
        let Some(mount) = grants.dirs.iter().find(|m| m.path == path) else {
            return Ok(None);
        };
        if !access.within(mount.access) {
            return Ok(None);
        }
        paths.push(path.clone());
        narrow.dirs.push(Mount {
            path,
            host: mount.host.clone(),
            access,
        });
    }
    if paths.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Ok(None);
    }
    let rules: Vec<NetRule> = super::net::rules(field(7)?)?;
    if !rules.is_empty() {
        let Some(net) = &grants.net else {
            return Ok(None);
        };
        if !rules.iter().all(|rule| {
            net.rules
                .iter()
                .any(|wide| hirsel_run_config::net_rule_within(rule, wide))
        }) {
            return Ok(None);
        }
        let mut net = net.clone();
        net.rules = rules;
        narrow.net = Some(net);
    }
    let rules: Vec<ProcessRule> = super::process::process_rules(field(8)?)?;
    if !rules.is_empty() {
        let Some(process) = &grants.process else {
            return Ok(None);
        };
        if !rules.iter().all(|rule| {
            process
                .rules
                .iter()
                .any(|wide| hirsel_run_config::process_rule_within(rule, wide))
        }) {
            return Ok(None);
        }
        let mut process = process.clone();
        process.rules = rules;
        narrow.process = Some(process);
    }
    Ok(Some(narrow))
}

/// The value canonical `bytes` of `schema` hold, as fresh storage: a callee run's
/// argument, decoded where its run starts.
pub fn decode(
    schema: &SerializableSchema,
    bytes: &[u8],
    runtime: &crate::aggregate::RuntimeTypes,
) -> Result<Value, Error> {
    super::codec::decode(schema, bytes, runtime)
}
impl Library {
    /// The run's root `Io`, for a callee entry of the call form.
    pub fn root_io(&mut self) -> Value {
        self.simulation.handle("Io", Grant::Root("Io".into()))
    }
}
