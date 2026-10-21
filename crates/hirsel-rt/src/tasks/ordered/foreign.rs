//! Head-only serial gates and saved-frame foreign thread dispatch.
use super::super::*;
use super::engine;
use crate::ordered::{Next, Progress};

pub(crate) fn enter(ctx: *mut TaskCtx) -> Stopped<()> {
    engine::head_effect(ctx, "extern.call", || ((), Next::Keep))?;
    Ok(())
}

pub(crate) fn gate_enter(ctx: *mut TaskCtx, gate: u32, site: u64) -> Stopped<bool> {
    engine::at_head(ctx)?;
    let s = scheduler(ctx);
    let me = s.current;
    if s.gate(gate).owner == Some(me) {
        s.gate(gate).depth += 1;
        return Ok(false);
    }
    let started = unsafe { (*ctx).run_ref() }
        .extern_metrics
        .as_ref()
        .map(|_| std::time::Instant::now());
    let (_, progress) = engine::head_effect(ctx, "gate.enter", || {
        let s = scheduler(ctx);
        let me = s.current;
        let row = s.gate(gate);
        if row.owner.is_none() {
            row.owner = Some(me);
            row.depth = 1;
            row.version += 1;
            ((), Next::Keep)
        } else {
            s.task(me).wait_site = site;
            s.park(Wait::Gate { gate });
            ((), Next::Park)
        }
    })?;
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
        let s = scheduler(ctx);
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
        ((), Next::Keep)
    };
    if outermost {
        engine::head_effect(ctx, "gate.leave", effect)?;
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
            let s = scheduler(ctx);
            let me = s.current;
            s.task(me).wait_site = site;
            s.park(Wait::Pool);
            ((), Next::Park)
        })?;
        engine::save_boundary(ctx, progress)?;
    };
    let s = scheduler(ctx);
    let me = s.current;
    let row = s.task(me);
    unpin(row);
    row.owner = Owner::Pool(slot);
    unsafe { engine::migrate(ctx) }?;
    Ok(slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foreign::*;
    use crate::ordered::Policy;

    #[test]
    fn ordered_home_pool_and_nested_callbacks_preserve_effective_thread_and_gate_depth() {
        for width in [2, 4, 8] {
            let home = std::thread::current().id();
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
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
