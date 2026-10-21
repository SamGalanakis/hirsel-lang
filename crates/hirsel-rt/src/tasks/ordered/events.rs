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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OwnerOutcome {
    ScopeExit {
        status: u8,
        expired: bool,
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
    Scalar(Vec<u8>),
    Unit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Unsupported {
    pub task: Vec<u32>,
    pub ordinal: u64,
    pub op: Option<&'static str>,
}

#[derive(Default)]
pub(super) struct Log {
    ordinals: BTreeMap<Vec<u32>, u64>,
    last_ops: BTreeMap<Vec<u32>, &'static str>,
    completed: BTreeSet<(Vec<u32>, u64)>,
    pub host_completions: Vec<HostCompletion>,
    pub owner_completions: Vec<OwnerCompletion>,
    pub boundaries: Vec<Boundary>,
    versions: BTreeMap<CanonicalItem, u64>,
    canonical: Vec<CanonicalEvent>,
    unsupported: Vec<Unsupported>,
}

impl Log {
    /// A partial projection must never be accepted as a complete trace.
    pub(super) fn canonical(&self) -> Result<&[CanonicalEvent], &[Unsupported]> {
        if self.unsupported.is_empty() {
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
    let result = match result {
        Ok(crate::io::Completion::View(view)) => Ok(crate::io::Completion::View(
            crate::io::MapView::of(view.contents()),
        )),
        value => value.clone(),
    };
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
            return Some((
                CanonicalItem::Cancellation(path.clone()),
                CanonicalOutcome::Scalar(result.image.clone()),
            ));
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
            Command::AtomicCreate { .. } => CanonicalOutcome::Created(item.clone()),
            Command::AtomicLoad { .. } => CanonicalOutcome::Scalar(result.image.clone()),
            Command::AtomicStore { .. } => CanonicalOutcome::Unit,
            _ => return None,
        };
        assert_eq!(result.status, 0);
        Some((CanonicalItem::Object(item.clone()), outcome))
    });
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let next = log.ordinals.entry(path.clone()).or_default();
    let ordinal = if operation.is_some() {
        let ordinal = *next;
        *next = next.checked_add(1).expect("operation ordinal overflow");
        log.last_ops
            .insert(path.clone(), operation.unwrap().0.descriptor.name);
        ordinal
    } else {
        next.checked_sub(1)
            .expect("a continuation follows an operation")
    };
    if let (Some((item, outcome)), Some((op, _))) = (canonical, operation) {
        let version = log.versions.entry(item.clone()).or_default();
        *version = version
            .checked_add(1)
            .expect("canonical object version overflow");
        if matches!(outcome, CanonicalOutcome::Created(_)) {
            assert_eq!(*version, 1, "creation is the first object subevent");
        }
        log.canonical.push(CanonicalEvent {
            task: path.clone(),
            ordinal,
            subevent: 0,
            op: op.descriptor.name,
            transition: transition.clone(),
            item,
            version: *version,
            outcome,
            retirement: retirement.index,
            before: retirement.before.clone(),
            credit: retirement.credit,
        });
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

    #[test]
    fn native_retirements_keep_owned_evidence_and_continuations_keep_the_ordinal() {
        for width in [2, 4, 8] {
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
        for width in [2, 4, 8] {
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
            driver.events.as_ref().unwrap().canonical.is_empty(),
            "unretired ops emit no canonical events"
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
                        assert_eq!(log.canonical.len(), 3);
                        assert_eq!(
                            log.canonical
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
        for width in [2, 4, 8] {
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
    fn receive_completion_keeps_status_and_refuses_unclassified_payloads() {
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
        for width in [2, 4, 8] {
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
                    assert_eq!(payload, &None);
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
