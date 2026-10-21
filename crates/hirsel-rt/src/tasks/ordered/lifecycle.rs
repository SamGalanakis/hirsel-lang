//! Strict-head native task effects over the saved-frame dispatcher.
//!
//! Follows the cooperative task tree and Frontier's checked call transitions.
//! Task mutation happens in a head callback, after its preceding stretch. A
//! holding frame is read only after idle save; completion retires its physical
//! token before publishing Done or returning to a caller. This test-only slice
//! does not yet admit detached tasks, task sets, deadlines or nonempty exits.

use super::super::*;
use super::engine;
use crate::ordered::objects::Kind;
use crate::ordered::{Next, Progress};

/// # Safety
/// As for TaskScopeEnter, with the ordered policy and role installed.
pub(crate) unsafe fn scope_enter(ctx: *mut TaskCtx, out: *mut u64) -> Stopped<u8> {
    let (word, progress) = engine::head_effect(ctx, "task.scope_enter", || {
        let (index, generation) = open_scope(ctx, None);
        (handle(index, generation), Next::Yield)
    })?;
    engine::driver(ctx).objects.inert(Kind::Scope, word);
    unsafe { out.write(word) };
    engine::continue_at_head(ctx, progress)?;
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskSpawn. The owned environment remains unstarted during its scan.
pub(crate) unsafe fn spawn(
    ctx: *mut TaskCtx,
    out: *mut u64,
    scope: *const u64,
    code: TaskCode,
    environment: *mut Header,
    operation_site: u64,
) -> Stopped<u8> {
    let ((index, generation, _), progress) = engine::head_effect(ctx, "task.async", || {
        // SAFETY: the callback runs immediately at this spawn's head commit.
        let child = unsafe { spawn_child(ctx, scope, code, environment, operation_site) };
        (child, Next::Yield)
    })?;
    // SAFETY: no notification has published this owned environment yet.
    let captures = unsafe {
        crate::ordered::capture::scan(
            ctx,
            crate::alloc::payload(environment),
            (*environment).index(),
        )
    };
    let word = handle(index, generation);
    let driver = engine::driver(ctx);
    let held = driver.objects.captured(&captures);
    // Task result types are erased by TaskSpawn's current native ABI. An
    // unknown binding conservatively blocks passes through a captured task.
    driver.objects.unknown(Kind::Task, word);
    driver.enqueue(word, held);
    unsafe { out.write(word) };
    engine::continue_at_head(ctx, progress)?;
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskFinish. The Slot belongs to the current task until idle save.
pub(crate) unsafe fn finish(
    ctx: *mut TaskCtx,
    slot: *mut u8,
    none: *const u8,
    size: u64,
) -> Stopped<u8> {
    engine::at_head(ctx)?;
    let me = scheduler(ctx).current;
    assert!(
        !scheduler(ctx).task(me).detached,
        "detached ordered task is not admitted"
    );
    let (parent, progress) = engine::head_effect(ctx, "task.end", || {
        let s = scheduler(ctx);
        let scope = s.task(me).scope.expect("a child has a scope");
        let parent = s.scope(scope).owner;
        let parent = handle(parent, s.task(parent).generation);
        let row = s.task(me);
        row.ended = true;
        row.slot = Some(Slot {
            slot,
            none,
            size: usize::try_from(size).expect("task result size fits usize"),
        });
        row.state = State::Holding;
        s.order(me);
        // A waiter's poll cannot inspect this slot until after physical save.
        (parent, Next::Hold)
    })?;
    let task = engine::current(ctx);
    engine::driver(ctx)
        .frontier
        .as_mut()
        .unwrap()
        .held
        .get_mut(&parent)
        .expect("a live scope owner")
        .holding
        .insert(task);
    engine::save_boundary(ctx, progress)?;
    Ok(STATUS_OK)
}

/// Execute a holding child's rest as a call, never as a FIFO enqueue.
/// # Safety
/// The child is holding a physically saved frame; the caller owns the role.
unsafe fn call(
    ctx: *mut TaskCtx,
    child: u32,
    then_yield: bool,
    delivery: Option<(*mut u8, u64)>,
) -> Stopped<()> {
    let effect = || {
        let s = scheduler(ctx);
        let me = s.current;
        assert!(readiness::saved(s, child));
        assert_eq!(s.task(child).state, State::Holding);
        if let Some((slot, size)) = delivery {
            let held = s.task(child).slot.take().expect("a holding result");
            // SAFETY: the child is saved and this callback owns the caller.
            unsafe { deliver(held, slot, size) };
        }
        s.task(me).state = State::Parked;
        let row = s.task(child);
        row.state = State::Ready;
        row.handoff = Some(me);
        let id = handle(child, row.generation);
        (
            (),
            Next::Call {
                members: vec![id],
                then_yield,
            },
        )
    };
    let (_, progress) = if delivery.is_some() {
        engine::head_effect(ctx, "task.join", effect)?
    } else {
        engine::head_continuation(ctx, effect)?
    };
    assert!(matches!(progress, Progress::Called(_)));
    engine::save_boundary(ctx, progress)
}

/// # Safety
/// As for TaskJoin. Only this task's slot or an idle-saved child's slot is used.
pub(crate) unsafe fn join(
    ctx: *mut TaskCtx,
    scope: *const u64,
    task: *mut u64,
    slot: *mut u8,
    size: u64,
    caller_site: u64,
) -> Stopped<u8> {
    engine::at_head(ctx)?;
    let s = scheduler(ctx);
    let scope = s.scope_handle(unsafe { scope.read() });
    let child = s.task_handle(unsafe { task.read() }, scope);
    match s.task(child).state {
        State::Holding => {
            unsafe { call(ctx, child, true, Some((slot, size))) }?;
        }
        State::Done => {
            let (_, progress) = engine::head_effect(ctx, "task.join", || ((), Next::Yield))?;
            engine::continue_at_head(ctx, progress)?;
        }
        State::Ready | State::Parked => {
            let (_, progress) = engine::head_effect(ctx, "task.join", || {
                let s = scheduler(ctx);
                let me = s.current;
                s.task(me).wait_site = caller_site;
                s.park(Wait::Join { child, slot, size });
                ((), Next::Park)
            })?;
            engine::save_boundary(ctx, progress)?;
            if scheduler(ctx).task(child).state == State::Holding {
                // The retirement poll already moved the held result. A return
                // from this child's rest continues the same turn, without poll.
                unsafe { call(ctx, child, false, None) }?;
            }
        }
    }
    let s = scheduler(ctx);
    assert_eq!(s.task(child).state, State::Done);
    assert!(readiness::saved(s, child));
    let chain = s.task(child).failure.take();
    s.scope(scope).children.retain(|&c| c != child);
    s.vacate_task(child);
    Ok(if let Some(chain) = chain {
        unsafe { (*ctx).failure.append_chain(chain) };
        STATUS_FAILURE
    } else {
        STATUS_OK
    })
}

/// Nonempty exits and their ordered reverse calls are a separate adapter.
/// # Safety
/// The scope has no remaining children or deadline; ctx owns its role.
pub(crate) unsafe fn empty_scope_exit(ctx: *mut TaskCtx, scope: *const u64) -> Stopped<u8> {
    let (_, progress) = engine::head_effect(ctx, "task.scope_exit", || {
        let s = scheduler(ctx);
        let scope = s.scope_handle(unsafe { scope.read() });
        assert!(
            s.scope(scope).children.is_empty(),
            "nonempty ordered exit is not admitted"
        );
        assert!(
            s.scope(scope).deadline.is_none(),
            "ordered deadline is not admitted"
        );
        let me = s.current;
        assert_eq!(s.task(me).scopes.pop(), Some(scope));
        s.cancel_epoch.changed();
        s.scopes[scope as usize] = None;
        s.vacant_scopes.push(scope);
        ((), Next::Keep)
    })?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    Ok(if unsafe { (*ctx).failure.active() } {
        STATUS_FAILURE
    } else {
        STATUS_OK
    })
}

/// # Safety
/// The child owns its last runtime frame and has dropped its environment.
pub(crate) unsafe fn complete(ctx: &mut TaskCtx, index: u32, status: u8) -> ! {
    unsafe { crate::stop::check(ctx, engine::at_head(ctx)) };
    let began_end = scheduler(ctx).task(index).ended;
    let ctx_ptr = std::ptr::from_mut(ctx);
    let effect = || {
        let s = scheduler(ctx_ptr);
        let row = s.task(index);
        let first_end = !std::mem::replace(&mut row.ended, true);
        let path = row.path.clone();
        let chain = unsafe { (*ctx_ptr).failure.take_chain(&path) };
        let failed = status != STATUS_OK;
        if failed != !chain.is_empty() {
            fault("a task's status disagrees with its failure state");
        }
        let s = scheduler(ctx_ptr);
        let row = s.task(index);
        row.finishing = true;
        if first_end && !failed {
            s.order(index);
        }
        if failed {
            s.task(index).failure = Some(chain);
            s.order(index);
            let scope = s.task(index).scope.expect("a child has a scope");
            s.cancel_scope(scope, CANCEL_CHILD_FAILED);
            let siblings = s.scope(scope).children.clone();
            s.wake_owner(scope);
            for sibling in siblings {
                if sibling != index {
                    s.cancel(sibling, CANCEL_CHILD_FAILED);
                }
            }
        }
        ((), Next::End)
    };
    let effect = if began_end {
        engine::head_continuation(ctx, effect)
    } else {
        engine::head_effect(ctx, "task.end", effect)
    };
    let (_, progress) = unsafe { crate::stop::check(ctx, effect) };
    let task = engine::current(ctx);
    let driver = engine::driver(ctx);
    if progress == Progress::Poll {
        driver.defer_poll_until_saved(task);
    } else {
        // The frontier returned into a caller without a scheduling point.
        driver.defer_return_until_saved(task);
    }
    driver.finishing(task);
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.complete();
    }
    if ctx.failure.active() || ctx.shield != 0 || !ctx.work.is_null() || !ctx.region.is_null() {
        fault("a task completed with context state left");
    }
    drop(std::mem::take(&mut ctx.failure));
    drop(std::mem::take(&mut ctx.frozen));
    drop(std::mem::take(&mut ctx.isolated));
    unsafe { crate::pool::free::finish_current(ctx) }
}

/// Called under IdleGuard after physical ownership invalidation/publication.
/// No TaskCtx bridge or switch is allowed here.
pub(super) fn after_save(s: &mut Scheduler, task: u32, completed: bool) {
    if completed && s.task(task).finishing {
        let row = s.task(task);
        row.finishing = false;
        row.state = State::Done;
        if let Some(stack) = row.stack.take() {
            // SAFETY: the completed token was invalidated and context state
            // consumed before idle. No other task can yet recycle this row.
            unsafe {
                std::ptr::drop_in_place(row.ctx);
                crate::pool::free::recycle_stack(stack);
            }
        }
        if let Some(parent) = s.task(task).handoff.take() {
            s.task(parent).state = State::Ready;
        }
    }
    if matches!(s.task(task).state, State::Holding | State::Done) {
        s.settle(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ordered::Policy;

    const NONE: [u64; 2] = [0, 0];

    unsafe extern "C" fn child(ctx: *mut TaskCtx, environment: *mut Header, _: u64, _: u64) -> u8 {
        let [id, atomic] = unsafe { crate::alloc::payload(environment).cast::<[u64; 2]>().read() };
        let local = [id; 32];
        for _ in 0..32 {
            let mut loaded = 0u64;
            assert_eq!(
                unsafe { hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut loaded).cast(), 8) },
                STATUS_OK
            );
            assert_eq!(loaded, id);
        }
        let mut result = [1, id * 100];
        assert_eq!(
            unsafe {
                hirsel_rt_v2_task_finish(ctx, result.as_mut_ptr().cast(), NONE.as_ptr().cast(), 16)
            },
            STATUS_OK
        );
        assert_eq!(result, NONE, "the join moved the holding result");
        assert_eq!(local, [id; 32], "a migrated rest retains its owned stack");
        // A rest's ordinary yield permits other turns before the no-poll call
        // return. The caller cannot vacate this row before physical completion.
        let value = id + 1000;
        unsafe { hirsel_rt_v2_atomic_store(ctx, &atomic, (&raw const value).cast(), 8) }
    }

    fn children_run(width: u32, holding_first: bool) {
        use hirsel_core_contract::handle_scan::{TYPED_HANDLE, WORDS};
        let scan = [TYPED_HANDLE, 8, Kind::Atomic.code(), 2, 0, 0, 0, 0];
        assert_eq!(scan.len(), WORDS);
        let environment = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
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
        let layouts = [&environment as *const crate::Layout];
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        // SAFETY: only the installed strict task and E boundaries are used.
        // Environments are dropless, there are no budgets/host/root effects,
        // every child is joined, and root completion is explicit below.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            move |ctx| unsafe {
                for _ in 0..3 {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
                    );
                    let mut children = Vec::new();
                    for id in 1..=8 {
                        let mut atomic = 0;
                        assert_eq!(
                            hirsel_rt_v2_atomic_create(
                                ctx,
                                std::ptr::null_mut(),
                                &mut atomic,
                                (&raw const id).cast(),
                                8
                            ),
                            STATUS_OK
                        );
                        let environment = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(environment)
                            .cast::<[u64; 2]>()
                            .write([id, atomic]);
                        let mut task = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut task,
                                &scope,
                                child,
                                environment
                            ),
                            STATUS_OK
                        );
                        children.push((task, atomic, id));
                    }
                    if holding_first {
                        // These serial head turns let every child's first half
                        // hold before a join starts its rest.
                        let atomic = children[0].1;
                        for _ in 0..64 {
                            let mut value = 0u64;
                            assert_eq!(
                                hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut value).cast(), 8),
                                STATUS_OK
                            );
                        }
                        role::enter(ctx);
                        assert!(
                            children
                                .iter()
                                .all(|&(task, _, _)| scheduler(ctx).task(task as u32).state
                                    == State::Holding)
                        );
                        role::release(ctx);
                    }
                    for (mut task, atomic, id) in children.into_iter().rev() {
                        let retired = if holding_first {
                            role::enter(ctx);
                            engine::at_head(ctx).unwrap();
                            let retired = engine::driver(ctx).frontier.as_ref().unwrap().retired();
                            role::release(ctx);
                            Some(retired)
                        } else { None };
                        let mut result = NONE;
                        assert_eq!(
                            hirsel_rt_v2_task_join(
                                ctx,
                                0,
                                0,
                                &scope,
                                &mut task,
                                result.as_mut_ptr().cast(),
                                16
                            ),
                            STATUS_OK
                        );
                        assert_eq!(result, [1, id * 100]);
                        if let Some(retired) = retired {
                            role::enter(ctx);
                            assert_eq!(
                                engine::driver(ctx).frontier.as_ref().unwrap().retired() - retired,
                                2,
                                "only the rest's store and the join yield poll; call return does not",
                            );
                            role::release(ctx);
                        }
                        let mut value = 0u64;
                        assert_eq!(
                            hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut value).cast(), 8),
                            STATUS_OK
                        );
                        assert_eq!(value, id + 1000, "join waits for the child's yielding rest");
                    }
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                }
                role::enter(ctx);
                engine::at_head(ctx).unwrap();
                assert!(scheduler(ctx).scopes.iter().all(Option::is_none));
                let task = engine::current(ctx);
                let driver = engine::driver(ctx);
                assert!(driver.frontier.as_ref().unwrap().retired() > 700);
                driver.finishing(task);
                role::release(ctx);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn native_spawn_park_join_and_call_rests_use_saved_ordered_frames() {
        for width in [2, 4, 8] {
            children_run(width, false);
        }
    }

    #[test]
    fn joining_already_holding_children_yields_only_after_their_rests_return() {
        for width in [2, 4, 8] {
            children_run(width, true);
        }
    }
}
