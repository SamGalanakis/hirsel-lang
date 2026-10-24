//! Head-only serial gates and saved-frame foreign thread dispatch.
use super::super::*;
use super::engine;
use crate::ordered::{Next, Progress};

pub(crate) fn enter(ctx: *mut TaskCtx) -> Stopped<()> {
    engine::head_effect(ctx, "extern.call", || ((), Next::Keep))?;
    Ok(())
}

/// Pin only the outer interval. Nested calls share that interval's owner.
pub(crate) fn pin(ctx: *mut TaskCtx) -> bool {
    let Some(owner) = crate::pool::pin_foreign_compute() else {
        return false;
    };
    role::with_role(ctx, |s| {
        let me = s.current;
        let row = s.task(me);
        unpin(row);
        assert_eq!(row.owner, Owner::Any);
        row.owner = owner;
    });
    true
}

pub(crate) fn unpin_compute(ctx: *mut TaskCtx) {
    let owner = role::with_role(ctx, |s| {
        let me = s.current;
        std::mem::replace(&mut s.task(me).owner, Owner::Any)
    });
    crate::pool::unpin_foreign_compute(owner);
}

pub(crate) fn gate_enter(ctx: *mut TaskCtx, gate: u32, site: u64) -> Stopped<bool> {
    engine::at_head(ctx)?;
    let nested = role::with_role(ctx, |s| {
        let me = s.current;
        let row = s.gate(gate);
        let nested = row.owner == Some(me);
        if nested {
            row.depth += 1;
        }
        nested
    });
    if nested {
        return Ok(false);
    }
    let started = unsafe { (*ctx).run_ref() }
        .extern_metrics
        .as_ref()
        .map(|_| std::time::Instant::now());
    let (_, progress) = engine::head_canonical(
        ctx,
        "gate.enter",
        || {
            role::with_role(ctx, |s| {
                let me = s.current;
                let row = s.gate(gate);
                if row.owner.is_none() {
                    row.owner = Some(me);
                    row.depth = 1;
                    row.version += 1;
                    (true, Next::Keep)
                } else {
                    s.task(me).wait_site = site;
                    s.park(Wait::Gate { gate });
                    (false, Next::Park)
                }
            })
        },
        |acquired| {
            super::events::gate_event(
                ctx,
                gate,
                if *acquired {
                    super::events::CanonicalOutcome::GateAcquired
                } else {
                    super::events::CanonicalOutcome::GateWaiting
                },
            )
        },
    )?;
    if progress == Progress::Poll {
        engine::save_boundary(ctx, progress)?;
    }
    if let (Some(started), Some(metrics)) = (started, unsafe { &(*ctx).run_ref().extern_metrics }) {
        metrics.gate_entry(started.elapsed());
    }
    Ok(true)
}

pub(crate) fn gate_leave(ctx: *mut TaskCtx, gate: u32, outermost: bool) -> Stopped<()> {
    engine::at_head(ctx)?;
    let effect = || {
        role::with_role(ctx, |s| {
            let me = s.current;
            let row = s.gate(gate);
            assert_eq!(row.owner, Some(me));
            assert_eq!(row.depth == 1, outermost);
            row.depth -= 1;
            if outermost {
                row.owner = None;
                row.version += 1;
                for task in std::mem::take(&mut row.waiters) {
                    s.wake(task);
                }
            }
        });
        ((), Next::Keep)
    };
    if outermost {
        engine::head_canonical(ctx, "gate.leave", effect, |_| {
            super::events::gate_event(ctx, gate, super::events::CanonicalOutcome::GateReleased)
        })?;
    } else {
        effect();
    }
    Ok(())
}

/// # Safety
/// Outside C, owned by this head, with no worker/table borrow across migration.
pub(crate) unsafe fn to_pool(ctx: *mut TaskCtx, site: u64) -> Stopped<u16> {
    engine::at_head(ctx)?;
    let slot = loop {
        if let Some(slot) = crate::pool::claim(ctx) {
            break slot;
        }
        let (_, progress) = engine::head_continuation(ctx, || {
            role::with_role(ctx, |s| {
                let me = s.current;
                s.task(me).wait_site = site;
                s.park(Wait::Pool);
            });
            ((), Next::Park)
        })?;
        engine::save_boundary(ctx, progress)?;
    };
    role::with_role(ctx, |s| {
        let me = s.current;
        let row = s.task(me);
        unpin(row);
        row.owner = Owner::Pool(slot);
    });
    unsafe { engine::migrate(ctx) }?;
    Ok(slot)
}

/// Release physical pool capacity as soon as C has returned, before waiting
/// for this task's logical head. Reuse is safe: the old exec cannot accept a
/// new saved image until this outgoing frame reaches its idle continuation.
pub(crate) unsafe fn from_pool(ctx: *mut TaskCtx, slot: u16) -> Stopped<()> {
    let owner = role::with_role(ctx, |s| {
        let me = s.current;
        std::mem::replace(&mut s.task(me).owner, Owner::Any)
    });
    assert_eq!(owner, Owner::Pool(slot));
    crate::pool::release(ctx, slot);
    role::with_role(ctx, |s| {
        for task in std::mem::take(&mut s.pool_waiters) {
            s.wake(task);
        }
    });
    unsafe { engine::migrate(ctx) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foreign::*;
    use crate::ordered::Policy;

    #[test]
    fn ordered_home_pool_and_nested_callbacks_preserve_effective_thread_and_gate_depth() {
        ordered_home_pool_and_nested_callbacks_preserve_effective_thread_and_gate_depth_commit(
            hirsel_run_config::Commit::Shared,
        );
    }

    #[test]
    fn dedicated_commit_preserves_home_pool_and_nested_foreign_affinity() {
        const CHILD: &str = "HIRSEL_ORDERED_DEDICATED_FOREIGN_ONE_PERMIT";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tasks::ordered::foreign::tests::dedicated_commit_preserves_home_pool_and_nested_foreign_affinity", "--nocapture"])
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
        ordered_home_pool_and_nested_callbacks_preserve_effective_thread_and_gate_depth_commit(
            hirsel_run_config::Commit::Dedicated,
        );
    }

    fn ordered_home_pool_and_nested_callbacks_preserve_effective_thread_and_gate_depth_commit(
        commit: hirsel_run_config::Commit,
    ) {
        for width in [2, 4, 8] {
            let home = std::thread::current().id();
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    let mut policy = Policy::admit(&Default::default(), None);
                    policy.commit = commit;
                    engine::enable(ctx, policy);
                },
                move |ctx| unsafe {
                    for dispatch in [
                        FOREIGN_THREADS_HOME | FOREIGN_BLOCKING,
                        FOREIGN_THREADS_SERIAL | FOREIGN_BLOCKING | (7 << FOREIGN_PACKAGE_SHIFT),
                    ] {
                        let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                        hirsel_rt_v2_foreign_enter(ctx, 0, interval.as_mut_ptr().cast(), dispatch);
                        let thread = std::thread::current().id();
                        assert_eq!(
                            thread == home,
                            dispatch & FOREIGN_THREADS_MASK == FOREIGN_THREADS_HOME
                        );
                        assert_eq!(
                            crate::pool::on_pool(),
                            dispatch & FOREIGN_THREADS_MASK == FOREIGN_THREADS_SERIAL
                        );
                        assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                        let mut canceled = 99;
                        assert_eq!(
                            hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                            STATUS_OK
                        );
                        assert_eq!(
                            std::thread::current().id(),
                            thread,
                            "callback suspension stays on foreign thread"
                        );
                        let before = crate::tasks::gate_version(ctx, 7);
                        let mut nested = [0u64; FOREIGN_SCOPE_BYTES / 8];
                        // Re-enter the serial gate when already on pool; home
                        // calls retain home precedence even when blocking.
                        hirsel_rt_v2_foreign_enter(ctx, 0, nested.as_mut_ptr().cast(), dispatch);
                        assert_eq!(std::thread::current().id(), thread);
                        hirsel_rt_v2_foreign_leave(nested.as_ptr().cast());
                        if dispatch & FOREIGN_THREADS_MASK == FOREIGN_THREADS_SERIAL {
                            assert_eq!(crate::tasks::gate_version(ctx, 7), before);
                        }
                        hirsel_rt_v2_callback_leave(ctx);
                        hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                        assert!(!crate::pool::on_pool());
                        assert!(crate::pool::compute_index().is_some());
                        if dispatch & FOREIGN_THREADS_MASK == FOREIGN_THREADS_SERIAL {
                            assert_eq!(crate::tasks::gate_version(ctx, 7), before + 1);
                        }
                    }
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }
    #[test]
    fn caller_thread_callbacks_keep_their_exec_through_shrink_and_regrowth() {
        use std::sync::atomic::Ordering;
        for width in [2, 4, 8] {
            for dispatch in [
                FOREIGN_THREADS_ANY,
                FOREIGN_THREADS_SERIAL | (7 << FOREIGN_PACKAGE_SHIFT),
            ] {
                for exit_before_entry in [false, true] {
                    let mut run = crate::host::BodyRun::new(width, true);
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        |ctx| unsafe {
                            engine::enable(ctx, Policy::admit(&Default::default(), None))
                        },
                        move |ctx| unsafe {
                            // Choose a worker that shrink will remove. The test dispatcher
                            // normally migrates on every explicit migration boundary.
                            while crate::pool::compute_index() == Some(0) {
                                engine::migrate(ctx).unwrap();
                            }
                            if exit_before_entry {
                                (*ctx).run_ref().control.width.store(1, Ordering::Release);
                                // Queue retirement without transferring this running frame.
                                let _ = crate::pool::target(ctx, Owner::Any, None);
                            }
                            let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                            hirsel_rt_v2_foreign_enter(
                                ctx,
                                0,
                                interval.as_mut_ptr().cast(),
                                dispatch,
                            );
                            let thread = std::thread::current().id();
                            let owner = scheduler(ctx).task((*ctx).task_id).owner;
                            assert!(matches!(owner, Owner::Compute(_)));
                            assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                            // Resize queues Exit while this task is still running.
                            // Its next saved callback must defer that exit.
                            (*ctx).run_ref().control.width.store(1, Ordering::Release);
                            engine::migrate(ctx).unwrap();
                            for _ in 0..8 {
                                let mut canceled = 99;
                                assert_eq!(
                                    hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                                    STATUS_OK
                                );
                                assert_eq!(std::thread::current().id(), thread);
                                assert_eq!(scheduler(ctx).task((*ctx).task_id).owner, owner);
                            }
                            // Growth can reuse the old numeric worker index. Routing
                            // must retain the original exec, not select its replacement.
                            (*ctx)
                                .run_ref()
                                .control
                                .width
                                .store(width, Ordering::Release);
                            engine::migrate(ctx).unwrap();
                            let mut nested = [0u64; FOREIGN_SCOPE_BYTES / 8];
                            hirsel_rt_v2_foreign_enter(
                                ctx,
                                0,
                                nested.as_mut_ptr().cast(),
                                dispatch | FOREIGN_BLOCKING,
                            );
                            let mut canceled = 99;
                            assert_eq!(
                                hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                                STATUS_OK
                            );
                            assert_eq!(std::thread::current().id(), thread);
                            assert_eq!(scheduler(ctx).task((*ctx).task_id).owner, owner);
                            hirsel_rt_v2_foreign_leave(nested.as_ptr().cast());
                            assert_eq!(scheduler(ctx).task((*ctx).task_id).owner, owner);
                            hirsel_rt_v2_callback_leave(ctx);
                            hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                            assert_eq!(scheduler(ctx).task((*ctx).task_id).owner, Owner::Any);
                            engine::migrate(ctx).unwrap();
                            assert_ne!(std::thread::current().id(), thread);
                            STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(STATUS_OK));
                    assert_eq!(outcome.pending, 0);
                }
            }
        }
    }

    #[test]
    fn width_one_home_callbacks_keep_their_thread_across_growth_and_shrink() {
        use std::sync::atomic::Ordering;
        for commit in [
            hirsel_run_config::Commit::Shared,
            hirsel_run_config::Commit::Dedicated,
        ] {
            for dispatch in [
                FOREIGN_THREADS_ANY,
                FOREIGN_THREADS_SERIAL | (7 << FOREIGN_PACKAGE_SHIFT),
            ] {
                let home = std::thread::current().id();
                let mut run = crate::host::BodyRun::new(1, true);
                unsafe { run.use_ordered_home_dispatch() };
                let control = run.control();
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.commit = commit;
                        engine::enable(ctx, policy);
                    },
                    move |ctx| unsafe {
                        assert_eq!(std::thread::current().id(), home);
                        assert!(
                            !crate::pool::acquire_permit(ctx),
                            "Hirsel already holds its permit"
                        );
                        let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                        hirsel_rt_v2_foreign_enter(ctx, 0, interval.as_mut_ptr().cast(), dispatch);
                        let owner = scheduler(ctx).task((*ctx).task_id).owner;
                        assert!(matches!(owner, Owner::Compute(_)));
                        assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                        for width in [2, 1, 4] {
                            (*ctx)
                                .run_ref()
                                .control
                                .width
                                .store(width, Ordering::Release);
                            engine::migrate(ctx).unwrap();
                            assert_eq!(std::thread::current().id(), home);
                            assert_eq!(scheduler(ctx).task((*ctx).task_id).owner, owner);
                            assert!(!crate::pool::acquire_permit(ctx));
                            let mut nested = [0u64; FOREIGN_SCOPE_BYTES / 8];
                            hirsel_rt_v2_foreign_enter(
                                ctx,
                                0,
                                nested.as_mut_ptr().cast(),
                                dispatch | FOREIGN_BLOCKING,
                            );
                            assert_eq!(std::thread::current().id(), home);
                            hirsel_rt_v2_foreign_leave(nested.as_ptr().cast());
                        }
                        hirsel_rt_v2_callback_leave(ctx);
                        hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                        engine::migrate(ctx).unwrap();
                        assert_ne!(std::thread::current().id(), home);
                        assert!(crate::pool::compute_index().is_some());
                        assert!(!crate::pool::acquire_permit(ctx));
                        (*ctx).run_ref().control.width.store(1, Ordering::Release);
                        engine::migrate(ctx).unwrap();
                        assert_eq!(std::thread::current().id(), home);
                        assert!(!crate::pool::acquire_permit(ctx));
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert_eq!(control.worker_width(), 0);
            }
        }
    }

    #[test]
    fn a_stop_abandons_a_width_one_home_callback_after_growth() {
        use std::sync::atomic::Ordering;
        let mut run = crate::host::BodyRun::new(1, true);
        unsafe { run.use_ordered_home_dispatch() };
        let control = run.control();
        let home = std::thread::current().id();
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            move |ctx| unsafe {
                let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                hirsel_rt_v2_foreign_enter(
                    ctx,
                    0,
                    interval.as_mut_ptr().cast(),
                    FOREIGN_THREADS_ANY,
                );
                assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                (*ctx).run_ref().control.width.store(2, Ordering::Release);
                engine::migrate(ctx).unwrap();
                assert_eq!(std::thread::current().id(), home);
                control.stop();
                let mut canceled = 0;
                hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                panic!("a stopped home callback resumed");
            },
        );
        assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn a_stop_abandons_a_saved_callback_on_a_pinned_retiring_worker() {
        use std::sync::atomic::Ordering;
        for width in [2, 4, 8] {
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let control = run.control();
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
                move |ctx| unsafe {
                    while crate::pool::compute_index() == Some(0) {
                        engine::migrate(ctx).unwrap();
                    }
                    let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                    hirsel_rt_v2_foreign_enter(
                        ctx,
                        0,
                        interval.as_mut_ptr().cast(),
                        FOREIGN_THREADS_ANY,
                    );
                    assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                    (*ctx).run_ref().control.width.store(1, Ordering::Release);
                    engine::migrate(ctx).unwrap();
                    let mut canceled = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                        STATUS_OK
                    );
                    // Stop at a supported callback boundary. No C leave is
                    // required to release the pending worker retirement.
                    control.stop();
                    hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                    panic!("a stopped foreign callback resumed");
                },
            );
            assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
        }
    }

    #[test]
    fn foreign_callbacks_gates_pool_exhaustion_and_resize_run_with_one_compute_permit() {
        // Isolate the process-wide manager grant from unrelated harness tests.
        const CHILD: &str = "HIRSEL_ORDERED_FOREIGN_ONE_PERMIT";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tasks::ordered::foreign::tests::foreign_callbacks_gates_pool_exhaustion_and_resize_run_with_one_compute_permit",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        crate::pool::set_compute_grant(1);
        width_one_home_callbacks_keep_their_thread_across_growth_and_shrink();
        a_stop_abandons_a_width_one_home_callback_after_growth();
        assert_eq!(crate::pool::compute_permits(), (1, 0, 0));
        ordered_home_pool_and_nested_callbacks_preserve_effective_thread_and_gate_depth();
        a_serial_gate_waiter_parks_until_its_older_callback_leaves_the_gate();
        caller_thread_callbacks_keep_their_exec_through_shrink_and_regrowth();
        a_full_pool_retires_the_waiter_and_a_returning_foreign_call_wakes_it();
        a_stop_abandons_a_saved_callback_on_a_pinned_retiring_worker();
    }

    #[derive(Default)]
    struct GateProbe {
        allow: std::sync::atomic::AtomicBool,
        entered: std::sync::atomic::AtomicBool,
    }
    unsafe extern "C" fn contender(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        use std::sync::atomic::Ordering;
        let probe =
            unsafe { &*(crate::alloc::payload(env).cast::<u64>().read() as *const GateProbe) };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !probe.allow.load(Ordering::Acquire) {
            let mut canceled = 0;
            assert_eq!(
                unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
                STATUS_OK
            );
            assert!(std::time::Instant::now() < until);
        }
        let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
        unsafe {
            hirsel_rt_v2_foreign_enter(
                ctx,
                0,
                interval.as_mut_ptr().cast(),
                FOREIGN_THREADS_SERIAL | FOREIGN_BLOCKING | (7 << FOREIGN_PACKAGE_SHIFT),
            )
        };
        assert!(!probe.entered.swap(true, Ordering::AcqRel));
        assert!(crate::pool::on_pool());
        unsafe { hirsel_rt_v2_foreign_leave(interval.as_ptr().cast()) };
        STATUS_OK
    }

    unsafe extern "C" fn last_pool_call(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        use std::sync::atomic::Ordering;
        let probe =
            unsafe { &*(crate::alloc::payload(env).cast::<u64>().read() as *const GateProbe) };
        let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
        unsafe {
            hirsel_rt_v2_foreign_enter(
                ctx,
                0,
                interval.as_mut_ptr().cast(),
                FOREIGN_THREADS_ANY | FOREIGN_BLOCKING,
            );
            probe.allow.store(true, Ordering::Release);
            assert_eq!(hirsel_rt_v2_callback_context(), ctx);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let mut canceled = 0;
                assert_eq!(
                    hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                    STATUS_OK
                );
                engine::at_head(ctx).unwrap();
                let parked = {
                    let _role = role::Guard::new(ctx);
                    matches!(scheduler(ctx).task(0).wait, Some(Wait::Pool))
                };
                if parked {
                    break;
                }
                assert!(std::time::Instant::now() < until);
            }
            {
                let _role = role::Guard::new(ctx);
                assert!(matches!(scheduler(ctx).task(0).wait, Some(Wait::Pool)));
                assert!(readiness::saved(scheduler(ctx), 0));
            }
            probe.entered.store(true, Ordering::Release);
            hirsel_rt_v2_callback_leave(ctx);
            hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
        }
        STATUS_OK
    }

    #[test]
    fn a_full_pool_retires_the_waiter_and_a_returning_foreign_call_wakes_it() {
        use std::sync::{Arc, atomic::Ordering};
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
        for width in [2, 4, 8] {
            let probe = Arc::new(GateProbe::default());
            let seen = probe.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
                move |ctx| unsafe {
                    // Reserve every slot except the one used by the real child
                    // foreign call. Admission is still the production pool cap.
                    let reserved: Vec<_> = (0..crate::pool::POOL_THREADS - 1)
                        .map(|_| crate::pool::claim(ctx).expect("pool capacity"))
                        .collect();
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
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
                            last_pool_call,
                            env,
                            0
                        ),
                        STATUS_OK
                    );
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !seen.allow.load(Ordering::Acquire) {
                        let mut canceled = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                            STATUS_OK
                        );
                        assert!(std::time::Instant::now() < until);
                    }
                    assert!(!crate::pool::has_slot(ctx));
                    let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                    hirsel_rt_v2_foreign_enter(
                        ctx,
                        0,
                        interval.as_mut_ptr().cast(),
                        FOREIGN_THREADS_SERIAL | FOREIGN_BLOCKING | (7 << FOREIGN_PACKAGE_SHIFT),
                    );
                    assert!(seen.entered.load(Ordering::Acquire));
                    assert!(crate::pool::on_pool());
                    hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                    for slot in reserved {
                        crate::pool::release(ctx, slot);
                    }
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    assert_eq!(crate::tasks::gate_version(ctx, 7), 2);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }

    #[test]
    fn a_serial_gate_waiter_parks_until_its_older_callback_leaves_the_gate() {
        use std::sync::{Arc, atomic::Ordering};
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
        for width in [2, 4, 8] {
            let probe = Arc::new(GateProbe::default());
            let seen = probe.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
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
                            contender,
                            env,
                            0
                        ),
                        STATUS_OK
                    );
                    let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                    hirsel_rt_v2_foreign_enter(
                        ctx,
                        0,
                        interval.as_mut_ptr().cast(),
                        FOREIGN_THREADS_SERIAL | FOREIGN_BLOCKING | (7 << FOREIGN_PACKAGE_SHIFT),
                    );
                    let thread = std::thread::current().id();
                    seen.allow.store(true, Ordering::Release);
                    assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                    let mut canceled = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                        STATUS_OK
                    );
                    assert_eq!(std::thread::current().id(), thread);
                    assert!(!seen.entered.load(Ordering::Acquire));
                    {
                        let _role = role::Guard::new(ctx);
                        assert!(matches!(
                            scheduler(ctx).task(child as u32).wait,
                            Some(Wait::Gate { gate: 7 })
                        ));
                    }
                    hirsel_rt_v2_callback_leave(ctx);
                    hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    assert_eq!(crate::tasks::gate_version(ctx, 7), 4);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert!(probe.entered.load(Ordering::Acquire));
        }
    }
}
