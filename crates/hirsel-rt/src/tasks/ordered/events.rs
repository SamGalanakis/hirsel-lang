//! Owned native retirement evidence for the canonical event writer.
//! This log is optional and internal: native carrier bytes below still need
//! descriptor-aware value normalization before exporting a canonical trace.
//! It never samples a parked context or records an early commit.
use super::super::Logical;
use super::Outcome;
use crate::TaskCtx;
use crate::budget::stretch::Stretch;
use crate::ordered::footprint::TaskId;
use crate::ordered::{Next, Retirement};
use std::collections::BTreeMap;

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

#[derive(Default)]
pub(super) struct Log {
    ordinals: BTreeMap<Vec<u32>, u64>,
    pub boundaries: Vec<Boundary>,
}

/// Enable only at admission, before any operation can retire. No run identity
/// or production dispatcher is changed by installing the evidence collector.
#[cfg(test)]
pub(super) fn enable(ctx: *mut TaskCtx) {
    let driver = super::engine::driver(ctx);
    assert!(driver.recorded.is_empty());
    assert!(driver.events.replace(Log::default()).is_none());
}

pub(super) fn record(
    ctx: *mut TaskCtx,
    task: TaskId,
    operation: Option<(&'static str, &Outcome)>,
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
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let next = log.ordinals.entry(path.clone()).or_default();
    let ordinal = if operation.is_some() {
        let ordinal = *next;
        *next = next.checked_add(1).expect("operation ordinal overflow");
        ordinal
    } else {
        next.checked_sub(1)
            .expect("a continuation follows an operation")
    };
    log.boundaries.push(Boundary {
        task: path,
        ordinal,
        op: operation.map(|(name, _)| name),
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
}
