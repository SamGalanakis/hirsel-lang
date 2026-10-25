//! Thread ownership for Free callbacks with live C frames.

use super::super::*;

/// Match the Ordered foreign owner protocol: the pool retains the exact Exec
/// through resize until its outer C frame returns. Nested calls share its pin.
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

/// Return physical pool capacity before migrating back to compute. No new
/// user/C code runs on this exec until this frame has actually reached idle.
/// # Safety
/// The C interval returned and slot owns this running task.
pub(crate) unsafe fn from_pool(ctx: &mut TaskCtx, slot: u16) {
    role::with_role(ctx, |s| {
        let me = s.current;
        assert_eq!(
            std::mem::replace(&mut s.task(me).owner, Owner::Any),
            Owner::Pool(slot)
        );
    });
    crate::pool::release(ctx, slot);
    let available = crate::pool::has_slot(ctx);
    role::with_role(ctx, |s| {
        for task in s.pool_waiters.clone() {
            s.wake(task);
        }
        unsafe { readiness::poll_free_with_pool(s, available) };
    });
    unsafe {
        super::dispatch_ready(ctx);
        super::migrate(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::*;
    use crate::foreign::*;

    unsafe fn migrate(ctx: *mut TaskCtx) {
        let _role = unsafe { role::Guard::new(ctx) };
        let last = scheduler(ctx).task(unsafe { (*ctx).task_id }).last;
        let to = crate::pool::target(ctx, Owner::Any, last).expect("a different test exec");
        // Free may reclaim an Any notification on its current worker. Use an
        // exact destination for fixture placement; no C frame is live here.
        let owner = Owner::Compute(to.expose_provenance());
        scheduler(ctx).task(unsafe { (*ctx).task_id }).owner = owner;
        unsafe {
            crate::pool::free::schedule_saved(
                (*ctx).run_ptr(),
                crate::tasks::scheduler(ctx).key((*ctx).task_id),
                owner,
            );
            super::super::suspend(&mut *ctx, true);
        }
        scheduler(ctx).task(unsafe { (*ctx).task_id }).owner = Owner::Any;
    }

    unsafe fn save_callback(ctx: *mut TaskCtx) {
        let _role = unsafe { role::Guard::new(ctx) };
        let owner = dispatch_owner(ctx);
        // Apply width changes before saving: retirement must wait for C.
        let _ = crate::pool::target(ctx, owner, None);
        unsafe {
            crate::pool::free::schedule_saved(
                (*ctx).run_ptr(),
                crate::tasks::scheduler(ctx).key((*ctx).task_id),
                owner,
            );
            super::super::suspend(&mut *ctx, true);
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
                for (recorded, durable) in [(false, false), (true, false), (true, true)] {
                    for exit_before_entry in [false, true] {
                        let recording = recorded.then(|| {
                            super::super::record::Recording::new(b"free foreign resize", durable)
                        });
                        let install = recording.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.free = true;
                        unsafe { run.use_physical_dispatch() };
                        let outcome = run.run(
                            move |ctx| unsafe {
                                super::super::enable(ctx);
                                if let Some(recording) = &install {
                                    recording.root(ctx);
                                }
                            },
                            move |ctx| unsafe {
                                // Choose a worker that shrink will remove.
                                if crate::pool::compute_index() == Some(0) {
                                    migrate(ctx);
                                }
                                assert_ne!(crate::pool::compute_index(), Some(0));
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
                                save_callback(ctx);
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
                                save_callback(ctx);
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
                                own_slot(recorded);
                                migrate(ctx);
                                assert_ne!(std::thread::current().id(), thread);
                                // The task's encoder migrated; its next phase runs on the new
                                // executor's own registration, never a carried slot.
                                own_slot(recorded);
                                let mut canceled = 99;
                                assert_eq!(
                                    hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                                    STATUS_OK
                                );
                                STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(STATUS_OK));
                        assert_eq!(outcome.pending, 0);
                        if let Some(recording) = recording {
                            recording.recorder.stop("returned", None).unwrap().unwrap();
                            assert_executors_retired(&recording);
                            assert!(std::sync::Arc::try_unwrap(recording).is_ok());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_stop_abandons_a_saved_callback_on_a_pinned_retiring_worker() {
        use std::sync::atomic::Ordering;
        for width in [2, 4, 8] {
            for (recorded, durable) in [(false, false), (true, false), (true, true)] {
                let recording = recorded
                    .then(|| super::super::record::Recording::new(b"free foreign stop", durable));
                let install = recording.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let control = run.control();
                let stop = control.clone();
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        if let Some(recording) = &install {
                            recording.root(ctx);
                        }
                    },
                    move |ctx| unsafe {
                        if crate::pool::compute_index() == Some(0) {
                            migrate(ctx);
                        }
                        assert_ne!(crate::pool::compute_index(), Some(0));
                        let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                        hirsel_rt_v2_foreign_enter(
                            ctx,
                            0,
                            interval.as_mut_ptr().cast(),
                            FOREIGN_THREADS_ANY,
                        );
                        assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                        (*ctx).run_ref().control.width.store(1, Ordering::Release);
                        save_callback(ctx);
                        let mut canceled = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                            STATUS_OK
                        );
                        // Stop at a supported callback boundary. No C leave is
                        // required to release the pending worker retirement.
                        stop.stop();
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                        panic!("a stopped foreign callback resumed");
                    },
                );
                assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while control.status().reclamation_pending != 0 {
                    assert!(
                        std::time::Instant::now() < until,
                        "pinned callback reclaims"
                    );
                    std::thread::yield_now();
                }
                assert!(control.status().finished);
                if let Some(recording) = recording {
                    recording.recorder.stop("stopped", None).unwrap().unwrap();
                    // The last holder decrements its count immediately before
                    // reclaiming RunShared. Observe destruction as well as its
                    // public zero-pending status, under the same deadline.
                    while std::sync::Arc::strong_count(&recording) != 1 {
                        assert!(
                            std::time::Instant::now() < until,
                            "recording owner reclaims"
                        );
                        std::thread::yield_now();
                    }
                    assert_executors_retired(&recording);
                    assert!(std::sync::Arc::try_unwrap(recording).is_ok());
                }
            }
        }
    }

    /// The calling executor's own registered slot in a recorded run; `None` unrecorded.
    fn own_slot(recorded: bool) -> Option<usize> {
        let own = crate::pool::registration(|own| own.and_then(|own| own.index()));
        assert_eq!(
            own.is_some(),
            recorded,
            "a recorded run's executor is registered"
        );
        own
    }

    /// Every phase ran on its executor's own registration, and every executor retired.
    fn assert_executors_retired(recording: &super::super::record::Recording) {
        use std::sync::atomic::Ordering;
        assert_eq!(recording.transients.load(Ordering::Relaxed), 0);
        assert!(recording.recorder.registered().is_empty());
    }

    #[test]
    fn home_and_pool_callbacks_record_on_their_executing_thread() {
        for width in [2, 8] {
            for durable in [false, true] {
                let recording = super::super::record::Recording::new(b"free pool record", durable);
                let install = recording.clone();
                let inspect = recording.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let home = std::thread::current().id();
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        for dispatch in [
                            FOREIGN_THREADS_HOME | FOREIGN_BLOCKING,
                            FOREIGN_THREADS_ANY | FOREIGN_BLOCKING,
                            FOREIGN_THREADS_SERIAL
                                | FOREIGN_BLOCKING
                                | (7 << FOREIGN_PACKAGE_SHIFT),
                        ] {
                            let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                            hirsel_rt_v2_foreign_enter(
                                ctx,
                                0,
                                interval.as_mut_ptr().cast(),
                                dispatch,
                            );
                            let thread = std::thread::current().id();
                            assert_eq!(
                                thread == home,
                                dispatch & FOREIGN_THREADS_MASK == FOREIGN_THREADS_HOME
                            );
                            assert_eq!(
                                crate::pool::on_pool(),
                                dispatch & FOREIGN_THREADS_MASK != FOREIGN_THREADS_HOME
                            );
                            assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                            let mut canceled = 99;
                            assert_eq!(
                                hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                                STATUS_OK
                            );
                            assert_eq!(canceled, 0);
                            // An old exec can retain this numeric preferred slot
                            // while a replacement runs (unless this executor's own
                            // registration already holds it). The phase must run on
                            // an exclusive registered slot, never alias or panic.
                            // Home and blocking callbacks phase on their own executor's
                            // registration, distinct from the posting compute worker's.
                            assert!(own_slot(true).is_some());
                            let occupied = inspect.recorder.executor(crate::pool::recorder_slot());
                            assert_eq!(
                                hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                                STATUS_OK
                            );
                            drop(occupied);
                            assert_eq!(std::thread::current().id(), thread);
                            hirsel_rt_v2_callback_leave(ctx);
                            hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                            assert!(!crate::pool::on_pool());
                            assert!(crate::pool::compute_index().is_some());
                        }
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                assert_eq!(
                    cut.progress[&Vec::new()],
                    hirsel_free::phase::Progress::new(8, Phase::Done)
                );
                assert_executors_retired(&recording);
                assert!(std::sync::Arc::try_unwrap(recording).is_ok());
            }
        }
    }
    #[test]
    fn saved_home_and_pool_callbacks_reacquire_the_single_compute_permit() {
        const CHILD: &str = "HIRSEL_F1_CALLBACK_PERMIT";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tasks::free::foreign::tests::saved_home_and_pool_callbacks_reacquire_the_single_compute_permit", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }
        crate::pool::set_compute_grant(1);
        for width in [2, 8] {
            for (recorded, durable) in [(false, false), (true, false), (true, true)] {
                let recording = recorded
                    .then(|| super::super::record::Recording::new(b"callback permit", durable));
                let install = recording.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        if let Some(recording) = &install {
                            recording.root(ctx);
                        }
                    },
                    move |ctx| unsafe {
                        for dispatch in [
                            FOREIGN_THREADS_HOME | FOREIGN_BLOCKING,
                            FOREIGN_THREADS_ANY | FOREIGN_BLOCKING,
                            FOREIGN_THREADS_SERIAL
                                | FOREIGN_BLOCKING
                                | (7 << FOREIGN_PACKAGE_SHIFT),
                        ] {
                            let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                            hirsel_rt_v2_foreign_enter(
                                ctx,
                                0,
                                interval.as_mut_ptr().cast(),
                                dispatch,
                            );
                            assert_eq!(crate::pool::compute_permits().1, 0, "C holds no permit");
                            assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                            assert_eq!(crate::pool::compute_permits().1, 1);
                            let thread = std::thread::current().id();
                            save_callback(ctx);
                            assert_eq!(std::thread::current().id(), thread);
                            assert_eq!(
                                crate::pool::compute_permits().1,
                                1,
                                "a resumed callback holds its permit"
                            );
                            // A nested callback borrows the outer callback's
                            // permit and must not release it on return.
                            assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                            hirsel_rt_v2_callback_leave(ctx);
                            assert_eq!(crate::pool::compute_permits().1, 1);
                            let mut canceled = 99;
                            hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                            assert_eq!(canceled, 0);
                            hirsel_rt_v2_callback_leave(ctx);
                            assert_eq!(crate::pool::compute_permits().1, 0);
                            hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                            assert_eq!(crate::pool::compute_permits().1, 1);
                        }
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert_eq!(crate::pool::compute_permits().1, 0);
                if let Some(recording) = recording {
                    recording.recorder.stop("returned", None).unwrap().unwrap();
                    assert!(std::sync::Arc::try_unwrap(recording).is_ok());
                }
            }
        }
    }
    unsafe extern "C" fn handoff_child(
        ctx: *mut TaskCtx,
        environment: *mut Header,
        _: u64,
        _: u64,
    ) -> u8 {
        if unsafe { crate::alloc::payload(environment).cast::<u64>().read() } != 0 {
            unsafe { crate::tasks::yield_now(&mut *ctx) };
        }
        STATUS_OK
    }

    #[test]
    fn cooperative_handoffs_take_the_incoming_callbacks_permit() {
        const CHILD: &str = "HIRSEL_F1_HANDOFF_PERMIT";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tasks::free::foreign::tests::cooperative_handoffs_take_the_incoming_callbacks_permit", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }
        crate::pool::set_compute_grant(1);
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
        let layouts = [&raw const layout];
        for width in [1, 2, 8] {
            for yielded in [false, true] {
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                let outcome = run.run(
                    |_| {},
                    move |ctx| unsafe {
                        for dispatch in [
                            FOREIGN_THREADS_HOME | FOREIGN_BLOCKING,
                            FOREIGN_THREADS_ANY | FOREIGN_BLOCKING,
                        ] {
                            let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                            hirsel_rt_v2_foreign_enter(
                                ctx,
                                0,
                                interval.as_mut_ptr().cast(),
                                dispatch,
                            );
                            assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                            let thread = std::thread::current().id();
                            let mut scope = 0;
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0,
                            );
                            let environment = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                8,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(environment)
                                .cast::<u64>()
                                .write(u64::from(yielded));
                            let mut child = 0;
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                handoff_child,
                                environment,
                                0,
                            );
                            assert_eq!(std::thread::current().id(), thread);
                            assert_eq!(
                                crate::pool::compute_permits().1,
                                1,
                                "different-task handoff restores callback permit"
                            );
                            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                            assert_eq!(
                                crate::pool::compute_permits().1,
                                1,
                                "completion handoff restores callback permit"
                            );
                            hirsel_rt_v2_callback_leave(ctx);
                            assert_eq!(crate::pool::compute_permits().1, 0);
                            hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                        }
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert_eq!(crate::pool::compute_permits().1, 0);
            }
        }
    }
    #[derive(Default)]
    struct GateProbe {
        allow: std::sync::atomic::AtomicBool,
        entered: std::sync::atomic::AtomicBool,
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

                let parked = {
                    let _role = role::Guard::new(ctx);
                    let s = scheduler(ctx);
                    // Wait metadata is installed before the physical save.
                    // Observe the same saved-frame gate as production polling
                    // before releasing the slot that wakes this waiter.
                    matches!(s.task(0).wait, Some(Wait::Pool)) && readiness::saved(s, 0)
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
    fn a_full_pool_saves_a_free_waiter_and_a_returning_foreign_call_wakes_it() {
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
            for (recorded, durable) in [(false, false), (true, false), (true, true)] {
                let recording = recorded
                    .then(|| super::super::record::Recording::new(b"full free pool", durable));
                let install = recording.clone();
                let probe = Arc::new(GateProbe::default());
                let seen = probe.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        if let Some(recording) = &install {
                            recording.root(ctx);
                        }
                    },
                    move |ctx| unsafe {
                        // Reserve every slot except the one used by the real child
                        // foreign call. Admission is still the production pool cap.
                        let reserved: Vec<_> = (0..crate::pool::POOL_THREADS - 1)
                            .map(|_| crate::pool::claim(ctx).expect("pool capacity"))
                            .collect();
                        let mut scope = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0
                            ),
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
                        let mut waiting = [0u64; FOREIGN_SCOPE_BYTES / 8];
                        hirsel_rt_v2_foreign_enter(
                            ctx,
                            0,
                            waiting.as_mut_ptr().cast(),
                            FOREIGN_THREADS_ANY,
                        );
                        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                        while !seen.allow.load(Ordering::Acquire) {
                            assert!(std::time::Instant::now() < until);
                            std::thread::yield_now();
                        }
                        hirsel_rt_v2_foreign_leave(waiting.as_ptr().cast());
                        assert!(!crate::pool::has_slot(ctx));
                        let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                        hirsel_rt_v2_foreign_enter(
                            ctx,
                            0,
                            interval.as_mut_ptr().cast(),
                            FOREIGN_THREADS_SERIAL
                                | FOREIGN_BLOCKING
                                | (7 << FOREIGN_PACKAGE_SHIFT),
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
                if let Some(recording) = recording {
                    let cut = recording
                        .recorder
                        .stop("returned", None)
                        .unwrap()
                        .unwrap()
                        .0;
                    assert_eq!(
                        cut.progress[&Vec::new()],
                        hirsel_free::phase::Progress::new(5, Phase::Done)
                    );
                    assert!(std::sync::Arc::try_unwrap(recording).is_ok());
                }
            }
        }
    }
}
