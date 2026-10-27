//! Owned native retirement evidence for the canonical event writer.
//! This log is optional and internal: native carrier bytes below still need
//! descriptor-aware value normalization before exporting a canonical trace.
//! It never samples a parked context or records an early commit.
use super::super::Logical;
use super::{Command, Outcome};
use crate::TaskCtx;
use crate::budget::stretch::Stretch;
use crate::ordered::footprint::{Item, ObjectId, TaskId};
use crate::ordered::{Next, Operation, Retirement};
use std::collections::{BTreeMap, BTreeSet};

pub(super) mod host_keys;
pub(super) mod host_output;
pub(crate) mod host_phases;
mod host_requests;
mod host_terminal;
mod terminal_waits;
pub(super) use host_requests::HostAuthority;
mod host_values;
mod jsonl;
pub(super) use host_values::HostHandleId;
pub(crate) use jsonl::Trace;

mod blocked;
pub(super) mod charges;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Transition {
    Keep,
    Yield,
    Park,
    Call {
        members: Vec<Vec<u32>>,
        then_yield: bool,
    },
    Hold,
    End,
    Stop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Boundary {
    pub task: Vec<u32>,
    pub ordinal: u64,
    /// None denotes a continuation of the preceding op, not a second op.
    pub op: Option<&'static str>,
    pub transition: Transition,
    pub retirement: u64,
    pub before: Stretch,
    pub credit: u64,
    /// Native evidence only; raw handle/pointer bytes are not canonical values.
    pub native_outcome: Option<(u32, Vec<u8>)>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum CanonicalItem {
    Root,
    Terminal,
    SharedRoot(crate::ordered::root_ids::SharedRootId),
    Object(ObjectId),
    Cancellation(Vec<u32>),
    CompletionRank(ObjectId),
    ScopeCancellation(ObjectId),
    HostKey(ObjectId),
    HostOperation(ObjectId),
    HostResource(host_keys::Resource),
    HostHandle(HostHandleId),
}

/// The actual completion returned by an owned host request. It is retained
/// separately from its entry/park boundaries under the same ordinal. Host
/// handle values still require logical-key normalization before export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HostCompletion {
    pub task: Vec<u32>,
    pub ordinal: u64,
    pub op: &'static str,
    pub result: Result<crate::io::Completion, crate::io::AdapterError>,
    /// Initialized payload with handle words removed; missing identities reject.
    pub observation: Option<host_values::HostValue>,
}

/// The completion's scheduling position, before its owner can be unparked.
/// Idle polls share the next retirement position but retain their own ordered
/// sequence; native worker/host-token identities are deliberately absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HostPollCompletion {
    pub completion: HostCompletion,
    pub retirement: u64,
    pub idle: bool,
    pub sequence: u64,
}

/// A saved receive slot is filled by retirement readiness before its owner
/// can resume. This snapshot fixes that effect's original operation ordinal
/// and poll position without borrowing the parked task's context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ReceivePollCompletion {
    pub task: Vec<u32>,
    pub ordinal: u64,
    pub channel: ObjectId,
    pub status: u32,
    pub payload: Option<Vec<u8>>,
    pub retirement: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OwnerOutcome {
    ScopeExit {
        status: u8,
        expired: bool,
    },
    AwaitAny {
        set: ObjectId,
        index: Option<u64>,
        member: Option<ObjectId>,
        rank: Option<u64>,
    },
    Join {
        scope: Option<ObjectId>,
        child: Option<ObjectId>,
        child_path: Vec<u32>,
        status: u8,
        rank: u64,
        payload: Option<Vec<u8>>,
        /// None means a native failure lacked immutable source metadata.
        failures: Option<Vec<(hirsel_run_schema::Failure, Vec<u32>)>>,
    },
    /// Payload is unavailable until a descriptor-aware initialized-field
    /// decoder certifies it. Never copy padding or an unwritten carrier.
    Receive {
        status: u32,
        descriptor: u32,
        payload: Option<Vec<u8>>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OwnerCompletion {
    pub task: Vec<u32>,
    pub ordinal: u64,
    pub op: &'static str,
    pub outcome: OwnerOutcome,
}

/// A quiescent wait projected from scheduler rows, never saved stack slots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CanonicalWait {
    pub task: Vec<u32>,
    pub site: hirsel_run_schema::Site,
    pub kind: &'static str,
    pub targets: Vec<ObjectId>,
}

/// Canonical scalar event: no native handle word or physical worker enters it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CanonicalEvent {
    pub task: Vec<u32>,
    pub ordinal: u64,
    pub subevent: u32,
    pub op: &'static str,
    pub transition: Transition,
    pub item: CanonicalItem,
    pub version: u64,
    pub outcome: CanonicalOutcome,
    pub retirement: u64,
    pub before: Stretch,
    pub credit: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CanonicalOutcome {
    CpuControl {
        width: u32,
        action: u32,
        count: u64,
        outcome: u32,
    },
    HostRequest(host_requests::HostRequestValue),
    HostAccess {
        mode: crate::io_keys::Mode,
        result: host_values::HostValue,
    },
    HostResourceCreated,
    HostCreated(host_keys::Creation),
    HostAlias(host_keys::Creation),
    HostResult(host_values::HostValue),
    HostFinished,
    HostWaiting,
    /// A terminal cut owns this phase; no caller consumed a later result.
    HostAbandoned {
        completed: bool,
    },
    HostOutput(Vec<u8>),
    Created(ObjectId),
    Alias(ObjectId),
    Scalar(Vec<u8>),
    Argument {
        kind: &'static str,
        index: Option<u64>,
        value: Option<Vec<u8>>,
    },
    Value {
        status: u32,
        payload: Vec<u8>,
    },
    Spawned {
        scope: ObjectId,
        path: Vec<u32>,
    },
    ScopeExiting {
        members: Vec<ObjectId>,
    },
    ScopeExited {
        status: u8,
        expired: bool,
        failures: Vec<(hirsel_run_schema::Failure, Vec<u32>)>,
    },
    BulkCanceling {
        members: Vec<ObjectId>,
    },
    Awaiting {
        members: Vec<(u64, ObjectId)>,
    },
    /// The selected global stop abandoned a proved saved await prefix.
    /// No member result or completion rank was observed.
    AwaitAbandoned {
        task: Vec<u32>,
        ordinal: u64,
    },
    Selected {
        index: Option<u64>,
        member: Option<ObjectId>,
        rank: Option<u64>,
    },
    Failures(Vec<(hirsel_run_schema::Failure, Vec<u32>)>),
    RootReleased {
        last: bool,
    },
    Thaw {
        last: bool,
    },
    Released {
        kind: u64,
        leftovers: Vec<Vec<u8>>,
    },
    DeadlineArmed {
        deadline: u64,
        armed: bool,
    },
    DeadlineExpired,
    GateWaiting,
    GateAcquired,
    GateReleased,
    Receiving,
    Received {
        status: u32,
        payload: Option<Vec<u8>>,
    },
    Closed,
    BulkDetaching {
        members: Vec<ObjectId>,
    },
    DetachSkipped,
    Detached,
    JoinAbandoned {
        task: Vec<u32>,
        ordinal: u64,
    },
    Joining,
    Joined {
        status: u8,
        rank: u64,
        payload: Option<Vec<u8>>,
        failures: Vec<(hirsel_run_schema::Failure, Vec<u32>)>,
    },
    TaskEnd {
        holding: bool,
        result: Option<Vec<u8>>,
    },
    TaskFailed {
        failures: Vec<(hirsel_run_schema::Failure, Vec<u32>)>,
    },
    Stopped(hirsel_run_schema::RunOutcome),
    Blocked(Vec<CanonicalWait>),
    RootEnd {
        status: u8,
        result: Option<Vec<u8>>,
        failures: Vec<(hirsel_run_schema::Failure, Vec<u32>)>,
    },
    Unit,
}

pub(super) type Subevents = Vec<(CanonicalItem, CanonicalOutcome)>;

/// Use the scheduler's prepared linked-child walk, exactly as ordinary cancel
/// does. Capture every actual target's first-wins cause before any scheduling
/// point, including descendants not named by the issuing operation's handles.
/// Paths sort the subevents independently of table slots and child-link order.
pub(super) fn cancel_observed(
    s: &mut super::super::Scheduler,
    task: u32,
    cause: u32,
) -> Option<Subevents> {
    if !matches!(&s.logical, Logical::Ordered(driver) if driver.events.is_some()) {
        s.cancel(task, cause);
        return None;
    }
    let targets = s.prepare_cancel(task);
    s.cancel_prepared(task, cause, &targets);
    let mut events: Subevents = targets
        .into_iter()
        .map(|target| {
            let row = s.task(target);
            (
                CanonicalItem::Cancellation(row.path.clone()),
                CanonicalOutcome::Scalar(
                    row.canceled
                        .cause()
                        .map_or(0, |cause| cause + 1)
                        .to_le_bytes()
                        .to_vec(),
                ),
            )
        })
        .collect();
    events.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Some(events)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Unsupported {
    pub task: Vec<u32>,
    pub ordinal: u64,
    pub op: Option<&'static str>,
}

/// Result snapshot supplied by the compiled task wrapper before its slot is
/// delivered or discarded. Unknown metadata is explicit, never raw bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TaskResult {
    pub descriptor: u32,
    pub payload: Option<Vec<u8>>,
}

struct BulkDetachEvidence {
    set: ObjectId,
    members: Vec<ObjectId>,
    ordinal: u64,
    next: usize,
}

struct CancelAllEvidence {
    scope: ObjectId,
    set: ObjectId,
    members: Vec<ObjectId>,
    effect: Option<Subevents>,
    closed: bool,
}

struct AwaitAnyEvidence {
    /// Generation-qualified native lookup keys, never canonical identities.
    scope_word: u64,
    set_word: u64,
    scope: ObjectId,
    set: ObjectId,
    members: Vec<(u64, ObjectId)>,
}

struct ScopeExitEvidence {
    scope: ObjectId,
    members: Vec<ObjectId>,
    cancellations: Subevents,
}

struct DeadlineEvidence {
    path: Vec<u32>,
    ordinal: u64,
    scope: ObjectId,
    clock: Option<CanonicalItem>,
    at: u64,
}

#[derive(Default)]
pub(super) struct Log {
    ordinals: BTreeMap<Vec<u32>, u64>,
    last_ops: BTreeMap<Vec<u32>, &'static str>,
    end_ordinals: BTreeMap<Vec<u32>, u64>,
    failed_ends: BTreeMap<Vec<u32>, Option<Subevents>>,
    next_subevent: BTreeMap<(Vec<u32>, u64), u32>,
    completed: BTreeSet<(Vec<u32>, u64)>,
    pub host_handles: BTreeMap<u64, HostHandleId>,
    pub host_bindings: BTreeMap<HostHandleId, host_keys::Creation>,
    pub host_resources: host_keys::Resources,
    pub host_key_observations:
        BTreeMap<(Vec<u32>, u64), Option<Vec<(host_keys::Resource, crate::io_keys::Mode)>>>,
    pub host_roots: BTreeMap<u64, HostAuthority>,
    host_entries: BTreeMap<(Vec<u32>, u64), Boundary>,
    home_phases: BTreeMap<(Vec<u32>, u64), host_phases::Phase>,
    home_requests: BTreeSet<(Vec<u32>, u64)>,
    home_declared: BTreeSet<(Vec<u32>, u64)>,
    host_domains: BTreeMap<(Vec<u32>, u64), Vec<host_keys::Resource>>,
    concurrent_requests: BTreeSet<(Vec<u32>, u64)>,
    invalid_home: BTreeSet<(Vec<u32>, u64)>,
    host_certified: BTreeSet<(Vec<u32>, u64)>,
    host_outputs: BTreeMap<(Vec<u32>, u64), Result<Option<Vec<u8>>, &'static str>>,
    host_creations: BTreeMap<(Vec<u32>, u64), Vec<host_keys::Creation>>,
    pub host_requests: BTreeMap<(Vec<u32>, u64), Option<host_requests::HostRequestValue>>,
    pub host_completions: Vec<HostCompletion>,
    pub host_initials: BTreeMap<(Vec<u32>, u64), HostCompletion>,
    pub host_polls: Vec<HostPollCompletion>,
    /// Actual readiness retries, including Blocked; skipped probes emit none.
    pub host_attempts: Vec<HostPollCompletion>,
    host_polled: BTreeSet<(Vec<u32>, u64)>,
    pub owner_completions: Vec<OwnerCompletion>,
    pub task_results: BTreeMap<Vec<u32>, TaskResult>,
    pub receive_polls: Vec<ReceivePollCompletion>,
    receivers: BTreeMap<Vec<u32>, (ObjectId, u32)>,
    receiving: BTreeSet<Vec<u32>>,
    gate_waiting: BTreeMap<Vec<u32>, (u32, u64)>,
    gates: BTreeMap<u32, ObjectId>,
    deadlines: BTreeMap<u32, DeadlineEvidence>,
    arming: BTreeSet<Vec<u32>>,
    receive_entries: BTreeMap<Vec<u32>, Option<Subevents>>,
    receive_results: BTreeMap<(Vec<u32>, u64), (u32, Option<Vec<u8>>)>,
    joins: BTreeMap<Vec<u32>, (ObjectId, ObjectId)>,
    scope_exits: BTreeMap<Vec<u32>, ScopeExitEvidence>,
    bulk_detaches: BTreeMap<Vec<u32>, BulkDetachEvidence>,
    current_retirement: u64,
    cancel_all: BTreeMap<Vec<u32>, CancelAllEvidence>,
    awaiting: BTreeMap<Vec<u32>, AwaitAnyEvidence>,
    receive_polled: BTreeSet<(Vec<u32>, u64)>,
    pub boundaries: Vec<Boundary>,
    versions: BTreeMap<CanonicalItem, u64>,
    /// Generation-qualified native words are lookup keys only. Keep bindings
    /// after close so delayed evidence cannot be confused with a reused row.
    sets: BTreeMap<u64, ObjectId>,
    scopes: BTreeMap<u64, ObjectId>,
    tasks: BTreeMap<u64, ObjectId>,
    values: BTreeMap<u32, super::values::Program>,
    pub(super) canonical: Vec<CanonicalEvent>,
    unsupported: Vec<Unsupported>,
    closed_blocked: bool,
    charges: Option<crate::budget::stretch_trace::Observer>,
    budget_terminal: bool,
    #[cfg(test)]
    pub(super) rejected_budget_stretches: Vec<Stretch>,
}

/// Copy a mapped view while its backing storage is still owned by the host
/// request. All other completion variants already own their data.
pub(crate) fn owned_host_result(
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
) -> Result<crate::io::Completion, crate::io::AdapterError> {
    match result {
        Ok(crate::io::Completion::View(view)) => Ok(crate::io::Completion::View(
            crate::io::MapView::of(view.contents()),
        )),
        value => value.clone(),
    }
}

/// Drain actual decrements, including nested root walks, in mutation order.
/// Unknown imported roots reject the whole projection; pointer bytes are absent.
/// # Safety
/// This committing callback exclusively owns ctx's non-suspending root tail.
pub(super) unsafe fn root_releases(ctx: *mut TaskCtx) -> Option<Subevents> {
    unsafe { std::mem::take(&mut (*ctx).roots.trace_releases) }
        .into_iter()
        .map(|release| {
            release.map(|(identity, last)| {
                (
                    CanonicalItem::SharedRoot(identity),
                    CanonicalOutcome::RootReleased { last },
                )
            })
        })
        .collect()
}

/// Deadline scopes are created in the head's bounded entry, before the
/// home-affine arm call. A missing clock identity cannot certify that call.
pub(super) fn deadline_scope(
    ctx: *mut TaskCtx,
    word: u64,
    clock: u64,
    at: u64,
) -> Option<Subevents> {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return None;
    }
    let owner = unsafe { (*ctx).task_id };
    let path = s.tasks[owner as usize].as_ref().unwrap().path.clone();
    let cause = s
        .canceled_task(owner, unsafe { (*ctx).shield })
        .map_or(0, |cause| cause + 1);
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let clock = driver
        .events
        .as_ref()
        .unwrap()
        .host_handles
        .get(&clock)
        .cloned()
        .map(CanonicalItem::HostHandle)
        .or_else(|| {
            driver
                .objects
                .identity(crate::ordered::objects::Kind::Capability, clock)
                .cloned()
                .map(CanonicalItem::HostKey)
        });
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals.get(&path).copied().unwrap_or(0);
    let scope = ObjectId {
        creator: path.clone(),
        ordinal,
    };
    assert!(log.scopes.insert(word, scope.clone()).is_none());
    assert!(log.arming.insert(path.clone()));
    log.deadlines.insert(
        word as u32,
        DeadlineEvidence {
            path: path.clone(),
            ordinal,
            scope: scope.clone(),
            clock,
            at,
        },
    );
    Some(vec![
        (
            CanonicalItem::Object(scope.clone()),
            CanonicalOutcome::Created(scope.clone()),
        ),
        (
            CanonicalItem::CompletionRank(scope.clone()),
            CanonicalOutcome::Scalar(0u64.to_le_bytes().to_vec()),
        ),
        (
            CanonicalItem::ScopeCancellation(scope),
            CanonicalOutcome::Scalar(0u32.to_le_bytes().to_vec()),
        ),
        (
            CanonicalItem::Cancellation(path),
            CanonicalOutcome::Scalar(cause.to_le_bytes().to_vec()),
        ),
    ])
}

/// The arm call returned with no policy borrow held. Capture its result
/// before immediate expiry or the entry's Yield can publish cancellation.
pub(super) fn deadline_armed(ctx: *mut TaskCtx, scope: u32, armed: bool) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let before = Stretch::new(driver.budgets);
    let log = driver.events.as_mut().unwrap();
    let evidence = &log.deadlines[&scope];
    let path = evidence.path.clone();
    let ordinal = evidence.ordinal;
    assert!(log.arming.remove(&path));
    let Some(clock) = evidence.clock.clone() else {
        log.unsupported.push(Unsupported {
            task: path,
            ordinal,
            op: Some("task.scope_enter"),
        });
        return;
    };
    let events = vec![
        (
            clock,
            CanonicalOutcome::DeadlineArmed {
                deadline: evidence.at,
                armed,
            },
        ),
        (
            CanonicalItem::Object(evidence.scope.clone()),
            CanonicalOutcome::Unit,
        ),
    ];
    let boundary = Boundary {
        task: path,
        ordinal,
        op: None,
        transition: Transition::Keep,
        retirement,
        before,
        credit: 0,
        native_outcome: None,
    };
    log.subevents(&boundary, "task.scope_enter", events);
}

/// Apply real expiry once and capture all cancellation targets at that exact
/// poll position. Idle callers hold only Scheduler; parked contexts are unused.
pub(crate) fn expire(s: &mut super::super::Scheduler, scope: u32) {
    if !enabled(s) {
        s.expire(scope);
        return;
    }
    if s.scopes[scope as usize]
        .as_ref()
        .unwrap()
        .deadline
        .as_ref()
        .unwrap()
        .expired
    {
        return;
    }
    let owner = s.scope(scope).owner;
    let mut roots = s.scope(scope).children.clone();
    let inner: Vec<_> = s
        .task(owner)
        .scopes
        .iter()
        .skip_while(|&&id| id != scope)
        .skip(1)
        .copied()
        .collect();
    for inner in inner {
        roots.extend(s.scope(inner).children.clone());
    }
    let mut targets = BTreeSet::new();
    for root in roots {
        targets.extend(s.prepare_cancel(root));
    }
    s.expire(scope);
    let mut cancellations: Subevents = targets
        .into_iter()
        .map(|task| {
            let row = s.tasks[task as usize].as_ref().unwrap();
            (
                CanonicalItem::Cancellation(row.path.clone()),
                CanonicalOutcome::Scalar(
                    row.canceled
                        .cause()
                        .map_or(0, |cause| cause + 1)
                        .to_le_bytes()
                        .to_vec(),
                ),
            )
        })
        .collect();
    cancellations.push((
        CanonicalItem::Cancellation(s.tasks[owner as usize].as_ref().unwrap().path.clone()),
        CanonicalOutcome::Scalar(
            s.canceled_task(owner, 0)
                .map_or(0, |cause| cause + 1)
                .to_le_bytes()
                .to_vec(),
        ),
    ));
    cancellations.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let before = Stretch::new(driver.budgets);
    let log = driver.events.as_mut().unwrap();
    let evidence = &log.deadlines[&scope];
    let mut events = vec![
        (
            CanonicalItem::Object(evidence.scope.clone()),
            CanonicalOutcome::DeadlineExpired,
        ),
        (
            CanonicalItem::ScopeCancellation(evidence.scope.clone()),
            CanonicalOutcome::Scalar(
                s.scopes[scope as usize]
                    .as_ref()
                    .unwrap()
                    .canceled
                    .cause()
                    .map_or(0, |cause| cause + 1)
                    .to_le_bytes()
                    .to_vec(),
            ),
        ),
    ];
    events.extend(cancellations);
    let boundary = Boundary {
        task: evidence.path.clone(),
        ordinal: evidence.ordinal,
        op: None,
        transition: Transition::Keep,
        retirement,
        before,
        credit: 0,
        native_outcome: None,
    };
    log.subevents(&boundary, "task.scope_enter", events);
}

/// First outer entry mints the gate's creator identity at retirement. Native
/// package indices are retained only as lookup keys, never canonical values.
pub(super) fn gate_event(
    ctx: *mut TaskCtx,
    gate: u32,
    outcome: CanonicalOutcome,
) -> Option<Subevents> {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return None;
    }
    let path = s.tasks[unsafe { (*ctx).task_id } as usize]
        .as_ref()
        .unwrap()
        .path
        .clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals.get(&path).copied().unwrap_or(0);
    let mut events = Vec::new();
    let object = log
        .gates
        .entry(gate)
        .or_insert_with(|| {
            assert!(
                !matches!(outcome, CanonicalOutcome::GateReleased),
                "gate entry precedes leave"
            );
            let object = ObjectId {
                creator: path.clone(),
                ordinal,
            };
            events.push((
                CanonicalItem::Object(object.clone()),
                CanonicalOutcome::Created(object.clone()),
            ));
            object
        })
        .clone();
    if matches!(outcome, CanonicalOutcome::GateWaiting) {
        assert!(log.gate_waiting.insert(path, (gate, ordinal)).is_none());
    }
    events.push((CanonicalItem::Object(object), outcome));
    Some(events)
}

/// The successful readiness acquisition is part of the waiting entry's
/// original ordinal and is published before unpark, without a parked ctx.
pub(crate) fn record_gate_poll(s: &mut super::super::Scheduler, task: u32, gate: u32) {
    if !enabled(s) {
        return;
    }
    let path = s.tasks[task as usize].as_ref().unwrap().path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let before = Stretch::new(driver.budgets);
    let log = driver.events.as_mut().unwrap();
    let (expected, ordinal) = log
        .gate_waiting
        .remove(&path)
        .expect("gate entry before poll");
    assert_eq!(expected, gate);
    let boundary = Boundary {
        task: path,
        ordinal,
        op: None,
        transition: Transition::Keep,
        retirement,
        before,
        credit: 0,
        native_outcome: None,
    };
    log.subevents(
        &boundary,
        "gate.enter",
        vec![(
            CanonicalItem::Object(log.gates[&gate].clone()),
            CanonicalOutcome::GateAcquired,
        )],
    );
}

/// Caller owns the scheduler role. This lookup cannot allocate, bridge or
/// observe a stop, including on the disabled native host path.
pub(crate) fn observing(ctx: *mut TaskCtx) -> bool {
    let table = unsafe { &*(*(*ctx).run_ptr()).tasks.get() };
    table.as_deref().is_some_and(enabled)
}

pub(super) fn enabled(s: &super::super::Scheduler) -> bool {
    matches!(&s.logical, Logical::Ordered(driver) if driver.events.is_some())
}

fn host_identity(driver: &super::driver::Driver, word: u64) -> Option<HostHandleId> {
    driver
        .events
        .as_ref()?
        .host_handles
        .get(&word)
        .cloned()
        .or_else(|| {
            driver
                .objects
                .identity(crate::ordered::objects::Kind::Capability, word)
                .cloned()
                .map(Into::into)
        })
}

/// IdleGuard owns the scheduler, but no TaskCtx is installed or sampled.
pub(crate) fn record_host_poll(
    s: &mut super::super::Scheduler,
    task: u32,
    result: Result<crate::io::Completion, crate::io::AdapterError>,
    idle: bool,
) {
    let path = s.task(task).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let observation = host_values::project(&result, |word| host_identity(driver, word)).ok();
    let log = driver.events.as_mut().expect("poll evidence enabled");
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("host poll follows entry");
    let op = log.last_ops[&path];
    assert_eq!(op, "host.request");
    assert!(
        log.host_polled.insert((path.clone(), ordinal)),
        "one completing poll per host request"
    );
    log.host_polls.push(HostPollCompletion {
        completion: HostCompletion {
            task: path,
            ordinal,
            op,
            result,
            observation,
        },
        retirement,
        idle,
        sequence: log
            .host_polls
            .len()
            .try_into()
            .expect("host poll sequence fits u64"),
    });
}

/// Capture the first bounded attempt on its owner before it can park. The
/// snapshot owns mapping bytes and keeps the original ordinal even when a
/// future retry completes it from another executor.
pub(crate) fn record_host_initial(
    ctx: *mut TaskCtx,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() };
    let Some(s) = s.filter(|s| enabled(s)) else {
        return;
    };
    record_host_initial_for(s, unsafe { (*ctx).task_id }, result);
    unsafe { (*(*ctx).run_ptr()).ordered_home_results.lock().unwrap() }
        .remove(&unsafe { (*ctx).task_id });
}

fn record_host_initial_for(
    s: &mut super::super::Scheduler,
    task: u32,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
) {
    let path = s.task(task).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let result = owned_host_result(result);
    let ordinal = driver.events.as_ref().unwrap().ordinals[&path]
        .checked_sub(1)
        .expect("first attempt follows entry");
    let _ = driver
        .events
        .as_mut()
        .unwrap()
        .discover_home(&(path.clone(), ordinal), &result);
    let observation = host_values::project(&result, |word| host_identity(driver, word)).ok();
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("initial attempt follows entry");
    let op = log.last_ops[&path];
    assert!(matches!(op, "host.request" | "host.handle_drop"));
    let completion = HostCompletion {
        task: path.clone(),
        ordinal,
        op,
        result,
        observation,
    };
    assert!(
        log.host_initials
            .insert((path.clone(), ordinal), completion)
            .is_none(),
        "one initial host attempt per ordinal"
    );
    log.start_home(&path, ordinal);
}

/// Source-free evidence of one actual poll attempt, captured before any wake.
/// Completion-only host_polls stays separate until parked phases are certified.
pub(crate) fn record_host_attempt(
    s: &mut super::super::Scheduler,
    task: u32,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
    idle: bool,
) {
    let path = s.task(task).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let ordinal = driver.events.as_ref().unwrap().ordinals[&path]
        .checked_sub(1)
        .expect("retry follows entry");
    let _ = driver
        .events
        .as_mut()
        .unwrap()
        .discover_home(&(path.clone(), ordinal), result);
    let observation = host_values::project(result, |word| host_identity(driver, word)).ok();
    let log = driver.events.as_mut().expect("poll evidence enabled");
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("poll follows entry");
    let op = log.last_ops[&path];
    assert_eq!(op, "host.request");
    let key = (path.clone(), ordinal);
    log.host_attempts.push(HostPollCompletion {
        completion: HostCompletion {
            task: path,
            ordinal,
            op,
            result: result.clone(),
            observation,
        },
        retirement,
        idle,
        sequence: log
            .host_attempts
            .len()
            .try_into()
            .expect("host attempt sequence fits u64"),
    });
    let attempt = log.host_attempts.last().unwrap().clone();
    log.attempt_home(&key, &attempt, Stretch::new(driver.budgets));
}

impl Log {
    fn value_host_identity(&self, word: u64) -> Option<HostAuthority> {
        self.host_roots
            .get(&word)
            .cloned()
            .or_else(|| self.host_handles.get(&word).cloned().map(Into::into))
    }

    fn subevents(&mut self, boundary: &Boundary, name: &'static str, events: Subevents) {
        for (item, outcome) in events {
            let next = self
                .next_subevent
                .entry((boundary.task.clone(), boundary.ordinal))
                .or_default();
            let subevent = *next;
            *next = next
                .checked_add(1)
                .expect("canonical subevent index overflow");
            let version = self.versions.entry(item.clone()).or_default();
            *version = version
                .checked_add(1)
                .expect("canonical object version overflow");
            if matches!(
                outcome,
                CanonicalOutcome::Created(_)
                    | CanonicalOutcome::Spawned { .. }
                    | CanonicalOutcome::HostCreated(_)
                    | CanonicalOutcome::HostResourceCreated
            ) {
                assert_eq!(*version, 1, "creation is the first object subevent");
            }
            self.canonical.push(CanonicalEvent {
                task: boundary.task.clone(),
                ordinal: boundary.ordinal,
                subevent,
                op: name,
                transition: boundary.transition.clone(),
                item,
                version: *version,
                outcome,
                retirement: boundary.retirement,
                before: boundary.before.clone(),
                credit: boundary.credit,
            });
        }
    }

    /// A partial projection must never be accepted as a complete trace.
    pub(super) fn canonical(&self) -> Result<&[CanonicalEvent], &[Unsupported]> {
        if self.unsupported.is_empty()
            && self.invalid_home.is_empty()
            && self.host_entries.is_empty()
            && (self.closed_blocked || self.home_phases.is_empty())
            && self.host_certified.is_empty()
            && (self.closed_blocked
                || (self.joins.is_empty()
                    && self.scope_exits.is_empty()
                    && self.bulk_detaches.is_empty()
                    && self.cancel_all.is_empty()
                    && self.awaiting.is_empty()
                    && self.receiving.is_empty()
                    && self.gate_waiting.is_empty()
                    && self.arming.is_empty()))
        {
            Ok(&self.canonical)
        } else {
            Err(&self.unsupported)
        }
    }
}

/// The source-free quiet verdict already owns IdleGuard and every frame is
/// saved. Only its selected Blocked cause may close an unfinished wait prefix.
pub(in crate::tasks) fn blocked(
    s: &mut super::super::Scheduler,
    table: Option<crate::isolate::FailureTable>,
) {
    blocked::record(s, table);
}

/// Enable only at admission, before any operation can retire. No run identity
/// or production dispatcher is changed by installing the evidence collector.
pub(crate) fn enable(ctx: *mut TaskCtx) {
    unsafe { (*ctx).ordered_charge_trace.enable() };
    unsafe { (*ctx).run_ref() }
        .ordered_roots
        .get_or_init(Default::default);
    super::engine::with_driver(ctx, |driver| {
        assert!(driver.recorded.is_empty());
        assert!(driver.events.replace(Log::default()).is_none());
    });
}

/// # Safety
/// Test admission only. Program must describe the actual initialized fields
/// of every value of this descriptor, including its active variant tags.
#[cfg(test)]
pub(super) unsafe fn install_value(
    ctx: *mut TaskCtx,
    descriptor: u32,
    program: super::values::Program,
) {
    assert_eq!(
        unsafe { (*(*ctx).descriptor(descriptor)).size },
        program.extent() as u64
    );
    let log = super::engine::driver(ctx)
        .events
        .as_mut()
        .expect("evidence enabled at admission");
    assert!(log.values.insert(descriptor, program).is_none());
}

/// # Safety
/// The running child owns a live Some result of `descriptor`. The sentinel
/// u32::MAX in Rust stand-ins deliberately supplies no observation metadata.
pub(super) unsafe fn task_result(ctx: *mut TaskCtx, descriptor: u32, slot: *const u8) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    if descriptor != u32::MAX
        && let std::collections::btree_map::Entry::Vacant(entry) = log.values.entry(descriptor)
        && let Ok(program) = unsafe { super::values::Program::linked(ctx, descriptor) }
    {
        entry.insert(program);
    }
    let payload = log.values.get(&descriptor).and_then(|program| unsafe {
        program
            .encode_with_host(
                slot,
                |kind, word| {
                    use crate::ordered::objects::Kind;
                    match kind {
                        Kind::Task => log.tasks.get(&word),
                        Kind::TaskSet => log.sets.get(&word),
                        _ => driver.objects.identity(kind, word),
                    }
                    .cloned()
                },
                |word| log.value_host_identity(word),
            )
            .ok()
    });
    assert!(
        log.task_results
            .insert(
                path,
                TaskResult {
                    descriptor,
                    payload
                }
            )
            .is_none(),
        "one task result publication"
    );
}

/// Compile immutable metadata in the running owner's entry window. The idle
/// readiness callback later has no TaskCtx and must not perform this lookup.
/// # Safety
/// The running owner holds the role; the linked descriptors outlive the run.
pub(super) unsafe fn prepare_receive(ctx: *mut TaskCtx, word: u64, descriptor: u32) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let channel = driver
        .objects
        .identity(crate::ordered::objects::Kind::Receiver, word)
        .expect("an admitted receiver identity")
        .clone();
    assert!(log.receiving.insert(path.clone()));
    log.receivers.insert(path, (channel, descriptor));
    if let std::collections::btree_map::Entry::Vacant(entry) = log.values.entry(descriptor)
        && let Ok(program) = unsafe { super::values::Program::linked(ctx, descriptor) }
    {
        entry.insert(program);
    }
}

/// Snapshot the actual immediate effect before its yield, with the immutable
/// initialized-field program prepared by the receiving owner before entry.
/// # Safety
/// DONE initialized message; every other status leaves it unread.
pub(super) unsafe fn receive_immediate(ctx: *mut TaskCtx, status: Option<u32>, message: *const u8) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let task = unsafe { (*ctx).task_id };
    let path = s.task(task).path.clone();
    let cause = s
        .canceled_task(task, unsafe { (*ctx).shield })
        .map_or(0, |cause| cause + 1);
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let (channel, descriptor) = &log.receivers[&path];
    let channel = channel.clone();
    let payload = if status == Some(crate::channels::DONE) {
        log.values.get(descriptor).and_then(|program| unsafe {
            program
                .encode_with_host(
                    message,
                    |kind, word| {
                        use crate::ordered::objects::Kind;
                        match kind {
                            Kind::Task => log.tasks.get(&word),
                            Kind::TaskSet => log.sets.get(&word),
                            _ => driver.objects.identity(kind, word),
                        }
                        .cloned()
                    },
                    |word| log.value_host_identity(word),
                )
                .ok()
        })
    } else {
        None
    };
    let canonical = if let Some(status) = status {
        let ordinal = log.ordinals.get(&path).copied().unwrap_or(0);
        assert!(
            log.receive_results
                .insert((path.clone(), ordinal), (status, payload.clone()))
                .is_none()
        );
        if status == crate::channels::DONE && payload.is_none() {
            None
        } else {
            Some(CanonicalOutcome::Received { status, payload })
        }
    } else {
        Some(CanonicalOutcome::Receiving)
    };
    let events = canonical.map(|outcome| {
        vec![
            (
                CanonicalItem::Cancellation(path.clone()),
                CanonicalOutcome::Scalar(cause.to_le_bytes().to_vec()),
            ),
            (CanonicalItem::Object(channel), outcome),
        ]
    });
    assert!(log.receive_entries.insert(path, events).is_none());
}

/// # Safety
/// The role owns `s`; `slot` is a saved receive output. DONE initialized its
/// active fields. Every other status may leave the entire output unwritten.
pub(crate) unsafe fn record_receive_poll(
    s: &mut super::super::Scheduler,
    task: u32,
    slot: *const u8,
    status: u32,
) {
    if !enabled(s) {
        return;
    }
    let path = s.task(task).path.clone();
    let shield = s.task(task).shield;
    let cause = s.canceled_task(task, shield).map_or(0, |cause| cause + 1);
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let before = Stretch::new(driver.budgets);
    let log = driver.events.as_mut().unwrap();
    let (channel, descriptor) = log.receivers.get(&path).expect("receive entry before poll");
    let channel = channel.clone();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("receive entry ordinal");
    assert_eq!(log.last_ops[&path], "channel.receive");
    assert!(
        log.receive_polled.insert((path.clone(), ordinal)),
        "one completing receive poll"
    );
    let payload = if status == crate::channels::DONE {
        log.values.get(descriptor).and_then(|program| unsafe {
            program
                .encode_with_host(
                    slot,
                    |kind, word| {
                        use crate::ordered::objects::Kind;
                        match kind {
                            Kind::Task => log.tasks.get(&word),
                            Kind::TaskSet => log.sets.get(&word),
                            _ => driver.objects.identity(kind, word),
                        }
                        .cloned()
                    },
                    |word| log.value_host_identity(word),
                )
                .ok()
        })
    } else {
        None
    };
    assert!(
        log.receive_results
            .insert((path.clone(), ordinal), (status, payload.clone()))
            .is_none()
    );
    if status == crate::channels::DONE && payload.is_none() {
        log.unsupported.push(Unsupported {
            task: path.clone(),
            ordinal,
            op: Some("channel.receive"),
        });
    } else {
        let boundary = Boundary {
            task: path.clone(),
            ordinal,
            op: None,
            transition: Transition::Keep,
            retirement,
            before,
            credit: 0,
            native_outcome: None,
        };
        log.subevents(
            &boundary,
            "channel.receive",
            vec![
                (
                    CanonicalItem::Cancellation(path.clone()),
                    CanonicalOutcome::Scalar(cause.to_le_bytes().to_vec()),
                ),
                (
                    CanonicalItem::Object(channel.clone()),
                    CanonicalOutcome::Received {
                        status,
                        payload: payload.clone(),
                    },
                ),
            ],
        );
    }
    log.receive_polls.push(ReceivePollCompletion {
        task: path,
        ordinal,
        channel: channel.clone(),
        status,
        payload,
        retirement,
        sequence: log
            .receive_polls
            .len()
            .try_into()
            .expect("receive poll sequence fits u64"),
    });
}

/// # Safety
/// As for channel.receive: DONE guarantees a live owner-local value matching
/// descriptor. No bytes at message are read for any other status.
pub(super) unsafe fn complete_receive(
    ctx: *mut TaskCtx,
    status: u32,
    descriptor: u32,
    message: *const u8,
) {
    // Link only a descriptor actually observed by tracing, once per run.
    // No task/value read is needed to compile immutable metadata.
    if status == crate::channels::DONE && observing(ctx) {
        let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        if !driver
            .events
            .as_ref()
            .unwrap()
            .values
            .contains_key(&descriptor)
            && let Ok(program) = unsafe { super::values::Program::linked(ctx, descriptor) }
        {
            driver
                .events
                .as_mut()
                .unwrap()
                .values
                .insert(descriptor, program);
        }
    }
    // SAFETY: the receiving owner has its role; no bridge or stop observation.
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
    let Logical::Ordered(driver) = &s.logical else {
        unreachable!()
    };
    let Some(log) = &driver.events else { return };
    let payload = if status == crate::channels::DONE {
        log.values.get(&descriptor).and_then(|program| unsafe {
            program
                .encode_with_host(
                    message,
                    |kind, word| {
                        use crate::ordered::objects::Kind;
                        match kind {
                            Kind::Task => log.tasks.get(&word),
                            Kind::TaskSet => log.sets.get(&word),
                            _ => driver.objects.identity(kind, word),
                        }
                        .cloned()
                    },
                    |word| log.value_host_identity(word),
                )
                .ok()
        })
    } else {
        None
    };
    let path = s.tasks[unsafe { (*ctx).task_id } as usize]
        .as_ref()
        .unwrap()
        .path
        .clone();
    let ordinal = log.ordinals[&path] - 1;
    let fixed = log.receive_results.get(&(path.clone(), ordinal));
    if let Some((observed_status, observed_payload)) = fixed {
        assert_eq!(*observed_status, status);
        assert_eq!(
            *observed_payload, payload,
            "receive completion matches effect before wake/yield"
        );
    }
    let recorded = fixed.is_some();
    complete_owner(ctx, "channel.receive", || OwnerOutcome::Receive {
        status,
        descriptor,
        payload,
    });
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    log.receiving.remove(&path);
    if !recorded {
        log.unsupported.push(Unsupported {
            task: path,
            ordinal,
            op: Some("channel.receive"),
        });
    }
}

/// Normalize the chain fixed by this mutex effect, before the owner can
/// resume, clear its failure state or recycle any native storage.
/// # Safety
/// The caller owns the scheduler role and immutable run failure metadata.
pub(super) unsafe fn mutex_failures(
    ctx: *mut TaskCtx,
    s: &super::super::Scheduler,
    word: u64,
    chain: &[crate::failure::Chained],
) -> Option<Subevents> {
    let Logical::Ordered(driver) = &s.logical else {
        return None;
    };
    driver.events.as_ref()?;
    let item = CanonicalItem::Object(
        driver
            .objects
            .identity(crate::ordered::objects::Kind::Mutex, word)?
            .clone(),
    );
    let failures = if chain.is_empty() {
        Vec::new()
    } else {
        let table = unsafe { (*ctx).run_ref().failure_table }?;
        chain
            .iter()
            .map(|(record, path)| (table.failure(record), path.clone()))
            .collect()
    };
    Some(vec![(item, CanonicalOutcome::Failures(failures))])
}

/// Own FIFO leftover values before finalizers can overwrite or release them.
/// Empty cells/sender credits require no payload descriptor or carrier read.
/// # Safety
/// Each detached image is initialized for its linked descriptor and remains
/// exclusively owned by this close until the observer returns.
pub(super) unsafe fn close_observed(
    ctx: *mut TaskCtx,
    kind: crate::ordered::objects::Kind,
    word: u64,
    left: Option<(&crate::channels::Ring, u32)>,
) -> Option<Subevents> {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &s.logical else {
        return None;
    };
    driver.events.as_ref()?;
    let item = CanonicalItem::Object(driver.objects.identity(kind, word)?.clone());
    let mut leftovers = Vec::new();
    if let Some((images, descriptor)) = left {
        for index in 0..images.len() {
            let (_, payload) =
                unsafe { object_value(ctx, s, kind, word, descriptor, images.at(index)) }?;
            leftovers.push(payload);
        }
    }
    Some(vec![(
        item,
        CanonicalOutcome::Released {
            kind: kind.code(),
            leftovers,
        },
    )])
}

/// Snapshot while the effect still owns the initialized carrier. A successful
/// send/put overwrites it with None, so retirement must use this owned evidence
/// rather than inspect the command or mutable object again.
/// # Safety
/// `image` holds a live value of `descriptor`; caller owns the scheduler role.
pub(super) unsafe fn object_value(
    ctx: *mut TaskCtx,
    s: &mut super::super::Scheduler,
    kind: crate::ordered::objects::Kind,
    word: u64,
    descriptor: u32,
    image: *const u8,
) -> Option<(CanonicalItem, Vec<u8>)> {
    let Logical::Ordered(driver) = &mut s.logical else {
        return None;
    };
    let log = driver.events.as_mut()?;
    let item = CanonicalItem::Object(driver.objects.identity(kind, word)?.clone());
    if let std::collections::btree_map::Entry::Vacant(entry) = log.values.entry(descriptor) {
        entry.insert(unsafe { super::values::Program::linked(ctx, descriptor) }.ok()?);
    }
    let bytes = unsafe {
        log.values[&descriptor].encode_with_host(
            image,
            |kind, word| {
                use crate::ordered::objects::Kind;
                match kind {
                    Kind::Task => log.tasks.get(&word),
                    Kind::TaskSet => log.sets.get(&word),
                    _ => driver.objects.identity(kind, word),
                }
                .cloned()
            },
            |word| log.value_host_identity(word),
        )
    }
    .ok()?;
    Some((item, bytes))
}

pub(super) fn value_subevent(
    value: Option<(CanonicalItem, Vec<u8>)>,
    status: u32,
) -> Option<Subevents> {
    value.map(|(item, payload)| vec![(item, CanonicalOutcome::Value { status, payload })])
}

/// Admission names the actual initial grants; numeric handle values carry no
/// meaning. This creates no operations or initialized result-handle bindings.
pub(crate) fn initial_host(ctx: *mut TaskCtx, io: Option<u64>, env: Option<u64>) {
    super::engine::with_driver(ctx, |driver| {
        let Some(log) = driver.events.as_mut() else {
            return;
        };
        assert!(
            log.ordinals.is_empty(),
            "initial authorities precede all operations"
        );
        assert!(
            log.host_roots.is_empty(),
            "initial authorities admitted once"
        );
        assert!(
            io.is_none() || env.is_none() || io != env,
            "root authority words must differ"
        );
        if let Some(io) = io {
            log.host_roots.insert(io, HostAuthority::RootIo);
        }
        if let Some(env) = env {
            log.host_roots.insert(env, HostAuthority::RootEnv);
        }
    });
}

/// Args is an admitted entry root, not a numeric zero with implicit authority.
pub(crate) fn initial_args(ctx: *mut TaskCtx, args: u64) {
    super::engine::with_driver(ctx, |driver| {
        let Some(log) = driver.events.as_mut() else {
            return;
        };
        assert!(
            log.ordinals.is_empty(),
            "initial authorities precede operations"
        );
        assert!(
            log.host_roots
                .insert(args, HostAuthority::RootArgs)
                .is_none()
        );
    });
}

/// Borrowed output bypasses the owned wait protocol. Copy only when evidence
/// is enabled, while the caller still owns the exact visible byte slice.
pub(crate) fn request_print(ctx: *mut TaskCtx, handle: u64, stderr: bool, bytes: &[u8]) {
    if !observing(ctx) {
        return;
    }
    let request = if stderr {
        crate::io::Request::PrintStderr {
            stderr: handle,
            bytes: bytes.to_vec(),
        }
    } else {
        crate::io::Request::Print {
            stdout: handle,
            bytes: bytes.to_vec(),
        }
    };
    request_host(ctx, &request);
}

/// Capture only the actual output witness before the caller can yield.
pub(crate) fn print_output(
    ctx: *mut TaskCtx,
    bytes: &[u8],
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
    count: Option<u64>,
) {
    host_output::record(ctx, host_output::project(Some(bytes), result, count));
}

pub(crate) fn retained_output(
    bytes: Option<&[u8]>,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
    count: Option<u64>,
) -> Result<Option<Vec<u8>>, &'static str> {
    host_output::project(bytes, result, count)
}

/// Auxiliary domains are rooted in an existing logical resource. The owned
/// declaration snapshot contains no scheduler or adapter loan.
pub(crate) fn request_domains(ctx: *mut TaskCtx, domains: Vec<(crate::io_keys::Key, u64, String)>) {
    if domains.is_empty() {
        return;
    }
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path].checked_sub(1).unwrap();
    let key = (path, ordinal);
    let declarations = domains
        .into_iter()
        .map(|(key, owner, label)| {
            let handle = log
                .host_handles
                .get(&owner)
                .ok_or("unmapped domain owner")?;
            let resource = log
                .host_bindings
                .get(handle)
                .ok_or("missing domain owner discovery")?
                .resource
                .clone();
            Ok((key, resource, label))
        })
        .collect::<Result<Vec<_>, &'static str>>();
    let result = declarations.and_then(|rows| log.host_resources.domains(&rows));
    match result {
        Ok(created) => {
            assert!(log.host_domains.insert(key, created).is_none());
        }
        Err(_) => {
            log.invalid_home.insert(key);
        }
    }
}

/// Capture already discovered issue keys under the original operation ordinal.
/// The caller must retain no adapter or home borrow across this role-owned call.
pub(crate) fn request_keys(ctx: *mut TaskCtx, keys: &crate::io_keys::KeySet) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("keys follow host entry");
    let observation = log.host_resources.project(keys).ok();
    assert!(
        log.host_key_observations
            .insert((path, ordinal), observation)
            .is_none()
    );
}

/// Capture the exact request before ownership moves to the home service.
/// The head boundary has assigned its ordinal, and no bridge or scheduling
/// point occurs here. Missing bindings reject the observation without guessing.
pub(crate) fn request_host(ctx: *mut TaskCtx, request: &crate::io::Request) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let observation = host_requests::project(request, |word| {
        driver
            .events
            .as_ref()
            .unwrap()
            .host_roots
            .get(&word)
            .cloned()
            .or_else(|| host_identity(driver, word).map(HostAuthority::Created))
    })
    .ok();
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("request follows host entry");
    assert!(
        log.host_requests
            .insert((path, ordinal), observation)
            .is_none(),
        "one request per host operation"
    );
}

/// Parkable concurrent operations use the same entered/attempt/return phases
/// as home waits, with their declared resource footprint and owned discovery.
pub(crate) fn concurrent_phases(ctx: *mut TaskCtx) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path].checked_sub(1).unwrap();
    assert!(log.concurrent_requests.insert((path, ordinal)));
}

/// Install an owned adapter discovery snapshot before projecting its result.
/// Identity lookups ran outside this role; this window cannot suspend.
pub(crate) fn discover_host(
    ctx: *mut TaskCtx,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
    discovered: &BTreeMap<u64, (crate::io_keys::Key, crate::io_keys::Kind)>,
) -> Result<(), &'static str> {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    discover_host_for(s, unsafe { (*ctx).task_id }, result, discovered)
}

/// Source-free discovery at a reserved poll, before any completion or wake.
pub(crate) fn discover_host_for(
    s: &mut super::super::Scheduler,
    task: u32,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
    discovered: &BTreeMap<u64, (crate::io_keys::Key, crate::io_keys::Kind)>,
) -> Result<(), &'static str> {
    if !enabled(s) {
        return Ok(());
    }
    let path = s.task(task).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("discovery follows host entry");
    if matches!(result, Err(crate::io::AdapterError::InvalidHandle(_))) {
        return Ok(());
    }
    let key = (path.clone(), ordinal);
    if log.home_requests.contains(&key) && !log.home_declared.contains(&key) {
        return Ok(());
    }
    let created = log.host_created(
        ObjectId {
            creator: path,
            ordinal,
        },
        result,
        discovered,
    )?;
    let phased = log.concurrent_requests.contains(&key) || log.home_declared.contains(&key);
    assert!(log.host_creations.insert(key, created).is_none() || phased);
    Ok(())
}

pub(crate) fn home_declared(ctx: *mut TaskCtx) -> bool {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &s.logical else {
        return false;
    };
    let Some(log) = driver.events.as_ref() else {
        return false;
    };
    log.ordinals
        .get(&path)
        .and_then(|o| o.checked_sub(1))
        .is_some_and(|ordinal| log.home_declared.contains(&(path.clone(), ordinal)))
}

/// A declared parked request retains its complete logical footprint across
/// retries. Missing or changed domains cannot certify the entered phase.
pub(crate) fn home_retry_keys(
    s: &mut super::super::Scheduler,
    task: u32,
    keys: Option<&crate::io_keys::KeySet>,
) {
    if !enabled(s) {
        return;
    }
    let path = s.task(task).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path].checked_sub(1).unwrap();
    let key = (path, ordinal);
    if !log.home_declared.contains(&key) {
        return;
    }
    let observed = keys.and_then(|keys| log.host_resources.project(keys).ok());
    if log.host_key_observations.get(&key) != Some(&observed) || observed.is_none() {
        log.invalid_home.insert(key.clone());
        if let Some(phase) = log.home_phases.get_mut(&key) {
            phase.invalidate();
        }
    }
}

/// A home observer cannot change the adapter's live outcome. Invalid discovery
/// permanently refuses certification while retaining raw result/attempt evidence.
pub(crate) fn home_discovery(
    s: &mut super::super::Scheduler,
    task: u32,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
    discovered: Result<BTreeMap<u64, (crate::io_keys::Key, crate::io_keys::Kind)>, &'static str>,
) {
    if !enabled(s) {
        return;
    }
    let path = s.task(task).path.clone();
    let Logical::Ordered(driver) = &s.logical else {
        unreachable!()
    };
    let log = driver.events.as_ref().unwrap();
    let ordinal = log.ordinals[&path].checked_sub(1).unwrap();
    let key = (path, ordinal);
    if !log.home_declared.contains(&key) {
        return;
    }
    let result = discovered.and_then(|d| discover_host_for(s, task, result, &d));
    if result.is_err() {
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        let log = driver.events.as_mut().unwrap();
        log.invalid_home.insert(key.clone());
        log.host_creations.remove(&key);
        if let Some(phase) = log.home_phases.get_mut(&key) {
            phase.invalidate();
        }
    }
}

/// The explicitly admitted nonparking concurrent path has finished its effect
/// and owns complete snapshots. Generic home requests do not call this.
pub(crate) fn certify_host(ctx: *mut TaskCtx) -> Result<(), &'static str> {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return Ok(());
    }
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("host certification follows entry");
    log.certify_host(&path, ordinal)
}

/// Capture after the owned request returned, before its caller can yield.
/// This is non-suspending and keeps the entered operation's original ordinal.
pub(crate) fn complete_host(
    ctx: *mut TaskCtx,
    close: bool,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
) {
    // SAFETY: the host boundary has regained its scheduler role; no bridge
    // or stop-aware entry is permitted between the effect and its evidence.
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let index = unsafe { (*ctx).task_id };
    let task = super::super::handle(index, s.task(index).generation);
    let Logical::Ordered(driver) = &s.logical else {
        return;
    };
    if let Some(completed) = driver.host_returns.get(&task) {
        assert!(!close);
        assert_eq!(&completed.result, result, "certified return cannot change");
        return;
    }
    if driver.events.is_none() {
        return;
    }
    let task = index;
    let path = s.task(task).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    // Snapshot borrowed mappings once, before encoding the owned observation.
    let result = owned_host_result(result);
    let observation = host_values::project(&result, |word| host_identity(driver, word)).ok();
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("a host completion follows entry");
    let op = if close {
        "host.handle_drop"
    } else {
        "host.request"
    };
    assert_eq!(log.last_ops.get(&path), Some(&op));
    assert!(
        log.completed.insert((path.clone(), ordinal)),
        "one completion per host operation"
    );
    let key = (path.clone(), ordinal);
    log.host_completions.push(HostCompletion {
        task: path,
        ordinal,
        op,
        result,
        observation,
    });
    let completion = log.host_completions.last().unwrap().clone();
    let retirement = driver.frontier.as_ref().unwrap().retired();
    log.return_home(&key, &completion, retirement, Stretch::new(driver.budgets));
}

/// The owner fixes a waiting operation's final outcome after its saved
/// continuations and called rests return. The encoder never runs when disabled.
pub(super) fn complete_owner(
    ctx: *mut TaskCtx,
    op: &'static str,
    outcome: impl FnOnce() -> OwnerOutcome,
) {
    // SAFETY: caller owns this role and output image; this window cannot yield.
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &s.logical else {
        return;
    };
    if driver.events.is_none() {
        return;
    }
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("completion follows entry");
    assert_eq!(log.last_ops.get(&path), Some(&op));
    assert!(
        log.completed.insert((path.clone(), ordinal)),
        "one completion per owner operation"
    );
    log.owner_completions.push(OwnerCompletion {
        task: path,
        ordinal,
        op,
        outcome: outcome(),
    });
}

/// Resolve identities before detach can retire an already-completed row.
/// The effect owns no result observation; called cleanup retains its task-end
/// events and detached running children publish their own later completion.
pub(super) fn detach_observed(
    s: &super::super::Scheduler,
    scope: u64,
    child: u64,
) -> Option<Subevents> {
    let Logical::Ordered(driver) = &s.logical else {
        return None;
    };
    let log = driver.events.as_ref()?;
    let scope = log.scopes.get(&scope)?.clone();
    let child = log.tasks.get(&child)?.clone();
    Some(vec![
        (
            CanonicalItem::Object(scope),
            CanonicalOutcome::Alias(child.clone()),
        ),
        (CanonicalItem::Object(child), CanonicalOutcome::Detached),
    ])
}

/// A bulk command commits only at the head. Capture its add-order members
/// before the first called rest can mutate or retire another native row.
pub(super) fn begin_bulk_detach(
    s: &mut super::super::Scheduler,
    caller: TaskId,
    scope_word: u64,
    set_word: u64,
    words: &[u64],
) -> Option<Subevents> {
    if !enabled(s) {
        return None;
    }
    let path = s.task(caller as u32).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let scope = log.scopes.get(&scope_word)?.clone();
    let set = log.sets.get(&set_word)?.clone();
    let members = words
        .iter()
        .map(|word| log.tasks.get(word).cloned())
        .collect::<Option<Vec<_>>>()?;
    let ordinal = log.ordinals.get(&path).copied().unwrap_or(0);
    assert!(
        log.bulk_detaches
            .insert(
                path,
                BulkDetachEvidence {
                    set: set.clone(),
                    members: members.clone(),
                    ordinal,
                    next: 0
                }
            )
            .is_none()
    );
    Some(vec![
        (
            CanonicalItem::Object(scope),
            CanonicalOutcome::Alias(set.clone()),
        ),
        (
            CanonicalItem::Object(set),
            CanonicalOutcome::BulkDetaching { members },
        ),
    ])
}

/// Called by the actual bounded member effect, including inert members that
/// an earlier called rest already retired. No parked context is consulted.
pub(super) fn bulk_detach_member(
    s: &mut super::super::Scheduler,
    caller: TaskId,
    word: u64,
    applied: bool,
) {
    if !enabled(s) {
        return;
    }
    let path = s.task(caller as u32).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let before = Stretch::new(driver.budgets);
    let log = driver.events.as_mut().unwrap();
    let Some(pending) = log.bulk_detaches.get_mut(&path) else {
        return;
    };
    let child = log
        .tasks
        .get(&word)
        .expect("retained bulk member identity")
        .clone();
    assert_eq!(
        pending.members.get(pending.next),
        Some(&child),
        "bulk effects follow add order"
    );
    pending.next += 1;
    let boundary = Boundary {
        task: path,
        ordinal: pending.ordinal,
        op: None,
        transition: Transition::Keep,
        retirement: log.current_retirement,
        before,
        credit: 0,
        native_outcome: None,
    };
    log.subevents(
        &boundary,
        "task.detach_all",
        vec![(
            CanonicalItem::Object(child),
            if applied {
                CanonicalOutcome::Detached
            } else {
                CanonicalOutcome::DetachSkipped
            },
        )],
    );
}

pub(super) fn complete_bulk_detach(s: &mut super::super::Scheduler, caller: TaskId) {
    if !enabled(s) {
        return;
    }
    let path = s.task(caller as u32).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let before = Stretch::new(driver.budgets);
    let log = driver.events.as_mut().unwrap();
    let Some(pending) = log.bulk_detaches.remove(&path) else {
        return;
    };
    assert_eq!(pending.next, pending.members.len());
    let boundary = Boundary {
        task: path,
        ordinal: pending.ordinal,
        op: None,
        transition: Transition::Keep,
        retirement: log.current_retirement,
        before,
        credit: 0,
        native_outcome: None,
    };
    log.subevents(
        &boundary,
        "task.detach_all",
        vec![(CanonicalItem::Object(pending.set), CanonicalOutcome::Unit)],
    );
}

pub(super) fn prepare_cancel_all(ctx: *mut TaskCtx, scope_word: u64, set_word: u64) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let owner = unsafe { (*ctx).task_id };
    let path = s.task(owner).path.clone();
    let scope = s.scope_handle(scope_word);
    let set = s.set_handle(set_word, scope);
    let words: Vec<_> = s.set(set).members.iter().flatten().copied().collect();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let evidence = log
        .scopes
        .get(&scope_word)
        .cloned()
        .zip(log.sets.get(&set_word).cloned())
        .and_then(|(scope, set)| {
            let members = words
                .iter()
                .map(|word| log.tasks.get(word).cloned())
                .collect::<Option<Vec<_>>>()?;
            Some(CancelAllEvidence {
                scope,
                set,
                members,
                effect: None,
                closed: false,
            })
        });
    if let Some(evidence) = evidence {
        assert!(log.cancel_all.insert(path, evidence).is_none());
    }
}

/// Record cancellation and detach together in the actual bounded member
/// effect. A resumed rest cannot rewrite its already-observed first cause.
pub(super) fn cancel_all_member(s: &mut super::super::Scheduler, owner: u32, child: u32) {
    let events = cancel_observed(s, child, super::super::CANCEL_REQUESTED);
    s.task(child).detached = true;
    let Some(mut events) = events else {
        return;
    };
    let path = s.task(owner).path.clone();
    let word = super::super::handle(child, s.task(child).generation);
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    if let Some(pending) = log.cancel_all.get_mut(&path) {
        let Some(child) = log.tasks.get(&word).cloned() else {
            return;
        };
        events.push((CanonicalItem::Object(child), CanonicalOutcome::Detached));
        assert!(pending.effect.replace(events).is_none());
    }
}

/// Closing is a real final continuation; its event is consumed by that same
/// continuation's record callback before complete trace admission is allowed.
pub(super) fn cancel_all_closed(s: &mut super::super::Scheduler, owner: u32) {
    if !enabled(s) {
        return;
    }
    let path = s.task(owner).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    if let Some(pending) = driver.events.as_mut().unwrap().cancel_all.get_mut(&path) {
        pending.closed = true;
        assert!(
            pending
                .effect
                .replace(vec![(
                    CanonicalItem::Object(pending.set.clone()),
                    CanonicalOutcome::Closed
                )])
                .is_none()
        );
    }
}

fn cancel_all_subevents(log: &mut Log, path: &[u32], initial: bool) -> Option<Subevents> {
    let pending = log.cancel_all.get_mut(path)?;
    if initial {
        return Some(vec![
            (
                CanonicalItem::Object(pending.scope.clone()),
                CanonicalOutcome::Alias(pending.set.clone()),
            ),
            (
                CanonicalItem::Object(pending.set.clone()),
                CanonicalOutcome::BulkCanceling {
                    members: pending.members.clone(),
                },
            ),
        ]);
    }
    Some(pending.effect.take().unwrap_or_else(|| {
        vec![(
            CanonicalItem::Object(pending.set.clone()),
            CanonicalOutcome::Unit,
        )]
    }))
}

/// Retain scope membership in spawn order before saved waits and reverse
/// cleanup calls can vacate child rows. Missing identities remain unsupported.
pub(super) fn prepare_scope_exit(ctx: *mut TaskCtx, scope_word: u64) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
    if !enabled(s) {
        return;
    }
    let path = s.tasks[unsafe { (*ctx).task_id } as usize]
        .as_ref()
        .unwrap()
        .path
        .clone();
    let scope = s.scopes[scope_word as u32 as usize].as_ref().unwrap();
    assert_eq!(scope.generation, (scope_word >> 32) as u32);
    let Logical::Ordered(driver) = &s.logical else {
        unreachable!()
    };
    let log = driver.events.as_ref().unwrap();
    let evidence = log.scopes.get(&scope_word).cloned().and_then(|id| {
        let members = scope
            .children
            .iter()
            .map(|&index| {
                let row = s.tasks[index as usize].as_ref().unwrap();
                log.tasks
                    .get(&super::super::handle(index, row.generation))
                    .cloned()
            })
            .collect::<Option<Vec<_>>>()?;
        Some(ScopeExitEvidence {
            scope: id,
            members,
            cancellations: Vec::new(),
        })
    });
    if let Some(evidence) = evidence {
        let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        assert!(
            driver
                .events
                .as_mut()
                .unwrap()
                .scope_exits
                .insert(path, evidence)
                .is_none()
        );
    }
}

/// Scope cancellation records first-wins causes in the same bounded effect,
/// before readiness or cleanup can observe them.
pub(super) fn scope_cancel(s: &mut super::super::Scheduler, owner: u32, child: u32) {
    let Some(events) = cancel_observed(s, child, super::super::CANCEL_PARENT) else {
        return;
    };
    let path = s.task(owner).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    if let Some(pending) = driver.events.as_mut().unwrap().scope_exits.get_mut(&path) {
        pending.cancellations.extend(events);
    }
}

fn scope_exit_subevents(log: &Log, path: &[u32], initial: bool) -> Option<Subevents> {
    let pending = log.scope_exits.get(path)?;
    let mut events = vec![(
        CanonicalItem::Object(pending.scope.clone()),
        if initial {
            CanonicalOutcome::ScopeExiting {
                members: pending.members.clone(),
            }
        } else {
            CanonicalOutcome::Unit
        },
    )];
    if initial {
        events.extend(pending.members.iter().map(|child| {
            (
                CanonicalItem::Object(child.clone()),
                CanonicalOutcome::Alias(pending.scope.clone()),
            )
        }));
        events.extend(pending.cancellations.clone());
    }
    Some(events)
}

pub(super) fn complete_scope_exit(ctx: *mut TaskCtx, status: u8, expired: bool) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
    if !enabled(s) {
        return;
    }
    let path = s.tasks[unsafe { (*ctx).task_id } as usize]
        .as_ref()
        .unwrap()
        .path
        .clone();
    let chain = unsafe { (*ctx).failure.chain(&path) };
    let failures = if chain.is_empty() {
        Some(Vec::new())
    } else {
        unsafe { (*ctx).run_ref().failure_table }.map(|table| {
            chain
                .iter()
                .map(|(record, path)| (table.failure(record), path.clone()))
                .collect()
        })
    };
    complete_owner(ctx, "task.scope_exit", || OwnerOutcome::ScopeExit {
        status,
        expired,
    });
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let before = Stretch::new(driver.budgets);
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path] - 1;
    if let (Some(pending), Some(failures)) = (log.scope_exits.remove(&path), failures) {
        let mut events: Subevents = pending
            .members
            .into_iter()
            .map(|child| (CanonicalItem::Object(child), CanonicalOutcome::Unit))
            .collect();
        events.push((
            CanonicalItem::Object(pending.scope),
            CanonicalOutcome::ScopeExited {
                status,
                expired,
                failures,
            },
        ));
        let boundary = Boundary {
            task: path,
            ordinal,
            op: None,
            transition: Transition::Keep,
            retirement,
            before,
            credit: 0,
            native_outcome: None,
        };
        log.subevents(&boundary, "task.scope_exit", events);
    } else {
        log.unsupported.push(Unsupported {
            task: path,
            ordinal,
            op: Some("task.scope_exit"),
        });
    }
}

/// Fix generation-qualified join identities before its first head boundary.
/// No saved frame or result storage is retained by this binding.
pub(super) fn prepare_join(ctx: *mut TaskCtx, scope: u64, child: u64) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    if let (Some(scope), Some(child)) = (log.scopes.get(&scope), log.tasks.get(&child)) {
        assert!(
            log.joins
                .insert(path, (scope.clone(), child.clone()))
                .is_none()
        );
    }
}

fn join_subevents(log: &Log, path: &[u32]) -> Option<Subevents> {
    let (scope, child) = log.joins.get(path)?;
    Some(vec![
        (
            CanonicalItem::Object(scope.clone()),
            CanonicalOutcome::Alias(child.clone()),
        ),
        (
            CanonicalItem::Object(child.clone()),
            CanonicalOutcome::Joining,
        ),
    ])
}

/// Capture the consumed child's outcome before its generation-qualified row
/// is vacated. Results come from TaskFinish's owned initialized-field snapshot;
/// no join output or inactive failure carrier is read.
pub(super) fn complete_join(
    ctx: *mut TaskCtx,
    scope_word: u64,
    child_word: u64,
    chain: Option<&[crate::failure::Chained]>,
) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
    if !enabled(s) {
        return;
    }
    let row = s.tasks[child_word as u32 as usize]
        .as_ref()
        .expect("joined child before retirement");
    assert_eq!(row.generation, (child_word >> 32) as u32);
    let child_path = row.path.clone();
    let rank = row.completed;
    let Logical::Ordered(driver) = &s.logical else {
        unreachable!()
    };
    let log = driver.events.as_ref().unwrap();
    let scope = log.scopes.get(&scope_word).cloned();
    let child = log.tasks.get(&child_word).cloned();
    let status = if chain.is_some() {
        crate::STATUS_FAILURE
    } else {
        crate::STATUS_OK
    };
    let payload = if chain.is_none() {
        log.task_results
            .get(&child_path)
            .and_then(|result| result.payload.clone())
    } else {
        None
    };
    let failures = match chain {
        None => Some(Vec::new()),
        Some(chain) => unsafe { (*ctx).run_ref().failure_table }.map(|table| {
            chain
                .iter()
                .map(|(record, path)| (table.failure(record), path.clone()))
                .collect()
        }),
    };
    // Completion runs in the caller's current head turn. There is no
    // language computation or budget charge between resume and this observer,
    // and no extra scheduling point is introduced to publish the outcome.
    let path = s.tasks[unsafe { (*ctx).task_id } as usize]
        .as_ref()
        .unwrap()
        .path
        .clone();
    let ordinal = log.ordinals[&path] - 1;
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let before = Stretch::new(driver.budgets);
    let canonical = match (&scope, &child, &failures) {
        (Some(scope), Some(child), Some(failures)) if chain.is_some() || payload.is_some() => {
            Some(vec![
                (
                    CanonicalItem::Object(child.clone()),
                    CanonicalOutcome::Joined {
                        status,
                        rank,
                        payload: payload.clone(),
                        failures: failures.clone(),
                    },
                ),
                (CanonicalItem::Object(scope.clone()), CanonicalOutcome::Unit),
            ])
        }
        _ => None,
    };
    complete_owner(ctx, "task.join", || OwnerOutcome::Join {
        scope,
        child,
        child_path,
        status,
        rank,
        payload,
        failures,
    });
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    log.joins.remove(&path);
    if let Some(events) = canonical {
        let boundary = Boundary {
            task: path,
            ordinal,
            op: None,
            transition: Transition::Keep,
            retirement,
            before,
            credit: 0,
            native_outcome: None,
        };
        log.subevents(&boundary, "task.join", events);
    } else {
        log.unsupported.push(Unsupported {
            task: path,
            ordinal,
            op: Some("task.join"),
        });
    }
}

pub(super) fn prepare_await_any(ctx: *mut TaskCtx, scope_word: u64, set_word: u64) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let owner = unsafe { (*ctx).task_id };
    let path = s.task(owner).path.clone();
    let scope = s.scope_handle(scope_word);
    let set = s.set_handle(set_word, scope);
    let words = s.set(set).members.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let evidence = log
        .scopes
        .get(&scope_word)
        .cloned()
        .zip(log.sets.get(&set_word).cloned())
        .and_then(|(scope, set)| {
            let members = words
                .iter()
                .enumerate()
                .filter_map(|(index, word)| word.map(|word| (index, word)))
                .map(|(index, word)| log.tasks.get(&word).cloned().map(|id| (index as u64, id)))
                .collect::<Option<Vec<_>>>()?;
            Some(AwaitAnyEvidence {
                scope_word,
                set_word,
                scope,
                set,
                members,
            })
        });
    if let Some(evidence) = evidence {
        assert!(log.awaiting.insert(path, evidence).is_none());
    }
}

fn await_any_subevents(log: &Log, path: &[u32], initial: bool) -> Option<Subevents> {
    let pending = log.awaiting.get(path)?;
    let mut events = vec![(
        CanonicalItem::Object(pending.set.clone()),
        if initial {
            CanonicalOutcome::Awaiting {
                members: pending.members.clone(),
            }
        } else {
            CanonicalOutcome::Unit
        },
    )];
    if initial {
        events.extend(
            pending
                .members
                .iter()
                .map(|(_, child)| (CanonicalItem::Object(child.clone()), CanonicalOutcome::Unit)),
        );
    }
    Some(events)
}

/// Fix the chosen member and its actual completion rank in the owner's
/// return window. Neither a later completion nor row recycling may rewrite
/// this observation. Native table words stay inside the lookup maps.
pub(super) fn complete_await_any(ctx: *mut TaskCtx, set_word: u64, index: Option<u64>) {
    // SAFETY: the owner has resumed under the role; this window cannot yield.
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &s.logical else {
        return;
    };
    if driver.events.is_none() {
        return;
    }
    let selected = index.map(|index| {
        let word = s.set(set_word as u32).members[index as usize].expect("a selected set member");
        let row = s.task(word as u32);
        assert_eq!(row.generation, (word >> 32) as u32);
        (word, row.completed)
    });
    let Logical::Ordered(driver) = &s.logical else {
        unreachable!()
    };
    let log = driver.events.as_ref().unwrap();
    let outcome = OwnerOutcome::AwaitAny {
        set: log.sets[&set_word].clone(),
        index,
        member: selected.map(|(word, _)| log.tasks[&word].clone()),
        rank: selected.map(|(_, rank)| rank),
    };
    let path = s.tasks[unsafe { (*ctx).task_id } as usize]
        .as_ref()
        .unwrap()
        .path
        .clone();
    let OwnerOutcome::AwaitAny { member, rank, .. } = &outcome else {
        unreachable!()
    };
    let member = member.clone();
    let rank = *rank;
    complete_owner(ctx, "task.await_any", || outcome);
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let before = Stretch::new(driver.budgets);
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path] - 1;
    if let Some(pending) = log.awaiting.remove(&path) {
        let mut events = vec![(
            CanonicalItem::Object(pending.set.clone()),
            CanonicalOutcome::Selected {
                index,
                member: member.clone(),
                rank,
            },
        )];
        if let Some(child) = member {
            assert!(
                pending
                    .members
                    .iter()
                    .any(|(slot, id)| Some(*slot) == index && id == &child)
            );
            events.push((
                CanonicalItem::Object(child),
                CanonicalOutcome::Alias(pending.set),
            ));
            events.push((
                CanonicalItem::CompletionRank(pending.scope),
                CanonicalOutcome::Scalar(
                    rank.expect("selected member rank").to_le_bytes().to_vec(),
                ),
            ));
        }
        let boundary = Boundary {
            task: path,
            ordinal,
            op: None,
            transition: Transition::Keep,
            retirement,
            before,
            credit: 0,
            native_outcome: None,
        };
        log.subevents(&boundary, "task.await_any", events);
    } else {
        log.unsupported.push(Unsupported {
            task: path,
            ordinal,
            op: Some("task.await_any"),
        });
    }
}

/// Capture a task-local terminal cause while its owner and immutable failure
/// metadata are still live. Unknown blocked targets cannot certify a stop.
/// # Safety
/// The bounded strict-head callback owns the role and ctx's failure state.
pub(super) unsafe fn terminal(ctx: *mut TaskCtx, stop: &crate::stop::Stop) -> Option<Subevents> {
    let outcome = match stop {
        crate::stop::Stop::Blocked(_) => return None,
        crate::stop::Stop::Failed => {
            let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
            let path = &s.tasks[unsafe { (*ctx).task_id } as usize].as_ref()?.path;
            let chain = unsafe { (*ctx).failure.chain(path) };
            if chain.is_empty() {
                return None;
            }
            let table = unsafe { (*ctx).run_ref().failure_table }?;
            CanonicalOutcome::Failures(
                chain
                    .iter()
                    .map(|(record, path)| (table.failure(record), path.clone()))
                    .collect(),
            )
        }
        _ => CanonicalOutcome::Stopped(stop.clone().outcome()),
    };
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &mut s.logical else {
        return None;
    };
    let log = driver.events.as_mut()?;
    let mut events = log.terminal_hosts().ok()?;
    events.push((CanonicalItem::Terminal, outcome));
    Some(events)
}

/// Final root status after mandatory returned-result cleanup, using the
/// initialized result snapshot taken before the reporting sink can yield.
/// # Safety
/// The root owns the role, its failure state and immutable run metadata.
pub(super) unsafe fn root_end(ctx: *mut TaskCtx, status: u8) -> Option<Subevents> {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
    let Logical::Ordered(driver) = &s.logical else {
        return None;
    };
    let log = driver.events.as_ref()?;
    let result = match log.task_results.get(&Vec::new()) {
        Some(result) => Some(result.payload.clone()?),
        None => None,
    };
    let chain = unsafe { (*ctx).failure.chain(&[]) };
    let failures = if chain.is_empty() {
        Vec::new()
    } else {
        let table = unsafe { (*ctx).run_ref().failure_table }?;
        chain
            .iter()
            .map(|(record, path)| (table.failure(record), path.clone()))
            .collect()
    };
    Some(vec![(
        CanonicalItem::Root,
        CanonicalOutcome::RootEnd {
            status,
            result,
            failures,
        },
    )])
}

/// Snapshot a failing end after its actual rank/cancellation effects. The
/// end writer consumes this evidence under the original end ordinal, even
/// when the failure arose in a called Holding cleanup after other operations.
/// # Safety
/// The bounded end callback owns the role and the run's immutable metadata.
pub(super) unsafe fn failed_end(
    ctx: *mut TaskCtx,
    s: &mut super::super::Scheduler,
    task: u32,
    cancellations: Subevents,
) {
    if !enabled(s) {
        return;
    }
    let row = s.tasks[task as usize].as_ref().unwrap();
    let path = row.path.clone();
    let canonical = (|| {
        let Logical::Ordered(driver) = &s.logical else {
            unreachable!()
        };
        let log = driver.events.as_ref().unwrap();
        let object = log
            .tasks
            .get(&super::super::handle(task, row.generation))?
            .clone();
        let scope = s.scopes[row.scope? as usize].as_ref()?;
        let scope_id = log
            .scopes
            .get(&super::super::handle(row.scope?, scope.generation))?
            .clone();
        let table = unsafe { (*ctx).run_ref().failure_table }?;
        let failures = row
            .failure
            .as_ref()?
            .iter()
            .map(|(record, path)| (table.failure(record), path.clone()))
            .collect();
        let mut events = vec![
            (
                CanonicalItem::Object(object),
                CanonicalOutcome::TaskFailed { failures },
            ),
            (
                CanonicalItem::CompletionRank(scope_id.clone()),
                CanonicalOutcome::Scalar(row.completed.to_le_bytes().to_vec()),
            ),
            (
                CanonicalItem::ScopeCancellation(scope_id),
                CanonicalOutcome::Scalar(
                    scope
                        .canceled
                        .cause()
                        .map_or(0, |cause| cause + 1)
                        .to_le_bytes()
                        .to_vec(),
                ),
            ),
        ];
        events.extend(cancellations);
        Some(events)
    })();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    assert!(
        driver
            .events
            .as_mut()
            .unwrap()
            .failed_ends
            .insert(path, canonical)
            .is_none()
    );
}

/// The bounded end effect has already fixed result readiness and rank. A
/// cleanup continuation stays under its original end ordinal, and never
/// reads a result slot that a join may already have moved.
fn end_subevents(
    s: &super::super::Scheduler,
    task: TaskId,
    continuing: bool,
    holding: bool,
) -> Option<Subevents> {
    let row = s.tasks[task as u32 as usize].as_ref()?;
    let Logical::Ordered(driver) = &s.logical else {
        return None;
    };
    let log = driver.events.as_ref()?;
    if row.failure.is_some() {
        return log.failed_ends.get(&row.path)?.clone();
    }
    let object = log.tasks.get(&task)?.clone();
    let result = if continuing {
        None
    } else {
        Some(log.task_results.get(&row.path)?.payload.clone()?)
    };
    let mut events = vec![(
        CanonicalItem::Object(object),
        CanonicalOutcome::TaskEnd { holding, result },
    )];
    if !continuing {
        let scope_index = row.scope?;
        let scope = s.scopes[scope_index as usize].as_ref()?;
        let id = log
            .scopes
            .get(&super::super::handle(scope_index, scope.generation))?
            .clone();
        events.push((
            CanonicalItem::CompletionRank(id),
            CanonicalOutcome::Scalar(row.completed.to_le_bytes().to_vec()),
        ));
    }
    Some(events)
}

pub(super) fn record(
    ctx: *mut TaskCtx,
    task: TaskId,
    operation: Option<(&Operation<Command>, &Outcome)>,
    retirement: Retirement<'_>,
) {
    let canonical_before = charges::retirement(ctx, task, retirement.before, retirement.credit);
    // Default execution pays no summary, path or carrier copy. The role owns
    // this lookup, even while the frontier is moved out for a bounded effect.
    // SAFETY: the bounded retirement callback already owns the role. A
    // stop-aware scheduler lookup could abandon the fiber while Frontier
    // lives in with_frontier's local, before restoration. Do not re-enter.
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &s.logical else {
        // Standalone native-adapter tests can drive their own Frontier
        // without installing a logical policy or evidence collector.
        return;
    };
    if driver.events.is_none() {
        return;
    }
    assert_eq!(s.task(task as u32).generation, (task >> 32) as u32);
    let path = s.task(task as u32).path.clone();
    let transition = match retirement.transition {
        Next::Keep => Transition::Keep,
        Next::Yield => Transition::Yield,
        Next::Park => Transition::Park,
        Next::Hold => Transition::Hold,
        Next::End => Transition::End,
        Next::Stop => Transition::Stop,
        Next::Call {
            members,
            then_yield,
        } => Transition::Call {
            members: members
                .iter()
                .map(|&id| {
                    assert_eq!(s.task(id as u32).generation, (id >> 32) as u32);
                    s.task(id as u32).path.clone()
                })
                .collect(),
            then_yield: *then_yield,
        },
    };
    let receive = {
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        let log = driver.events.as_mut().unwrap();
        if matches!(
            operation.map(|(op, _)| &op.payload),
            Some(Command::Head {
                name: "channel.receive"
            })
        ) {
            log.receive_entries.remove(&path).flatten()
        } else if operation.is_none() && log.last_ops.get(&path) == Some(&"channel.receive") {
            log.receivers.get(&path).map(|(channel, _)| {
                vec![(
                    CanonicalItem::Object(channel.clone()),
                    CanonicalOutcome::Receiving,
                )]
            })
        } else {
            None
        }
    };
    let cancel_all = {
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        let log = driver.events.as_mut().unwrap();
        if matches!(
            operation.map(|(op, _)| &op.payload),
            Some(Command::Head {
                name: "task.cancel_all"
            })
        ) {
            cancel_all_subevents(log, &path, true)
        } else if operation.is_none() && log.last_ops.get(&path) == Some(&"task.cancel_all") {
            cancel_all_subevents(log, &path, false)
        } else {
            None
        }
    };
    let canonical = receive.or(cancel_all).or_else(|| {
        operation.and_then(|(op, result)| {
            if let Some(events) = &result.canonical {
                return Some(events.clone());
            }
            if matches!(
                op.payload,
                Command::Head {
                    name: "task.check_canceled"
                }
            ) {
                assert_eq!(
                    result.image.len(),
                    4,
                    "cancellation has a captured u32 result"
                );
                return Some(vec![(
                    CanonicalItem::Cancellation(path.clone()),
                    CanonicalOutcome::Scalar(result.image.clone()),
                )]);
            }
            let Logical::Ordered(driver) = &s.logical else {
                unreachable!()
            };
            let log = driver.events.as_ref().unwrap();
            match &op.payload {
                Command::ScopeEnter { .. } => {
                    let object = ObjectId {
                        creator: path.clone(),
                        ordinal: log.ordinals.get(&path).copied().unwrap_or(0),
                    };
                    return Some(vec![
                        (
                            CanonicalItem::Object(object.clone()),
                            CanonicalOutcome::Created(object.clone()),
                        ),
                        (
                            CanonicalItem::CompletionRank(object),
                            CanonicalOutcome::Scalar(0u64.to_le_bytes().to_vec()),
                        ),
                    ]);
                }
                Command::Head {
                    name: "task.scope_exit",
                } => return scope_exit_subevents(log, &path, true),
                Command::Head {
                    name: "task.await_any",
                } => return await_any_subevents(log, &path, true),
                Command::Head { name: "task.join" } => return join_subevents(log, &path),
                Command::Head { name: "task.end" } => {
                    return end_subevents(
                        s,
                        task,
                        false,
                        matches!(retirement.transition, Next::Hold),
                    );
                }
                Command::TaskSpawn { .. } => {
                    let word = u64::from_ne_bytes(
                        result
                            .image
                            .as_slice()
                            .try_into()
                            .expect("spawn returns task word"),
                    );
                    let child = s.tasks[word as u32 as usize]
                        .as_ref()
                        .expect("installed child");
                    assert_eq!(child.generation, (word >> 32) as u32);
                    let index = child.scope.expect("child scope");
                    let scope = s.scopes[index as usize].as_ref().expect("installed scope");
                    let scope = log
                        .scopes
                        .get(&super::super::handle(index, scope.generation))?
                        .clone();
                    let object = ObjectId {
                        creator: path.clone(),
                        ordinal: log.ordinals.get(&path).copied().unwrap_or(0),
                    };
                    let inherited = result
                        .inheritance
                        .expect("spawn fixed its parent cancellation read");
                    return Some(vec![
                        (
                            CanonicalItem::Object(object.clone()),
                            CanonicalOutcome::Spawned {
                                scope: scope.clone(),
                                path: child.path.clone(),
                            },
                        ),
                        (
                            CanonicalItem::Object(scope),
                            CanonicalOutcome::Alias(object),
                        ),
                        (
                            CanonicalItem::Cancellation(path.clone()),
                            CanonicalOutcome::Scalar(
                                inherited
                                    .map_or(0, |cause| cause + 1)
                                    .to_le_bytes()
                                    .to_vec(),
                            ),
                        ),
                        (
                            CanonicalItem::Cancellation(child.path.clone()),
                            CanonicalOutcome::Scalar(
                                child
                                    .canceled
                                    .cause()
                                    .map_or(0, |cause| cause + 1)
                                    .to_le_bytes()
                                    .to_vec(),
                            ),
                        ),
                    ]);
                }
                Command::TaskSetCreate { .. } => {
                    let object = ObjectId {
                        creator: path.clone(),
                        ordinal: log.ordinals.get(&path).copied().unwrap_or(0),
                    };
                    return Some(vec![(
                        CanonicalItem::Object(object.clone()),
                        CanonicalOutcome::Created(object),
                    )]);
                }
                Command::TaskSetAdd { set, child, .. } => {
                    let object = log.sets.get(set)?.clone();
                    let child = log.tasks.get(child)?.clone();
                    return Some(vec![
                        (CanonicalItem::Object(object), CanonicalOutcome::Unit),
                        (CanonicalItem::Object(child), CanonicalOutcome::Unit),
                    ]);
                }
                Command::TaskSetClose { set } => {
                    return Some(vec![(
                        CanonicalItem::Object(log.sets.get(set)?.clone()),
                        CanonicalOutcome::Unit,
                    )]);
                }
                Command::Head {
                    name: "task.set_has",
                } => {
                    assert_eq!(result.image.len(), 9, "set_has captures its set and bool");
                    let set = u64::from_ne_bytes(result.image[..8].try_into().unwrap());
                    return Some(vec![(
                        CanonicalItem::Object(log.sets.get(&set)?.clone()),
                        CanonicalOutcome::Scalar(vec![result.image[8]]),
                    )]);
                }
                Command::Head {
                    name: "task.set_take",
                } => {
                    assert_eq!(
                        result.image.len(),
                        16,
                        "set_take captures its set and returned task"
                    );
                    let set = u64::from_ne_bytes(result.image[..8].try_into().unwrap());
                    let child = u64::from_ne_bytes(result.image[8..].try_into().unwrap());
                    let object = log.sets.get(&set)?.clone();
                    let child = log.tasks.get(&child)?.clone();
                    return Some(vec![
                        (
                            CanonicalItem::Object(object),
                            CanonicalOutcome::Alias(child.clone()),
                        ),
                        (
                            CanonicalItem::Object(child.clone()),
                            CanonicalOutcome::Alias(child),
                        ),
                    ]);
                }
                _ => {}
            }
            let items: std::collections::BTreeSet<_> = op
                .footprint
                .reads
                .iter()
                .chain(&op.footprint.writes)
                .collect();
            let [Item::Object(item)] = items.into_iter().collect::<Vec<_>>().as_slice() else {
                return None;
            };
            let outcome = match &op.payload {
                Command::AtomicCreate { .. }
                | Command::MutexCreate { .. }
                | Command::ChannelOpen { .. } => CanonicalOutcome::Created(item.clone()),
                Command::SenderDup { .. } => CanonicalOutcome::Alias(item.clone()),
                Command::SenderClose { .. } => CanonicalOutcome::Released {
                    kind: crate::ordered::objects::Kind::Sender.code(),
                    leftovers: Vec::new(),
                },
                Command::AtomicLoad { .. } => CanonicalOutcome::Scalar(result.image.clone()),
                Command::AtomicStore { .. } => CanonicalOutcome::Unit,
                _ => return None,
            };
            assert_eq!(result.status, 0);
            Some(vec![(CanonicalItem::Object(item.clone()), outcome)])
        })
    });
    let canonical = canonical.or_else(|| {
        if operation.is_none() {
            let Logical::Ordered(driver) = &mut s.logical else {
                unreachable!()
            };
            let log = driver.events.as_mut()?;
            if let Some(ordinal) = log.ordinals.get(&path).and_then(|next| next.checked_sub(1))
                && let Some(events) =
                    log.continue_home(&(path.clone(), ordinal), &retirement.transition)
            {
                return Some(events);
            }
            if matches!(
                log.last_ops.get(&path),
                Some(&"host.request" | &"host.handle_drop")
            ) && matches!(retirement.transition, Next::Keep | Next::Yield)
            {
                let ordinal = log.ordinals.get(&path)?.checked_sub(1)?;
                if log.host_certified.contains(&(path.clone(), ordinal)) {
                    return Some(vec![(
                        CanonicalItem::HostOperation(ObjectId {
                            creator: path.clone(),
                            ordinal,
                        }),
                        CanonicalOutcome::HostFinished,
                    )]);
                }
            }
            if log.last_ops.get(&path) == Some(&"task.scope_enter") {
                let ordinal = log.ordinals.get(&path)?.checked_sub(1)?;
                let deadline = log
                    .deadlines
                    .values()
                    .find(|deadline| deadline.path == path && deadline.ordinal == ordinal)?;
                return Some(vec![(
                    CanonicalItem::Object(deadline.scope.clone()),
                    CanonicalOutcome::Unit,
                )]);
            }
            if log.last_ops.get(&path) == Some(&"task.await_any") {
                return await_any_subevents(log, &path, false);
            }
            if log.last_ops.get(&path) == Some(&"task.scope_exit") {
                return scope_exit_subevents(log, &path, false);
            }
            if log.last_ops.get(&path) == Some(&"task.join") {
                return join_subevents(log, &path);
            }
            if matches!(retirement.transition, Next::End) && log.end_ordinals.contains_key(&path) {
                return end_subevents(s, task, true, matches!(retirement.transition, Next::Hold));
            }
        }
        None
    });
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    log.current_retirement = retirement.index;
    let next = log.ordinals.entry(path.clone()).or_default();
    let ordinal = if let Some((op, _)) = operation {
        let ordinal = *next;
        *next = next.checked_add(1).expect("operation ordinal overflow");
        log.last_ops.insert(path.clone(), op.descriptor.name);
        if op.descriptor.name == "task.end"
            && matches!(retirement.transition, Next::Hold | Next::End)
        {
            assert!(
                log.end_ordinals.insert(path.clone(), ordinal).is_none(),
                "one task-end entry"
            );
        }
        ordinal
    } else if matches!(retirement.transition, Next::End)
        && let Some(&ordinal) = log.end_ordinals.get(&path)
    {
        ordinal
    } else {
        next.checked_sub(1)
            .expect("a continuation follows an operation")
    };
    if let Some((
        Operation {
            payload: Command::TaskSpawn { .. },
            ..
        },
        result,
    )) = operation
    {
        let word = u64::from_ne_bytes(
            result
                .image
                .as_slice()
                .try_into()
                .expect("spawn returns one task word"),
        );
        let object = ObjectId {
            creator: path.clone(),
            ordinal,
        };
        assert!(
            log.tasks.insert(word, object.clone()).is_none(),
            "one identity per task generation"
        );
    }
    if let Some((
        Operation {
            payload: Command::ScopeEnter {
                index, generation, ..
            },
            ..
        },
        _,
    )) = operation
    {
        assert!(
            log.scopes
                .insert(
                    super::super::handle(*index, *generation),
                    ObjectId {
                        creator: path.clone(),
                        ordinal
                    }
                )
                .is_none(),
            "one identity per scope generation"
        );
    }
    if let Some((
        Operation {
            payload: Command::TaskSetCreate {
                index, generation, ..
            },
            ..
        },
        _,
    )) = operation
    {
        let word = crate::channels::ENDPOINT
            | super::super::TASK_SET
            | super::super::handle(*index, *generation);
        assert!(
            log.sets
                .insert(
                    word,
                    ObjectId {
                        creator: path.clone(),
                        ordinal
                    }
                )
                .is_none(),
            "one identity per task-set generation"
        );
    }
    if let Some(events) = canonical {
        let name = operation.map_or_else(
            || {
                if matches!(retirement.transition, Next::End)
                    && log.end_ordinals.get(&path) == Some(&ordinal)
                {
                    "task.end"
                } else {
                    log.last_ops[&path]
                }
            },
            |(op, _)| op.descriptor.name,
        );
        let boundary = Boundary {
            task: path.clone(),
            ordinal,
            op: operation.map(|(op, _)| op.descriptor.name),
            transition: transition.clone(),
            retirement: retirement.index,
            before: canonical_before
                .clone()
                .unwrap_or_else(|| retirement.before.clone()),
            credit: retirement.credit,
            native_outcome: None,
        };
        log.subevents(&boundary, name, events);
    } else if operation
        .is_some_and(|(op, _)| matches!(op.descriptor.name, "host.request" | "host.handle_drop"))
    {
        assert!(
            log.host_entries
                .insert(
                    (path.clone(), ordinal),
                    Boundary {
                        task: path.clone(),
                        ordinal,
                        op: operation.map(|(op, _)| op.descriptor.name),
                        transition: transition.clone(),
                        retirement: retirement.index,
                        before: retirement.before.clone(),
                        credit: retirement.credit,
                        native_outcome: None,
                    }
                )
                .is_none()
        );
    } else {
        log.unsupported.push(Unsupported {
            task: path.clone(),
            ordinal,
            op: operation.map(|(op, _)| op.descriptor.name),
        });
    }
    if log
        .cancel_all
        .get(&path)
        .is_some_and(|pending| pending.closed)
    {
        log.cancel_all.remove(&path);
    }
    if matches!(retirement.transition, Next::End) {
        log.failed_ends.remove(&path);
    }
    if operation.is_none()
        && matches!(retirement.transition, Next::Keep | Next::Yield)
        && matches!(
            log.last_ops.get(&path),
            Some(&"host.request" | &"host.handle_drop")
        )
    {
        log.host_certified.remove(&(path.clone(), ordinal));
    }
    log.boundaries.push(Boundary {
        task: path,
        ordinal,
        op: operation.map(|(op, _)| op.descriptor.name),
        transition,
        retirement: retirement.index,
        before: retirement.before.clone(),
        credit: retirement.credit,
        native_outcome: operation.map(|(_, result)| (result.status, result.image.clone())),
    });
}

#[cfg(test)]
mod terminal_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ordered::Policy;

    unsafe extern "C" fn held_set_member(
        ctx: *mut TaskCtx,
        env: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        if unsafe { crate::alloc::payload(env).cast::<u64>().read() } != 0 {
            use crate::tasks::*;
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let waiting = {
                    let _guard = unsafe { role::Guard::new(ctx) };
                    let s = scheduler(ctx);
                    matches!(s.task(0).wait, Some(Wait::Any { .. })) && readiness::saved(s, 0)
                };
                if waiting {
                    break;
                }
                assert!(
                    std::time::Instant::now() < until,
                    "await_any never published its saved wait"
                );
                let mut cause = 0;
                assert_eq!(
                    unsafe { hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
                    crate::STATUS_OK
                );
            }
        }
        let mut result = [1u64, 42];
        unsafe {
            crate::tasks::hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                u32::MAX,
            )
        }
    }

    #[test]
    fn await_any_completion_keeps_one_ordinal_across_a_saved_wait_and_records_the_rank() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&environment as *const crate::Layout];
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let output = Arc::new(Mutex::new((Vec::new(), Vec::new())));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        super::super::engine::enable(ctx, policy);
                        enable(ctx);
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0
                            ),
                            crate::STATUS_OK
                        );
                        let mut set = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope),
                            crate::STATUS_OK
                        );
                        let mut answer = IndexOption {
                            tag: 99,
                            padding: 99,
                            index: 99,
                        };
                        assert_eq!(
                            hirsel_rt_v2_task_await_any(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut answer,
                                &scope,
                                &set
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(answer.tag, 0);
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env).cast::<u64>().write(1);
                        let mut child = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                held_set_member,
                                env,
                                0
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(
                            hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut child),
                            crate::STATUS_OK
                        );
                        assert_eq!(
                            hirsel_rt_v2_task_await_any(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut answer,
                                &scope,
                                &set
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!((answer.tag, answer.index), (1, 0));
                        let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                        let completions = &log.owner_completions;
                        assert_eq!(completions.len(), 2);
                        let set_id = ObjectId {
                            creator: vec![],
                            ordinal: 1,
                        };
                        assert_eq!(
                            completions[0],
                            OwnerCompletion {
                                task: vec![],
                                ordinal: 2,
                                op: "task.await_any",
                                outcome: OwnerOutcome::AwaitAny {
                                    set: set_id.clone(),
                                    index: None,
                                    member: None,
                                    rank: None
                                }
                            }
                        );
                        assert_eq!(
                            completions[1],
                            OwnerCompletion {
                                task: vec![],
                                ordinal: 5,
                                op: "task.await_any",
                                outcome: OwnerOutcome::AwaitAny {
                                    set: set_id,
                                    index: Some(0),
                                    member: Some(ObjectId {
                                        creator: vec![],
                                        ordinal: 3
                                    }),
                                    rank: Some(0)
                                }
                            }
                        );
                        assert!(log.boundaries.iter().any(|row| row.task.is_empty()
                            && row.ordinal == 5
                            && row.op.is_none()
                            && row.transition == Transition::Park));
                        assert_eq!(
                            log.boundaries
                                .iter()
                                .filter(
                                    |row| row.task.is_empty() && row.op == Some("task.await_any")
                                )
                                .count(),
                            2,
                            "retries add no operation ordinal"
                        );
                        // The same held child is immediately ready on the
                        // next await. This adds an op, but no saved park.
                        assert_eq!(
                            hirsel_rt_v2_task_await_any(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut answer,
                                &scope,
                                &set
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!((answer.tag, answer.index), (1, 0));
                        answer.tag = 0;
                        answer.index = 99;
                        assert_eq!((answer.tag, answer.index), (0, 99));
                        let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                        let rows: Vec<_> = log
                            .canonical
                            .iter()
                            .filter(|row| row.op == "task.await_any")
                            .cloned()
                            .collect();
                        assert_eq!(rows.len(), 13);
                        assert!(log.awaiting.is_empty());
                        assert!(
                            log.unsupported
                                .iter()
                                .all(|row| row.op != Some("task.await_any"))
                        );
                        let child_id = ObjectId {
                            creator: vec![],
                            ordinal: 3,
                        };
                        let set_id = ObjectId {
                            creator: vec![],
                            ordinal: 1,
                        };
                        for (ordinal, expected) in [(2, 2), (5, 6), (6, 5)] {
                            let group: Vec<_> =
                                rows.iter().filter(|row| row.ordinal == ordinal).collect();
                            assert_eq!(group.len(), expected);
                            for (subevent, row) in group.iter().enumerate() {
                                assert!(row.task.is_empty());
                                assert_eq!(row.subevent as usize, subevent);
                            }
                            assert_eq!(group[0].transition, Transition::Yield);
                            assert_eq!(
                                group[0].outcome,
                                CanonicalOutcome::Awaiting {
                                    members: if ordinal == 2 {
                                        Vec::new()
                                    } else {
                                        vec![(0, child_id.clone())]
                                    },
                                }
                            );
                            let selected = group
                                .iter()
                                .find(|row| {
                                    matches!(row.outcome, CanonicalOutcome::Selected { .. })
                                })
                                .unwrap();
                            assert_eq!(selected.item, CanonicalItem::Object(set_id.clone()));
                            assert_eq!(selected.transition, Transition::Keep);
                            assert_eq!(
                                selected.outcome,
                                if ordinal == 2 {
                                    CanonicalOutcome::Selected {
                                        index: None,
                                        member: None,
                                        rank: None,
                                    }
                                } else {
                                    CanonicalOutcome::Selected {
                                        index: Some(0),
                                        member: Some(child_id.clone()),
                                        rank: Some(0),
                                    }
                                }
                            );
                            assert_eq!(
                                group
                                    .iter()
                                    .filter(|row| row.transition == Transition::Park)
                                    .count(),
                                usize::from(ordinal == 5)
                            );
                            if ordinal != 2 {
                                assert_eq!(
                                    group.last().unwrap().item,
                                    CanonicalItem::CompletionRank(ObjectId {
                                        creator: vec![],
                                        ordinal: 0
                                    })
                                );
                                assert_eq!(
                                    group.last().unwrap().outcome,
                                    CanonicalOutcome::Scalar(0u64.to_le_bytes().to_vec())
                                );
                            }
                        }
                        assert_eq!(log.owner_completions.len(), 3);
                        *seen.lock().unwrap() = (log.owner_completions.clone(), rows);
                        assert_eq!(
                            crate::finalize::hirsel_rt_v2_close_handle(ctx, set, 0, 0),
                            crate::STATUS_OK
                        );
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                            crate::STATUS_OK
                        );
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let completions = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&completions, reference);
                } else {
                    reference = Some(completions);
                }
            }
        }
    }

    #[test]
    fn native_set_subevents_keep_presence_and_returned_spawn_paths_across_widths() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&environment as *const crate::Layout];
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let output = Arc::new(Mutex::new(Vec::new()));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        super::super::engine::enable(ctx, policy);
                        enable(ctx);
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0
                            ),
                            crate::STATUS_OK
                        );
                        {
                            let _guard = role::Guard::new(ctx);
                            crate::tasks::role::with_role(ctx, |s| {
                                s.sets.resize_with(width as usize, || None)
                            });
                        }
                        let mut set = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope),
                            crate::STATUS_OK
                        );
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env).cast::<u64>().write(0);
                        let mut child = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                held_set_member,
                                env,
                                0
                            ),
                            crate::STATUS_OK
                        );
                        let mut has = 99;
                        assert_eq!(
                            hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, 0),
                            crate::STATUS_OK
                        );
                        assert_eq!(has, 0);
                        assert_eq!(
                            hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut child),
                            crate::STATUS_OK
                        );
                        assert_eq!(
                            hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, 0),
                            crate::STATUS_OK
                        );
                        assert_eq!(has, 1);
                        let mut taken = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_set_take(
                                ctx,
                                std::ptr::null_mut(),
                                &mut taken,
                                &scope,
                                &mut set,
                                0
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(taken, child);
                        assert_eq!(
                            hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, 0),
                            crate::STATUS_OK
                        );
                        assert_eq!(has, 0);
                        assert_eq!(
                            crate::finalize::hirsel_rt_v2_close_handle(ctx, set, 0, 0),
                            crate::STATUS_OK
                        );
                        let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                        assert!(
                            log.canonical().is_err(),
                            "unclassified task lifecycle still rejects the complete trace"
                        );
                        let events = &log.canonical;
                        assert_eq!(events.len(), 15);
                        let set_id = CanonicalItem::Object(ObjectId {
                            creator: vec![],
                            ordinal: 1,
                        });
                        let set_rows: Vec<_> =
                            events.iter().filter(|row| row.item == set_id).collect();
                        assert_eq!(set_rows.len(), 7);
                        assert_eq!(
                            set_rows.iter().map(|row| row.version).collect::<Vec<_>>(),
                            [1, 2, 3, 4, 5, 6, 7]
                        );
                        assert_eq!(set_rows[1].outcome, CanonicalOutcome::Scalar(vec![0]));
                        assert_eq!(set_rows[3].outcome, CanonicalOutcome::Scalar(vec![1]));
                        let child_id = ObjectId {
                            creator: vec![],
                            ordinal: 2,
                        };
                        assert_eq!(
                            set_rows[4].outcome,
                            CanonicalOutcome::Alias(child_id.clone())
                        );
                        assert_eq!(set_rows[5].outcome, CanonicalOutcome::Scalar(vec![0]));
                        let member_rows: Vec<_> = events
                            .iter()
                            .filter(|row| row.item == CanonicalItem::Object(child_id.clone()))
                            .collect();
                        assert_eq!(member_rows.len(), 3);
                        assert_eq!(
                            member_rows
                                .iter()
                                .map(|row| (row.version, row.subevent))
                                .collect::<Vec<_>>(),
                            [(1, 0), (2, 1), (3, 1)]
                        );
                        assert_eq!(member_rows[2].outcome, CanonicalOutcome::Alias(child_id));
                        *seen.lock().unwrap() = events.clone();
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                            crate::STATUS_OK
                        );
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let events = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&events, reference);
                } else {
                    reference = Some(events);
                }
            }
        }
    }

    #[test]
    fn native_creations_and_sender_aliases_normalize_perturbed_handle_allocations() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let image = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let close = crate::Step {
            kind: crate::STEP_CLOSE_HANDLE,
            field: 0,
            variant: 0,
            span: 0,
            offset: 0,
            stride: 0,
            count: 0,
            count_offset: 0,
            layout: std::ptr::null(),
        };
        let wrapper = crate::Layout {
            size: 8,
            flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
            steps: &close,
            step_count: 1,
            field_count: 1,
            ..image
        };
        let layouts = [&image as *const crate::Layout, &wrapper];
        let mut reference = None;
        let mut native_words = Vec::new();
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let output = Arc::new(Mutex::new(None));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = layouts.len() as u32;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        super::super::engine::enable(ctx, policy);
                        enable(ctx);
                    },
                    move |ctx| unsafe {
                        // Unpublished suffix reservations perturb every table
                        // word without introducing a language operation.
                        {
                            let _guard = role::Guard::new(ctx);
                            for _ in 0..width {
                                let _ = crate::tasks::role::with_role(ctx, |s| {
                                    s.channels.reserve_open()
                                });
                                let _ =
                                    crate::tasks::role::with_role(ctx, |s| s.sync.reserve_mutex());
                            }
                        }
                        let none = [0u64; 2];
                        let mut first = [0u64; 2];
                        super::super::engine::channel_open(
                            ctx,
                            first.as_mut_ptr(),
                            none.as_ptr().cast(),
                            2,
                        )
                        .unwrap();
                        let mut duplicate = 0;
                        super::super::engine::sender_dup(ctx, first[0], &mut duplicate).unwrap();
                        let mut second = [0u64; 2];
                        super::super::engine::channel_open(
                            ctx,
                            second.as_mut_ptr(),
                            none.as_ptr().cast(),
                            2,
                        )
                        .unwrap();
                        let mut duplicate_again = 0;
                        super::super::engine::sender_dup(ctx, duplicate, &mut duplicate_again)
                            .unwrap();
                        let mut payload = [1u64, 77];
                        let mut mutex = 0;
                        super::super::engine::mutex_create(
                            ctx,
                            &mut mutex,
                            payload.as_mut_ptr().cast(),
                            none.as_ptr().cast(),
                            2,
                            3,
                        )
                        .unwrap();
                        assert_eq!(payload, none);
                        let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                        let events = log.canonical().unwrap().to_vec();
                        assert_eq!(events.len(), 5);
                        let object = |ordinal| ObjectId {
                            creator: Vec::new(),
                            ordinal,
                        };
                        assert_eq!(
                            events.iter().map(|event| event.version).collect::<Vec<_>>(),
                            [1, 2, 1, 3, 1]
                        );
                        assert_eq!(
                            events
                                .iter()
                                .map(|event| event.outcome.clone())
                                .collect::<Vec<_>>(),
                            [
                                CanonicalOutcome::Created(object(0)),
                                CanonicalOutcome::Alias(object(0)),
                                CanonicalOutcome::Created(object(2)),
                                CanonicalOutcome::Alias(object(0)),
                                CanonicalOutcome::Created(object(4)),
                            ]
                        );
                        assert!(
                            events.iter().all(|event| event.subevent == 0
                                && event.transition == Transition::Yield)
                        );
                        *seen.lock().unwrap() = Some((events, first[0]));
                        // Release live values after inspecting the complete
                        // creation/alias prefix; drops have separate events.
                        for endpoint in first
                            .into_iter()
                            .chain(second)
                            .chain([duplicate, duplicate_again])
                        {
                            assert_eq!(
                                crate::finalize::hirsel_rt_v2_close_handle(ctx, endpoint, 0, 0),
                                crate::STATUS_OK
                            );
                        }
                        assert_eq!(
                            crate::roots::hirsel_rt_v2_root_drop(
                                ctx,
                                mutex as *mut crate::Header,
                                0,
                                0
                            ),
                            crate::STATUS_OK
                        );
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let (events, native_word) = output.lock().unwrap().take().unwrap();
                native_words.push(native_word);
                if let Some(reference) = &reference {
                    assert_eq!(&events, reference);
                } else {
                    reference = Some(events);
                }
            }
        }
        assert_ne!(
            native_words[0], native_words[2],
            "fixture changed native allocation"
        );
        assert_ne!(native_words[2], native_words[4]);
    }

    #[test]
    fn cancel_subevents_keep_descendant_causes_in_path_order_across_native_row_orders() {
        use crate::budget::stretch::{Budgets, Settled};
        use crate::ordered::Backend;
        use crate::tasks::{Scheduler, Scope, Task, handle, scheduler};
        let mut reference = None;
        for (order, observing) in [
            ([1, 2, 3, 4], true),
            ([4, 3, 1, 2], true),
            ([1, 2, 3, 4], false),
        ] {
            let mut ctx = std::ptr::null_mut();
            assert_eq!(
                unsafe { crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx) },
                0
            );
            let s = scheduler(ctx);
            let root = handle(0, s.task(0).generation);
            s.logical = Logical::Ordered(super::super::driver::Driver::new(
                root,
                Policy::admit(&Default::default(), None),
                Settled {
                    level: 0,
                    limit: u64::MAX,
                    fuel: u64::MAX,
                },
                Default::default(),
            ));
            if observing {
                enable(ctx);
            }
            let s = scheduler(ctx);
            s.tasks.resize_with(5, || None);
            let [target, first, second, unrelated] = order;
            for (index, path) in [
                (target, vec![0]),
                (first, vec![0, 0]),
                (second, vec![0, 1]),
                (unrelated, vec![1]),
            ] {
                s.tasks[index as usize] = Some(Task::new(
                    index + 7,
                    std::ptr::null_mut(),
                    Some(0),
                    path,
                    None,
                ));
            }
            s.scopes.push(Some(Scope {
                generation: 1,
                owner: 0,
                children: order.to_vec(),
                canceled: Default::default(),
                deadline: None,
                completions: Default::default(),
            }));
            s.link_child(0, target);
            s.link_child(0, unrelated);
            // Reverse links deliberately disagree with canonical path order.
            s.link_child(target, second);
            s.link_child(target, first);
            s.task(second)
                .canceled
                .cancel(crate::tasks::CANCEL_DEADLINE);
            let word = handle(target, s.task(target).generation);
            let op = Command::TaskCancel {
                scope: handle(0, 1),
                child: word,
            }
            .operation(Default::default());
            let mut ignored_record = |_, _, _: &Outcome, _| {};
            let mut objects = unsafe { super::super::Objects::deferred(ctx, &mut ignored_record) };
            let stretch = Stretch::new(Budgets {
                fuel: false,
                memory: false,
            });
            for retirement in 0..2 {
                let receipt = objects.commit(root, &op, false);
                if observing {
                    assert_eq!(
                        super::super::engine::driver(ctx)
                            .events
                            .as_ref()
                            .unwrap()
                            .canonical
                            .len(),
                        retirement as usize * 3,
                        "commit alone emits no event"
                    );
                    assert!(receipt.result.canonical.is_some());
                } else {
                    assert!(
                        receipt.result.canonical.is_none(),
                        "disabled trace retains no snapshot"
                    );
                }
                // Simulate row reuse after the effect. Recording must use the
                // immutable snapshot rather than consulting a native target.
                let old_path = std::mem::replace(&mut scheduler(ctx).task(first).path, vec![99]);
                objects.record_retirement(
                    root,
                    &op,
                    &receipt.result,
                    Retirement {
                        index: retirement,
                        before: &stretch,
                        transition: &receipt.next,
                        credit: 0,
                    },
                );
                scheduler(ctx).task(first).path = old_path;
            }
            assert_eq!(
                crate::tasks::role::with_role(ctx, |s| s.task(unrelated).canceled.cause()),
                None
            );
            if observing {
                let events = super::super::engine::driver(ctx)
                    .events
                    .as_ref()
                    .unwrap()
                    .canonical()
                    .unwrap()
                    .to_vec();
                assert_eq!(events.len(), 6);
                for (ordinal, chunk) in events.as_chunks::<3>().0.iter().enumerate() {
                    for (subevent, (row, (path, cause))) in chunk
                        .iter()
                        .zip([
                            (vec![0], crate::tasks::CANCEL_REQUESTED),
                            (vec![0, 0], crate::tasks::CANCEL_PARENT),
                            (vec![0, 1], crate::tasks::CANCEL_DEADLINE),
                        ])
                        .enumerate()
                    {
                        assert_eq!(row.item, CanonicalItem::Cancellation(path));
                        assert_eq!(
                            row.outcome,
                            CanonicalOutcome::Scalar((cause + 1).to_le_bytes().to_vec())
                        );
                        assert_eq!(row.ordinal, ordinal as u64);
                        assert_eq!(row.subevent, subevent as u32);
                        assert_eq!(row.version, ordinal as u64 + 1);
                    }
                }
                if let Some(reference) = &reference {
                    assert_eq!(&events, reference);
                } else {
                    reference = Some(events);
                }
            }
            // Synthetic rows have no physical contexts or ownership to destroy.
            let s: &mut Scheduler = scheduler(ctx);
            s.tasks.truncate(1);
            s.task(0).children.clear();
            s.scopes.clear();
            unsafe { crate::context::hirsel_rt_v2_ctx_destroy(ctx) };
        }
    }

    #[test]
    fn native_retirements_keep_owned_evidence_and_continuations_keep_the_ordinal() {
        for width in [1, 2, 4, 8] {
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                    enable(ctx);
                },
                move |ctx| {
                    let ((), progress) =
                        super::super::engine::head_effect(ctx, "host.request", || ((), Next::Keep))
                            .unwrap();
                    assert_eq!(
                        progress,
                        crate::ordered::Progress::Absorbed(super::super::engine::current(ctx))
                    );
                    let (_, progress) =
                        super::super::engine::head_continuation(ctx, || ((), Next::Yield)).unwrap();
                    super::super::engine::continue_at_head(ctx, progress).unwrap();
                    super::super::engine::head_effect(ctx, "task.end", || ((), Next::Keep))
                        .unwrap();
                    let driver = super::super::engine::driver(ctx);
                    let log = driver.events.as_ref().unwrap();
                    assert_eq!(log.boundaries.len(), 3);
                    let rows = &log.boundaries;
                    assert_eq!(
                        rows.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
                        [0, 0, 1]
                    );
                    assert_eq!(
                        rows.iter().map(|row| row.op).collect::<Vec<_>>(),
                        [Some("host.request"), None, Some("task.end")]
                    );
                    assert_eq!(rows[1].transition, Transition::Yield);
                    assert_eq!(rows[0].retirement, rows[1].retirement);
                    assert_eq!(rows[2].retirement, rows[1].retirement + 1);
                    assert!(
                        rows.iter()
                            .all(|row| row.task.is_empty() && row.credit == 0)
                    );
                    assert_eq!(rows[0].native_outcome, Some((0, Vec::new())));
                    assert_eq!(rows[1].native_outcome, None);
                    let rejected = log.canonical().unwrap_err();
                    assert_eq!(rejected.len(), 2, "continuation and end remain unsupported");
                    assert_eq!(
                        log.host_entries.len(),
                        1,
                        "entry awaits complete owned host evidence"
                    );
                    assert!(log.host_entries.contains_key(&(vec![], 0)));
                    assert_eq!(
                        rejected.len() + log.host_entries.len(),
                        3,
                        "every partial boundary is still rejected"
                    );
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }
    #[test]
    fn host_stop_during_recording_restores_the_frontier_before_abandonment() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        for enabled in [false, true] {
            let restored = Arc::new(AtomicBool::new(false));
            let proof = restored.clone();
            let mut run = crate::host::BodyRun::new(4, true);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                    if enabled {
                        enable(ctx);
                    }
                },
                move |ctx| unsafe {
                    super::super::engine::head_effect(ctx, "host.request", || {
                        (*ctx)
                            .run_ref()
                            .control
                            .request_inside(crate::stop::Stop::Requested);
                        ((), Next::Keep)
                    })
                    .unwrap();
                    // Still inside the same role-owned boundary. Inspect through
                    // raw projection before the explicit supported stop check.
                    let s = (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap();
                    let Logical::Ordered(driver) = &s.logical else {
                        unreachable!()
                    };
                    assert!(driver.frontier.is_some());
                    assert_eq!(driver.recorded.values().copied().sum::<u64>(), 1);
                    if enabled {
                        assert_eq!(driver.events.as_ref().unwrap().boundaries.len(), 1);
                    }
                    proof.store(true, Ordering::Release);
                    crate::stop::check(ctx, crate::stop::observe(ctx));
                    panic!("stopped task resumed");
                },
            );
            assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
            assert!(restored.load(Ordering::Acquire));
        }
    }
    #[test]
    fn native_atomic_events_normalize_identities_and_match_across_widths_and_reorder() {
        use std::sync::{Arc, Mutex};
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let result = Arc::new(Mutex::new(Vec::new()));
                let output = result.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        super::super::engine::enable(ctx, policy);
                        enable(ctx);
                    },
                    move |ctx| unsafe {
                        let initial = 0x1122334455667788u64.to_le_bytes();
                        let mut word = 0;
                        super::super::engine::atomic_create(
                            ctx,
                            &mut word,
                            initial.as_ptr(),
                            8,
                            0,
                            8,
                        )
                        .unwrap();
                        let mut loaded = [0u8; 8];
                        super::super::engine::atomic_load(ctx, word, loaded.as_mut_ptr(), 8)
                            .unwrap();
                        assert_eq!(loaded, initial);
                        super::super::engine::atomic_store(
                            ctx,
                            word,
                            9u64.to_le_bytes().as_ptr(),
                            8,
                        )
                        .unwrap();
                        super::super::engine::atomic_load(ctx, word, loaded.as_mut_ptr(), 8)
                            .unwrap();
                        assert_eq!(loaded, 9u64.to_le_bytes());
                        let driver = super::super::engine::driver(ctx);
                        let log = driver.events.as_ref().unwrap();
                        let events = log.canonical().expect("all four atomic events classified");
                        assert_eq!(events.len(), 4);
                        assert_eq!(
                            events.iter().map(|event| event.version).collect::<Vec<_>>(),
                            [1, 2, 3, 4]
                        );
                        assert_eq!(
                            events.iter().map(|event| event.ordinal).collect::<Vec<_>>(),
                            [0, 1, 2, 3]
                        );
                        let item = ObjectId {
                            creator: Vec::new(),
                            ordinal: 0,
                        };
                        assert!(
                            events
                                .iter()
                                .all(|event| event.item == CanonicalItem::Object(item.clone())
                                    && event.subevent == 0)
                        );
                        assert_eq!(events[0].outcome, CanonicalOutcome::Created(item));
                        assert_eq!(
                            events[1].outcome,
                            CanonicalOutcome::Scalar(initial.to_vec())
                        );
                        assert_eq!(events[2].outcome, CanonicalOutcome::Unit);
                        assert_eq!(
                            events[3].outcome,
                            CanonicalOutcome::Scalar(9u64.to_le_bytes().to_vec())
                        );
                        *output.lock().unwrap() = events.to_vec();
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                let events = result.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&events, reference);
                } else {
                    reference = Some(events);
                }
            }
        }
    }
    #[test]
    fn native_scope_and_spawn_events_use_creator_identities_and_effect_time_inheritance() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        for shield in [0, 1] {
            let mut reference = None;
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    let output = Arc::new(Mutex::new(Vec::new()));
                    let seen = output.clone();
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.config.layouts = layouts.as_ptr();
                    run.config.layout_count = 1;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            super::super::engine::enable(ctx, policy);
                            enable(ctx);
                        },
                        move |ctx| unsafe {
                            {
                                let _role = role::Guard::new(ctx);
                                let s = scheduler(ctx);
                                s.tasks.resize_with(width as usize + 1, || None);
                                s.scopes.resize_with(width as usize, || None);
                            }
                            let mut scope = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_enter(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut scope,
                                    &0
                                ),
                                crate::STATUS_OK
                            );
                            {
                                let _role = role::Guard::new(ctx);
                                (*ctx).shield = shield;
                                crate::tasks::role::with_role(ctx, |s| {
                                    s.cancel(0, CANCEL_REQUESTED)
                                });
                            }
                            let env = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                8,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(env).cast::<u64>().write(0);
                            let mut child = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_spawn(
                                    ctx,
                                    0,
                                    0,
                                    std::ptr::null_mut(),
                                    &mut child,
                                    &scope,
                                    held_set_member,
                                    env,
                                    0
                                ),
                                crate::STATUS_OK
                            );
                            {
                                let _role = role::Guard::new(ctx);
                                let s = scheduler(ctx);
                                // A later competing cause preserves the
                                // first-wins inheritance fixed at installation.
                                s.task(0).canceled.cancel(CANCEL_DEADLINE);
                                let Logical::Ordered(driver) = &s.logical else {
                                    unreachable!()
                                };
                                let rows: Vec<_> = driver
                                    .events
                                    .as_ref()
                                    .unwrap()
                                    .canonical
                                    .iter()
                                    .filter(|row| row.task.is_empty())
                                    .cloned()
                                    .collect();
                                assert_eq!(rows.len(), 6);
                                let scope_id = ObjectId {
                                    creator: vec![],
                                    ordinal: 0,
                                };
                                let child_id = ObjectId {
                                    creator: vec![],
                                    ordinal: 1,
                                };
                                assert_eq!(
                                    rows[0].outcome,
                                    CanonicalOutcome::Created(scope_id.clone())
                                );
                                assert_eq!(
                                    rows[1].item,
                                    CanonicalItem::CompletionRank(scope_id.clone())
                                );
                                assert_eq!(rows[1].version, 1);
                                assert_eq!(rows[2].item, CanonicalItem::Object(child_id.clone()));
                                assert_eq!(rows[2].version, 1);
                                assert_eq!(
                                    rows[2].outcome,
                                    CanonicalOutcome::Spawned {
                                        scope: scope_id.clone(),
                                        path: vec![0]
                                    }
                                );
                                assert_eq!(
                                    (rows[3].item.clone(), rows[3].version),
                                    (CanonicalItem::Object(scope_id), 2)
                                );
                                assert_eq!(rows[3].outcome, CanonicalOutcome::Alias(child_id));
                                assert_eq!(rows[4].item, CanonicalItem::Cancellation(vec![]));
                                assert_eq!(rows[5].item, CanonicalItem::Cancellation(vec![0]));
                                assert_eq!(
                                    rows[4].outcome,
                                    CanonicalOutcome::Scalar(
                                        if shield == 0 { CANCEL_REQUESTED + 1 } else { 0 }
                                            .to_le_bytes()
                                            .to_vec()
                                    )
                                );
                                assert_eq!(
                                    rows[5].outcome,
                                    CanonicalOutcome::Scalar(
                                        if shield == 0 { CANCEL_PARENT + 1 } else { 0 }
                                            .to_le_bytes()
                                            .to_vec()
                                    )
                                );
                                *seen.lock().unwrap() = rows;
                            }
                            assert_eq!(
                                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                                crate::STATUS_OK
                            );
                            crate::STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                    let rows = output.lock().unwrap().clone();
                    if let Some(reference) = &reference {
                        assert_eq!(&rows, reference);
                    } else {
                        reference = Some(rows);
                    }
                }
            }
        }
    }

    unsafe extern "C" fn typed_result_child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        if unsafe { crate::alloc::payload(environment).cast::<u64>().read() } != 0 {
            let mut cause = 99;
            assert_eq!(
                unsafe { crate::tasks::hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
                crate::STATUS_OK
            );
            assert_eq!(cause, 0);
        }
        let mut result = [1u64, 42];
        assert_eq!(
            unsafe {
                crate::tasks::hirsel_rt_v2_task_finish(
                    ctx,
                    result.as_mut_ptr().cast(),
                    [0u64; 2].as_ptr().cast(),
                    16,
                    3,
                )
            },
            crate::STATUS_OK
        );
        // A cleanup operation has a later ordinal; actual completion still
        // belongs to the original task.end, with its next sub-event index.
        let mut cause = 99;
        assert_eq!(
            unsafe { crate::tasks::hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
            crate::STATUS_OK
        );
        assert_eq!(cause, 0);
        crate::STATUS_OK
    }

    #[test]
    fn task_end_events_keep_result_rank_and_cleanup_under_one_ordinal() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let image = crate::Layout {
            size: 16,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &image];
        let mut references = [None, None];
        for parked in [false, true] {
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    let output = Arc::new(Mutex::new((Vec::new(), Vec::new(), Vec::new())));
                    let seen = output.clone();
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.config.layouts = layouts.as_ptr();
                    run.config.layout_count = 2;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            super::super::engine::enable(ctx, policy);
                            enable(ctx);
                            install_value(ctx, 3, received_integer_program());
                        },
                        move |ctx| unsafe {
                            let mut scope = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_enter(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut scope,
                                    &0
                                ),
                                crate::STATUS_OK
                            );
                            let env = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                8,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(env)
                                .cast::<u64>()
                                .write(u64::from(parked));
                            let mut child = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_spawn(
                                    ctx,
                                    0,
                                    0,
                                    std::ptr::null_mut(),
                                    &mut child,
                                    &scope,
                                    typed_result_child,
                                    env,
                                    0
                                ),
                                crate::STATUS_OK
                            );
                            let mut result = [0u64; 2];
                            assert_eq!(
                                hirsel_rt_v2_task_join(
                                    ctx,
                                    0,
                                    0,
                                    &scope,
                                    &mut child,
                                    result.as_mut_ptr().cast(),
                                    16
                                ),
                                crate::STATUS_OK
                            );
                            assert_eq!(result, [1, 42]);
                            result.fill(0);
                            let _role = role::Guard::new(ctx);
                            let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                            let ends: Vec<_> = log
                                .canonical
                                .iter()
                                .filter(|row| row.op == "task.end")
                                .cloned()
                                .collect();
                            assert_eq!(ends.len(), 3);
                            let child_id = ObjectId {
                                creator: vec![],
                                ordinal: 1,
                            };
                            let scope_id = ObjectId {
                                creator: vec![],
                                ordinal: 0,
                            };
                            assert_eq!(ends[0].item, CanonicalItem::Object(child_id.clone()));
                            assert_eq!(ends[0].version, if parked { 3 } else { 2 });
                            assert_eq!(
                                ends[0].outcome,
                                CanonicalOutcome::TaskEnd {
                                    holding: true,
                                    result: Some(
                                        [1u32.to_le_bytes().as_slice(), &42u64.to_le_bytes()]
                                            .concat()
                                    )
                                }
                            );
                            assert_eq!(ends[1].item, CanonicalItem::CompletionRank(scope_id));
                            assert_eq!(ends[1].version, 2);
                            assert_eq!(
                                ends[1].outcome,
                                CanonicalOutcome::Scalar(0u64.to_le_bytes().to_vec())
                            );
                            assert_eq!(ends[2].item, CanonicalItem::Object(child_id));
                            assert_eq!(ends[2].version, if parked { 6 } else { 4 });
                            assert_eq!(
                                ends[2].outcome,
                                CanonicalOutcome::TaskEnd {
                                    holding: false,
                                    result: None
                                }
                            );
                            assert!(
                                ends.iter()
                                    .all(|row| row.task == [0] && row.ordinal == u64::from(parked))
                            );
                            assert_eq!(ends[0].subevent, 0);
                            assert_eq!(ends[1].subevent, 1);
                            assert_eq!(ends[2].subevent, 2);
                            assert_eq!(ends[0].transition, Transition::Hold);
                            assert_eq!(ends[2].transition, Transition::End);
                            let joins: Vec<_> = log
                                .owner_completions
                                .iter()
                                .filter(|row| row.op == "task.join")
                                .cloned()
                                .collect();
                            assert_eq!(joins.len(), 1);
                            assert_eq!(joins[0].task, Vec::<u32>::new());
                            assert_eq!(joins[0].ordinal, 2);
                            assert_eq!(
                                joins[0].outcome,
                                OwnerOutcome::Join {
                                    scope: Some(ObjectId {
                                        creator: vec![],
                                        ordinal: 0
                                    }),
                                    child: Some(ObjectId {
                                        creator: vec![],
                                        ordinal: 1
                                    }),
                                    child_path: vec![0],
                                    status: crate::STATUS_OK,
                                    rank: 0,
                                    payload: Some(
                                        [1u32.to_le_bytes().as_slice(), &42u64.to_le_bytes()]
                                            .concat()
                                    ),
                                    failures: Some(Vec::new()),
                                }
                            );
                            let canonical_joins: Vec<_> = log
                                .canonical
                                .iter()
                                .filter(|row| row.op == "task.join")
                                .cloned()
                                .collect();
                            assert_eq!(canonical_joins.len(), if parked { 8 } else { 4 });
                            assert!(
                                canonical_joins
                                    .iter()
                                    .all(|row| row.task.is_empty() && row.ordinal == 2)
                            );
                            assert!(
                                log.unsupported
                                    .iter()
                                    .all(|row| row.op != Some("task.join"))
                            );
                            for (index, row) in canonical_joins.iter().enumerate() {
                                assert_eq!(row.subevent as usize, index);
                            }
                            let completed = if parked {
                                assert_eq!(canonical_joins[0].transition, Transition::Yield);
                                assert_eq!(canonical_joins[2].transition, Transition::Park);
                                assert_eq!(
                                    canonical_joins[4].transition,
                                    Transition::Call {
                                        members: vec![vec![0]],
                                        then_yield: false,
                                    }
                                );
                                6
                            } else {
                                assert_eq!(
                                    canonical_joins[0].transition,
                                    Transition::Call {
                                        members: vec![vec![0]],
                                        then_yield: true,
                                    }
                                );
                                2
                            };
                            assert_eq!(canonical_joins[completed].transition, Transition::Keep);
                            assert_eq!(
                                canonical_joins[completed].version,
                                if parked { 7 } else { 5 }
                            );
                            assert_eq!(
                                canonical_joins[completed].outcome,
                                CanonicalOutcome::Joined {
                                    status: crate::STATUS_OK,
                                    rank: 0,
                                    payload: Some(
                                        [1u32.to_le_bytes().as_slice(), &42u64.to_le_bytes()]
                                            .concat()
                                    ),
                                    failures: Vec::new(),
                                }
                            );
                            *seen.lock().unwrap() = (ends, joins, canonical_joins);
                            drop(_role);
                            assert_eq!(
                                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                                crate::STATUS_OK
                            );
                            crate::STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                    let ends = output.lock().unwrap().clone();
                    if let Some(reference) = &references[usize::from(parked)] {
                        assert_eq!(&ends, reference);
                    } else {
                        references[usize::from(parked)] = Some(ends);
                    }
                }
            }
        }
    }

    unsafe extern "C" fn scope_trace_child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        use crate::tasks::*;
        let [log, index, wait, expected] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 4]>().read() };
        if wait != 0 {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let saved = {
                    let _role = unsafe { role::Guard::new(ctx) };
                    let s = scheduler(ctx);
                    (if wait == 2 {
                        matches!(s.task(0).wait, Some(Wait::Drain { .. }))
                    } else {
                        matches!(s.task(0).wait, Some(Wait::Exit { .. }))
                    }) && readiness::saved(s, 0)
                };
                if saved {
                    break;
                }
                assert!(
                    std::time::Instant::now() < until,
                    "scope owner never saved its wait"
                );
                let mut cause = 99;
                assert_eq!(
                    unsafe { hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
                    crate::STATUS_OK
                );
            }
        }
        let mut result = [1u64, index + 40];
        assert_eq!(
            unsafe {
                hirsel_rt_v2_task_finish(
                    ctx,
                    result.as_mut_ptr().cast(),
                    [0u64; 2].as_ptr().cast(),
                    16,
                    3,
                )
            },
            crate::STATUS_OK
        );
        unsafe { &*(log as *const std::sync::Mutex<Vec<u64>>) }
            .lock()
            .unwrap()
            .push(index);
        let mut cause = 99;
        assert_eq!(
            unsafe { hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
            crate::STATUS_OK
        );
        assert_eq!(u64::from(cause), expected);
        crate::STATUS_OK
    }

    #[test]
    fn scope_exit_events_keep_members_cancellation_and_reverse_called_rests() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 32,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let image = crate::Layout {
            size: 16,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &image];
        for wait in [false, true] {
            for cancel in [false, true] {
                let mut reference = None;
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        let cleanup = Arc::new(Mutex::new(Vec::<u64>::new()));
                        let order = cleanup.clone();
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = layouts.as_ptr();
                        run.config.layout_count = 2;
                        unsafe { run.use_physical_dispatch() };
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy = Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                enable(ctx);
                                install_value(ctx, 3, received_integer_program());
                            },
                            move |ctx| unsafe {
                                let mut scope = 0;
                                assert_eq!(
                                    hirsel_rt_v2_task_scope_enter(
                                        ctx,
                                        std::ptr::null_mut(),
                                        &mut scope,
                                        &0
                                    ),
                                    crate::STATUS_OK
                                );
                                let mut children = [0; 2];
                                for (index, child) in children.iter_mut().enumerate() {
                                    let env = crate::alloc::hirsel_rt_v2_alloc(
                                        ctx,
                                        2,
                                        32,
                                        8,
                                        crate::alloc::DOMAIN_HEAP,
                                        0,
                                    );
                                    let expected = if cancel {
                                        if index == 0 { 1 } else { 2 }
                                    } else {
                                        0
                                    };
                                    crate::alloc::payload(env).cast::<[u64; 4]>().write([
                                        Arc::as_ptr(&order) as u64,
                                        index as u64,
                                        u64::from(wait),
                                        expected,
                                    ]);
                                    assert_eq!(
                                        hirsel_rt_v2_task_spawn(
                                            ctx,
                                            0,
                                            0,
                                            std::ptr::null_mut(),
                                            child,
                                            &scope,
                                            scope_trace_child,
                                            env,
                                            0
                                        ),
                                        crate::STATUS_OK
                                    );
                                }
                                if cancel {
                                    assert_eq!(
                                        hirsel_rt_v2_task_cancel(ctx, &scope, &children[0]),
                                        crate::STATUS_OK
                                    );
                                }
                                assert_eq!(
                                    hirsel_rt_v2_task_scope_exit(
                                        ctx,
                                        0,
                                        0,
                                        &scope,
                                        u8::from(cancel)
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(*order.lock().unwrap(), [1, 0]);
                                let _role = role::Guard::new(ctx);
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows: Vec<_> = log
                                    .canonical
                                    .iter()
                                    .filter(|row| row.op == "task.scope_exit")
                                    .cloned()
                                    .collect();
                                assert!(
                                    log.unsupported
                                        .iter()
                                        .all(|row| row.op != Some("task.scope_exit"))
                                );
                                assert!(log.scope_exits.is_empty());
                                assert!(
                                    rows.iter()
                                        .all(|row| row.ordinal == if cancel { 4 } else { 3 }
                                            && row.task.is_empty())
                                );
                                for (subevent, row) in rows.iter().enumerate() {
                                    assert_eq!(row.subevent as usize, subevent);
                                }
                                let members = vec![
                                    ObjectId {
                                        creator: vec![],
                                        ordinal: 1,
                                    },
                                    ObjectId {
                                        creator: vec![],
                                        ordinal: 2,
                                    },
                                ];
                                assert_eq!(
                                    rows[0].outcome,
                                    CanonicalOutcome::ScopeExiting {
                                        members: members.clone()
                                    }
                                );
                                assert_eq!(
                                    rows[0].transition,
                                    if wait {
                                        Transition::Park
                                    } else {
                                        Transition::Keep
                                    }
                                );
                                let calls: Vec<_> = rows
                                    .iter()
                                    .filter_map(|row| {
                                        if let Transition::Call {
                                            members,
                                            then_yield,
                                        } = &row.transition
                                        {
                                            assert!(!then_yield);
                                            Some(members.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                assert_eq!(calls, [vec![vec![1]], vec![vec![0]]]);
                                let causes: Vec<_> = rows
                                    .iter()
                                    .filter(|row| {
                                        matches!(row.item, CanonicalItem::Cancellation(_))
                                    })
                                    .collect();
                                if cancel {
                                    assert_eq!(causes.len(), 2);
                                    assert_eq!(
                                        causes[0].item,
                                        CanonicalItem::Cancellation(vec![0])
                                    );
                                    assert_eq!(
                                        causes[0].outcome,
                                        CanonicalOutcome::Scalar(1u32.to_le_bytes().to_vec())
                                    );
                                    assert_eq!(
                                        causes[1].item,
                                        CanonicalItem::Cancellation(vec![1])
                                    );
                                    assert_eq!(
                                        causes[1].outcome,
                                        CanonicalOutcome::Scalar(2u32.to_le_bytes().to_vec())
                                    );
                                } else {
                                    assert!(causes.is_empty());
                                }
                                let end = rows.last().unwrap();
                                assert_eq!(
                                    end.item,
                                    CanonicalItem::Object(ObjectId {
                                        creator: vec![],
                                        ordinal: 0
                                    })
                                );
                                assert_eq!(
                                    end.outcome,
                                    CanonicalOutcome::ScopeExited {
                                        status: crate::STATUS_OK,
                                        expired: false,
                                        failures: Vec::new()
                                    }
                                );
                                assert_eq!(
                                    rows[rows.len() - 3].item,
                                    CanonicalItem::Object(members[0].clone())
                                );
                                assert_eq!(
                                    rows[rows.len() - 2].item,
                                    CanonicalItem::Object(members[1].clone())
                                );
                                assert!(scheduler(ctx).scopes[scope as u32 as usize].is_none());
                                *seen.lock().unwrap() = rows;
                                crate::STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn shared_root_trace_refuses_unregistered_roots_and_retires_shallow_thaws() {
        use crate::tasks::*;
        let root_layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: crate::LAYOUT_FROZEN,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&root_layout as *const crate::Layout];
        for width in [1, 2, 4, 8] {
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                    enable(ctx);
                },
                |ctx| unsafe {
                    let root = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(root).cast::<u64>().write(99);
                    crate::shared::hirsel_rt_v2_freeze(ctx, root);
                    let mut last = 99;
                    assert_eq!(
                        crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root),
                        crate::STATUS_OK
                    );
                    assert_eq!(last, 1);
                    crate::shared::hirsel_rt_v2_thaw(ctx, root);
                    assert!(
                        (*ctx)
                            .run_ref()
                            .ordered_roots
                            .get()
                            .unwrap()
                            .get(root)
                            .is_none()
                    );
                    (*ctx).heap().free(root);
                    // An imported root without freeze provenance must never be
                    // certified merely because its release and bytes are valid.
                    let unknown = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(unknown).cast::<u64>().write(77);
                    assert_eq!(
                        crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, unknown),
                        crate::STATUS_OK
                    );
                    crate::roots::hirsel_rt_v2_root_release(ctx, unknown);
                    let _role = role::Guard::new(ctx);
                    super::super::engine::at_head(ctx).unwrap();
                    let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                    assert!(log.canonical().is_err());
                    assert!(
                        log.unsupported
                            .iter()
                            .any(|row| row.op == Some("shared.thaw"))
                    );
                    assert!(
                        log.unsupported
                            .iter()
                            .any(|row| row.op == Some("root.release"))
                    );
                    assert!((*ctx).roots.trace_releases.is_empty());
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        }
    }

    #[derive(Default)]
    struct FreezeOrder {
        wide: bool,
        second: std::sync::atomic::AtomicBool,
        order: std::sync::Mutex<Vec<u64>>,
    }

    unsafe extern "C" fn shared_identity_child(
        ctx: *mut TaskCtx,
        env: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        use std::sync::atomic::Ordering;
        unsafe {
            let [probe, index] = crate::alloc::payload(env).cast::<[u64; 2]>().read();
            let probe = &*(probe as *const FreezeOrder);
            // Let the parent's second spawn retire before the first child's
            // long head segment. The queued sibling may then freeze early.
            if index == 0 {
                let mut cause = 99;
                assert_eq!(
                    crate::tasks::hirsel_rt_v2_task_canceled(ctx, &mut cause, &0),
                    crate::STATUS_OK
                );
                assert_eq!(cause, 0);
            }
            if index == 0 && probe.wide {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !probe.second.load(Ordering::Acquire) {
                    assert!(
                        std::time::Instant::now() < until,
                        "younger pure segment never froze"
                    );
                    std::thread::yield_now();
                }
            }
            let root = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(root).cast::<u64>().write(100 + index);
            crate::shared::hirsel_rt_v2_freeze(ctx, root);
            probe.order.lock().unwrap().push(index);
            if index == 1 {
                probe.second.store(true, Ordering::Release);
            }
            crate::roots::hirsel_rt_v2_root_dup(ctx, root);
            let mut last = 99;
            assert_eq!(
                crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root),
                crate::STATUS_OK
            );
            assert_eq!(last, 0);
            crate::roots::hirsel_rt_v2_root_release(ctx, root);
            assert_eq!(
                crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root),
                crate::STATUS_OK
            );
            assert_eq!(last, 1);
            crate::roots::hirsel_rt_v2_root_release(ctx, root);
            let mut result = [1u64, index];
            crate::tasks::hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                4,
            )
        }
    }

    #[test]
    fn shared_root_ids_ignore_opposite_physical_freeze_orders_and_preserve_release_thaw_events() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let root_layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: crate::LAYOUT_FROZEN,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let environment = crate::Layout {
            size: 16,
            flags: 0,
            ..root_layout
        };
        let result = environment;
        let layouts = [&root_layout as *const crate::Layout, &environment, &result];
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let probe = Arc::new(FreezeOrder {
                    wide: width > 1,
                    ..Default::default()
                });
                let child_probe = probe.clone();
                let output = Arc::new(Mutex::new(Vec::new()));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 3;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        super::super::engine::enable(ctx, policy);
                        enable(ctx);
                        install_value(ctx, 4, received_integer_program());
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0
                            ),
                            crate::STATUS_OK
                        );
                        for index in 0..2 {
                            let env = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                3,
                                16,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(env)
                                .cast::<[u64; 2]>()
                                .write([Arc::as_ptr(&child_probe) as u64, index]);
                            let mut child = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_spawn(
                                    ctx,
                                    0,
                                    0,
                                    std::ptr::null_mut(),
                                    &mut child,
                                    &scope,
                                    shared_identity_child,
                                    env,
                                    0
                                ),
                                crate::STATUS_OK
                            );
                        }
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                            crate::STATUS_OK
                        );
                        let _role = role::Guard::new(ctx);
                        let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                        let rows = log.canonical().unwrap();
                        for index in 0..2u32 {
                            let id = crate::ordered::root_ids::SharedRootId {
                                creator: vec![index],
                                freeze: 0,
                            };
                            let root_rows: Vec<_> = rows
                                .iter()
                                .filter(|row| row.item == CanonicalItem::SharedRoot(id.clone()))
                                .collect();
                            assert_eq!(root_rows.len(), 4);
                            assert_eq!(
                                root_rows
                                    .iter()
                                    .map(|row| row.outcome.clone())
                                    .collect::<Vec<_>>(),
                                [
                                    CanonicalOutcome::Thaw { last: false },
                                    CanonicalOutcome::RootReleased { last: false },
                                    CanonicalOutcome::Thaw { last: true },
                                    CanonicalOutcome::RootReleased { last: true },
                                ]
                            );
                            assert_eq!(
                                root_rows.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
                                if index == 0 {
                                    [1, 2, 3, 4]
                                } else {
                                    [0, 1, 2, 3]
                                }
                            );
                            assert_eq!(
                                root_rows.iter().map(|row| row.version).collect::<Vec<_>>(),
                                [1, 2, 3, 4]
                            );
                            assert!(
                                root_rows
                                    .iter()
                                    .all(|row| row.transition == Transition::Keep)
                            );
                        }
                        *seen.lock().unwrap() = rows.to_vec();
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(
                    *probe.order.lock().unwrap(),
                    if width == 1 { vec![0, 1] } else { vec![1, 0] }
                );
                let rows = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&rows, reference);
                } else {
                    reference = Some(rows);
                }
            }
        }
    }

    unsafe extern "C" fn gate_trace_child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        unsafe {
            let gate = crate::alloc::payload(environment).cast::<u64>().read() as u32;
            {
                let _role = crate::tasks::role::Guard::new(ctx);
                assert!(super::super::foreign::gate_enter(ctx, gate, 0).unwrap());
                assert!(!super::super::foreign::gate_enter(ctx, gate, 0).unwrap());
                super::super::foreign::gate_leave(ctx, gate, false).unwrap();
                super::super::foreign::gate_leave(ctx, gate, true).unwrap();
            }
            let mut result = [1u64, 9];
            crate::tasks::hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                3,
            )
        }
    }

    #[test]
    fn gate_events_keep_saved_acquisition_in_the_enter_ordinal_and_hide_nested_reentry() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let result = crate::Layout {
            size: 16,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &result];
        for contended in [false, true] {
            let mut reference = None;
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    let output = Arc::new(Mutex::new(Vec::new()));
                    let seen = output.clone();
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.config.layouts = layouts.as_ptr();
                    run.config.layout_count = 2;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            super::super::engine::enable(ctx, policy);
                            enable(ctx);
                            install_value(ctx, 3, received_integer_program());
                        },
                        move |ctx| unsafe {
                            let gate = 7 + width;
                            let mut scope = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_enter(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut scope,
                                    &0
                                ),
                                crate::STATUS_OK
                            );
                            if contended {
                                let _role = role::Guard::new(ctx);
                                assert!(super::super::foreign::gate_enter(ctx, gate, 0).unwrap());
                            }
                            let env = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                8,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(env)
                                .cast::<u64>()
                                .write(u64::from(gate));
                            let mut child = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_spawn(
                                    ctx,
                                    0,
                                    0,
                                    std::ptr::null_mut(),
                                    &mut child,
                                    &scope,
                                    gate_trace_child,
                                    env,
                                    0
                                ),
                                crate::STATUS_OK
                            );
                            if contended {
                                let _role = role::Guard::new(ctx);
                                // Spawn is deferrable: inspect only after this
                                // parent regains head behind the child's park.
                                super::super::engine::at_head(ctx).unwrap();
                                let s = scheduler(ctx);
                                assert!(matches!(
                                    s.task(child as u32).wait,
                                    Some(Wait::Gate { gate: waiting }) if waiting == gate
                                ));
                                assert!(readiness::saved(s, child as u32));
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                assert!(
                                    log.canonical().is_err(),
                                    "a pending gate cannot certify a complete prefix"
                                );
                                assert_eq!(log.gate_waiting.len(), 1);
                                super::super::foreign::gate_leave(ctx, gate, true).unwrap();
                            }
                            assert_eq!(
                                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                                crate::STATUS_OK
                            );
                            let _role = role::Guard::new(ctx);
                            let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                            assert!(log.gate_waiting.is_empty());
                            let rows: Vec<_> = log
                                .canonical()
                                .unwrap()
                                .iter()
                                .filter(|row| {
                                    row.item == CanonicalItem::Object(log.gates[&gate].clone())
                                })
                                .collect();
                            assert_eq!(rows.len(), if contended { 6 } else { 3 });
                            for (index, row) in rows.iter().enumerate() {
                                assert_eq!(row.version, index as u64 + 1);
                            }
                            assert!(matches!(rows[0].outcome, CanonicalOutcome::Created(_)));
                            assert_eq!(
                                log.gates[&gate],
                                ObjectId {
                                    creator: if contended { vec![] } else { vec![0] },
                                    ordinal: if contended { 1 } else { 0 },
                                }
                            );
                            let child_rows: Vec<_> = rows
                                .iter()
                                .filter(|row| {
                                    row.task == [0]
                                        && !matches!(row.outcome, CanonicalOutcome::Created(_))
                                })
                                .collect();
                            assert_eq!(child_rows.len(), if contended { 3 } else { 2 });
                            if contended {
                                assert_eq!(child_rows[0].outcome, CanonicalOutcome::GateWaiting);
                                assert_eq!(child_rows[0].transition, Transition::Park);
                                assert_eq!(child_rows[1].outcome, CanonicalOutcome::GateAcquired);
                                assert_eq!(child_rows[1].transition, Transition::Keep);
                                assert_eq!((child_rows[0].ordinal, child_rows[1].ordinal), (0, 0));
                                assert_eq!(
                                    (child_rows[0].subevent, child_rows[1].subevent),
                                    (0, 1)
                                );
                                assert!(child_rows[0].retirement < child_rows[1].retirement);
                            } else {
                                assert_eq!(child_rows[0].outcome, CanonicalOutcome::GateAcquired);
                            }
                            let last = child_rows.last().unwrap();
                            assert_eq!(last.op, "gate.leave");
                            assert_eq!(last.ordinal, 1);
                            assert_eq!(last.outcome, CanonicalOutcome::GateReleased);
                            assert_eq!(last.transition, Transition::Keep);
                            *seen.lock().unwrap() = log.canonical.clone();
                            crate::STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                    let rows = output.lock().unwrap().clone();
                    if let Some(reference) = &reference {
                        assert_eq!(&rows, reference);
                    } else {
                        reference = Some(rows);
                    }
                }
            }
        }
    }

    #[test]
    fn root_end_events_fix_status_failures_and_returned_values_before_cleanup() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let image = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&image as *const crate::Layout];
        for mode in 0..4 {
            for known in [false, true] {
                let mut reference = None;
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = layouts.as_ptr();
                        run.config.layout_count = 1;
                        unsafe { run.use_physical_dispatch() };
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy = Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                enable(ctx);
                                if known {
                                    install_value(ctx, 2, received_integer_program());
                                    static TABLE: [u8; 40] = {
                                        let mut bytes = [0; 40];
                                        bytes[0] = 1;
                                        bytes[8] = 1;
                                        bytes[16] = 17;
                                        bytes[20] = 3;
                                        bytes
                                    };
                                    (*ctx).run().failure_table =
                                        crate::isolate::FailureTable::new(&TABLE);
                                }
                            },
                            move |ctx| unsafe {
                                let _role = role::Guard::new(ctx);
                                if mode >= 2 {
                                    let mut result = [1u64, 42];
                                    let returned = crate::exit_report::Returned {
                                        value: result.as_mut_ptr().cast(),
                                        layout: 2,
                                        payload_offset: 8,
                                        node: std::ptr::null(),
                                    };
                                    super::super::engine::root_report(ctx, &returned).unwrap();
                                    result.fill(0);
                                    let mut cause = 99;
                                    assert_eq!(
                                        hirsel_rt_v2_task_canceled(ctx, &mut cause, &0),
                                        crate::STATUS_OK
                                    );
                                    assert_eq!(cause, 0);
                                }
                                let failed = mode == 1 || mode == 3;
                                let status = if failed {
                                    crate::failure::hirsel_rt_v2_failure_begin(
                                        ctx,
                                        1,
                                        hirsel_core_contract::RUNTIME_RULE_BASE,
                                        1,
                                        0,
                                    )
                                } else {
                                    crate::STATUS_OK
                                };
                                super::super::engine::root_end(ctx, status).unwrap();
                                super::super::engine::root_end(ctx, status).unwrap();
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let ends: Vec<_> = log
                                    .canonical
                                    .iter()
                                    .filter(|row| row.op == "task.end")
                                    .collect();
                                if known || mode == 0 {
                                    assert_eq!(ends.len(), 1);
                                    let end = ends[0];
                                    assert_eq!(end.item, CanonicalItem::Root);
                                    assert_eq!(end.transition, Transition::End);
                                    assert_eq!((end.version, end.subevent), (1, 0));
                                    assert_eq!(
                                        end.outcome,
                                        CanonicalOutcome::RootEnd {
                                            status,
                                            result: (mode >= 2).then(|| [
                                                1u32.to_le_bytes().as_slice(),
                                                &42u64.to_le_bytes()
                                            ]
                                            .concat()),
                                            failures: if failed {
                                                vec![(
                                                    hirsel_run_schema::Failure {
                                                        rule: hirsel_core_contract::RUNTIME_RULES
                                                            [0]
                                                        .into(),
                                                        site: hirsel_run_schema::Site {
                                                            definition: String::new(),
                                                            file: String::new(),
                                                            line: 17,
                                                            column: 3,
                                                        },
                                                        operation: None,
                                                    },
                                                    vec![],
                                                )]
                                            } else {
                                                Vec::new()
                                            },
                                        }
                                    );
                                    assert!(log.canonical().is_ok());
                                } else {
                                    assert!(ends.is_empty());
                                    assert!(log.canonical().is_err());
                                    assert_eq!(
                                        log.unsupported.last().unwrap().op,
                                        Some("task.end")
                                    );
                                }
                                *seen.lock().unwrap() = log.canonical.clone();
                                status
                            },
                        );
                        assert_eq!(
                            outcome.ended,
                            Ok(if mode == 1 || mode == 3 {
                                crate::STATUS_FAILURE
                            } else {
                                crate::STATUS_OK
                            })
                        );
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    unsafe extern "C" fn failure_end_child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let mode = unsafe { crate::alloc::payload(environment).cast::<u64>().read() };
        if mode != 0 {
            let mut result = [1u64, 9];
            assert_eq!(
                unsafe {
                    crate::tasks::hirsel_rt_v2_task_finish(
                        ctx,
                        result.as_mut_ptr().cast(),
                        [0u64; 2].as_ptr().cast(),
                        16,
                        3,
                    )
                },
                crate::STATUS_OK
            );
            let mut cause = 99;
            assert_eq!(
                unsafe { crate::tasks::hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
                crate::STATUS_OK
            );
            if mode == 2 {
                return crate::STATUS_OK;
            }
        }
        unsafe {
            crate::failure::hirsel_rt_v2_failure_begin(
                ctx,
                1,
                hirsel_core_contract::RUNTIME_RULE_BASE,
                1,
                0,
            )
        }
    }

    #[test]
    fn failing_task_end_captures_rank_scope_and_first_wins_sibling_cancellation() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let result = crate::Layout {
            size: 16,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &result];
        for cleanup in [false, true] {
            for precancel in [false, true] {
                for known in [false, true] {
                    let mut reference = None;
                    for width in [1, 2, 4, 8] {
                        for reorder in [
                            hirsel_run_config::Reorder::Off,
                            hirsel_run_config::Reorder::Auto,
                        ] {
                            let output = Arc::new(Mutex::new(Vec::new()));
                            let seen = output.clone();
                            let mut run = crate::host::BodyRun::new(width, true);
                            run.config.layouts = layouts.as_ptr();
                            run.config.layout_count = 2;
                            unsafe { run.use_physical_dispatch() };
                            let outcome = run.run(
                                |ctx| unsafe {
                                    let mut policy = Policy::admit(&Default::default(), None);
                                    policy.reorder = reorder;
                                    super::super::engine::enable(ctx, policy);
                                    enable(ctx);
                                    install_value(ctx, 3, received_integer_program());
                                    if known {
                                        static TABLE: [u8; 40] = {
                                            let mut bytes = [0; 40];
                                            bytes[0] = 1;
                                            bytes[8] = 1;
                                            bytes[16] = 17;
                                            bytes[20] = 3;
                                            bytes
                                        };
                                        (*ctx).run().failure_table =
                                            crate::isolate::FailureTable::new(&TABLE);
                                    }
                                },
                                move |ctx| unsafe {
                                    let mut scope = 0;
                                    assert_eq!(
                                        hirsel_rt_v2_task_scope_enter(
                                            ctx,
                                            std::ptr::null_mut(),
                                            &mut scope,
                                            &0
                                        ),
                                        crate::STATUS_OK
                                    );
                                    let mut children = [0; 2];
                                    for (index, mode) in
                                        [2, u64::from(cleanup)].into_iter().enumerate()
                                    {
                                        let env = crate::alloc::hirsel_rt_v2_alloc(
                                            ctx,
                                            2,
                                            8,
                                            8,
                                            crate::alloc::DOMAIN_HEAP,
                                            0,
                                        );
                                        crate::alloc::payload(env).cast::<u64>().write(mode);
                                        assert_eq!(
                                            hirsel_rt_v2_task_spawn(
                                                ctx,
                                                0,
                                                0,
                                                std::ptr::null_mut(),
                                                &mut children[index],
                                                &scope,
                                                failure_end_child,
                                                env,
                                                0
                                            ),
                                            crate::STATUS_OK
                                        );
                                        if index == 0 && precancel {
                                            assert_eq!(
                                                hirsel_rt_v2_task_cancel(ctx, &scope, &children[0]),
                                                crate::STATUS_OK
                                            );
                                        }
                                    }
                                    assert_eq!(
                                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                                        crate::STATUS_FAILURE
                                    );
                                    let _role = role::Guard::new(ctx);
                                    let log =
                                        super::super::engine::driver(ctx).events.as_ref().unwrap();
                                    let rows: Vec<_> = log
                                        .canonical
                                        .iter()
                                        .filter(|row| row.task == [1] && row.op == "task.end")
                                        .cloned()
                                        .collect();
                                    assert!(log.failed_ends.is_empty());
                                    if known {
                                        let failed = rows
                                            .iter()
                                            .position(|row| {
                                                matches!(
                                                    row.outcome,
                                                    CanonicalOutcome::TaskFailed { .. }
                                                )
                                            })
                                            .unwrap();
                                        let end = &rows[failed..];
                                        assert_eq!(end.len(), 4);
                                        assert_eq!(
                                            end[0].outcome,
                                            CanonicalOutcome::TaskFailed {
                                                failures: vec![(
                                                    hirsel_run_schema::Failure {
                                                        rule: hirsel_core_contract::RUNTIME_RULES
                                                            [0]
                                                        .into(),
                                                        site: hirsel_run_schema::Site {
                                                            definition: String::new(),
                                                            file: String::new(),
                                                            line: 17,
                                                            column: 3
                                                        },
                                                        operation: None,
                                                    },
                                                    vec![1],
                                                )]
                                            }
                                        );
                                        let scope_id = ObjectId {
                                            creator: vec![],
                                            ordinal: 0,
                                        };
                                        assert_eq!(
                                            end[1].item,
                                            CanonicalItem::CompletionRank(scope_id.clone())
                                        );
                                        assert_eq!(
                                            end[1].outcome,
                                            CanonicalOutcome::Scalar(
                                                (if cleanup { 2u64 } else { 1 })
                                                    .to_le_bytes()
                                                    .to_vec()
                                            )
                                        );
                                        assert_eq!(
                                            end[2].item,
                                            CanonicalItem::ScopeCancellation(scope_id)
                                        );
                                        assert_eq!(
                                            end[2].outcome,
                                            CanonicalOutcome::Scalar(
                                                (CANCEL_CHILD_FAILED + 1).to_le_bytes().to_vec()
                                            )
                                        );
                                        assert_eq!(
                                            end[3].item,
                                            CanonicalItem::Cancellation(vec![0])
                                        );
                                        assert_eq!(
                                            end[3].outcome,
                                            CanonicalOutcome::Scalar(
                                                (1 + if precancel {
                                                    CANCEL_REQUESTED
                                                } else {
                                                    CANCEL_CHILD_FAILED
                                                })
                                                .to_le_bytes()
                                                .to_vec()
                                            )
                                        );
                                        assert!(
                                            end.iter().all(|row| row.transition == Transition::End
                                                && row.ordinal == 0)
                                        );
                                        assert_eq!(end[0].subevent, if cleanup { 2 } else { 0 });
                                        assert!(log.canonical().is_ok());
                                    } else {
                                        assert!(!rows.iter().any(|row| matches!(
                                            row.outcome,
                                            CanonicalOutcome::TaskFailed { .. }
                                        )));
                                        assert!(log.canonical().is_err());
                                    }
                                    let chain = (*ctx).failure.take_chain(&[]);
                                    assert_eq!(chain.len(), 1);
                                    assert_eq!(chain[0].1, [1]);
                                    *seen.lock().unwrap() = log.canonical.clone();
                                    crate::STATUS_OK
                                },
                            );
                            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                            let rows = output.lock().unwrap().clone();
                            if let Some(reference) = &reference {
                                assert_eq!(&rows, reference);
                            } else {
                                reference = Some(rows);
                            }
                        }
                    }
                }
            }
        }
    }

    unsafe extern "C" fn failed_scope_child(
        ctx: *mut TaskCtx,
        _: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        unsafe {
            crate::failure::hirsel_rt_v2_failure_begin(
                ctx,
                1,
                hirsel_core_contract::RUNTIME_RULE_BASE,
                1,
                0,
            )
        }
    }

    #[test]
    fn scope_exit_failure_events_preserve_child_paths_and_refuse_missing_source_metadata() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&environment as *const crate::Layout];
        for known in [false, true] {
            let mut reference = None;
            for width in [1, 2, 4, 8] {
                let output = Arc::new(Mutex::new(Vec::new()));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                        enable(ctx);
                        if known {
                            static TABLE: [u8; 40] = {
                                let mut bytes = [0; 40];
                                bytes[0] = 1;
                                bytes[8] = 1;
                                bytes[16] = 17;
                                bytes[20] = 3;
                                bytes
                            };
                            (*ctx).run().failure_table = crate::isolate::FailureTable::new(&TABLE);
                        }
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0
                            ),
                            crate::STATUS_OK
                        );
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env).cast::<u64>().write(0);
                        let mut child = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                failed_scope_child,
                                env,
                                0
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                            crate::STATUS_FAILURE
                        );
                        let _role = role::Guard::new(ctx);
                        let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                        let rows: Vec<_> = log
                            .canonical
                            .iter()
                            .filter(|row| row.op == "task.scope_exit")
                            .cloned()
                            .collect();
                        assert!(log.scope_exits.is_empty());
                        if known {
                            assert!(
                                log.unsupported
                                    .iter()
                                    .all(|row| row.op != Some("task.scope_exit"))
                            );
                            assert_eq!(
                                rows.last().unwrap().outcome,
                                CanonicalOutcome::ScopeExited {
                                    status: crate::STATUS_FAILURE,
                                    expired: false,
                                    failures: vec![(
                                        hirsel_run_schema::Failure {
                                            rule: hirsel_core_contract::RUNTIME_RULES[0].into(),
                                            site: hirsel_run_schema::Site {
                                                definition: String::new(),
                                                file: String::new(),
                                                line: 17,
                                                column: 3
                                            },
                                            operation: None,
                                        },
                                        vec![0]
                                    )],
                                }
                            );
                        } else {
                            assert!(
                                log.unsupported
                                    .iter()
                                    .any(|row| row.op == Some("task.scope_exit"))
                            );
                            assert!(!rows.iter().any(|row| matches!(
                                row.outcome,
                                CanonicalOutcome::ScopeExited { .. }
                            )));
                            assert!(log.canonical().is_err());
                        }
                        let chain = (*ctx).failure.take_chain(&[]);
                        assert_eq!(chain.len(), 1);
                        assert_eq!(chain[0].1, [0]);
                        *seen.lock().unwrap() = rows;
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                let rows = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&rows, reference);
                } else {
                    reference = Some(rows);
                }
            }
        }
    }

    #[test]
    fn detach_events_survive_running_and_holding_child_cleanup() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 32,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let image = crate::Layout {
            size: 16,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &image];
        for running in [false, true] {
            let mut reference = None;
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    let cleanup = Arc::new(Mutex::new(Vec::<u64>::new()));
                    let order = cleanup.clone();
                    let output = Arc::new(Mutex::new(Vec::new()));
                    let seen = output.clone();
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.config.layouts = layouts.as_ptr();
                    run.config.layout_count = 2;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            super::super::engine::enable(ctx, policy);
                            enable(ctx);
                            install_value(ctx, 3, received_integer_program());
                        },
                        move |ctx| unsafe {
                            let mut scope = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_enter(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut scope,
                                    &0
                                ),
                                crate::STATUS_OK
                            );
                            let env = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                32,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(env).cast::<[u64; 4]>().write([
                                Arc::as_ptr(&order) as u64,
                                0,
                                u64::from(running),
                                0,
                            ]);
                            let mut child = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_spawn(
                                    ctx,
                                    0,
                                    0,
                                    std::ptr::null_mut(),
                                    &mut child,
                                    &scope,
                                    scope_trace_child,
                                    env,
                                    0
                                ),
                                crate::STATUS_OK
                            );
                            assert_eq!(
                                hirsel_rt_v2_task_detach(ctx, &scope, &mut child),
                                crate::STATUS_OK
                            );
                            assert_eq!(
                                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                                crate::STATUS_OK
                            );
                            assert_eq!(*order.lock().unwrap(), [0]);
                            let _role = role::Guard::new(ctx);
                            let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                            let rows: Vec<_> = log
                                .canonical
                                .iter()
                                .filter(|row| row.op == "task.detach")
                                .cloned()
                                .collect();
                            assert_eq!(rows.len(), 2);
                            assert!(
                                rows.iter()
                                    .all(|row| row.task.is_empty() && row.ordinal == 2)
                            );
                            assert_eq!(rows[0].subevent, 0);
                            assert_eq!(rows[1].subevent, 1);
                            assert_eq!(
                                rows[0].item,
                                CanonicalItem::Object(ObjectId {
                                    creator: vec![],
                                    ordinal: 0
                                })
                            );
                            let id = ObjectId {
                                creator: vec![],
                                ordinal: 1,
                            };
                            assert_eq!(rows[0].outcome, CanonicalOutcome::Alias(id.clone()));
                            assert_eq!(rows[1].item, CanonicalItem::Object(id));
                            assert_eq!(rows[1].outcome, CanonicalOutcome::Detached);
                            assert_eq!(rows[1].version, if running { 2 } else { 3 });
                            assert_eq!(
                                rows[1].transition,
                                if running {
                                    Transition::Yield
                                } else {
                                    Transition::Call {
                                        members: vec![vec![0]],
                                        then_yield: true,
                                    }
                                }
                            );
                            assert!(
                                log.unsupported
                                    .iter()
                                    .all(|row| row.op != Some("task.detach"))
                            );
                            assert!(scheduler(ctx).tasks[child as u32 as usize].is_none());
                            *seen.lock().unwrap() = rows;
                            crate::STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                    let rows = output.lock().unwrap().clone();
                    if let Some(reference) = &reference {
                        assert_eq!(&rows, reference);
                    } else {
                        reference = Some(rows);
                    }
                }
            }
        }
    }

    #[test]
    fn bulk_detach_events_follow_add_order_through_yielding_rests_and_empty_sets() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 32,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let image = crate::Layout {
            size: 16,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &image];
        for count in [0usize, 2] {
            for running in [false, true] {
                let mut reference = None;
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        let cleanup = Arc::new(Mutex::new(Vec::<u64>::new()));
                        let order = cleanup.clone();
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = layouts.as_ptr();
                        run.config.layout_count = 2;
                        unsafe { run.use_physical_dispatch() };
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy = Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                enable(ctx);
                                install_value(ctx, 3, received_integer_program());
                            },
                            move |ctx| unsafe {
                                let mut scope = 0;
                                assert_eq!(
                                    hirsel_rt_v2_task_scope_enter(
                                        ctx,
                                        std::ptr::null_mut(),
                                        &mut scope,
                                        &0
                                    ),
                                    crate::STATUS_OK
                                );
                                let mut children = [0; 2];
                                for (index, child) in children.iter_mut().enumerate().take(count) {
                                    let env = crate::alloc::hirsel_rt_v2_alloc(
                                        ctx,
                                        2,
                                        32,
                                        8,
                                        crate::alloc::DOMAIN_HEAP,
                                        0,
                                    );
                                    crate::alloc::payload(env).cast::<[u64; 4]>().write([
                                        Arc::as_ptr(&order) as u64,
                                        index as u64,
                                        u64::from(running),
                                        0,
                                    ]);
                                    assert_eq!(
                                        hirsel_rt_v2_task_spawn(
                                            ctx,
                                            0,
                                            0,
                                            std::ptr::null_mut(),
                                            child,
                                            &scope,
                                            scope_trace_child,
                                            env,
                                            0
                                        ),
                                        crate::STATUS_OK
                                    );
                                }
                                let mut set = 0;
                                assert_eq!(
                                    hirsel_rt_v2_task_set(
                                        ctx,
                                        std::ptr::null_mut(),
                                        &mut set,
                                        &scope
                                    ),
                                    crate::STATUS_OK
                                );
                                for member in (0..count).rev() {
                                    assert_eq!(
                                        hirsel_rt_v2_task_add(
                                            ctx,
                                            &scope,
                                            &mut set,
                                            &mut children[member]
                                        ),
                                        crate::STATUS_OK
                                    );
                                }
                                assert_eq!(
                                    hirsel_rt_v2_task_detach_all(ctx, &scope, &mut set),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                                    crate::STATUS_OK
                                );
                                let cleaned = order.lock().unwrap();
                                if count == 0 {
                                    assert!(cleaned.is_empty());
                                } else if running {
                                    let mut members = cleaned.clone();
                                    members.sort_unstable();
                                    assert_eq!(members, [0, 1]);
                                } else {
                                    assert_eq!(*cleaned, [1, 0]);
                                }
                                drop(cleaned);
                                let _role = role::Guard::new(ctx);
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows: Vec<_> = log
                                    .canonical
                                    .iter()
                                    .filter(|row| row.op == "task.detach_all")
                                    .cloned()
                                    .collect();
                                assert_eq!(rows.len(), count + 3);
                                let ordinal = 2 + 2 * count as u64;
                                assert!(
                                    rows.iter()
                                        .all(|row| row.task.is_empty() && row.ordinal == ordinal)
                                );
                                for (index, row) in rows.iter().enumerate() {
                                    assert_eq!(row.subevent as usize, index);
                                }
                                let set_id = ObjectId {
                                    creator: vec![],
                                    ordinal: 1 + count as u64,
                                };
                                assert_eq!(
                                    rows[0].outcome,
                                    CanonicalOutcome::Alias(set_id.clone())
                                );
                                assert_eq!(rows[1].item, CanonicalItem::Object(set_id.clone()));
                                let members: Vec<_> = (0..count)
                                    .rev()
                                    .map(|index| ObjectId {
                                        creator: vec![],
                                        ordinal: 1 + index as u64,
                                    })
                                    .collect();
                                assert_eq!(
                                    rows[1].outcome,
                                    CanonicalOutcome::BulkDetaching {
                                        members: members.clone()
                                    }
                                );
                                assert_eq!(
                                    rows[1].transition,
                                    Transition::Call {
                                        members: (0..count)
                                            .rev()
                                            .map(|index| vec![index as u32])
                                            .collect(),
                                        then_yield: true
                                    }
                                );
                                for (index, member) in members.iter().enumerate() {
                                    assert_eq!(
                                        rows[2 + index].item,
                                        CanonicalItem::Object(member.clone())
                                    );
                                    assert_eq!(rows[2 + index].outcome, CanonicalOutcome::Detached);
                                    assert_eq!(rows[2 + index].transition, Transition::Keep);
                                }
                                assert_eq!(
                                    rows.last().unwrap().item,
                                    CanonicalItem::Object(set_id)
                                );
                                assert_eq!(rows.last().unwrap().outcome, CanonicalOutcome::Unit);
                                assert_eq!(rows.last().unwrap().transition, Transition::Keep);
                                assert_eq!(rows.last().unwrap().version, 3 + count as u64);
                                assert!(log.bulk_detaches.is_empty());
                                assert!(
                                    log.unsupported
                                        .iter()
                                        .all(|row| row.op != Some("task.detach_all"))
                                );
                                assert!(scheduler(ctx).sets[set as u32 as usize].is_none());
                                *seen.lock().unwrap() = rows;
                                crate::STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn cancel_all_events_keep_causes_and_saved_drain_under_one_ordinal() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 32,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let image = crate::Layout {
            size: 16,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &image];
        for count in [0usize, 2] {
            for running in [false, true] {
                let mut reference = None;
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        let cleanup = Arc::new(Mutex::new(Vec::<u64>::new()));
                        let order = cleanup.clone();
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = layouts.as_ptr();
                        run.config.layout_count = 2;
                        unsafe { run.use_physical_dispatch() };
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy = Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                enable(ctx);
                                install_value(ctx, 3, received_integer_program());
                            },
                            move |ctx| unsafe {
                                let mut scope = 0;
                                assert_eq!(
                                    hirsel_rt_v2_task_scope_enter(
                                        ctx,
                                        std::ptr::null_mut(),
                                        &mut scope,
                                        &0
                                    ),
                                    crate::STATUS_OK
                                );
                                let mut children = [0; 2];
                                for (index, child) in children.iter_mut().enumerate().take(count) {
                                    let env = crate::alloc::hirsel_rt_v2_alloc(
                                        ctx,
                                        2,
                                        32,
                                        8,
                                        crate::alloc::DOMAIN_HEAP,
                                        0,
                                    );
                                    crate::alloc::payload(env).cast::<[u64; 4]>().write([
                                        Arc::as_ptr(&order) as u64,
                                        index as u64,
                                        if running { 2 } else { 0 },
                                        1,
                                    ]);
                                    assert_eq!(
                                        hirsel_rt_v2_task_spawn(
                                            ctx,
                                            0,
                                            0,
                                            std::ptr::null_mut(),
                                            child,
                                            &scope,
                                            scope_trace_child,
                                            env,
                                            0
                                        ),
                                        crate::STATUS_OK
                                    );
                                }
                                let mut set = 0;
                                assert_eq!(
                                    hirsel_rt_v2_task_set(
                                        ctx,
                                        std::ptr::null_mut(),
                                        &mut set,
                                        &scope
                                    ),
                                    crate::STATUS_OK
                                );
                                for member in (0..count).rev() {
                                    assert_eq!(
                                        hirsel_rt_v2_task_add(
                                            ctx,
                                            &scope,
                                            &mut set,
                                            &mut children[member]
                                        ),
                                        crate::STATUS_OK
                                    );
                                }
                                assert_eq!(
                                    hirsel_rt_v2_task_cancel_all(ctx, 0, 0, &scope, &mut set),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                                    crate::STATUS_OK
                                );
                                let cleaned = order.lock().unwrap();
                                if count == 0 {
                                    assert!(cleaned.is_empty());
                                } else if running {
                                    let mut members = cleaned.clone();
                                    members.sort_unstable();
                                    assert_eq!(members, [0, 1]);
                                } else {
                                    assert_eq!(*cleaned, [1, 0]);
                                }
                                drop(cleaned);
                                let _role = role::Guard::new(ctx);
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows: Vec<_> = log
                                    .canonical
                                    .iter()
                                    .filter(|row| row.op == "task.cancel_all")
                                    .cloned()
                                    .collect();
                                assert!(rows.len() >= 4 + count * 2);
                                let ordinal = 2 + 2 * count as u64;
                                assert!(
                                    rows.iter()
                                        .all(|row| row.task.is_empty() && row.ordinal == ordinal)
                                );
                                for (index, row) in rows.iter().enumerate() {
                                    assert_eq!(row.subevent as usize, index);
                                }
                                let set_id = ObjectId {
                                    creator: vec![],
                                    ordinal: 1 + count as u64,
                                };
                                assert_eq!(
                                    rows[0].outcome,
                                    CanonicalOutcome::Alias(set_id.clone())
                                );
                                assert_eq!(rows[1].item, CanonicalItem::Object(set_id.clone()));
                                let members: Vec<_> = (0..count)
                                    .rev()
                                    .map(|index| ObjectId {
                                        creator: vec![],
                                        ordinal: 1 + index as u64,
                                    })
                                    .collect();
                                assert_eq!(
                                    rows[1].outcome,
                                    CanonicalOutcome::BulkCanceling {
                                        members: members.clone()
                                    }
                                );
                                assert_eq!(rows[1].transition, Transition::Keep);
                                let detached: Vec<_> = rows
                                    .iter()
                                    .filter(|row| row.outcome == CanonicalOutcome::Detached)
                                    .collect();
                                assert_eq!(detached.len(), count);
                                for (row, member) in detached.iter().zip(&members) {
                                    assert_eq!(row.item, CanonicalItem::Object(member.clone()));
                                    assert_eq!(row.transition, Transition::Keep);
                                }
                                let causes: Vec<_> = rows
                                    .iter()
                                    .filter(|row| {
                                        matches!(row.item, CanonicalItem::Cancellation(_))
                                    })
                                    .collect();
                                assert_eq!(causes.len(), count);
                                for (row, index) in causes.iter().zip((0..count).rev()) {
                                    assert_eq!(
                                        row.item,
                                        CanonicalItem::Cancellation(vec![index as u32])
                                    );
                                    assert_eq!(
                                        row.outcome,
                                        CanonicalOutcome::Scalar(1u32.to_le_bytes().to_vec())
                                    );
                                }
                                let parks = rows
                                    .iter()
                                    .filter(|row| row.transition == Transition::Park)
                                    .count();
                                assert_eq!(parks, usize::from(running && count != 0));
                                let calls: Vec<_> = rows
                                    .iter()
                                    .filter_map(|row| {
                                        if let Transition::Call {
                                            members,
                                            then_yield,
                                        } = &row.transition
                                        {
                                            assert!(!then_yield);
                                            Some(members.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                if !running && count != 0 {
                                    assert_eq!(calls, [vec![vec![1]], vec![vec![0]]]);
                                } else {
                                    assert!(calls.is_empty());
                                }
                                assert_eq!(
                                    rows.last().unwrap().item,
                                    CanonicalItem::Object(set_id)
                                );
                                assert_eq!(rows.last().unwrap().outcome, CanonicalOutcome::Closed);
                                assert_eq!(rows.last().unwrap().transition, Transition::Keep);
                                let set_rows: Vec<_> =
                                    rows.iter().filter(|row| row.item == rows[1].item).collect();
                                for pair in set_rows.windows(2) {
                                    assert_eq!(pair[1].version, pair[0].version + 1);
                                }
                                assert!(log.cancel_all.is_empty());
                                assert!(
                                    log.unsupported
                                        .iter()
                                        .all(|row| row.op != Some("task.cancel_all"))
                                );
                                assert!(scheduler(ctx).sets[set as u32 as usize].is_none());
                                *seen.lock().unwrap() = rows;
                                crate::STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn mutex_failure_events_own_ordered_chains_and_reject_missing_source_metadata() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let close = crate::Step {
            kind: crate::STEP_CLOSE_HANDLE,
            field: 0,
            variant: 0,
            span: 0,
            offset: 0,
            stride: 0,
            count: 0,
            count_offset: 0,
            layout: std::ptr::null(),
        };
        let image = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let wrapper = crate::Layout {
            size: 8,
            flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
            steps: &close,
            step_count: 1,
            field_count: 1,
            ..image
        };
        let layouts = [&image as *const crate::Layout, &wrapper];
        for known in [false, true] {
            let mut reference = None;
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    let output = Arc::new(Mutex::new(Vec::new()));
                    let seen = output.clone();
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.config.layouts = layouts.as_ptr();
                    run.config.layout_count = 2;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            super::super::engine::enable(ctx, policy);
                            enable(ctx);
                            install_value(ctx, 2, received_integer_program());
                            if known {
                                static TABLE: [u8; 40] = {
                                    let mut bytes = [0; 40];
                                    bytes[0] = 1;
                                    bytes[8] = 1;
                                    bytes[16] = 17;
                                    bytes[20] = 3;
                                    bytes
                                };
                                (*ctx).run().failure_table =
                                    crate::isolate::FailureTable::new(&TABLE);
                            }
                        },
                        move |ctx| unsafe {
                            let mut payload = [1u64, 77];
                            let mut mutex = 0;
                            assert_eq!(
                                hirsel_rt_v2_mutex_create(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut mutex,
                                    payload.as_mut_ptr().cast(),
                                    [0u64; 2].as_ptr().cast(),
                                    2,
                                    3
                                ),
                                crate::STATUS_OK
                            );
                            let mut status = 99;
                            assert_eq!(
                                hirsel_rt_v2_mutex_take(
                                    ctx,
                                    &mut status,
                                    &mutex,
                                    payload.as_mut_ptr().cast()
                                ),
                                crate::STATUS_OK
                            );
                            assert_eq!((status, payload), (0, [1, 77]));
                            let record = crate::failure::FailureRecord {
                                kind: 1,
                                rule: hirsel_core_contract::RUNTIME_RULE_BASE,
                                site: 1,
                                operation_site: 1,
                            };
                            let chain = vec![(record, vec![4, 2]), (record, vec![])];
                            (*ctx).failure.append_chain(chain.clone());
                            assert_eq!(hirsel_rt_v2_mutex_kill(ctx, &mutex), crate::STATUS_OK);
                            // The stored chain survives the original owner's reset. Each
                            // cause reads that chain, rather than the caller's active state.
                            (*ctx).failure.clear();
                            for _ in 0..2 {
                                assert_eq!(hirsel_rt_v2_mutex_cause(ctx, &mutex), crate::STATUS_OK);
                            }
                            assert_eq!((*ctx).failure.chain(&[]), [chain.clone(), chain].concat());
                            (*ctx).failure.clear();
                            {
                                let _role = role::Guard::new(ctx);
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows: Vec<_> = log
                                    .canonical
                                    .iter()
                                    .filter(|row| matches!(row.op, "mutex.kill" | "mutex.cause"))
                                    .cloned()
                                    .collect();
                                if known {
                                    assert_eq!(rows.len(), 3);
                                    let failure = hirsel_run_schema::Failure {
                                        rule: hirsel_core_contract::RUNTIME_RULES[0].into(),
                                        site: hirsel_run_schema::Site {
                                            definition: String::new(),
                                            file: String::new(),
                                            line: 17,
                                            column: 3,
                                        },
                                        operation: None,
                                    };
                                    for (index, row) in rows.iter().enumerate() {
                                        assert_eq!(
                                            row.item,
                                            CanonicalItem::Object(ObjectId {
                                                creator: vec![],
                                                ordinal: 0,
                                            })
                                        );
                                        assert_eq!(
                                            row.outcome,
                                            CanonicalOutcome::Failures(vec![
                                                (failure.clone(), vec![4, 2]),
                                                (failure.clone(), vec![]),
                                            ])
                                        );
                                        assert_eq!(row.version, 3 + index as u64);
                                        assert_eq!(row.ordinal, 2 + index as u64);
                                        assert_eq!(row.subevent, 0);
                                        assert_eq!(
                                            row.transition,
                                            if index == 0 {
                                                Transition::Yield
                                            } else {
                                                Transition::Keep
                                            }
                                        );
                                    }
                                    assert!(log.canonical().is_ok());
                                } else {
                                    assert!(rows.is_empty());
                                    assert!(log.canonical().is_err());
                                    assert_eq!(
                                        log.unsupported
                                            .iter()
                                            .filter(|row| matches!(
                                                row.op,
                                                Some("mutex.kill" | "mutex.cause")
                                            ))
                                            .count(),
                                        3
                                    );
                                }
                                *seen.lock().unwrap() = log.canonical.clone();
                            }
                            assert_eq!(
                                crate::roots::hirsel_rt_v2_root_drop(
                                    ctx,
                                    mutex as *mut crate::Header,
                                    0,
                                    0
                                ),
                                crate::STATUS_OK
                            );
                            crate::STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                    let rows = output.lock().unwrap().clone();
                    if let Some(reference) = &reference {
                        assert_eq!(&rows, reference);
                    } else {
                        reference = Some(rows);
                    }
                }
            }
        }
    }

    static CLOSE_TRACE: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "C" fn overwrite_close_value(
        ctx: *mut TaskCtx,
        value: *mut u8,
        _: u64,
        _: u64,
    ) -> u8 {
        let image = unsafe { value.cast::<[u64; 2]>().read() };
        assert_eq!(image[0], 1);
        CLOSE_TRACE.lock().unwrap().push(image[1]);
        unsafe { value.cast::<[u64; 2]>().write([0; 2]) };
        let mut cause = 99;
        assert_eq!(
            unsafe { crate::tasks::hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
            crate::STATUS_OK
        );
        assert_eq!(cause, 0);
        crate::STATUS_OK
    }

    #[test]
    fn close_events_own_fifo_payloads_before_overwriting_yielding_finalizers() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex};
        let close = crate::Step {
            kind: crate::STEP_CLOSE_HANDLE,
            field: 0,
            variant: 0,
            span: 0,
            offset: 0,
            stride: 0,
            count: 0,
            count_offset: 0,
            layout: std::ptr::null(),
        };
        for finalizing in [false, true] {
            for decoded in [false, true] {
                let base = crate::Layout {
                    magic: crate::LAYOUT_MAGIC,
                    version: crate::LAYOUT_VERSION,
                    size: 16,
                    align: 8,
                    finalizer: None,
                    steps: std::ptr::null(),
                    step_count: 0,
                    field_count: 0,
                    flags: 0,
                    logical: 0,
                    logical_count: std::ptr::null(),
                    scan: std::ptr::null(),
                    scan_words: 0,
                };
                let image = if finalizing {
                    crate::tasks::ordered::drops::tests::finalized(
                        crate::Layout {
                            finalizer: Some(overwrite_close_value),
                            ..base
                        },
                        &[],
                    )
                } else {
                    &base
                };
                let wrapper = crate::Layout {
                    size: 8,
                    finalizer: None,
                    flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
                    steps: &close,
                    step_count: 1,
                    field_count: 1,
                    ..base
                };
                let layouts = [image as *const crate::Layout, &wrapper];
                let mut reference = None;
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        CLOSE_TRACE.lock().unwrap().clear();
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = layouts.as_ptr();
                        run.config.layout_count = 2;
                        unsafe { run.use_physical_dispatch() };
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy = Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                enable(ctx);
                                if decoded {
                                    install_value(ctx, 2, received_integer_program());
                                }
                            },
                            move |ctx| unsafe {
                                let mut endpoints = [0; 2];
                                assert_eq!(
                                    hirsel_rt_v2_channel_open(
                                        ctx,
                                        std::ptr::null_mut(),
                                        endpoints.as_mut_ptr(),
                                        &0,
                                        [0u64; 2].as_ptr().cast(),
                                        2
                                    ),
                                    crate::STATUS_OK
                                );
                                let mut duplicate = 0;
                                assert_eq!(
                                    hirsel_rt_v2_sender_dup(
                                        ctx,
                                        std::ptr::null_mut(),
                                        &mut duplicate,
                                        &0,
                                        &endpoints[0]
                                    ),
                                    crate::STATUS_OK
                                );
                                for (mut sender, value) in [(endpoints[0], 42), (duplicate, 43)] {
                                    let mut payload = [1u64, value];
                                    let mut status = 99;
                                    assert_eq!(
                                        hirsel_rt_v2_channel_send(
                                            ctx,
                                            &mut status,
                                            &mut sender,
                                            payload.as_mut_ptr().cast()
                                        ),
                                        crate::STATUS_OK
                                    );
                                    assert_eq!(status, crate::channels::DONE);
                                    assert_eq!(payload, [0; 2]);
                                }
                                for word in [endpoints[0], duplicate, endpoints[1]] {
                                    assert_eq!(
                                        crate::finalize::hirsel_rt_v2_close_handle(ctx, word, 0, 0),
                                        crate::STATUS_OK
                                    );
                                }
                                let mut payload = [1u64, 77];
                                let mut mutex = 0;
                                assert_eq!(
                                    hirsel_rt_v2_mutex_create(
                                        ctx,
                                        std::ptr::null_mut(),
                                        &mut mutex,
                                        payload.as_mut_ptr().cast(),
                                        [0u64; 2].as_ptr().cast(),
                                        2,
                                        3
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(payload, [0; 2]);
                                crate::roots::hirsel_rt_v2_root_dup(ctx, mutex as *mut crate::Header);
                                assert_eq!(crate::roots::hirsel_rt_v2_root_drop(
                                    ctx, mutex as *mut crate::Header, 0, 0), crate::STATUS_OK);
                                assert_eq!(
                                    crate::roots::hirsel_rt_v2_root_drop(
                                        ctx,
                                        mutex as *mut crate::Header,
                                        0,
                                        0
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    *CLOSE_TRACE.lock().unwrap(),
                                    if finalizing {
                                        vec![42, 43, 77]
                                    } else {
                                        Vec::new()
                                    }
                                );
                                let _role = role::Guard::new(ctx);
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows: Vec<_> = log
                                    .canonical
                                    .iter()
                                    .filter(|row| {
                                        matches!(row.outcome, CanonicalOutcome::Released { .. })
                                    })
                                    .cloned()
                                    .collect();
                                assert_eq!(rows.len(), if decoded { 4 } else { 2 });
                                let releases: Vec<_> = log.canonical.iter().filter(|row| row.op == "root.release").collect();
                                assert_eq!(releases.len(), 2);
                                assert_eq!(releases[0].outcome, CanonicalOutcome::RootReleased { last: false });
                                assert_eq!(releases[1].outcome, CanonicalOutcome::RootReleased { last: true });
                                assert_eq!(releases[0].item, releases[1].item);
                                assert!(releases.iter().all(|row| row.transition == Transition::Keep));
                                let channel = CanonicalItem::Object(ObjectId {
                                    creator: vec![],
                                    ordinal: 0,
                                });
                                for row in &rows[..2] {
                                    assert_eq!(row.item, channel);
                                    assert_eq!(row.op, "drop.trivial");
                                    assert_eq!(row.transition, Transition::Keep);
                                    assert_eq!(
                                        row.outcome,
                                        CanonicalOutcome::Released {
                                            kind: crate::ordered::objects::Kind::Sender.code(),
                                            leftovers: Vec::new()
                                        }
                                    );
                                }
                                if decoded {
                                    assert!(log.canonical().is_ok(), "initialized closes and mutex root releases form a complete prefix");
                                    assert_eq!(releases[1].item, rows[3].item);
                                    let release = log.canonical.iter().position(|row| row == releases[1]).unwrap();
                                    let close = log.canonical.iter().position(|row| row == &rows[3]).unwrap();
                                    assert!(release < close, "last root is recorded before payload cleanup");
                                    let encode = |value: u64| {
                                        [1u32.to_le_bytes().as_slice(), &value.to_le_bytes()]
                                            .concat()
                                    };
                                    assert_eq!(rows[2].item, channel);
                                    assert_eq!(
                                        rows[2].outcome,
                                        CanonicalOutcome::Released {
                                            kind: crate::ordered::objects::Kind::Receiver.code(),
                                            leftovers: vec![encode(42), encode(43)]
                                        }
                                    );
                                    assert_eq!(
                                        rows[3].outcome,
                                        CanonicalOutcome::Released {
                                            kind: crate::ordered::objects::Kind::Mutex.code(),
                                            leftovers: vec![encode(77)]
                                        }
                                    );
                                    assert!(rows[2..].iter().all(|row| row.op
                                        == if finalizing {
                                            "drop.leftovers"
                                        } else {
                                            "drop.trivial"
                                        }));
                                    if finalizing {
                                        let first = log
                                            .canonical
                                            .iter()
                                            .position(|row| row == &rows[2])
                                            .unwrap();
                                        let cleanup = log
                                            .canonical
                                            .iter()
                                            .position(|row| row.op == "task.check_canceled")
                                            .unwrap();
                                        assert!(
                                            first < cleanup,
                                            "close snapshot precedes user finalizer operations"
                                        );
                                    }
                                } else {
                                    assert!(log.unsupported.iter().any(|row| row.op
                                        == Some(if finalizing {
                                            "drop.leftovers"
                                        } else {
                                            "drop.trivial"
                                        })));
                                    assert!(log.canonical().is_err());
                                }
                                *seen.lock().unwrap() = rows;
                                crate::STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    #[derive(Default)]
    struct AheadProbe {
        issued: std::sync::atomic::AtomicBool,
        word: std::sync::atomic::AtomicU64,
        early: std::sync::atomic::AtomicBool,
        auto: bool,
    }

    unsafe extern "C" fn unresolved_head(
        ctx: *mut TaskCtx,
        env: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        use std::sync::atomic::Ordering;
        let probe =
            unsafe { &*(crate::alloc::payload(env).cast::<u64>().read() as *const AheadProbe) };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !probe.issued.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < until,
                "queued atomic creator did not pass the head"
            );
            std::thread::yield_now();
        }
        let _guard = unsafe { crate::tasks::role::Guard::new(ctx) };
        let s = crate::tasks::scheduler(ctx);
        let initialized = s.sync.initialized(probe.word.load(Ordering::Acquire));
        assert_eq!(
            initialized, probe.auto,
            "actual internal commit follows reorder policy"
        );
        let Logical::Ordered(driver) = &s.logical else {
            unreachable!()
        };
        assert!(
            driver
                .events
                .as_ref()
                .unwrap()
                .canonical
                .iter()
                .all(|row| !row.op.starts_with("atomic.")),
            "unretired atomic ops emit no canonical events"
        );
        probe.early.store(initialized, Ordering::Release);
        crate::STATUS_OK
    }

    #[test]
    fn an_actual_early_atomic_creation_emits_only_when_its_owner_retires() {
        use crate::tasks::*;
        use std::sync::{Arc, Mutex, atomic::Ordering};
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        let mut reference = None;
        for width in [2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let probe = Arc::new(AheadProbe {
                    auto: reorder == hirsel_run_config::Reorder::Auto,
                    ..Default::default()
                });
                let seen = probe.clone();
                let result = Arc::new(Mutex::new(Vec::new()));
                let output = result.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        super::super::engine::enable(ctx, policy);
                        enable(ctx);
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0
                            ),
                            crate::STATUS_OK
                        );
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env)
                            .cast::<u64>()
                            .write(Arc::as_ptr(&seen) as u64);
                        let mut child = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                unresolved_head,
                                env,
                                0
                            ),
                            crate::STATUS_OK
                        );
                        let mut word = 0;
                        super::super::engine::atomic_create(
                            ctx,
                            &mut word,
                            7u64.to_le_bytes().as_ptr(),
                            8,
                            0,
                            8,
                        )
                        .unwrap();
                        seen.word.store(word, Ordering::Release);
                        seen.issued.store(true, Ordering::Release);
                        super::super::engine::atomic_store(
                            ctx,
                            word,
                            9u64.to_le_bytes().as_ptr(),
                            8,
                        )
                        .unwrap();
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                            crate::STATUS_OK
                        );
                        let mut loaded = [0u8; 8];
                        super::super::engine::atomic_load(ctx, word, loaded.as_mut_ptr(), 8)
                            .unwrap();
                        assert_eq!(loaded, 9u64.to_le_bytes());
                        let driver = super::super::engine::driver(ctx);
                        let log = driver.events.as_ref().unwrap();
                        assert!(
                            log.canonical().is_err(),
                            "task operations are still unclassified"
                        );
                        let atomics: Vec<_> = log
                            .canonical
                            .iter()
                            .filter(|row| row.op.starts_with("atomic."))
                            .collect();
                        assert_eq!(atomics.len(), 3);
                        assert_eq!(
                            atomics
                                .iter()
                                .map(|event| event.version)
                                .collect::<Vec<_>>(),
                            [1, 2, 3]
                        );
                        *output.lock().unwrap() = log.canonical.clone();
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(probe.early.load(Ordering::Acquire), probe.auto);
                let events = result.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&events, reference);
                } else {
                    reference = Some(events);
                }
            }
        }
    }
    #[test]
    fn cancellation_trace_captures_the_observed_cause_before_the_check_yields() {
        use crate::tasks::*;
        for width in [1, 2, 4, 8] {
            for cause in [None, Some(0), Some(2)] {
                let mut run = crate::host::BodyRun::new(width, true);
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                        enable(ctx);
                    },
                    move |ctx| unsafe {
                        if let Some(cause) = cause {
                            let _guard = role::Guard::new(ctx);
                            crate::tasks::role::with_role(ctx, |s| s.cancel(0, cause));
                        }
                        let mut observed = u32::MAX;
                        assert_eq!(
                            hirsel_rt_v2_task_canceled(ctx, &mut observed, &0),
                            crate::STATUS_OK
                        );
                        let expected = cause.map_or(0, |cause| cause + 1);
                        assert_eq!(observed, expected);
                        // Later cancellation must not rewrite the recorded answer.
                        {
                            let _guard = role::Guard::new(ctx);
                            crate::tasks::role::with_role(ctx, |s| s.cancel(0, 3));
                        }
                        let driver = super::super::engine::driver(ctx);
                        let log = driver.events.as_ref().unwrap();
                        let rows = log.canonical().unwrap();
                        assert_eq!(rows.len(), 1);
                        assert_eq!(rows[0].item, CanonicalItem::Cancellation(Vec::new()));
                        assert_eq!(
                            rows[0].outcome,
                            CanonicalOutcome::Scalar(expected.to_le_bytes().to_vec())
                        );
                        assert_eq!(rows[0].transition, Transition::Yield);
                        assert_eq!(rows[0].version, 1);
                        assert_eq!(
                            log.boundaries[0].native_outcome,
                            Some((0, expected.to_le_bytes().to_vec()))
                        );
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            }
        }
    }
    #[test]
    fn disabled_head_evidence_never_calls_the_encoder() {
        let mut run = crate::host::BodyRun::new(2, true);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                super::super::engine::enable(ctx, Policy::admit(&Default::default(), None))
            },
            |ctx| {
                let (value, _) = super::super::engine::head_observed(
                    ctx,
                    "task.check_canceled",
                    || (7u32, Next::Keep),
                    |_| panic!("disabled evidence must not allocate or encode"),
                )
                .unwrap();
                assert_eq!(value, 7);
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    }
    #[test]
    fn receive_completion_decodes_certified_fields_and_refuses_missing_metadata() {
        use crate::tasks::*;
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        for (width, decoded) in [1, 2, 4, 8]
            .into_iter()
            .flat_map(|width| [(width, false), (width, true)])
        {
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                    enable(ctx);
                    if decoded {
                        install_value(ctx, 2, received_integer_program());
                    }
                },
                move |ctx| unsafe {
                    let none = [0u64; 2];
                    let mut endpoints = [0u64; 2];
                    assert_eq!(
                        hirsel_rt_v2_channel_open(
                            ctx,
                            std::ptr::null_mut(),
                            endpoints.as_mut_ptr(),
                            &0,
                            none.as_ptr().cast(),
                            2
                        ),
                        crate::STATUS_OK
                    );
                    let mut image = [1u64, 42];
                    let mut status = 99;
                    assert_eq!(
                        hirsel_rt_v2_channel_send(
                            ctx,
                            &mut status,
                            &mut endpoints[0],
                            image.as_mut_ptr().cast()
                        ),
                        crate::STATUS_OK
                    );
                    assert_eq!(status, crate::channels::DONE);
                    assert_eq!(
                        hirsel_rt_v2_channel_receive(
                            ctx,
                            0,
                            0,
                            &mut status,
                            &mut endpoints[1],
                            image.as_mut_ptr().cast()
                        ),
                        crate::STATUS_OK
                    );
                    assert_eq!(image, [1, 42]);
                    image.fill(0);
                    let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(log.owner_completions.len(), 1);
                    let done = &log.owner_completions[0];
                    assert_eq!(done.op, "channel.receive");
                    assert_eq!(done.ordinal, 2);
                    let OwnerOutcome::Receive {
                        status,
                        descriptor,
                        payload,
                    } = &done.outcome
                    else {
                        panic!("receive outcome")
                    };
                    assert_eq!(*status, crate::channels::DONE);
                    assert_eq!(*descriptor, 2);
                    let expected = decoded
                        .then(|| [1u32.to_le_bytes().as_slice(), &42u64.to_le_bytes()].concat());
                    assert_eq!(payload, &expected);
                    if decoded {
                        let rows: Vec<_> = log
                            .canonical()
                            .unwrap()
                            .iter()
                            .filter(|row| row.op == "channel.receive")
                            .collect();
                        assert_eq!(rows.len(), 2);
                        assert_eq!(rows[0].item, CanonicalItem::Cancellation(vec![]));
                        assert_eq!(
                            rows[1].item,
                            CanonicalItem::Object(ObjectId {
                                creator: vec![],
                                ordinal: 0
                            })
                        );
                        assert_eq!(
                            rows[1].outcome,
                            CanonicalOutcome::Received {
                                status: crate::channels::DONE,
                                payload: expected
                            }
                        );
                        assert_eq!(
                            (rows[1].ordinal, rows[1].subevent, rows[1].version),
                            (2, 1, 3)
                        );
                        assert_eq!(rows[1].transition, Transition::Yield);
                        assert!(log.receiving.is_empty());
                    } else {
                        assert!(
                            log.canonical().is_err(),
                            "unsupported carrier cannot become a complete trace"
                        );
                    }
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        }
    }

    #[test]
    fn channel_and_mutex_events_keep_initialized_payloads_after_moves() {
        use hirsel_core_contract::handle_scan::*;
        use std::sync::{Arc, Mutex};
        // Option[u64]: tag at 0, padding at 4, initialized payload only in arm1.
        let bits = [VALUE, VALUE_BITS, 8, 0, 0, 0, 0, 0];
        let option = [
            VALUE,
            VALUE_VARIANT,
            16,
            0,
            0,
            0,
            2,
            0,
            VALUE_FIELD,
            8,
            3,
            1,
            0,
            0,
            0,
            0,
        ];
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: option.as_ptr(),
            scan_words: option.len() as u64,
        };
        let scalar = crate::Layout {
            size: 8,
            scan: bits.as_ptr(),
            scan_words: bits.len() as u64,
            ..layout
        };
        let layouts = [&layout as *const crate::Layout, &scalar];
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let output = Arc::new(Mutex::new(Vec::new()));
                let saved = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        super::super::engine::enable(ctx, policy);
                        enable(ctx);
                    },
                    move |ctx| unsafe {
                        let none = [0u64; 2];
                        let mut endpoints = [0u64; 2];
                        super::super::engine::channel_open(
                            ctx,
                            endpoints.as_mut_ptr(),
                            none.as_ptr().cast(),
                            2,
                        )
                        .unwrap();
                        // Deliberately different padding must not enter the event.
                        let mut payload = [1 | (0xdeadbeefu64 << 32), 42];
                        let mut status = 99;
                        super::super::engine::channel_send(
                            ctx,
                            endpoints[0],
                            payload.as_mut_ptr().cast(),
                            &mut status,
                        )
                        .unwrap();
                        assert_eq!(status, crate::channels::DONE);
                        assert_eq!(payload, none);
                        payload = [1 | (0xabad1deau64 << 32), 42];
                        let mut wrapper = 0;
                        super::super::engine::mutex_create(
                            ctx,
                            &mut wrapper,
                            payload.as_mut_ptr().cast(),
                            none.as_ptr().cast(),
                            2,
                            2,
                        )
                        .unwrap();
                        let word = crate::alloc::payload(wrapper as *mut crate::Header)
                            .cast::<u64>()
                            .read();
                        assert_eq!(payload, none);
                        super::super::engine::mutex_take(
                            ctx,
                            word,
                            payload.as_mut_ptr().cast(),
                            &mut status,
                        )
                        .unwrap();
                        assert_eq!(status, 0);
                        assert_eq!(payload[1], 42);
                        payload[1] = 99;
                        super::super::engine::mutex_put(ctx, word, payload.as_mut_ptr().cast())
                            .unwrap();
                        assert_eq!(payload, none);
                        super::super::engine::mutex_take(
                            ctx,
                            word,
                            payload.as_mut_ptr().cast(),
                            &mut status,
                        )
                        .unwrap();
                        payload.fill(0);
                        let driver = super::super::engine::driver(ctx);
                        let events = driver.events.as_ref().unwrap().canonical().unwrap();
                        assert_eq!(events.len(), 6);
                        assert_eq!(
                            events.iter().map(|e| e.version).collect::<Vec<_>>(),
                            [1, 2, 1, 2, 3, 4]
                        );
                        for (index, number) in [(1, 42u64), (3, 42), (4, 99), (5, 99)] {
                            assert_eq!(
                                events[index].outcome,
                                CanonicalOutcome::Value {
                                    status: 0,
                                    payload: [1u32.to_le_bytes().as_slice(), &number.to_le_bytes()]
                                        .concat()
                                }
                            );
                        }
                        *saved.lock().unwrap() = events.to_vec();
                        // Exercise the actual early-effect path as well: it must
                        // retain evidence without emitting an event or publishing.
                        use crate::ordered::Backend;
                        let channel = crate::tasks::scheduler(ctx)
                            .channels
                            .channel(endpoints[1], false);
                        let mut received = [0u64; 2];
                        assert_eq!(
                            crate::tasks::scheduler(ctx)
                                .channels
                                .receive(channel, received.as_mut_ptr().cast()),
                            Ok(true)
                        );
                        assert_eq!(received[1], 42);
                        assert!(!crate::tasks::role::with_role(ctx, |s| s
                            .channels
                            .ready(channel)));
                        let id = super::super::engine::driver(ctx)
                            .objects
                            .identity(crate::ordered::objects::Kind::Sender, endpoints[0])
                            .unwrap()
                            .clone();
                        let op = Command::Send {
                            sender: endpoints[0],
                            image: std::cell::RefCell::new(
                                [1u64.to_ne_bytes(), 77u64.to_ne_bytes()].concat(),
                            ),
                        }
                        .operation(crate::ordered::footprint::Footprint {
                            writes: std::collections::BTreeSet::from([Item::Object(id.clone())]),
                            ..Default::default()
                        });
                        let mut recorded =
                            |_, _, _: &Outcome, _| panic!("early commit cannot record");
                        let mut polled = || panic!("early commit cannot poll");
                        let mut backend =
                            super::super::Objects::new(ctx, &mut recorded, &mut polled);
                        let receipt = backend.commit(0, &op, true);
                        let Command::Send { image, .. } = &op.payload else {
                            unreachable!()
                        };
                        image.borrow_mut().fill(0);
                        assert_eq!(receipt.result.image, vec![0; 16]);
                        assert_eq!(
                            receipt.result.canonical,
                            Some(vec![(
                                CanonicalItem::Object(id),
                                CanonicalOutcome::Value {
                                    status: 0,
                                    payload: [1u32.to_le_bytes().as_slice(), &77u64.to_le_bytes()]
                                        .concat()
                                }
                            )])
                        );
                        assert_eq!(
                            super::super::engine::driver(ctx)
                                .events
                                .as_ref()
                                .unwrap()
                                .canonical
                                .len(),
                            6
                        );
                        assert!(
                            !crate::tasks::role::with_role(ctx, |s| s.channels.ready(channel)),
                            "pending send stays invisible"
                        );
                        backend.publish(0, &op, &receipt.result);
                        assert!(
                            crate::tasks::role::with_role(ctx, |s| s.channels.ready(channel)),
                            "retirement publication exposes send"
                        );

                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                let events = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&events, reference);
                } else {
                    reference = Some(events);
                }
            }
        }
    }

    #[test]
    fn failed_join_evidence_does_not_read_a_result_or_guess_failure_locations() {
        use crate::tasks::{State, Task, handle, scheduler};
        for known in [false, true] {
            let mut run = crate::host::BodyRun::new(1, true);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                    enable(ctx);
                    if known {
                        static TABLE: [u8; 53] = {
                            let mut bytes = [0; 53];
                            bytes[0] = 1;
                            bytes[8] = 0xe7;
                            bytes[9] = 3; // site999
                            bytes[16] = 17;
                            bytes[20] = 3; // source line/column
                            bytes[28] = 5;
                            bytes[32] = 5;
                            bytes[36] = 8;
                            let text = *b"childtest.hir";
                            let mut i = 0;
                            while i < text.len() {
                                bytes[40 + i] = text[i];
                                i += 1;
                            }
                            bytes
                        };
                        (*ctx).run().failure_table = crate::isolate::FailureTable::new(&TABLE);
                    }
                },
                move |ctx| unsafe {
                    let _role = crate::tasks::role::Guard::new(ctx);
                    let s = scheduler(ctx);
                    let mut child = Task::new(7, std::ptr::null_mut(), None, vec![3], None);
                    child.state = State::Done;
                    child.completed = 12;
                    s.tasks.push(Some(child));
                    let word = handle(1, 7);
                    let scope = handle(4, 9);
                    let Logical::Ordered(driver) = &mut s.logical else {
                        unreachable!()
                    };
                    let log = driver.events.as_mut().unwrap();
                    log.ordinals.insert(vec![], 6);
                    log.last_ops.insert(vec![], "task.join");
                    let scope_id = ObjectId {
                        creator: vec![],
                        ordinal: 1,
                    };
                    let child_id = ObjectId {
                        creator: vec![],
                        ordinal: 2,
                    };
                    log.scopes.insert(scope, scope_id.clone());
                    log.tasks.insert(word, child_id.clone());
                    // A stale successful snapshot must never be used for failure.
                    log.task_results.insert(
                        vec![3],
                        TaskResult {
                            descriptor: u32::MAX,
                            payload: Some(vec![99]),
                        },
                    );
                    let chain = [(
                        crate::failure::FailureRecord {
                            kind: 1,
                            rule: hirsel_core_contract::RUNTIME_RULE_BASE,
                            site: 999,
                            operation_site: 999,
                        },
                        vec![3],
                    )];
                    prepare_join(ctx, scope, word);
                    assert!(
                        super::super::engine::driver(ctx)
                            .events
                            .as_ref()
                            .unwrap()
                            .canonical()
                            .is_err(),
                        "an unfinished join cannot certify a complete trace"
                    );
                    let failures = known.then(|| {
                        vec![(
                            hirsel_run_schema::Failure {
                                rule: hirsel_core_contract::RUNTIME_RULES[0].into(),
                                site: hirsel_run_schema::Site {
                                    definition: "child".into(),
                                    file: "test.hir".into(),
                                    line: 17,
                                    column: 3,
                                },
                                operation: None,
                            },
                            vec![3],
                        )]
                    });
                    complete_join(ctx, scope, word, Some(&chain));
                    let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(
                        log.owner_completions,
                        [OwnerCompletion {
                            task: vec![],
                            ordinal: 5,
                            op: "task.join",
                            outcome: OwnerOutcome::Join {
                                scope: Some(scope_id.clone()),
                                child: Some(child_id.clone()),
                                child_path: vec![3],
                                status: crate::STATUS_FAILURE,
                                rank: 12,
                                payload: None,
                                failures: failures.clone(),
                            }
                        }]
                    );
                    if let Some(failures) = failures {
                        let rows = log.canonical().unwrap();
                        assert_eq!(rows.len(), 2);
                        assert_eq!(rows[0].item, CanonicalItem::Object(child_id));
                        assert_eq!(
                            rows[0].outcome,
                            CanonicalOutcome::Joined {
                                status: crate::STATUS_FAILURE,
                                rank: 12,
                                payload: None,
                                failures,
                            }
                        );
                        assert_eq!(rows[1].item, CanonicalItem::Object(scope_id));
                        assert_eq!(rows[1].outcome, CanonicalOutcome::Unit);
                        assert_eq!(
                            (rows[0].ordinal, rows[0].subevent, rows[1].subevent),
                            (5, 0, 1)
                        );
                    } else {
                        assert!(log.canonical().is_err());
                        assert_eq!(log.unsupported.last().unwrap().op, Some("task.join"));
                        assert!(log.canonical.is_empty());
                    }
                    assert!(log.joins.is_empty());
                    crate::tasks::role::with_role(ctx, |s| s.tasks.pop());
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        }
    }

    #[test]
    fn unavailable_task_result_metadata_never_reads_a_result_carrier() {
        let mut run = crate::host::BodyRun::new(1, true);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                enable(ctx);
            },
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                task_result(ctx, u32::MAX, std::ptr::null());
                let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                assert_eq!(
                    log.task_results[&Vec::new()],
                    TaskResult {
                        descriptor: u32::MAX,
                        payload: None
                    }
                );
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    }

    #[test]
    fn receive_poll_observer_uses_saved_slots_without_parked_contexts() {
        use crate::budget::stretch::Settled;
        use crate::tasks::{Logical, Scheduler, Task, Wait};
        for case in 0..3 {
            let mut s = Scheduler::default();
            s.tasks
                .push(Some(Task::new(1, std::ptr::null_mut(), None, vec![], None)));
            s.tasks.push(Some(Task::new(
                2,
                std::ptr::null_mut(),
                None,
                vec![7],
                None,
            )));
            let layout = crate::Layout {
                magic: crate::LAYOUT_MAGIC,
                version: crate::LAYOUT_VERSION,
                size: 16,
                align: 8,
                finalizer: None,
                steps: std::ptr::null(),
                step_count: 0,
                field_count: 0,
                flags: 0,
                logical: 0,
                logical_count: std::ptr::null(),
                scan: std::ptr::null(),
                scan_words: 0,
            };
            let none = [0u64; 2];
            let element =
                unsafe { crate::channels::Element::new(2, &layout, none.as_ptr().cast()) };
            let (sender, receiver) = s.channels.open(element, &layout);
            let channel = s.channels.channel(receiver, false);
            let identity = ObjectId {
                creator: vec![],
                ordinal: 3,
            };
            let mut driver = super::super::driver::Driver::new(
                0,
                Policy::admit(&Default::default(), None),
                Settled {
                    level: 0,
                    limit: u64::MAX,
                    fuel: u64::MAX,
                },
                Default::default(),
            );
            let mut log = Log::default();
            log.ordinals.insert(vec![7], 10);
            log.last_ops.insert(vec![7], "channel.receive");
            log.receivers.insert(vec![7], (identity.clone(), 2));
            log.values.insert(2, received_integer_program());
            driver.events = Some(log);
            s.logical = Logical::Ordered(driver);
            let mut output = std::mem::MaybeUninit::<[u64; 2]>::uninit();
            let slot = if case == 0 {
                output.as_mut_ptr().cast()
            } else {
                std::ptr::null_mut()
            };
            s.current = 1;
            s.park(Wait::Receive { channel, slot });
            if case == 0 {
                let mut message = [1u64, 55];
                assert_eq!(
                    unsafe { s.channels.send(channel, message.as_mut_ptr().cast()) },
                    crate::channels::DONE
                );
            } else if case == 1 {
                s.channels.close(sender);
            } else {
                s.cancel(1, crate::tasks::CANCEL_REQUESTED);
            }
            s.wake(1);
            assert!(unsafe { crate::tasks::readiness::poll_ordered(&mut s, true) });
            assert!(s.task(1).wait.is_none());
            assert!(s.task(1).ctx.is_null());
            if case == 0 {
                unsafe { output.as_mut_ptr().write([0, 0]) };
            }
            let Logical::Ordered(driver) = &s.logical else {
                unreachable!()
            };
            let log = driver.events.as_ref().unwrap();
            let rows: Vec<_> = log
                .canonical
                .iter()
                .filter(|row| row.op == "channel.receive")
                .collect();
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().all(|row| row.task == [7] && row.ordinal == 9));
            assert_eq!(rows[0].item, CanonicalItem::Cancellation(vec![7]));
            assert_eq!(
                rows[0].outcome,
                CanonicalOutcome::Scalar((if case == 2 { 1u32 } else { 0 }).to_le_bytes().to_vec())
            );
            assert_eq!(rows[1].item, CanonicalItem::Object(identity.clone()));
            assert_eq!(rows[1].transition, Transition::Keep);
            let polls = &driver.events.as_ref().unwrap().receive_polls;
            assert_eq!(polls.len(), 1);
            assert_eq!(
                rows[1].outcome,
                CanonicalOutcome::Received {
                    status: polls[0].status,
                    payload: polls[0].payload.clone()
                }
            );
            assert_eq!(
                polls[0],
                ReceivePollCompletion {
                    task: vec![7],
                    ordinal: 9,
                    channel: identity,
                    status: match case {
                        0 => crate::channels::DONE,
                        1 => crate::channels::CLOSED,
                        _ => crate::channels::CANCELED + crate::tasks::CANCEL_REQUESTED,
                    },
                    payload: (case == 0).then(|| [
                        1u32.to_le_bytes().as_slice(),
                        &55u64.to_le_bytes()
                    ]
                    .concat()),
                    retirement: 0,
                    sequence: 0,
                }
            );
        }
    }

    fn received_integer_program() -> super::super::values::Program {
        use super::super::values::{Field, Node, Program, Tag};
        Program::new(
            vec![
                Node::Variant {
                    extent: 16,
                    tag: Tag::Ordinal { offset: 0 },
                    arms: vec![vec![], vec![Field { offset: 8, node: 1 }]],
                },
                Node::Bits(8),
            ],
            0,
        )
        .unwrap()
    }
    #[test]
    fn closed_and_canceled_receive_evidence_never_reads_the_unwritten_output() {
        use crate::tasks::*;
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        for canceled in [false, true] {
            let mut run = crate::host::BodyRun::new(2, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
                    enable(ctx);
                    install_value(ctx, 2, received_integer_program());
                },
                move |ctx| unsafe {
                    let mut endpoints = [0u64; 2];
                    assert_eq!(
                        hirsel_rt_v2_channel_open(
                            ctx,
                            std::ptr::null_mut(),
                            endpoints.as_mut_ptr(),
                            &0,
                            [0u64; 2].as_ptr().cast(),
                            2
                        ),
                        crate::STATUS_OK
                    );
                    if canceled {
                        let _guard = role::Guard::new(ctx);
                        crate::tasks::role::with_role(ctx, |s| s.cancel(0, CANCEL_REQUESTED));
                    } else {
                        assert_eq!(
                            crate::finalize::hirsel_rt_v2_close_handle(ctx, endpoints[0], 0, 0),
                            crate::STATUS_OK
                        );
                    }
                    let mut untouched = std::mem::MaybeUninit::<[u64; 2]>::uninit();
                    let mut status = u32::MAX;
                    assert_eq!(
                        hirsel_rt_v2_channel_receive(
                            ctx,
                            0,
                            0,
                            &mut status,
                            &mut endpoints[1],
                            untouched.as_mut_ptr().cast()
                        ),
                        crate::STATUS_OK
                    );
                    let expected = if canceled {
                        crate::channels::CANCELED + CANCEL_REQUESTED
                    } else {
                        crate::channels::CLOSED
                    };
                    assert_eq!(status, expected);
                    let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(log.owner_completions.len(), 1);
                    assert_eq!(
                        log.owner_completions[0].outcome,
                        OwnerOutcome::Receive {
                            status: expected,
                            descriptor: 2,
                            payload: None
                        }
                    );
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        }
    }
}
