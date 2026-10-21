//! Thread ownership for Free callbacks with live C frames.

use super::super::*;

/// Match the Ordered foreign owner protocol: the pool retains the exact Exec
/// through resize until its outer C frame returns. Nested calls share its pin.
pub(crate) fn pin(ctx: *mut TaskCtx) -> bool {
    let Some(owner) = crate::pool::pin_foreign_compute() else {
        return false;
    };
    let s = scheduler(ctx);
    let me = s.current;
    let row = s.task(me);
    unpin(row);
    assert_eq!(row.owner, Owner::Any);
    row.owner = owner;
    true
}

pub(crate) fn unpin_compute(ctx: *mut TaskCtx) {
    let s = scheduler(ctx);
    let me = s.current;
    let owner = std::mem::replace(&mut s.task(me).owner, Owner::Any);
    crate::pool::unpin_foreign_compute(owner);
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
            crate::pool::free::schedule_saved((*ctx).run_ptr(), (*ctx).task_id, owner);
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
            crate::pool::free::schedule_saved((*ctx).run_ptr(), (*ctx).task_id, owner);
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
                                migrate(ctx);
                                assert_ne!(std::thread::current().id(), thread);
                                STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(STATUS_OK));
                        assert_eq!(outcome.pending, 0);
                        if let Some(recording) = recording {
                            recording.recorder.stop("returned", None).unwrap().unwrap();
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
                    assert!(std::sync::Arc::try_unwrap(recording).is_ok());
                }
            }
        }
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
                            // while a replacement runs. The phase must obtain a
                            // different exclusive token, never alias or panic.
                            let occupied = inspect
                                .recorder
                                .worker(crate::pool::recorder_slot())
                                .unwrap();
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
                assert!(std::sync::Arc::try_unwrap(recording).is_ok());
            }
        }
    }
}
