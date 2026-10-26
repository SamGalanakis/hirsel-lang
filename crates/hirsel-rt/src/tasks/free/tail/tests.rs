use super::*;
use crate::continuation::{Continuation, OwnedResumeFrame, SavedRepr};
use crate::stop::Stop;
use std::cell::Cell;

const TASK: TaskKey = TaskKey {
    slot: 3,
    generation: 7,
};

fn site(kind: Kind) -> Site {
    Site {
        object: (crate::free::MUTEX, 9),
        kind,
        anchor: 11,
        operation: 13,
    }
}

fn complete(record: &mut Record) {
    assert_eq!(
        record.poll(|| Some(41), |_| Ok(true), |_| Ok(STATUS_OK)),
        Ok(Poll::Ready(STATUS_OK))
    );
}

#[test]
fn pending_polls_and_saved_frame_reactivation_never_reissue_begin_or_done() {
    for kind in [
        Kind::Put,
        Kind::Kill,
        Kind::Cause,
        Kind::CloseMutex,
        Kind::CloseChannel,
    ] {
        let mut records = Records::default();
        let ticket = records.prepare(TASK, site(kind), Owners::default());
        let begins = Cell::new(0);
        let enters = Cell::new(0);
        let effects = Cell::new(0);
        let mut continuation = Continuation::Fresh;
        continuation.start();
        for poll in 0..6 {
            let record = records.active(TASK, ticket);
            assert_eq!(record.site, site(kind));
            let result = record
                .poll(
                    || {
                        begins.set(begins.get() + 1);
                        Some(41)
                    },
                    |entered| {
                        if !*entered {
                            enters.set(enters.get() + 1);
                            *entered = true;
                        }
                        // Spurious and duplicate wake hints do not grant admission.
                        Ok(poll == 5)
                    },
                    |_| {
                        effects.set(effects.get() + 1);
                        Ok(STATUS_OK)
                    },
                )
                .unwrap();
            assert_eq!(record.ordinal, Some(41));
            if poll < 5 {
                assert_eq!(result, Poll::Pending);
                let mut frame = OwnedResumeFrame::new(16);
                unsafe { frame.as_mut_ptr().cast::<u32>().write(1) };
                continuation.save(SavedRepr::Resumable(frame));
                // No independent tail activation: the existing owner takes the
                // same saved PC before any access to Records resumes.
                assert!(matches!(continuation.activate(), SavedRepr::Resumable(_)));
            } else {
                assert_eq!(result, Poll::Ready(STATUS_OK));
            }
        }
        // A completed effect can be polled for its suffix without executing any
        // closure: in particular it must not restore the parent's old ordinal.
        let record = records.active(TASK, ticket);
        assert_eq!(
            record.poll(
                || panic!("reissued begin"),
                |_| panic!("reissued wait"),
                |_| panic!("reissued effect")
            ),
            Ok(Poll::Ready(STATUS_OK))
        );
        record.finish();
        assert_eq!(records.retire(TASK, ticket), STATUS_OK);
        continuation.complete();
        assert_eq!((begins.get(), enters.get(), effects.get()), (1, 1, 1));
    }
}

#[test]
#[should_panic(expected = "child Free operation before its parent's Done")]
fn bad_nested_operation_cannot_steal_a_waiting_parents_ordinal() {
    let mut records = Records::default();
    let parent = records.prepare(TASK, site(Kind::CloseMutex), Owners::default());
    assert_eq!(
        records
            .active(TASK, parent)
            .poll(|| Some(41), |_| Ok(false), |_| unreachable!()),
        Ok(Poll::Pending)
    );
    records.prepare(TASK, site(Kind::CloseChannel), Owners::default());
}

#[test]
fn good_nested_effects_keep_pinned_ancestors_and_accumulate_cleanup_failure_in_order() {
    let mut records = Records::default();
    let parent = records.prepare(TASK, site(Kind::CloseChannel), Owners::default());
    complete(records.active(TASK, parent));
    records.active(TASK, parent).releasing();
    let address = records.active(TASK, parent) as *mut Record;
    let mut children = Vec::new();
    for _ in 0..32 {
        let child = records.prepare(TASK, site(Kind::CloseMutex), Owners::default());
        complete(records.active(TASK, child));
        records.active(TASK, child).releasing();
        children.push(child);
    }
    // The active child holds its own failure status; a failed cleanup still
    // finishes its descendants and hands status back one level at a time.
    let mut status = STATUS_FAILURE;
    for child in children.into_iter().rev() {
        let record = records.active(TASK, child);
        record.released(status);
        record.finish();
        status = records.retire(TASK, child);
        assert_eq!(status, STATUS_FAILURE);
    }
    assert_eq!(records.active(TASK, parent) as *mut Record, address);
    records.active(TASK, parent).released(status);
    records.active(TASK, parent).finish();
    assert_eq!(records.retire(TASK, parent), STATUS_FAILURE);
    assert!(records.stack.is_empty());
}

#[test]
#[should_panic(expected = "stale or paused Free tail")]
fn bad_paused_parent_cannot_run_before_nested_cleanup() {
    let mut records = Records::default();
    let parent = records.prepare(TASK, site(Kind::CloseChannel), Owners::default());
    complete(records.active(TASK, parent));
    records.prepare(TASK, site(Kind::CloseMutex), Owners::default());
    records.active(TASK, parent);
}

#[test]
#[should_panic(expected = "pending Free tail is not a cleanup completion")]
fn bad_pending_status_cannot_be_orred_into_cleanup_failure() {
    let mut records = Records::default();
    let ticket = records.prepare(TASK, site(Kind::CloseMutex), Owners::default());
    complete(records.active(TASK, ticket));
    records.active(TASK, ticket).releasing();
    records
        .active(TASK, ticket)
        .released(crate::STATUS_SUSPENDED);
}

#[test]
fn good_generation_and_invocation_are_checked_even_at_a_repeated_site() {
    let mut records = Records::default();
    let first = records.prepare(TASK, site(Kind::Put), Owners::default());
    complete(records.active(TASK, first));
    records.active(TASK, first).finish();
    records.retire(TASK, first);
    let second = records.prepare(TASK, site(Kind::Put), Owners::default());
    assert_ne!(first, second);
    for (task, ticket) in [
        (TASK, first),
        (
            TaskKey {
                generation: 8,
                ..TASK
            },
            second,
        ),
    ] {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                records.active(task, ticket);
            }))
            .is_err()
        );
    }
    complete(records.active(TASK, second));
    records.active(TASK, second).finish();
    records.retire(TASK, second);
}

#[test]
#[should_panic(expected = "Free tail lost a payload owner")]
fn bad_completion_cannot_drop_an_unconsumed_carrier() {
    let mut records = Records::default();
    let ticket = records.prepare(
        TASK,
        site(Kind::Put),
        Owners {
            image: Some(Ring::new(16, 1)),
            ..Owners::default()
        },
    );
    complete(records.active(TASK, ticket));
    records.active(TASK, ticket).finish();
}

#[test]
fn good_stop_before_and_after_done_reclaims_only_physical_owners() {
    for cut in 0..4 {
        let chain = Arc::new(Vec::new());
        let weak = Arc::downgrade(&chain);
        let mut records = Records::default();
        let ticket = records.prepare(
            TASK,
            site(Kind::Kill),
            Owners {
                chain: Some(chain),
                image: Some(Ring::new(16, 1)),
                ..Owners::default()
            },
        );
        if cut == 1 {
            assert_eq!(
                records.active(TASK, ticket).poll(
                    || Some(41),
                    |_| Err(Stop::Requested),
                    |_| panic!("unadmitted effect")
                ),
                Err(Stop::Requested)
            );
        } else if cut == 2 {
            assert_eq!(
                records.active(TASK, ticket).poll(
                    || Some(41),
                    |_| Ok(true),
                    |_| Err(Stop::Requested)
                ),
                Err(Stop::Requested)
            );
        } else if cut == 3 {
            complete(records.active(TASK, ticket));
        }
        drop(records);
        assert!(
            weak.upgrade().is_none(),
            "run stop does not retain failure storage"
        );
    }
}

#[test]
#[should_panic(expected = "terminal or active Free tail")]
fn bad_stopped_effect_cannot_be_retried() {
    let mut records = Records::default();
    let ticket = records.prepare(TASK, site(Kind::Cause), Owners::default());
    assert_eq!(
        records
            .active(TASK, ticket)
            .poll(|| Some(41), |_| Ok(true), |_| Err(Stop::Requested)),
        Err(Stop::Requested)
    );
    complete(records.active(TASK, ticket));
}

#[test]
fn good_task_context_owns_tickets_across_terminal_teardown() {
    let mut chain = Some(Arc::new(Vec::new()));
    let weak = Arc::downgrade(chain.as_ref().unwrap());
    let mut run = crate::host::BodyRun::new(1, false);
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        |ctx| unsafe { super::super::enable(ctx) },
        move |ctx| unsafe {
            let task = crate::tasks::role::with_role(ctx, |s| s.key(s.current));
            (*ctx).free_ops.tails.prepare(
                task,
                site(Kind::Kill),
                Owners {
                    chain: chain.take(),
                    ..Owners::default()
                },
            );
            STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(STATUS_OK));
    assert!(weak.upgrade().is_none());
}

#[test]
fn good_real_recorder_keeps_one_ordinal_and_cause_snapshot_across_pending_polls() {
    use crate::failure::{FailureRecord, FailureState};
    use crate::tasks::free::record;
    use hirsel_free::phase::{Phase, Progress};

    for durable in [false, true] {
        let recording = record::Recording::new(b"owned tail ordinal", durable);
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(1, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let observed = calls.clone();
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let task = crate::tasks::role::with_role(ctx, |s| s.key(s.current));
                let failure = |rule| FailureRecord {
                    kind: 1,
                    rule,
                    site: 101,
                    operation_site: 201,
                };
                let mut original = FailureState::default();
                original.record(failure(11));
                let snapshot = Arc::new(original.chain(&[2]));
                let ticket = (*ctx).free_ops.tails.prepare(
                    task,
                    site(Kind::Cause),
                    Owners {
                        chain: Some(snapshot),
                        descriptor: Some(17),
                        ..Owners::default()
                    },
                );
                let record = (*ctx).free_ops.tails.active(task, ticket) as *mut Record;
                (*ctx).failure.record(failure(13));
                for poll in 0..6 {
                    let result = (*record)
                        .poll(
                            || {
                                record::begin(ctx);
                                Some(record::position(ctx).1.op)
                            },
                            |entered| {
                                if !*entered {
                                    record::publish(ctx, Phase::Entered, |_| ())?;
                                    *entered = true;
                                }
                                Ok(poll == 5)
                            },
                            |owners| {
                                record::publish(ctx, Phase::Done, |_| {
                                    observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                })?;
                                // Cause append is a post-publication suffix;
                                // its failure-vector allocation is outside the section.
                                let chain = owners.chain.take().unwrap();
                                (*ctx).failure.append_chain(chain.as_ref().clone());
                                assert_eq!(owners.descriptor, Some(17));
                                Ok(STATUS_OK)
                            },
                        )
                        .unwrap();
                    assert_eq!(
                        record::position(ctx).1.op,
                        1,
                        "pending must keep the original ordinal"
                    );
                    assert_eq!(
                        result,
                        if poll == 5 {
                            Poll::Ready(STATUS_OK)
                        } else {
                            Poll::Pending
                        }
                    );
                }
                assert_eq!(
                    (*record).poll(
                        || panic!("begin repeated"),
                        |_| panic!("wait repeated"),
                        |_| panic!("cause appended twice")
                    ),
                    Ok(Poll::Ready(STATUS_OK))
                );
                // Later cleanup appends after the admitted snapshot, without
                // changing that snapshot or repeating FAIL-4 on resumption.
                (*ctx).failure.record(failure(19));
                assert_eq!(
                    (*ctx)
                        .failure
                        .chain(&[])
                        .iter()
                        .map(|(r, _)| r.rule)
                        .collect::<Vec<_>>(),
                    [13, 11, 19]
                );
                crate::failure::hirsel_rt_v2_failure_clear(ctx);
                (*record).finish();
                assert_eq!((*ctx).free_ops.tails.retire(task, ticket), STATUS_OK);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Done));
    }
}

#[test]
#[should_panic(expected = "Free tail lost a cause owner")]
fn bad_completion_cannot_silently_discard_an_unappended_cause() {
    let mut records = Records::default();
    let ticket = records.prepare(
        TASK,
        site(Kind::Cause),
        Owners {
            chain: Some(Arc::new(Vec::new())),
            ..Owners::default()
        },
    );
    complete(records.active(TASK, ticket));
    records.active(TASK, ticket).finish();
}
