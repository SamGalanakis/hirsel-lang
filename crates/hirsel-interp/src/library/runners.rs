//! Runners (docs/spec/runners.md). `io.runner` projects a runner the run's configuration
//! names; narrowing mints one over a subset of its grant set or allowlist. A call
//! encodes its argument, checks authority and cancellation, then meets its place's
//! contract: `local.threads@1` admits it up to its threads and queue, and a `sim@1`
//! node draws its configured faults from the fault seed. A sent call's callee runs as
//! its own run in this execution, in a world projected from the runner's grants
//! (`take_calls`); the calling task waits until the execution reports the callee's end
//! (`call_ended`). Arguments and results cross through the canonical codec.
use super::dispatch::{arg, text, variant};
use super::simulation::{Grant, SimulationConfig};
use super::{Library, collection, error, ok};
use crate::observation::FailureRecord;
use crate::{
    BlockedWait, CalleeEnd, CalleeStart, Error, OperationCall, OperationContext, OperationResult,
    Operations, RunBudgets, Value, invalid,
};
use hirsel_run_config::{Access, IoGrants, Mount, NetRule, ProcessRule, RunnerEntry};
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
    cancels: Vec<u64>,
    /// Requests sent so far (RUN-11's request ordinals).
    sent: u64,
    /// Each place's state in this run, by runner name.
    places: BTreeMap<String, Place>,
}
impl Runners {
    /// Whether `token` is a sent call's wait.
    pub(super) fn waits_on(&self, token: u64) -> bool {
        self.pending.contains_key(&token)
    }
}
struct Pending {
    /// The waiting task: the caller's own, or `runner.spawn`'s child.
    task: u64,
    child: bool,
    result: SerializableSchema,
    error: SerializableSchema,
    /// The program's distinct nominals, which decoding the outcome reads.
    nominals: crate::aggregate::RuntimeTypes,
    place: String,
    /// `sim@1`'s `lose_reply`: the outcome is lost after the callee completes.
    lose_reply: bool,
    /// The cause of the cancellation forwarded to the callee, once forwarded.
    forwarded: Option<u32>,
    end: Option<CalleeEnd>,
}
#[derive(Default)]
struct Place {
    /// The fault sites enabled for this run, decided at the place's first call.
    enabled: Option<BTreeSet<String>>,
    /// Callees sent and not yet ended.
    running: u64,
    partitioned: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Form {
    Compute,
    Call,
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

fn refused(outcome: Refused) -> Value {
    error(match outcome {
        Refused::NotRun(reason) => variant(0, vec![variant(reason, vec![])]),
        Refused::InDoubt(reason) => variant(1, vec![variant(reason, vec![])]),
        Refused::Canceled(cause) => variant(4, vec![variant(cause, vec![])]),
    })
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
            "runner.compute" => return self.send(Form::Compute, None, a, call, context),
            "runner.call" => return self.send(Form::Call, None, a, call, context),
            "runner.spawn" => {
                let scope = super::tasks::id(arg(a, 0)?, "Scope")?;
                return self.send(Form::Call, Some(scope), &a[1..], call, context);
            }
            _ => return Err(invalid("unknown runner operation")),
        };
        Ok(OperationResult::returned(value))
    }

    /// RUN-4: the checks in order, then the place's contract, then the send. `scope` is
    /// `runner.spawn`'s: its child task performs the call and holds the outcome.
    fn send(
        &mut self,
        form: Form,
        scope: Option<u64>,
        a: &[Value],
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let runner = self.runner(arg(a, 0)?)?.clone();
        let Value::FunctionReference(entry) = arg(a, 1)? else {
            return Err(invalid("a runner call names an Entry"));
        };
        let path = entry.definition.clone();
        let [
            GenericArgument::Type(argument),
            GenericArgument::Type(result),
            GenericArgument::Type(failure),
        ] = *call.generics
        else {
            return Err(invalid("runner call type actuals"));
        };
        let argument_schema = super::codec::schema(call, argument)?;
        let result = super::codec::schema(call, result)?;
        let error = super::codec::schema(call, failure)?;
        // 1. The argument encodes within the request budget.
        let bytes = super::codec::encode(&argument_schema, arg(a, 2)?)?;
        let task = match scope {
            Some(scope) => self.tasks.native_child(scope)?,
            None => self.tasks.current,
        };
        let outcome = match bytes {
            Err(_) => Err(Refused::NotRun(REJECTED)),
            Ok(bytes) => {
                // 2. Authority at the caller: the allowlist.
                let listed = runner
                    .definitions
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&path.to_string()));
                if !listed {
                    Err(Refused::NotRun(DENIED))
                } else if let Some(cause) = self.tasks.canceled_task(task) {
                    // 3. Cancellation: nothing is sent.
                    Err(Refused::Canceled(cause))
                } else {
                    self.place(&runner, form, &path, &bytes, argument_schema, call)?
                }
            }
        };
        match outcome {
            Err(refusal) => {
                let value = refused(refusal);
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
                        result,
                        error,
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
                        lose_reply,
                        forwarded: None,
                        end: None,
                    },
                );
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
    fn place(
        &mut self,
        runner: &RunnerGrant,
        form: Form,
        path: &hirsel_syntax::DefPath,
        bytes: &[u8],
        argument: SerializableSchema,
        call: &OperationCall<'_>,
    ) -> Result<Result<(CalleeStart, bool), Refused>, Error> {
        let entry = &runner.entry;
        let seed = self.simulation.config.fault_seed.to_le_bytes();
        let ordinal = self.runners.sent;
        self.runners.sent += 1;
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
        let key = hirsel_run_config::hash_parts(
            "hirsel.runner.key.v1",
            [&self.simulation.config.instance[..], &ordinal.to_le_bytes()],
        );
        let seed = |name: &str| draw("hirsel.runner.seed.v1", &[&key, name.as_bytes()]);
        let mut instance = [0; 16];
        instance.copy_from_slice(
            &hirsel_run_config::hash_parts("hirsel.runner.instance.v1", [&key[..]])[..16],
        );
        let configuration = hirsel_tir::RunConfiguration {
            entry: path.clone(),
            hash_seed: seed("hash"),
            inputs: vec![],
        };
        let world = callee_world(
            &runner.grants,
            seed("random"),
            seed("test"),
            instance,
            self.simulation.config.fault_seed,
        );
        let mut callee = Library::new(&configuration, world)?;
        let mut arguments = vec![];
        if form == Form::Call {
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
            },
            lose_reply,
        )))
    }

    /// A sent call's outcome, once its callee ended: the result value the caller sees.
    fn runner_outcome(
        &mut self,
        pending: &mut Pending,
        context: &mut OperationContext<'_>,
    ) -> Result<Value, Error> {
        let end = pending
            .end
            .take()
            .ok_or_else(|| invalid("a runner call ended twice"))?;
        if pending.lose_reply && !matches!(end, CalleeEnd::Canceled) {
            return Ok(refused(Refused::InDoubt(LOST)));
        }
        Ok(match end {
            CalleeEnd::Returned(Value::Variant {
                variant: tag,
                fields,
            }) => {
                let [Some(value)] = &fields[..] else {
                    return Err(invalid("a callee's Result without its payload"));
                };
                let schema = if tag == 0 {
                    &pending.result
                } else {
                    &pending.error
                };
                // The caller receives fresh storage decoded from the outcome's bytes.
                let bytes = match super::codec::encode(schema, value)? {
                    Ok(bytes) => bytes,
                    Err(_) => return Ok(refused(Refused::NotRun(REJECTED))),
                };
                let fresh = super::codec::decode(schema, &bytes, &pending.nominals)?;
                if tag == 0 {
                    ok(fresh)
                } else {
                    error(variant(2, vec![fresh]))
                }
            }
            CalleeEnd::Returned(_) => return Err(invalid("a callee entry returns a Result")),
            CalleeEnd::Failed(chain) => error(variant(3, vec![faulted(&chain, context)?])),
            CalleeEnd::BudgetExceeded(_) => {
                return Err(Error::Unavailable(
                    "per-call budgets are not offered by any runner provider".into(),
                ));
            }
            CalleeEnd::Canceled => refused(Refused::Canceled(pending.forwarded.unwrap_or(0))),
            CalleeEnd::Abandoned => refused(Refused::InDoubt(ABANDONED)),
        })
    }

    pub(super) fn poll_runner(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let pending = self
            .runners
            .pending
            .get(&token)
            .ok_or_else(|| invalid("unknown runner wait"))?;
        if pending.end.is_none() {
            return Ok(None);
        }
        let mut pending = self.runners.pending.remove(&token).unwrap();
        let value = self.runner_outcome(&mut pending, context)?;
        Ok(Some(OperationResult::returned(value)))
    }

    /// RUN-9: each sent call whose task was cancelled has its cancellation forwarded once.
    pub(super) fn take_runner_cancels(&mut self) -> Vec<u64> {
        let mut cancels = std::mem::take(&mut self.runners.cancels);
        for (token, pending) in &mut self.runners.pending {
            if pending.forwarded.is_none()
                && pending.end.is_none()
                && let Some(cause) = self.tasks.canceled_task(pending.task)
            {
                pending.forwarded = Some(cause);
                cancels.push(*token);
            }
        }
        cancels
    }

    pub(super) fn runner_ended(
        &mut self,
        token: u64,
        end: CalleeEnd,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let pending = self
            .runners
            .pending
            .get_mut(&token)
            .ok_or_else(|| invalid("an outcome for an unknown runner call"))?;
        if let Some(place) = self.runners.places.get_mut(&pending.place) {
            place.running = place.running.saturating_sub(1);
        }
        pending.end = Some(end);
        if pending.child {
            let mut pending = self.runners.pending.remove(&token).unwrap();
            let value = self.runner_outcome(&mut pending, context)?;
            self.task_finished(pending.task, OperationResult::returned(value), context)?;
        }
        Ok(())
    }

    pub(super) fn take_runner_starts(&mut self) -> Vec<CalleeStart> {
        std::mem::take(&mut self.runners.starts)
    }
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
) -> SimulationConfig {
    let mut world = SimulationConfig {
        random_seed,
        test_seed,
        instance,
        fault_seed,
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
