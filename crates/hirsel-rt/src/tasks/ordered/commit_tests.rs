use super::*;
use crate::tasks::{self, role};
use hirsel_run_config::{Commit, Reorder};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct Siblings {
    started: AtomicUsize,
    workers: Mutex<std::collections::HashSet<std::thread::ThreadId>>,
    commit: Mutex<std::collections::HashSet<std::thread::ThreadId>>,
}

unsafe extern "C" fn sibling(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    let [probe, atomic, identity, width] =
        unsafe { crate::alloc::payload(environment).cast::<[u64; 4]>().read() };
    let probe = unsafe { &*(probe as *const Siblings) };
    if identity == 1 {
        if width > 1 {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while probe.started.load(Ordering::Acquire) & 1 == 0 {
                assert!(
                    std::time::Instant::now() < until,
                    "older sibling did not enter its pure segment"
                );
                std::thread::yield_now();
            }
        }
        probe
            .workers
            .lock()
            .unwrap()
            .insert(std::thread::current().id());
        probe.started.fetch_or(2, Ordering::Release);
    }
    let value = 10 + identity;
    unsafe { tasks::hirsel_rt_v2_atomic_store(ctx, &atomic, (&raw const value).cast(), 8) };
    if identity == 0 {
        probe
            .workers
            .lock()
            .unwrap()
            .insert(std::thread::current().id());
        probe.started.fetch_or(1, Ordering::Release);
    }
    if width > 1 && identity == 0 {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while probe.started.load(Ordering::Acquire) != 3 {
            assert!(
                std::time::Instant::now() < until,
                "independent sibling did not run ahead"
            );
            std::thread::yield_now();
        }
    }
    for step in 0..4 {
        let _role = unsafe { role::Guard::new(ctx) };
        if let Some(service) = unsafe { (*ctx).run_ref().ordered_commit.get() } {
            let id = unsafe { service.call(ctx, || std::thread::current().id()) };
            probe.commit.lock().unwrap().insert(id);
        }
        drop(_role);
        let value = 100 + identity + step;
        unsafe { tasks::hirsel_rt_v2_atomic_store(ctx, &atomic, (&raw const value).cast(), 8) };
    }
    let mut result = [1u64, identity];
    unsafe {
        tasks::hirsel_rt_v2_task_finish(
            ctx,
            result.as_mut_ptr().cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            3,
        )
    }
}

#[test]
fn dedicated_commit_competing_siblings_use_actual_workers_and_one_commit_thread() {
    use hirsel_core_contract::handle_scan::{TYPED_HANDLE, WORDS};
    let scan = [
        TYPED_HANDLE,
        8,
        crate::ordered::objects::Kind::Atomic.code(),
        2,
        0,
        0,
        0,
        0,
    ];
    assert_eq!(scan.len(), WORDS);
    let layout = crate::Layout {
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
        scan: scan.as_ptr(),
        scan_words: scan.len() as u64,
    };
    let result = crate::Layout {
        size: 16,
        scan: std::ptr::null(),
        scan_words: 0,
        ..layout
    };
    let layouts = [&layout as *const crate::Layout, &result];
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [Reorder::Off, Reorder::Auto] {
            for commit in [Commit::Shared, Commit::Dedicated] {
                let probe = Arc::new(Siblings::default());
                let seen = probe.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let (outcome, trace) = run.run_traced(
                    move |ctx| unsafe {
                        let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                        policy.commit = commit;
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        events::enable(ctx);
                        use super::values::{Field, Node, Program, Tag};
                        events::install_value(
                            ctx,
                            3,
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
                            .unwrap(),
                        );
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        tasks::hirsel_rt_v2_task_scope_enter(
                            ctx,
                            std::ptr::null_mut(),
                            &mut scope,
                            &0,
                        );
                        let mut atomics = [0u64; 2];
                        for atomic in &mut atomics {
                            tasks::hirsel_rt_v2_atomic_create(
                                ctx,
                                std::ptr::null_mut(),
                                atomic,
                                (&0u64 as *const u64).cast(),
                                8,
                            );
                        }
                        for identity in 0..2 {
                            let environment = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                32,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(environment)
                                .cast::<[u64; 4]>()
                                .write([
                                    Arc::as_ptr(&seen) as u64,
                                    atomics[identity],
                                    identity as u64,
                                    u64::from(width),
                                ]);
                            let mut task = 0;
                            tasks::hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut task,
                                &scope,
                                sibling,
                                environment,
                                0,
                            );
                        }
                        tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                        for (identity, atomic) in atomics.iter().enumerate() {
                            let mut value = 0u64;
                            tasks::hirsel_rt_v2_atomic_load(
                                ctx,
                                atomic,
                                (&raw mut value).cast(),
                                8,
                            );
                            assert_eq!(value, 103 + identity as u64);
                        }
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert_eq!(probe.started.load(Ordering::Acquire), 3);
                if width > 1 {
                    assert!(probe.workers.lock().unwrap().len() >= 2);
                }
                assert_eq!(
                    probe.commit.lock().unwrap().len(),
                    usize::from(commit == Commit::Dedicated)
                );
                let trace = trace.expect("competing sibling canonical bytes");
                if let Some(reference) = &reference {
                    assert_eq!(trace.bytes(), reference);
                } else {
                    reference = Some(trace.bytes().to_vec());
                }
            }
        }
    }
}

#[test]
fn dedicated_commit_runs_on_one_distinct_thread_and_matches_shared_canonical_bytes() {
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [Reorder::Off, Reorder::Auto] {
            for commit in [Commit::Shared, Commit::Dedicated] {
                let mut run = crate::host::BodyRun::new(width, true);
                unsafe { run.use_physical_dispatch() };
                let (outcome, trace) = run.run_traced(
                    move |ctx| unsafe {
                        let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                        policy.commit = commit;
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        events::enable(ctx);
                    },
                    move |ctx| unsafe {
                        let mut atomic = 0;
                        let initial = 17u64;
                        tasks::hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut atomic,
                            (&raw const initial).cast(),
                            8,
                        );
                        for value in [18u64, 19, 23] {
                            tasks::hirsel_rt_v2_atomic_store(
                                ctx,
                                &atomic,
                                (&raw const value).cast(),
                                8,
                            );
                            let mut actual = 0u64;
                            tasks::hirsel_rt_v2_atomic_load(
                                ctx,
                                &atomic,
                                (&raw mut actual).cast(),
                                8,
                            );
                            assert_eq!(actual, value);
                        }
                        let _role = role::Guard::new(ctx);
                        if let Some(service) = (*ctx).run_ref().ordered_commit.get() {
                            assert!(
                                service.calls() >= 7,
                                "actual native commits reached the dedicated executor"
                            );
                            let caller = std::thread::current().id();
                            let mut dedicated = None;
                            for _ in 0..4 {
                                let id = service.call(ctx, || {
                                    assert!(crate::pool::ordered_commit::is_current());
                                    let before = crate::pool::compute_permits().1;
                                    assert!(before >= 1);
                                    std::thread::current().id()
                                });
                                assert_ne!(id, caller);
                                if let Some(prior) = dedicated {
                                    assert_eq!(id, prior);
                                }
                                dedicated = Some(id);
                            }
                        }
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let trace = trace.expect("shared/dedicated canonical trace");
                if let Some(reference) = &reference {
                    assert_eq!(trace.bytes(), reference);
                } else {
                    reference = Some(trace.bytes().to_vec());
                }
            }
        }
    }
}

#[test]
fn dedicated_commit_lends_one_process_permit_and_preserves_affine_head_callbacks() {
    const CHILD: &str = "HIRSEL_ORDERED_DEDICATED_ONE_PERMIT";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::ordered::commit_tests::dedicated_commit_lends_one_process_permit_and_preserves_affine_head_callbacks", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    crate::pool::set_compute_grant(1);
    dedicated_commit_runs_on_one_distinct_thread_and_matches_shared_canonical_bytes();
    for width in [1, 8] {
        let mut run = crate::host::BodyRun::new(width, true);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                policy.commit = Commit::Dedicated;
                engine::enable(ctx, policy);
            },
            |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                let caller = std::thread::current().id();
                let (value, progress) = engine::head_effect(ctx, "host.request", || {
                    assert_eq!(std::thread::current().id(), caller);
                    assert!(!crate::pool::ordered_commit::is_current());
                    assert_eq!(crate::pool::compute_permits(), (1, 1, 0));
                    (41, Next::Keep)
                })
                .unwrap();
                assert_eq!(value, 41);
                engine::continue_at_head(ctx, progress).unwrap();
                assert_eq!(crate::pool::compute_permits(), (1, 1, 0));
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(crate::pool::compute_permits().1, 0);
    }
}

#[test]
fn dedicated_commit_stop_retains_inflight_callback_loans_and_reclaims_the_lent_permit() {
    const CHILD: &str = "HIRSEL_ORDERED_DEDICATED_STOP";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::ordered::commit_tests::dedicated_commit_stop_retains_inflight_callback_loans_and_reclaims_the_lent_permit", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    crate::pool::set_compute_grant(1);
    for affine in [false, true] {
        let entered = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        let (inside, release, end) = (entered.clone(), released.clone(), finished.clone());
        let mut run = crate::host::BodyRun::new(8, true);
        unsafe { run.use_physical_dispatch() };
        let control = run.control();
        let stopping = control.clone();
        let stopper = std::thread::spawn(move || {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while entered.load(Ordering::Acquire) == 0 {
                assert!(
                    std::time::Instant::now() < until,
                    "callback loan did not enter"
                );
                std::thread::yield_now();
            }
            stopping.stop();
        });
        let outcome = run.run(
            |ctx| unsafe {
                let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                policy.commit = Commit::Dedicated;
                engine::enable(ctx, policy);
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                let callback = || {
                    inside.store(1, Ordering::Release);
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while release.load(Ordering::Acquire) == 0 {
                        assert!(
                            std::time::Instant::now() < until,
                            "host did not release bounded test callback"
                        );
                        std::thread::yield_now();
                    }
                    end.fetch_add(1, Ordering::Release);
                    42
                };
                if affine {
                    let (value, progress) =
                        engine::head_effect(ctx, "host.request", || (callback(), Next::Keep))
                            .unwrap();
                    assert_eq!(value, 42);
                    crate::stop::check(ctx, engine::continue_at_head(ctx, progress));
                } else {
                    let service = (*ctx).run_ref().ordered_commit.get().unwrap();
                    assert_eq!(service.call(ctx, callback), 42);
                    crate::stop::check(ctx, crate::stop::observe(ctx));
                }
                panic!("stopped callback resumed compiled suffix")
            },
        );
        stopper.join().unwrap();
        assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
        assert_eq!(outcome.pending, 1);
        assert_eq!(finished.load(Ordering::Acquire), 0);
        assert_eq!(crate::pool::lost_compute_permits(), 1);
        assert_eq!(crate::pool::compute_permits().1, 1);
        released.store(1, Ordering::Release);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while control.status().reclamation_pending != 0 {
            assert!(
                std::time::Instant::now() < until,
                "callback run was not reclaimed"
            );
            std::thread::yield_now();
        }
        assert_eq!(finished.load(Ordering::Acquire), 1);
        assert_eq!(crate::pool::lost_compute_permits(), 0);
        assert_eq!(crate::pool::compute_permits().1, 0);
    }
}
