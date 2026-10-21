//! Native Free task-tree boundaries. Allocation and identity reservation
//! precede a phase; its publication installs the observable owner topology.

use super::super::*;

/// # Safety
/// As for TaskFinish: the current child's result slot remains live at save.
pub(crate) unsafe fn finish(
    ctx: *mut TaskCtx,
    slot: *mut u8,
    none: *const u8,
    size: u64,
) -> Stopped<u8> {
    let s = scheduler(ctx) as *mut Scheduler;
    let me = unsafe { (*s).current };
    let row = unsafe { (*s).task(me) };
    assert!(row.scope.is_some(), "the root cannot finish as a child");
    if row.detached {
        return Ok(STATUS_OK);
    }
    let held = Slot {
        slot,
        none,
        size: usize::try_from(size).expect("a result slot fits memory"),
    };
    unsafe { (*s).woken.reserve((*s).set_waiters.len() + 2) };
    super::record::end(ctx, || unsafe {
        let s = &mut *s;
        let row = s.task(me);
        row.ended = true;
        row.slot = Some(held);
        row.state = State::Holding;
        s.order(me);
        s.settle(me);
    })?;
    unsafe { run_next(&mut *ctx) }?;
    Ok(STATUS_OK)
}

/// # Safety
/// This is the running child after body/environment cleanup, with the role.
pub(super) unsafe fn completion(ctx: *mut TaskCtx, index: u32, status: u8) -> Stopped<()> {
    let s = scheduler(ctx) as *mut Scheduler;
    let path = unsafe { (*s).task(index).path.clone() };
    let first_end = !unsafe { (*s).task(index).ended };
    if !first_end {
        if super::record::on(ctx) {
            assert!(
                !unsafe { (*ctx).roots.has_pending() },
                "recorded rest root cleanup is not admitted"
            );
        } else {
            crate::roots::apply_pending(ctx);
        }
    }
    let chain = unsafe { (*ctx).failure.take_chain(&path) };
    let failed = status != STATUS_OK;
    assert!(
        first_end || !failed || !super::record::on(ctx),
        "a recorded failing rest is not admitted"
    );
    assert_eq!(
        failed,
        !chain.is_empty(),
        "a task status agrees with its failures"
    );
    let chain = failed.then_some(chain);
    let scope = unsafe { (*s).task(index).scope.expect("a child has a scope") };
    let mut cancellations = Vec::new();
    if failed {
        let siblings = unsafe { (*s).scope(scope).children.clone() };
        for sibling in siblings {
            if sibling != index {
                cancellations.push((sibling, unsafe { (*s).prepare_cancel(sibling) }));
            }
        }
    }
    unsafe { (*s).woken.reserve((*s).tasks.len()) };
    let effect = || unsafe {
        let s = &mut *s;
        let row = s.task(index);
        row.ended = true;
        row.finishing = true;
        row.state = State::Ready;
        if !failed && first_end {
            s.order(index);
        }
        if failed {
            assert!(s.task(index).failure.is_none());
            s.task(index).failure = chain;
            s.order(index);
            s.cancel_scope(scope, CANCEL_CHILD_FAILED);
            s.wake_owner(scope);
            for (sibling, targets) in &cancellations {
                s.cancel_prepared(*sibling, CANCEL_CHILD_FAILED, targets);
            }
        }
    };
    if first_end {
        super::record::end(ctx, effect)?;
    } else {
        // A holding child already published its end. Its returned rest
        // relinquishes the physical frame here, without repeating that op.
        effect();
    }
    drop(cancellations);
    drop(path);
    if let Some(metrics) = &unsafe { (*ctx).run_ref() }.worker_metrics {
        metrics.complete();
    }
    Ok(())
}

/// # Safety
/// The role is held; handles and the owned closure satisfy TaskSpawn's ABI.
pub(crate) unsafe fn spawn(
    ctx: *mut TaskCtx,
    out: *mut u64,
    scope: u64,
    code: TaskCode,
    environment: *mut Header,
    operation_site: u64,
) -> Stopped<()> {
    super::record::begin(ctx);
    let started = unsafe { (*ctx).run_ref() }
        .worker_metrics
        .as_ref()
        .map(|_| std::time::Instant::now());
    let s = scheduler(ctx) as *mut Scheduler;
    let (scope, current, generation, index, path, canceled) = unsafe {
        let s = &mut *s;
        let scope = s.scope_handle(scope);
        let current = s.current;
        let canceled = s.canceled((*ctx).shield).map(|_| CANCEL_PARENT);
        let parent = s.task(current);
        let mut path = parent.path.clone();
        path.push(parent.spawned);
        parent.children.reserve(1);
        s.scope(scope).children.reserve(1);
        let generation = s.fresh_generation();
        let index = s.vacant_tasks.pop().unwrap_or_else(|| {
            s.tasks.push(None);
            (s.tasks.len() - 1) as u32
        });
        (scope, current, generation, index, path, canceled)
    };
    let stack = unsafe { (*ctx).worker() }
        .idle_stacks
        .pop()
        .unwrap_or_else(Stack::new);
    let child_ctx = stack.context();
    unsafe {
        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
        (*child_ctx).task_id = index;
        (*child_ctx).task_sp =
            stack.prepare(task_entry, child_ctx.cast(), index as usize as *mut u8);
    }
    let mut task = Task::new(generation, child_ctx, Some(scope), path, canceled);
    task.code = Some((code, environment, operation_site));
    task.stack = Some(stack);
    let log = super::record::prepare_child(ctx);
    super::record::spawn(ctx, log, |log| unsafe {
        if let Some(log) = &log {
            assert_eq!(log.path(), task.path);
        }
        (*child_ctx).recording = log;
        let s = &mut *s;
        let parent = s.task(current);
        parent.spawned = parent
            .spawned
            .checked_add(1)
            .expect("spawn path ordinal overflow");
        assert!(s.tasks[index as usize].is_none());
        s.tasks[index as usize] = Some(task);
        s.scope(scope).children.push(index);
        s.link_child(current, index);
        out.write(handle(index, generation));
    })?;
    if let (Some(started), Some(metrics)) =
        (started, unsafe { (*ctx).run_ref() }.worker_metrics.as_ref())
    {
        metrics.spawn(started.elapsed());
    }
    unsafe {
        crate::pool::free::schedule_saved((*ctx).run_ptr(), index, Owner::Any);
        try_yield(&mut *ctx)
    }
}

/// # Safety
/// The role is held and out is writable for the scope word.
pub(crate) unsafe fn scope_enter(ctx: *mut TaskCtx, out: *mut u64) -> Stopped<()> {
    super::record::begin(ctx);
    let s = scheduler(ctx) as *mut Scheduler;
    let (index, generation, current) = unsafe {
        let s = &mut *s;
        let generation = s.fresh_generation();
        let index = match s.vacant_scopes.pop() {
            Some(index) => index,
            None => {
                s.scopes.push(None);
                (s.scopes.len() - 1) as u32
            }
        };
        let current = s.current;
        s.task(current).scopes.reserve(1);
        (index, generation, current)
    };
    let row = Scope {
        generation,
        owner: current,
        children: Vec::new(),
        canceled: Default::default(),
        deadline: None,
        completions: AtomicU64::new(0),
    };
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        let s = &mut *s;
        assert!(s.scopes[index as usize].is_none());
        s.scopes[index as usize] = Some(row);
        s.task(current).scopes.push(index);
        s.cancel_epoch.changed();
        out.write(handle(index, generation));
    })?;
    unsafe { try_yield(&mut *ctx) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::free::record;

    #[test]
    fn a_holding_native_result_has_published_its_end_before_its_saved_image_is_observed() {
        unsafe extern "C" fn result(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
            let mut image = [1u64, 99];
            let none = [0u64; 2];
            unsafe {
                hirsel_rt_v2_task_finish(ctx, image.as_mut_ptr().cast(), none.as_ptr().cast(), 16)
            }
        }
        let recording = record::Recording::new(b"native holding end", false);
        let install = recording.clone();
        let cutter = recording.clone();
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
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let mut scope = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                let environment =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                let mut child = 0;
                hirsel_rt_v2_task_spawn(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut child,
                    &scope,
                    result,
                    environment,
                );
                let id = child as u32;
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                loop {
                    let saved = {
                        let _role = role::Guard::new(ctx);
                        let s = scheduler(ctx);
                        s.task(id).state == State::Holding && readiness::saved(s, id)
                    };
                    if saved {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < until,
                        "the native result must reach idle"
                    );
                    std::thread::yield_now();
                }
                let cut = cutter.recorder.flush().unwrap().unwrap();
                assert_eq!(
                    cut.progress[&vec![0]],
                    hirsel_free::phase::Progress::new(1, Phase::Done)
                );
                // This admission fixture resumes the rest directly. General
                // recorded join/call boundaries are a separate integration.
                let _role = role::Guard::new(ctx);
                let held = scheduler(ctx).task(id).slot.take().unwrap();
                let mut out = [0u64; 2];
                deliver(held, out.as_mut_ptr().cast(), 16);
                assert_eq!(out, [1, 99]);
                hand_off(&mut *ctx, id);
                assert_eq!(scheduler(ctx).task(id).state, State::Done);
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
            cut.progress[&vec![0]],
            hirsel_free::phase::Progress::new(1, Phase::Done)
        );
    }

    #[test]
    fn a_spawn_after_the_terminal_cut_cannot_publish_a_handle_or_start_a_child() {
        unsafe extern "C" fn never(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
            panic!("a spawn rejected at the terminal cut cannot start");
        }
        let recording = record::Recording::new(b"native spawn terminal", false);
        let install = recording.clone();
        let cutter = recording.clone();
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let mut scope = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                let cut = cutter.recorder.stop("cut", None).unwrap().unwrap().0;
                let _role = role::Guard::new(ctx);
                let mut out = u64::MAX;
                assert!(spawn(ctx, &mut out, scope, never, std::ptr::null_mut(), 0).is_err());
                assert_eq!(out, u64::MAX);
                let s = scheduler(ctx);
                let index = s.scope_handle(scope);
                assert!(s.scope(index).children.is_empty());
                assert!(s.task(ROOT).children.is_empty());
                assert_eq!(s.task(ROOT).spawned, 0);
                assert!(s.tasks.iter().skip(1).all(Option::is_none));
                assert_eq!(cut.progress.len(), 1);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn a_scope_cannot_join_its_owners_topology_after_a_terminal_cut() {
        let recording = record::Recording::new(b"native scope terminal", false);
        let install = recording.clone();
        let cutter = recording.clone();
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let cut = cutter.recorder.stop("cut", None).unwrap().unwrap().0;
                let _role = role::Guard::new(ctx);
                let mut out = u64::MAX;
                assert!(scope_enter(ctx, &mut out).is_err());
                assert_eq!(out, u64::MAX);
                assert!(scheduler(ctx).task(ROOT).scopes.is_empty());
                assert!(scheduler(ctx).scopes.iter().all(Option::is_none));
                assert_eq!(cut.progress[&Vec::new()].op, 0);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }
}
