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
    Object(ObjectId),
    Cancellation(Vec<u32>),
    CompletionRank(ObjectId),
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
    Created(ObjectId),
    Alias(ObjectId),
    Scalar(Vec<u8>),
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

struct ScopeExitEvidence {
    scope: ObjectId,
    members: Vec<ObjectId>,
    cancellations: Subevents,
}

#[derive(Default)]
pub(super) struct Log {
    ordinals: BTreeMap<Vec<u32>, u64>,
    last_ops: BTreeMap<Vec<u32>, &'static str>,
    end_ordinals: BTreeMap<Vec<u32>, u64>,
    next_subevent: BTreeMap<(Vec<u32>, u64), u32>,
    completed: BTreeSet<(Vec<u32>, u64)>,
    pub host_completions: Vec<HostCompletion>,
    pub host_polls: Vec<HostPollCompletion>,
    host_polled: BTreeSet<(Vec<u32>, u64)>,
    pub owner_completions: Vec<OwnerCompletion>,
    pub task_results: BTreeMap<Vec<u32>, TaskResult>,
    pub receive_polls: Vec<ReceivePollCompletion>,
    receivers: BTreeMap<Vec<u32>, (ObjectId, u32)>,
    joins: BTreeMap<Vec<u32>, (ObjectId, ObjectId)>,
    scope_exits: BTreeMap<Vec<u32>, ScopeExitEvidence>,
    receive_polled: BTreeSet<(Vec<u32>, u64)>,
    pub boundaries: Vec<Boundary>,
    versions: BTreeMap<CanonicalItem, u64>,
    /// Generation-qualified native words are lookup keys only. Keep bindings
    /// after close so delayed evidence cannot be confused with a reused row.
    sets: BTreeMap<u64, ObjectId>,
    scopes: BTreeMap<u64, ObjectId>,
    tasks: BTreeMap<u64, ObjectId>,
    values: BTreeMap<u32, super::values::Program>,
    canonical: Vec<CanonicalEvent>,
    unsupported: Vec<Unsupported>,
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

/// Caller owns the scheduler role. This lookup cannot allocate, bridge or
/// observe a stop, including on the disabled native host path.
pub(crate) fn observing(ctx: *mut TaskCtx) -> bool {
    let table = unsafe { &*(*(*ctx).run_ptr()).tasks.get() };
    table.as_deref().is_some_and(enabled)
}

pub(super) fn enabled(s: &super::super::Scheduler) -> bool {
    matches!(&s.logical, Logical::Ordered(driver) if driver.events.is_some())
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

impl Log {
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
                CanonicalOutcome::Created(_) | CanonicalOutcome::Spawned { .. }
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
        if self.unsupported.is_empty() && self.joins.is_empty() && self.scope_exits.is_empty() {
            Ok(&self.canonical)
        } else {
            Err(&self.unsupported)
        }
    }
}

/// Enable only at admission, before any operation can retire. No run identity
/// or production dispatcher is changed by installing the evidence collector.
#[cfg(test)]
pub(super) fn enable(ctx: *mut TaskCtx) {
    let driver = super::engine::driver(ctx);
    assert!(driver.recorded.is_empty());
    assert!(driver.events.replace(Log::default()).is_none());
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
            .encode(slot, |kind, word| {
                use crate::ordered::objects::Kind;
                match kind {
                    Kind::Task => log.tasks.get(&word),
                    Kind::TaskSet => log.sets.get(&word),
                    _ => driver.objects.identity(kind, word),
                }
                .cloned()
            })
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
    log.receivers.insert(path, (channel, descriptor));
    if let std::collections::btree_map::Entry::Vacant(entry) = log.values.entry(descriptor)
        && let Ok(program) = unsafe { super::values::Program::linked(ctx, descriptor) }
    {
        entry.insert(program);
    }
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
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let log = driver.events.as_mut().unwrap();
    let (channel, descriptor) = log.receivers.get(&path).expect("receive entry before poll");
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
                .encode(slot, |kind, word| {
                    use crate::ordered::objects::Kind;
                    match kind {
                        Kind::Task => log.tasks.get(&word),
                        Kind::TaskSet => log.sets.get(&word),
                        _ => driver.objects.identity(kind, word),
                    }
                    .cloned()
                })
                .ok()
        })
    } else {
        None
    };
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
                .encode(message, |kind, word| {
                    use crate::ordered::objects::Kind;
                    match kind {
                        Kind::Task => log.tasks.get(&word),
                        Kind::TaskSet => log.sets.get(&word),
                        _ => driver.objects.identity(kind, word),
                    }
                    .cloned()
                })
                .ok()
        })
    } else {
        None
    };
    complete_owner(ctx, "channel.receive", || OwnerOutcome::Receive {
        status,
        descriptor,
        payload,
    });
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
        log.values[&descriptor].encode(image, |kind, word| {
            use crate::ordered::objects::Kind;
            match kind {
                Kind::Task => log.tasks.get(&word),
                Kind::TaskSet => log.sets.get(&word),
                _ => driver.objects.identity(kind, word),
            }
            .cloned()
        })
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
    let Logical::Ordered(driver) = &s.logical else {
        return;
    };
    if driver.events.is_none() {
        return;
    }
    let task = unsafe { (*ctx).task_id };
    let path = s.task(task).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
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
    // A MapView can borrow host mapping storage. Copy its actual bytes while
    // the request still owns the view, rather than retaining a native address.
    let result = owned_host_result(result);
    log.host_completions.push(HostCompletion {
        task: path,
        ordinal,
        op,
        result,
    });
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
    complete_owner(ctx, "task.await_any", || outcome);
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
    if row.failure.is_some() {
        return None;
    }
    let Logical::Ordered(driver) = &s.logical else {
        return None;
    };
    let log = driver.events.as_ref()?;
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
    let canonical = operation.and_then(|(op, result)| {
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
            Command::Head { name: "task.join" } => return join_subevents(log, &path),
            Command::Head { name: "task.end" } => {
                return end_subevents(s, task, false, matches!(retirement.transition, Next::Hold));
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
            Command::AtomicLoad { .. } => CanonicalOutcome::Scalar(result.image.clone()),
            Command::AtomicStore { .. } => CanonicalOutcome::Unit,
            _ => return None,
        };
        assert_eq!(result.status, 0);
        Some(vec![(CanonicalItem::Object(item.clone()), outcome)])
    });
    let canonical = canonical.or_else(|| {
        if operation.is_none() {
            let Logical::Ordered(driver) = &s.logical else {
                unreachable!()
            };
            let log = driver.events.as_ref()?;
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
            before: retirement.before.clone(),
            credit: retirement.credit,
            native_outcome: None,
        };
        log.subevents(&boundary, name, events);
    } else {
        log.unsupported.push(Unsupported {
            task: path.clone(),
            ordinal,
            op: operation.map(|(op, _)| op.descriptor.name),
        });
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
        for width in [2, 4, 8] {
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
                        *seen.lock().unwrap() = completions.clone();
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
                            scheduler(ctx).sets.resize_with(width as usize, || None);
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
                                let _ = scheduler(ctx).channels.reserve_open();
                                let _ = scheduler(ctx).sync.reserve_mutex();
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
            assert_eq!(scheduler(ctx).task(unrelated).canceled.cause(), None);
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
                    assert_eq!(
                        log.canonical().unwrap_err().len(),
                        3,
                        "partial trace must be refused"
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
                        super::super::engine::atomic_create(ctx, &mut word, initial.as_ptr(), 8)
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
                                scheduler(ctx).cancel(0, CANCEL_REQUESTED);
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
                    matches!(s.task(0).wait, Some(Wait::Exit { .. })) && readiness::saved(s, 0)
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
                            scheduler(ctx).cancel(0, cause);
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
                            scheduler(ctx).cancel(0, 3);
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
                    assert!(
                        log.canonical().is_err(),
                        "unsupported carrier cannot become a complete trace"
                    );
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
                        assert!(!crate::tasks::scheduler(ctx).channels.ready(channel));
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
                            !crate::tasks::scheduler(ctx).channels.ready(channel),
                            "pending send stays invisible"
                        );
                        backend.publish(0, &op, &receipt.result);
                        assert!(
                            crate::tasks::scheduler(ctx).channels.ready(channel),
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
                    scheduler(ctx).tasks.pop();
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
            let polls = &driver.events.as_ref().unwrap().receive_polls;
            assert_eq!(polls.len(), 1);
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
                        scheduler(ctx).cancel(0, CANCEL_REQUESTED);
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
