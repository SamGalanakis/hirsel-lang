//! Tokio's pending/readiness and FIFO vectors (ce685ee6,
//! tests/sync_mutex.rs:43-59,184-204) with compact task custody. Unlike
//! Tokio's future, a Hirsel take discards its stack, but keeps its one FIFO
//! reservation and recording ordinal. No upstream ignore is carried over.
use super::tests::{Layouts, data, isolated_at, mutex, plain, wait_until, wrapper};
use super::*;
use crate::tasks::free::record::Recording;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const N: usize = 3;
const NONE: [u64; 2] = [0, 0];

struct Shared {
    handle: u64,
    labels: AtomicU32,
    ids: [AtomicU32; N],
    resolved: AtomicU32,
    go: AtomicU32,
    saving: AtomicBool,
    release_save: AtomicBool,
    leaves: [AtomicU32; N],
    order: Mutex<Vec<(u32, u32, u64)>>,
}

unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    env: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        let shared = data::<Shared>(env);
        let payload = frame.add(16).cast::<u64>();
        if frame.cast::<u32>().read() == 2 {
            return hirsel_rt_v2_task_finish(
                ctx,
                payload.cast(),
                NONE.as_ptr().cast(),
                16,
                u32::MAX,
            );
        }
        if frame.cast::<u32>().read() == 0 {
            let label = shared.labels.fetch_add(1, Ordering::AcqRel);
            frame.add(8).cast::<u32>().write(label);
            shared.ids[label as usize].store((*ctx).task_id, Ordering::Release);
            let mut status = 99;
            // Resolve an ordinary live lease before the certified boundary.
            assert_eq!(
                hirsel_rt_v2_mutex_take(ctx, &mut status, &shared.handle, payload.cast()),
                0
            );
            assert_eq!(status, 0);
            assert_eq!(
                hirsel_rt_v2_mutex_put(ctx, &shared.handle, payload.cast()),
                0
            );
            shared.resolved.fetch_add(1, Ordering::AcqRel);
            wait_until("the waiter's FIFO registration turn", || {
                shared.go.load(Ordering::Acquire) > label
            });
            frame.cast::<u32>().write(1);
            assert_eq!(
                crate::continuation::hirsel_rt_v2_resume_point(
                    ctx,
                    frame,
                    1 | crate::RESUME_LEAVES
                ),
                0
            );
        }
        let label = frame.add(8).cast::<u32>().read();
        let out = frame.add(32).cast::<u32>();
        out.write(99);
        let status = hirsel_rt_v2_mutex_take(ctx, out, &shared.handle, payload.cast());
        if status == crate::STATUS_SUSPENDED {
            assert_eq!(out.read(), 99, "a pending take writes no result");
            assert_eq!(
                std::slice::from_raw_parts(payload, 2),
                NONE,
                "a pending take owns no payload"
            );
            return status;
        }
        assert_eq!(status, 0);
        let dead = out.read();
        shared
            .order
            .lock()
            .unwrap()
            .push((label, dead, payload.add(1).read()));
        if dead == 0 {
            assert_eq!(payload.read(), 1);
            payload.add(1).write(payload.add(1).read() + 1);
            assert_eq!(
                hirsel_rt_v2_mutex_put(ctx, &shared.handle, payload.cast()),
                0
            );
            assert_eq!(std::slice::from_raw_parts(payload, 2), NONE);
        } else {
            assert_eq!(std::slice::from_raw_parts(payload, 2), NONE);
            crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 13, 103, 203);
            hirsel_rt_v2_mutex_cause(ctx, &shared.handle);
            assert_eq!(
                (*ctx)
                    .failure
                    .chain(&[])
                    .iter()
                    .map(|(r, _)| r.rule)
                    .collect::<Vec<_>>(),
                [13, 11]
            );
            crate::failure::hirsel_rt_v2_failure_clear(ctx);
        }
        // The held result is in the compact frame as well.
        payload.write(1);
        payload.add(1).write(100 + u64::from(label));
        frame.cast::<u32>().write(2);
        hirsel_rt_v2_task_finish(ctx, payload.cast(), NONE.as_ptr().cast(), 16, u32::MAX)
    }
}

unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 48) }
}

#[test]
fn compact_takers_keep_fifo_and_tokens_across_spurious_duplicate_and_early_wakes() {
    let name = "tasks::free::objects::compact_mutex_tests::compact_takers_keep_fifo_and_tokens_across_spurious_duplicate_and_early_wakes";
    if isolated_at(name, "HIRSEL_COMPACT_MUTEX_VECTORS") {
        vectors(true);
    }
}

#[test]
fn compact_takers_reclaim_internal_stop_after_physical_save() {
    let name = "tasks::free::objects::compact_mutex_tests::compact_takers_reclaim_internal_stop_after_physical_save";
    if isolated_at(name, "HIRSEL_COMPACT_MUTEX_INTERNAL_STOP") {
        vectors(false);
    }
}

fn vectors(external_stop: bool) {
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    for recorded in [false, true] {
        for end in 0..3 {
            let dead = end == 1;
            let stopped = end == 2;
            let recording = recorded.then(|| Recording::new(b"compact mutex vectors", false));
            let install = recording.clone();
            let flush = recording.clone();
            let mut run = crate::host::BodyRun::new(4, false);
            let reclaimed = Arc::new(Mutex::new(None::<std::sync::Weak<crate::sync::MutexCell>>));
            let observe_reclaimed = reclaimed.clone();
            let control = run.control();
            let monitor = control.clone();
            run.free = true;
            let mut pointers = Vec::new();
            layouts.install(&mut run, &mut pointers);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    if let Some(r) = &install {
                        r.root(ctx);
                    }
                },
                move |ctx| unsafe {
                    let handle = mutex(ctx, 7);
                    let shared = Arc::new(Shared {
                        handle,
                        labels: AtomicU32::new(0),
                        ids: Default::default(),
                        resolved: AtomicU32::new(0),
                        go: AtomicU32::new(0),
                        saving: AtomicBool::new(false),
                        release_save: AtomicBool::new(false),
                        leaves: Default::default(),
                        order: Mutex::default(),
                    });
                    let hook = shared.clone();
                    crate::pool::free::test_before_save(ctx, move |task, completed| {
                        if !completed
                            && hook.go.load(Ordering::Acquire) > 0
                            && task == hook.ids[0].load(Ordering::Acquire)
                            && !hook.release_save.load(Ordering::Acquire)
                        {
                            hook.saving.store(true, Ordering::Release);
                            wait_until("the root's wake before save", || {
                                hook.release_save.load(Ordering::Acquire)
                            });
                        }
                    });
                    let hook = shared.clone();
                    crate::pool::free::test_after_compact(ctx, move |task, _, pooled| {
                        for (label, id) in hook.ids.iter().enumerate() {
                            if id.load(Ordering::Acquire) == task {
                                assert!(pooled, "the discarded stack is returned to its worker");
                                hook.leaves[label].fetch_add(1, Ordering::AcqRel);
                            }
                        }
                    });
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    let mut children = [0u64; N];
                    for task in &mut children {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            4,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env)
                            .cast::<*const Shared>()
                            .write(Arc::as_ptr(&shared));
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            task,
                            &scope,
                            child,
                            env,
                            0,
                        );
                    }
                    wait_until("leases resolved", || {
                        shared.resolved.load(Ordering::Acquire) == N as u32
                    });
                    let mut payload = NONE;
                    let mut status = 99;
                    hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
                    assert_eq!((status, payload), (0, [1, 7]));
                    let word = mutex_word(&handle);
                    let cell = {
                        let _role = role::Guard::new(ctx);
                        role::with_held(ctx, |s| s.sync.mutex(word))
                    };
                    for label in 0..N {
                        shared.go.store(label as u32 + 1, Ordering::Release);
                        wait_until("one FIFO link per child", || {
                            cell.lock().permit.waiting() == label + 1
                        });
                    }
                    wait_until("first save held", || shared.saving.load(Ordering::Acquire));
                    let first = shared.ids[0].load(Ordering::Acquire);
                    let token = {
                        let _role = role::Guard::new(ctx);
                        role::with_held(ctx, |s| {
                            let row = s.task(first);
                            assert!(row.stack.is_some());
                            assert!(row.wait.is_some());
                            crate::pool::free::token((*ctx).run_ptr(), s.key(first), Owner::Any)
                                .unwrap()
                        })
                    };
                    // A duplicate spurious wake before save must reissue without
                    // adding a queue link or completing its recording ordinal.
                    crate::pool::free::notify((*ctx).run_ptr(), &token);
                    crate::pool::free::notify((*ctx).run_ptr(), &token);
                    shared.release_save.store(true, Ordering::Release);
                    wait_until("the spurious reissue leaves again", || {
                        shared.leaves[0].load(Ordering::Acquire) >= 2
                    });
                    wait_until("every taker's physical save completes", || {
                        let _role = role::Guard::new(ctx);
                        role::with_held(ctx, |s| {
                            shared
                                .ids
                                .iter()
                                .all(|id| s.task(id.load(Ordering::Acquire)).stack.is_none())
                        })
                    });
                    {
                        let _role = role::Guard::new(ctx);
                        for id in &shared.ids {
                            let row = scheduler(ctx).task(id.load(Ordering::Acquire));
                            assert!(
                                row.stack.is_none(),
                                "a pending compact take retains no stack"
                            );
                        }
                    }
                    assert_eq!(cell.lock().permit.waiting(), N);
                    assert!(shared.order.lock().unwrap().is_empty());
                    // Permit waits are not cancellation points. Cancellation of
                    // the reserved front must not let a younger waiter overtake.
                    let front = children.iter().find(|&&task| task as u32 == first).unwrap();
                    hirsel_rt_v2_task_cancel(ctx, &scope, front);
                    if let Some(r) = &flush {
                        r.recorder.flush().unwrap().unwrap();
                    }
                    if stopped {
                        *observe_reclaimed.lock().unwrap() = Some(Arc::downgrade(&cell));
                        drop(token);
                        drop(cell);
                        drop(shared);
                        if external_stop {
                            // Keep the original external-stop-before-park
                            // ordering: home may return before this root saves.
                            control.stop();
                            crate::tasks::park_final(ctx);
                        }
                        crate::stop::raise(ctx, crate::stop::Stop::Requested);
                    }
                    if dead {
                        crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 11, 101, 201);
                        hirsel_rt_v2_mutex_kill(ctx, &handle);
                        crate::failure::hirsel_rt_v2_failure_clear(ctx);
                    } else {
                        hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                        assert_eq!(payload, NONE);
                    }
                    crate::pool::free::notify((*ctx).run_ptr(), &token);
                    crate::pool::free::notify((*ctx).run_ptr(), &token);
                    drop(token);
                    let mut results = Vec::new();
                    for task in &mut children {
                        let mut result = NONE;
                        hirsel_rt_v2_task_join(
                            ctx,
                            0,
                            0,
                            &scope,
                            task,
                            result.as_mut_ptr().cast(),
                            16,
                        );
                        assert_eq!(result[0], 1);
                        assert!(
                            (100..103).contains(&result[1]),
                            "held result has exactly one owner"
                        );
                        results.push(result[1]);
                    }
                    results.sort_unstable();
                    assert_eq!(results, [100, 101, 102]);
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                    let mut order = shared.order.lock().unwrap().clone();
                    if dead {
                        order.sort_unstable();
                        assert_eq!(order, [(0, 1, 0), (1, 1, 0), (2, 1, 0)]);
                    } else {
                        assert_eq!(order, [(0, 0, 7), (1, 0, 8), (2, 0, 9)]);
                        hirsel_rt_v2_mutex_take(
                            ctx,
                            &mut status,
                            &handle,
                            payload.as_mut_ptr().cast(),
                        );
                        assert_eq!((status, payload), (0, [1, 10]));
                        hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                    }
                    assert_eq!(cell.lock().permit.waiting(), 0);
                    assert!(
                        cell.lock().take_tokens().is_empty(),
                        "no wake token retained after acquisition/death"
                    );
                    STATUS_OK
                },
            );
            assert_eq!(
                outcome.ended,
                if stopped {
                    Err(crate::stop::Stop::Requested)
                } else {
                    Ok(STATUS_OK)
                },
                "recorded={recorded} end={end}"
            );
            if stopped && external_stop {
                // The external stop may return while the stopping root is
                // still running. It must quiesce and release all takers, with
                // the same zero outstanding owners as an internal stop.
                wait_until("external stop reclaims every active owner", || {
                    monitor.status().reclamation_pending == 0
                });
                assert_eq!(monitor.status().reclamation_pending, 0);
            } else {
                assert_eq!(outcome.pending, 0);
            }
            if stopped {
                wait_until(
                    "stopped take custody releases its cell and queued tokens",
                    || {
                        reclaimed
                            .lock()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .upgrade()
                            .is_none()
                    },
                );
            }
            if let Some(r) = recording {
                r.recorder
                    .stop(if stopped { "stopped" } else { "returned" }, None)
                    .unwrap()
                    .unwrap();
                let trace = r.trace();
                for task in 0..N {
                    let takes: Vec<_> = trace
                        .iter()
                        .filter(|s| s.task == [task as u32])
                        .filter_map(|s| match &s.event {
                            hirsel_free::projection::Event::Versioned { outcome, .. }
                                if outcome == "take" =>
                            {
                                Some(s.progress.op)
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(
                        takes,
                        if stopped { vec![1] } else { vec![1, 3] },
                        "one Done at the original take ordinal, despite reissue"
                    );
                }
                let bytes = Arc::try_unwrap(r)
                    .ok()
                    .expect("all task recorder owners retired")
                    .recorder
                    .into_sink()
                    .bytes;
                if stopped {
                    let tape = hirsel_free::tape::parse(&bytes).unwrap();
                    for task in 0..N {
                        assert_eq!(
                            tape.cut.get(&vec![task as u32]),
                            Some(&hirsel_free::phase::Progress::new(3, Phase::Entered)),
                            "a stopped queued take retains the original Entered phase"
                        );
                    }
                }
            }
        }
    }
}
