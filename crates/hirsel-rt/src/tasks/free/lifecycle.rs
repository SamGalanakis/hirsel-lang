//! Native Free task-tree boundaries. Allocation and identity reservation
//! precede a phase; its publication installs the observable owner topology.

use super::super::*;

/// # Safety
/// As for ordinary TaskScopeExit, retaining any deadline through child cleanup.
pub(crate) unsafe fn scope_exit(
    ctx: *mut TaskCtx,
    word: u64,
    error: u8,
    caller_site: u64,
) -> Stopped<u8> {
    if let Some(site) = unsafe { crate::continuation::binding::take_bound(ctx, 18) } {
        return unsafe { scope_exit::poll(ctx, word, error, caller_site, None, site) };
    }
    unsafe { exit_scope(ctx, word, error, caller_site, None) }
}

/// # Safety
/// As for TaskScopeExitUntil; out is writable for its Expired bit.
pub(crate) unsafe fn scope_exit_until(
    ctx: *mut TaskCtx,
    word: u64,
    error: u8,
    caller_site: u64,
    out: *mut u8,
) -> Stopped<u8> {
    if let Some(site) = unsafe { crate::continuation::binding::take_bound(ctx, 19) } {
        return unsafe { scope_exit::poll(ctx, word, error, caller_site, Some(out), site) };
    }
    unsafe { exit_scope(ctx, word, error, caller_site, Some(out)) }
}

unsafe fn exit_scope(
    ctx: *mut TaskCtx,
    word: u64,
    error: u8,
    caller_site: u64,
    out: Option<*mut u8>,
) -> Stopped<u8> {
    let operation = scope_exit::start(ctx, word, error);
    loop {
        match unsafe { scope_exit::drive(ctx, operation, out, false) }? {
            scope_exit::Step::Admission | scope_exit::Step::Cut => {
                unreachable!("stackful admission completes synchronously")
            }
            scope_exit::Step::Wait(wait) => {
                unsafe { try_block(&mut *ctx, wait, false, caller_site) }?;
            }
            scope_exit::Step::Rest(child) => unsafe { hand_off(&mut *ctx, child.slot) },
            scope_exit::Step::Yield => {
                unsafe { try_yield(&mut *ctx) }?;
                scope_exit::yielded(ctx, operation);
            }
            scope_exit::Step::Complete { status } => return Ok(status),
        }
    }
}

pub(in crate::tasks) mod scope_enter;
pub(in crate::tasks) mod scope_exit;

/// Take a holding child's result slot for its owner to resume the rest.
fn take_held(s: &mut Scheduler, child: u32) -> bool {
    let row = s.task(child);
    let holding = row.state == State::Holding;
    if holding {
        row.slot = None;
    }
    holding
}

/// Reserve `child`'s retirement: a row and its promoted children's links.
fn reserve_retirement(s: &mut Scheduler, child: u32) {
    s.reserve_retirements(1);
    let row = s.task(child);
    if let Some((parent, _)) = row.parent {
        let children = row.children.len();
        s.task(parent).children.reserve(children);
    }
}

/// # Safety
/// As for TaskDetach. The role owns the child metadata; a held result is
/// touched only after its physical image reaches idle.
pub(crate) unsafe fn detach(ctx: *mut TaskCtx, scope: u64, word: u64) -> Stopped<()> {
    super::record::begin(ctx);
    let (scope, child, saved, state) = role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        let child = s.task_handle(word, scope);
        (
            scope,
            child,
            readiness::saved(s, child),
            s.task(child).state,
        )
    });
    if saved && state == State::Holding {
        // The called rest can run finalizers or suspend. Keep the parent at
        // entry until it returns; no recorder section spans the user code.
        super::record::publish(ctx, Phase::Entered, |_| unsafe {
            role::with_held(ctx, |s| {
                let row = s.task(child);
                row.detached = true;
                row.slot = None;
            })
        })?;
        unsafe { hand_off(&mut *ctx, child) };
        // Successful detached completion retired the row at idle; a failure
        // remains owned by the scope. Never look it up through its old index.
        super::record::publish(ctx, Phase::Done, |_| ())?;
    } else {
        let retire = saved
            && state == State::Done
            && unsafe {
                role::with_held(ctx, |s| {
                    let retire = s.task(child).failure.is_none();
                    if retire {
                        reserve_retirement(s, child);
                    }
                    retire
                })
            };
        let retired = super::record::publish(ctx, Phase::Done, |_| unsafe {
            role::with_held(ctx, |s| {
                s.task(child).detached = true;
                if retire {
                    s.scope(scope).children.retain(|&id| id != child);
                    Some(s.retire_task(child))
                } else {
                    None
                }
            })
        })?;
        drop(retired);
        // A not-yet-saved Holding frame observes detached in after_save and
        // resumes its own cleanup there, without a remote stack access here.
    }
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// As for TaskJoin: handles are live and slot is this owner's None image.
pub(crate) unsafe fn join(
    ctx: *mut TaskCtx,
    scope: u64,
    task: u64,
    slot: *mut u8,
    size: u64,
    caller_site: u64,
) -> Stopped<u8> {
    // Claim an unpublished adjacent child before begin() flushes deferred work.
    let inline = role::with_role(ctx, |s| {
        let scope_id = s.scope_handle(scope);
        let child = s.task_handle(task, scope_id);
        (unsafe { (*ctx).free_ops.deferred } == Some(child)).then(|| {
            unsafe { (*ctx).free_ops.deferred = None };
            child
        })
    });
    super::record::begin(ctx);
    if let Some(child) = inline {
        super::record::publish(ctx, Phase::Entered, |_| ())?;
        unsafe { super::spawn::run(ctx, child, Some((slot, size))) }?;
    }
    let (scope, child, state, image) = role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        let child = s.task_handle(task, scope);
        let state = if readiness::saved(s, child) {
            s.task(child).state
        } else {
            State::Parked
        };
        let image = match state {
            State::Done => s.task(child).inline_value.take(),
            _ => None,
        };
        (scope, child, state, image)
    });
    match state {
        State::Done => {
            if let Some((image, expected)) = image {
                assert_eq!(expected as u64, size);
                unsafe {
                    std::ptr::copy_nonoverlapping(image.as_ptr().cast::<u8>(), slot, expected)
                };
            }
        }
        State::Holding => {
            super::record::publish(ctx, Phase::Entered, |_| ())?;
            let held = role::with_role(ctx, |s| {
                s.task(child).slot.take().expect("a saved held result")
            });
            unsafe {
                deliver(held, slot, size);
                hand_off(&mut *ctx, child);
            }
        }
        State::Ready | State::Parked => {
            super::record::publish(ctx, Phase::Entered, |_| ())?;
            unsafe {
                try_block(
                    &mut *ctx,
                    Wait::Join { child, slot, size },
                    false,
                    caller_site,
                )
            }?;
            if role::with_role(ctx, |s| s.task(child).state == State::Holding) {
                // The saved-frame poll moved the result. Only its owner
                // resumes the rest, outside the recorder section.
                unsafe { hand_off(&mut *ctx, child) };
            }
        }
    }
    role::with_role(ctx, |s| {
        assert_eq!(s.task(child).state, State::Done);
        assert!(readiness::saved(s, child));
        reserve_retirement(s, child);
    });
    let (retired, chain) = super::record::publish(ctx, Phase::Done, |_| unsafe {
        role::with_held(ctx, |s| {
            let chain = s.task(child).failure.take();
            s.scope(scope).children.retain(|&id| id != child);
            (s.retire_task(child), chain)
        })
    })?;
    drop(retired);
    let status = if let Some(chain) = chain {
        unsafe { (*ctx).failure.append_chain(chain) };
        STATUS_FAILURE
    } else {
        STATUS_OK
    };
    unsafe { try_yield(&mut *ctx) }?;
    Ok(status)
}

/// # Safety
/// As for TaskFinish: the current child's result slot remains live at save.
pub(crate) unsafe fn finish(
    ctx: *mut TaskCtx,
    slot: *mut u8,
    none: *const u8,
    size: u64,
) -> Stopped<u8> {
    if crate::tasks::compact::held(ctx) {
        // The reissued end of a result held from a saved compact frame: its
        // parent's hand-off resumed it, as `run_next` returns below.
        super::replay::start(ctx);
        return Ok(STATUS_OK);
    }
    if unsafe { super::spawn::finish(ctx, slot, none, size) }? {
        return Ok(STATUS_OK);
    }
    let (me, detached) = role::with_role(ctx, |s| {
        let me = s.current;
        let row = s.task(me);
        assert!(row.scope.is_some(), "the root cannot finish as a child");
        (me, row.detached)
    });
    if detached {
        // Detachment changes result ownership, not the child's semantic ops.
        // A replay may reach this point on either side of the parent's detach.
        super::record::end(ctx, || unsafe {
            role::with_held(ctx, |s| {
                s.task(me).ended = true;
                s.order(me);
            })
        })?;
        return Ok(STATUS_OK);
    }
    // SAFETY: caller contract.
    let (held, image) = unsafe { Slot::held(slot, none, size) };
    unsafe {
        role::with_held(ctx, |s| {
            let additional = s.set_waiters.len() + 2;
            s.woken.reserve(additional);
        })
    };
    super::record::end(ctx, || unsafe {
        role::with_held(ctx, |s| {
            let row = s.task(me);
            row.ended = true;
            row.slot = Some(held);
            row.held_none = Some(image);
            row.state = State::Holding;
            s.order(me);
            s.settle(me);
        })
    })?;
    if unsafe { crate::tasks::compact::can_hold(ctx, slot, size) } {
        // The result stays in the compact frame; the frame leaves its stack.
        crate::tasks::compact::hold(ctx);
        unsafe { super::dispatch_ready(ctx) };
        return Ok(crate::STATUS_SUSPENDED);
    }
    unsafe { run_next(&mut *ctx) }?;
    super::replay::start(ctx);
    Ok(STATUS_OK)
}

/// # Safety
/// This is the running child after body/environment cleanup, with the role.
pub(super) unsafe fn completion(ctx: *mut TaskCtx, index: u32, status: u8) -> Stopped<()> {
    // The task waits on no object again; its row's retirement must not wait
    // for the frame reference its own wake token keeps.
    super::objects::release_wake(ctx);
    let (path, first_end, scope) = role::with_role(ctx, |s| {
        let row = s.task(index);
        let scope = row.scope.expect("a child has a scope");
        (row.path.clone(), !row.ended, scope)
    });
    let chain = unsafe { (*ctx).failure.take_chain(&path) };
    let failed = status != STATUS_OK;
    assert_eq!(
        failed,
        !chain.is_empty(),
        "a task status agrees with its failures"
    );
    let chain = failed.then_some(chain);
    let cancellations = unsafe {
        role::with_held(ctx, |s| {
            let mut cancellations = Vec::new();
            if failed {
                let siblings = s.scope(scope).children.clone();
                for sibling in siblings {
                    if sibling != index {
                        cancellations.push((sibling, s.prepare_cancel(sibling)));
                    }
                }
            }
            let tasks = s.tasks.len();
            s.woken.reserve(tasks);
            cancellations
        })
    };
    let effect = || unsafe {
        role::with_held(ctx, |s| {
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
        })
    };
    // TaskFinish published result readiness; actual frame completion is a
    // distinct op even when its rest released no roots. This keeps ordinals
    // independent of count races and admits cleanup/failures before the join.
    super::record::end(ctx, effect)?;
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
    flags: u64,
) -> Stopped<()> {
    super::record::begin(ctx);
    let started = unsafe { (*ctx).run_ref() }
        .worker_metrics
        .as_ref()
        .map(|_| std::time::Instant::now());
    let (scope, current, generation, index, path, canceled) = role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        let current = s.current;
        let canceled = s.canceled(unsafe { (*ctx).shield }).map(|_| CANCEL_PARENT);
        let parent = s.task(current);
        let mut path = parent.path.clone();
        path.push(parent.spawned);
        parent.children.reserve(1);
        s.scope(scope).children.reserve(1);
        let (index, generation) = s.take_task_slot();
        (scope, current, generation, index, path, canceled)
    });
    let fast = super::spawn::allowed(ctx, flags);
    let stack = (!fast).then(|| unsafe { (*ctx).worker().idle_stacks.take() });
    let mut owned = fast.then(|| {
        Box::new(std::cell::UnsafeCell::new(unsafe {
            TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr())
        }))
    });
    let child_ctx = match (&stack, &mut owned) {
        (Some(stack), _) => stack.context(),
        (_, Some(owned)) => owned.get(),
        _ => unreachable!(),
    };
    unsafe {
        if let Some(stack) = &stack {
            child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
            (*child_ctx).task_sp = stack.prepare(
                task_entry,
                child_ctx.cast(),
                handle(index, generation) as usize as *mut u8,
            );
        }
        (*child_ctx).task_id = index;
    }
    let mut task = Task::new(generation, child_ctx, Some(scope), path, canceled);
    inherit_cpu(ctx, &mut task);
    let owner = task.owner;
    task.code = Some((code, environment, operation_site));
    task.stack = stack;
    task.inline_context = owned;
    let log = super::record::prepare_child(ctx);
    let mut starts = true;
    super::record::spawn(ctx, log, |log| unsafe {
        if let Some(log) = &log {
            assert_eq!(log.path(), task.path);
        }
        (*child_ctx).recording = log;
        starts = super::replay::child(ctx, &task.path);
        if !starts {
            task.state = State::Parked;
            task.wait = Some(Wait::Final);
        }
        role::with_held(ctx, |s| {
            let parent = s.task(current);
            parent.spawned = parent
                .spawned
                .checked_add(1)
                .expect("spawn path ordinal overflow");
            assert!(s.tasks[index as usize].is_none());
            #[cfg(feature = "ownership-trace")]
            {
                crate::ownership_trace::task(
                    (*ctx).run_ref(),
                    handle(index, generation),
                    &task.path,
                );
                let binding = crate::ownership_trace::take_spawn(ctx);
                crate::ownership_trace::spawned(
                    (*ctx).run_ref(),
                    binding,
                    handle(index, generation),
                );
            }
            s.tasks[index as usize] = Some(task);
            s.scope(scope).children.push(index);
            s.link_child(current, index);
        });
        out.write(handle(index, generation));
    })?;
    if let (Some(started), Some(metrics)) =
        (started, unsafe { (*ctx).run_ref() }.worker_metrics.as_ref())
    {
        metrics.spawn(started.elapsed());
    }
    unsafe {
        if !starts {
            return try_yield(&mut *ctx);
        }
        if fast && flags & super::spawn::IMMEDIATE != 0 {
            (*ctx).free_ops.deferred = Some(index);
            // The compiler proved the next op is its join. Returning does not
            // expose an unstarted child to a worker before its destination exists.
            if crate::pool::free::proceed(ctx) {
                return Ok(());
            }
            // Count spawn as an operation even when its physical switch is
            // elided; the parent's 128-op fairness bound still applies. This
            // op already spent its budget: give up the worker without a
            // second charge, which would renew the quantum instead.
            super::spawn::flush(ctx);
            return super::boundary(&mut *ctx, false);
        } else if fast {
            super::spawn::run(ctx, index, None)?;
        } else {
            crate::pool::free::schedule_saved(
                (*ctx).run_ptr(),
                TaskKey::new(index, generation),
                owner,
            );
        }
        try_yield(&mut *ctx)
    }
}

/// # Safety
/// The role is held and out is writable for the scope word.
pub(crate) unsafe fn scope_enter(ctx: *mut TaskCtx, out: *mut u64) -> Stopped<u8> {
    if let Some(site) = unsafe { crate::continuation::binding::take_bound(ctx, 16) } {
        return unsafe { scope_enter::poll(ctx, out, None, site) };
    }
    super::record::begin(ctx);
    let (index, current, row) = prepare_scope(ctx, None);
    let generation = row.generation;
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        role::with_held(ctx, |s| install_scope(s, index, current, row));
        out.write(handle(index, generation));
    })?;
    unsafe { try_yield(&mut *ctx) }?;
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskScopeEnterUntil, with a bound home adapter and writable out.
pub(crate) unsafe fn scope_enter_until(
    ctx: *mut TaskCtx,
    out: *mut u64,
    deadline: *const [u64; 2],
) -> Stopped<u8> {
    if let Some(site) = unsafe { crate::continuation::binding::take_bound(ctx, 17) } {
        return unsafe { scope_enter::poll(ctx, out, Some(deadline.read()), site) };
    }
    let [clock, at] = unsafe { deadline.read() };
    super::record::begin(ctx);
    let (index, current, row) = prepare_scope(
        ctx,
        Some(ScopeDeadline {
            token: None,
            expired: false,
        }),
    );
    let generation = row.generation;
    super::record::publish(ctx, Phase::Entered, |_| unsafe {
        role::with_held(ctx, |s| install_scope(s, index, current, row));
    })?;
    // Replay observes recorded cancellation checks and exit expiry. It has
    // no timer request or readiness source on this host.
    let replay = super::replay::on(ctx);
    let token = if replay {
        None
    } else {
        unsafe { crate::host_waits::arm(ctx, index, generation, clock, at) }?
    };
    // A fresh scope has no children or inner scopes, so immediate expiration
    // needs no allocation or graph walk inside this publication.
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        role::with_held(ctx, |s| match token {
            Some(token) => s.scope(index).deadline.as_mut().unwrap().token = Some(token),
            None if !replay => s.expire(index),
            None => {}
        });
        out.write(handle(index, generation));
    })?;
    unsafe { try_yield(&mut *ctx) }?;
    Ok(STATUS_OK)
}

fn prepare_scope(ctx: *mut TaskCtx, deadline: Option<ScopeDeadline>) -> (u32, u32, Scope) {
    let (index, generation, current) = role::with_role(ctx, |s| {
        let (index, generation) = s.take_scope_slot();
        let current = s.current;
        s.task(current).scopes.reserve(1);
        (index, generation, current)
    });
    let row = Scope {
        generation,
        owner: current,
        children: Vec::new(),
        canceled: Default::default(),
        deadline,
        completions: AtomicU64::new(0),
    };
    (index, current, row)
}

fn install_scope(s: &mut Scheduler, index: u32, current: u32, row: Scope) {
    assert!(s.scopes[index as usize].is_none());
    s.scopes[index as usize] = Some(row);
    s.task(current).scopes.push(index);
    s.cancel_epoch.changed();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::free::record;

    #[test]
    fn a_recorded_scope_exit_discards_holding_results_in_reverse_spawn_order() {
        use std::sync::{Arc, Mutex};
        unsafe extern "C" fn result(
            ctx: *mut TaskCtx,
            environment: *mut Header,
            _: u64,
            _: u64,
        ) -> u8 {
            let (id, order) = unsafe {
                environment
                    .add(1)
                    .cast::<(u64, Arc<Mutex<Vec<u64>>>)>()
                    .read()
            };
            let mut image = [1u64, id];
            let none = [0u64; 2];
            unsafe {
                hirsel_rt_v2_task_finish(
                    ctx,
                    image.as_mut_ptr().cast(),
                    none.as_ptr().cast(),
                    16,
                    u32::MAX,
                )
            };
            assert_eq!(
                image,
                [1, id],
                "scope exit retains the discarded value for its owner's cleanup"
            );
            order.lock().unwrap().push(id);
            STATUS_OK
        }
        for durable in [false, true] {
            let recording = record::Recording::new(b"native nonempty scope exit", durable);
            let install = recording.clone();
            let layout = crate::Layout {
                magic: crate::LAYOUT_MAGIC,
                version: crate::LAYOUT_VERSION,
                size: std::mem::size_of::<(u64, Arc<Mutex<Vec<u64>>>)>() as u64,
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
            let mut run = crate::host::BodyRun::new(4, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let order = Arc::new(Mutex::new(Vec::new()));
            let observed = order.clone();
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    for id in 0..8u64 {
                        let size = std::mem::size_of::<(u64, Arc<Mutex<Vec<u64>>>)>() as u64;
                        let environment = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            size,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        environment
                            .add(1)
                            .cast::<(u64, Arc<Mutex<Vec<u64>>>)>()
                            .write((id, order.clone()));
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
                            0,
                        );
                    }
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    let _role = role::Guard::new(ctx);
                    assert!(scheduler(ctx).task(ROOT).scopes.is_empty());
                    assert!(scheduler(ctx).tasks.iter().skip(1).all(Option::is_none));
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(*observed.lock().unwrap(), (0..8).rev().collect::<Vec<_>>());
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            assert_eq!(
                cut.progress[&vec![]],
                hirsel_free::phase::Progress::new(10, Phase::Done)
            );
            for id in 0..8 {
                assert_eq!(
                    cut.progress[&vec![id]],
                    hirsel_free::phase::Progress::new(2, Phase::Done)
                );
            }
        }
    }

    #[test]
    fn a_holding_native_result_has_published_its_end_before_its_saved_image_is_observed() {
        holding_native_result(false, false);
    }

    #[test]
    fn detaching_a_holding_native_result_keeps_the_parent_entered_through_its_rest() {
        for durable in [false, true] {
            holding_native_result(true, durable);
        }
    }

    fn holding_native_result(detach: bool, durable: bool) {
        #[derive(Default)]
        struct Rest {
            detach: bool,
            started: std::sync::atomic::AtomicBool,
            resume: std::sync::atomic::AtomicBool,
        }
        unsafe extern "C" fn result(
            ctx: *mut TaskCtx,
            environment: *mut Header,
            _: u64,
            _: u64,
        ) -> u8 {
            let rest = unsafe {
                Box::from_raw(
                    environment
                        .add(1)
                        .cast::<*mut std::sync::Arc<Rest>>()
                        .read(),
                )
            };
            let mut image = [1u64, 99];
            let none = [0u64; 2];
            let status = unsafe {
                hirsel_rt_v2_task_finish(
                    ctx,
                    image.as_mut_ptr().cast(),
                    none.as_ptr().cast(),
                    16,
                    u32::MAX,
                )
            };
            assert_eq!(image, if rest.detach { [1, 99] } else { none });
            rest.started.store(true, Ordering::Release);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !rest.resume.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < until,
                    "the rest's cut must finish"
                );
                std::thread::yield_now();
            }
            status
        }
        let recording = record::Recording::new(b"native holding end", durable);
        let install = recording.clone();
        let cutter = recording.clone();
        let rest = std::sync::Arc::new(Rest {
            detach,
            ..Rest::default()
        });
        let watched = rest.clone();
        let in_call = recording.clone();
        let observer = std::thread::spawn(move || {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !watched.started.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < until,
                    "the joined rest must start"
                );
                std::thread::yield_now();
            }
            let cut = in_call.recorder.flush().unwrap().unwrap();
            // Always release the called frame before asserting, so a bad
            // parent phase reports its value instead of stranding cleanup.
            watched.resume.store(true, Ordering::Release);
            assert_eq!(
                cut.progress[&Vec::new()],
                hirsel_free::phase::Progress::new(3, Phase::Entered)
            );
        });
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
                environment
                    .add(1)
                    .cast::<*mut std::sync::Arc<Rest>>()
                    .write(Box::into_raw(Box::new(rest.clone())));
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
                    0,
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
                let mut out = [0u64; 2];
                if detach {
                    assert_eq!(hirsel_rt_v2_task_detach(ctx, &scope, &mut child), STATUS_OK);
                    assert_eq!(out, [0, 0]);
                } else {
                    assert_eq!(
                        hirsel_rt_v2_task_join(
                            ctx,
                            0,
                            0,
                            &scope,
                            &mut child,
                            out.as_mut_ptr().cast(),
                            16
                        ),
                        STATUS_OK
                    );
                    assert_eq!(out, [1, 99]);
                }
                let _role = role::Guard::new(ctx);
                assert!(scheduler(ctx).tasks[id as usize].is_none());
                drop(_role);
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                STATUS_OK
            },
        );
        observer.join().unwrap();
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
            hirsel_free::phase::Progress::new(2, Phase::Done)
        );
        let trace = recording.trace();
        let marked = std::sync::Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink();
        for end in marked.marks {
            let bytes = &marked.bytes[..end];
            let tape = hirsel_free::tape::parse(bytes).unwrap();
            let mut replay = hirsel_free::session::Session::replay(bytes).unwrap();
            if tape.cut.is_empty() {
                // Durable header publication precedes registration. Its
                // truncated prefix admits neither the root nor its child.
                assert!(!replay.start(&[]));
                assert!(!replay.start(&[0]));
                assert_eq!(
                    replay.finish().unwrap(),
                    hirsel_free::projection::Projection::new("truncated", &tape.cut, &trace)
                );
                continue;
            }
            assert!(replay.start(&[]));
            assert!(replay.start(&[0]));
            for _ in 0..2 {
                replay.begin(&[]);
                replay.publish(&[], Phase::Done).unwrap();
            }
            for _ in 0..tape.cut[&vec![0]].op {
                replay.begin(&[0]);
                replay.publish(&[0], Phase::Entered).unwrap();
                replay.publish(&[0], Phase::Done).unwrap();
            }
            if tape.cut[&Vec::new()].op >= 3 {
                replay.begin(&[]);
                replay.publish(&[], Phase::Entered).unwrap();
                if tape.cut[&Vec::new()].op > 3 || tape.cut[&Vec::new()].phase == Phase::Done {
                    replay.publish(&[], Phase::Done).unwrap();
                }
            }
            if tape.cut[&Vec::new()].op == 4 {
                replay.begin(&[]);
                replay.publish(&[], Phase::Entered).unwrap();
                replay.publish(&[], Phase::Done).unwrap();
            }
            assert_eq!(
                replay.finish().unwrap(),
                hirsel_free::projection::Projection::new(
                    if end == marked.bytes.len() {
                        "returned"
                    } else {
                        "truncated"
                    },
                    &tape.cut,
                    &trace
                )
            );
        }
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
                assert!(spawn(ctx, &mut out, scope, never, std::ptr::null_mut(), 0, 0).is_err());
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

#[cfg(test)]
mod rest_tests;

#[cfg(test)]
mod deadline_tests;

#[cfg(test)]
mod gate_tests;
