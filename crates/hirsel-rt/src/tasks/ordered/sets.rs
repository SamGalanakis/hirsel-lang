//! Task sets and cancellation. Iteration follows native add order;
//! the rest of a holding member is a call, and only the final detach-all phase
//! yields. Keep operations refresh obtainable handles before returning.

use super::super::*;
use super::engine;
use super::lifecycle::{self, Call, ResultAction};
use crate::ordered::footprint::{Footprint, Item};
use crate::ordered::objects::Kind;
use crate::ordered::{Next, Progress};

fn writes(items: impl IntoIterator<Item = Item>) -> Footprint {
    Footprint {
        writes: items.into_iter().collect(),
        ..Footprint::default()
    }
}

/// # Safety
/// As for TaskSet, with the scheduler role and test-only ordered policy.
pub(crate) unsafe fn create(ctx: *mut TaskCtx, scope: *const u64, out: *mut u64) -> Stopped<u8> {
    let scope = unsafe { scope.read() };
    let (index, generation) = {
        let s = scheduler(ctx);
        let generation = s.fresh_generation() & 0x3fff_ffff;
        // Remove a vacant row from reuse, without publishing a native TaskSet.
        // The owning create command installs it only at retirement.
        let index = match s.vacant_sets.pop() {
            Some(index) => index,
            None => {
                s.sets.push(None);
                (s.sets.len() - 1) as u32
            }
        };
        (index, generation)
    };
    let word = crate::channels::ENDPOINT | TASK_SET | handle(index, generation);
    let task = engine::current(ctx);
    let driver = engine::driver(ctx);
    // TaskSet's result type is erased too: it can later expose any member's
    // Task and the result handles/finalizers reachable from that Task.
    driver.objects.unknown(Kind::TaskSet, word);
    driver.objects.obtained(
        driver
            .frontier
            .as_mut()
            .unwrap()
            .held
            .get_mut(&task)
            .unwrap(),
        Kind::TaskSet,
        word,
    );
    engine::submit(
        ctx,
        super::Command::TaskSetCreate {
            index,
            generation,
            scope,
        }
        .operation(writes([Item::Task(word)])),
    )?;
    unsafe { out.write(word) };
    Ok(STATUS_OK)
}

/// # Safety
/// scope/set/task satisfy TaskAdd's ABI. All handles are copied before logging.
pub(crate) unsafe fn add(
    ctx: *mut TaskCtx,
    scope: *const u64,
    set: *mut u64,
    task: *mut u64,
) -> Stopped<u8> {
    let scope = unsafe { scope.read() };
    let set = unsafe { set.read() };
    let child = unsafe { task.read() };
    engine::submit(
        ctx,
        super::Command::TaskSetAdd { scope, set, child }
            .operation(writes([Item::Task(set), Item::Task(child)])),
    )?;
    Ok(STATUS_OK)
}

/// Closing the set releases its table row; the members remain scope children.
pub(super) fn close(ctx: *mut TaskCtx, set: u64) -> Stopped<()> {
    engine::submit(
        ctx,
        super::Command::TaskSetClose { set }.operation(writes([Item::Task(set)])),
    )?;
    Ok(())
}

/// # Safety
/// As for TaskSetHas; this op keeps the current turn.
pub(crate) unsafe fn has(
    ctx: *mut TaskCtx,
    scope: *const u64,
    set: *const u64,
    index: u64,
    out: *mut u8,
) -> Stopped<u8> {
    let (has, progress) = engine::head_effect(ctx, "task.set_has", || {
        let s = scheduler(ctx);
        let scope = s.scope_handle(unsafe { scope.read() });
        let set = s.set_handle(unsafe { set.read() }, scope);
        let has = usize::try_from(index)
            .ok()
            .and_then(|i| s.set(set).members.get(i))
            .is_some_and(Option::is_some);
        (has, Next::Keep)
    })?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    unsafe { out.write(u8::from(has)) };
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskSetTake. Update Held before the following same-turn boundary.
pub(crate) unsafe fn take(
    ctx: *mut TaskCtx,
    scope: *const u64,
    set: *mut u64,
    index: u64,
    out: *mut u64,
) -> Stopped<u8> {
    let (word, progress) = engine::head_effect(ctx, "task.set_take", || {
        let s = scheduler(ctx);
        let scope = s.scope_handle(unsafe { scope.read() });
        let set = s.set_handle(unsafe { set.read() }, scope);
        let word = usize::try_from(index)
            .ok()
            .and_then(|i| s.set(set).members.get_mut(i))
            .and_then(Option::take)
            .unwrap_or_else(|| fault("a task set member taken twice"));
        (word, Next::Keep)
    })?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    let task = engine::current(ctx);
    let driver = engine::driver(ctx);
    driver.objects.obtained(
        driver
            .frontier
            .as_mut()
            .unwrap()
            .held
            .get_mut(&task)
            .unwrap(),
        Kind::Task,
        word,
    );
    unsafe { out.write(word) };
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskAwaitAny. The set remains borrowed through any parked frame.
pub(crate) unsafe fn await_any(
    ctx: *mut TaskCtx,
    scope: *const u64,
    set: *const u64,
    out: *mut IndexOption,
    caller_site: u64,
) -> Stopped<u8> {
    let ((set, waiting, must_wait), progress) = engine::head_effect(ctx, "task.await_any", || {
        let s = scheduler(ctx);
        let scope = s.scope_handle(unsafe { scope.read() });
        let set = s.set_handle(unsafe { set.read() }, scope);
        let waiting = s.set(set).members.iter().any(Option::is_some);
        let must_wait = waiting && s.first_completed(set).is_none();
        ((set, waiting, must_wait), Next::Yield)
    })?;
    engine::continue_at_head(ctx, progress)?;
    if must_wait {
        let (_, progress) = engine::head_continuation(ctx, || {
            let s = scheduler(ctx);
            let me = s.current;
            s.task(me).wait_site = caller_site;
            s.park(Wait::Any { set, member: None });
            ((), Next::Park)
        })?;
        engine::save_boundary(ctx, progress)?;
    }
    let index = if waiting {
        scheduler(ctx).first_completed(set)
    } else {
        None
    };
    unsafe {
        out.write(IndexOption {
            tag: u32::from(index.is_some()),
            padding: 0,
            index: index.unwrap_or(0),
        })
    };
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskCancel. Copy the handles before logging; cancellation changes
/// only at retirement, before that turn's poll.
pub(crate) unsafe fn cancel(ctx: *mut TaskCtx, scope: *const u64, task: *const u64) -> Stopped<u8> {
    let scope = unsafe { scope.read() };
    let child = unsafe { task.read() };
    engine::submit(
        ctx,
        super::Command::TaskCancel { scope, child }
            .operation(writes([Item::Cancel(child), Item::Task(child)])),
    )?;
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskCanceled. The cause is fixed before this check's yield.
pub(crate) unsafe fn canceled(ctx: *mut TaskCtx, out: *mut u32) -> Stopped<u8> {
    let (cause, progress) = engine::head_effect(ctx, "task.check_canceled", || {
        let cause = scheduler(ctx).canceled(unsafe { (*ctx).shield });
        (cause.map_or(0, |cause| cause + 1), Next::Yield)
    })?;
    unsafe { out.write(cause) };
    engine::continue_at_head(ctx, progress)?;
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskDetachAll/TaskCancelAll. Only this owner mutates the set.
pub(crate) unsafe fn release(
    ctx: *mut TaskCtx,
    scope: *const u64,
    set: *mut u64,
    cancel: bool,
    caller_site: u64,
) -> Stopped<u8> {
    if !cancel {
        let scope = unsafe { scope.read() };
        let set = unsafe { set.read() };
        engine::submit(
            ctx,
            super::Command::TaskDetachAll { scope, set }.operation(writes([Item::Task(set)])),
        )?;
        return Ok(STATUS_OK);
    }
    let name = if cancel {
        "task.cancel_all"
    } else {
        "task.detach_all"
    };
    let ((scope, set, members), _) = engine::head_effect(ctx, name, || {
        let s = scheduler(ctx);
        let scope = s.scope_handle(unsafe { scope.read() });
        let set = s.set_handle(unsafe { set.read() }, scope);
        let members = s
            .set(set)
            .members
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        ((scope, set, members), Next::Keep)
    })?;
    for word in members {
        // Earlier calls can complete and vacate other members. Native words
        // are generation-qualified; an already-retired member needs no effect.
        let Some(child) = scheduler(ctx).member(word) else {
            continue;
        };
        engine::head_continuation(ctx, || {
            let s = scheduler(ctx);
            if cancel {
                s.cancel(child, CANCEL_REQUESTED);
            }
            s.task(child).detached = true;
            ((), Next::Keep)
        })?;
        if scheduler(ctx).task(child).state == State::Holding {
            unsafe {
                lifecycle::call(
                    ctx,
                    child,
                    Call {
                        begin: None,
                        then_yield: false,
                        result: ResultAction::Discard,
                        detached: true,
                    },
                )
            }?;
        } else if scheduler(ctx).task(child).state == State::Done
            && scheduler(ctx).task(child).failure.is_none()
        {
            engine::head_continuation(ctx, || {
                let s = scheduler(ctx);
                assert!(readiness::saved(s, child));
                s.scope(scope).children.retain(|&c| c != child);
                s.vacate_task(child);
                ((), Next::Keep)
            })?;
        }
    }
    let wait = cancel && !unsafe { poll_drained(scheduler(ctx), set) };
    let (_, progress) = engine::head_continuation(ctx, || {
        if !cancel {
            scheduler(ctx).close_set(set);
        }
        ((), Next::Yield)
    })?;
    engine::continue_at_head(ctx, progress)?;
    if wait {
        let (_, progress) = engine::head_continuation(ctx, || {
            let s = scheduler(ctx);
            let me = s.current;
            s.task(me).wait_site = caller_site;
            s.park(Wait::Drain { set });
            ((), Next::Park)
        })?;
        engine::save_boundary(ctx, progress)?;
    }
    if cancel {
        engine::head_continuation(ctx, || {
            scheduler(ctx).close_set(set);
            ((), Next::Keep)
        })?;
    }
    Ok(STATUS_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ordered::Policy;

    const NONE: [u64; 2] = [0, 0];

    unsafe extern "C" fn child(ctx: *mut TaskCtx, environment: *mut Header, _: u64, _: u64) -> u8 {
        let [id, trace, mode, observed] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 4]>().read() };
        let mut cause = 0;
        for _ in 0..if mode >= 2 { 128 } else { 4 } {
            assert_eq!(
                unsafe { hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
                STATUS_OK
            );
            if mode >= 2 && cause != 0 {
                break;
            }
        }
        if mode >= 2 {
            assert_eq!(cause, 1 + CANCEL_REQUESTED);
        }
        let cause = u64::from(cause);
        assert_eq!(
            unsafe { hirsel_rt_v2_atomic_store(ctx, &observed, (&raw const cause).cast(), 8) },
            STATUS_OK
        );
        let mut result = [1, id];
        assert_eq!(
            unsafe {
                hirsel_rt_v2_task_finish(ctx, result.as_mut_ptr().cast(), NONE.as_ptr().cast(), 16)
            },
            STATUS_OK
        );
        assert_eq!(result, if matches!(mode, 0 | 3) { NONE } else { [1, id] });
        if mode < 2 {
            let mut value = 0u64;
            assert_eq!(
                unsafe { hirsel_rt_v2_atomic_load(ctx, &trace, (&raw mut value).cast(), 8) },
                STATUS_OK
            );
            value = value * 10 + id;
            assert_eq!(
                unsafe { hirsel_rt_v2_atomic_store(ctx, &trace, (&raw const value).cast(), 8) },
                STATUS_OK
            );
        }
        STATUS_OK
    }

    fn run_mode(width: u32, mode: u64) {
        use hirsel_core_contract::handle_scan::TYPED_HANDLE;
        let scan = [
            TYPED_HANDLE,
            8,
            Kind::Atomic.code(),
            2,
            0,
            0,
            0,
            0,
            TYPED_HANDLE,
            24,
            Kind::Atomic.code(),
            2,
            0,
            0,
            0,
            0,
        ];
        let environment = crate::Layout {
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
        let layouts = [&environment as *const crate::Layout];
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        // SAFETY: installed task/E boundaries only; no budgets, deadlines,
        // Shared roots, host or resource-bearing finalizers.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let mut set = 0;
                assert_eq!(
                    hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope),
                    STATUS_OK
                );
                let mut trace = 0;
                let initial = 0u64;
                assert_eq!(
                    hirsel_rt_v2_atomic_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut trace,
                        (&raw const initial).cast(),
                        8
                    ),
                    STATUS_OK
                );
                let mut children = Vec::new();
                for id in 1..=3u64 {
                    let mut observed = 0;
                    assert_eq!(
                        hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut observed,
                            (&raw const initial).cast(),
                            8
                        ),
                        STATUS_OK
                    );
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
                        .write([id, trace, mode, observed]);
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
                            environment,
                            0,
                        ),
                        STATUS_OK
                    );
                    children.push((task, observed));
                }
                // Add order differs from completion/spawn order.
                for member in [2, 0, 1] {
                    assert_eq!(
                        hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut children[member].0),
                        STATUS_OK
                    );
                }
                if mode < 2 {
                    for _ in 0..16 {
                        let mut cause = u32::MAX;
                        assert_eq!(
                            hirsel_rt_v2_task_canceled(ctx, &mut cause, &scope),
                            STATUS_OK
                        );
                        assert_eq!(cause, 0);
                    }
                }
                if matches!(mode, 0 | 3) {
                    if mode == 3 {
                        for member in [1, 2, 0] {
                            assert_eq!(
                                hirsel_rt_v2_task_cancel(ctx, &scope, &children[member].0),
                                STATUS_OK
                            );
                        }
                    }
                    let mut order = Vec::new();
                    for _ in 0..3 {
                        let mut index = IndexOption {
                            tag: 0,
                            padding: 0,
                            index: 0,
                        };
                        assert_eq!(
                            hirsel_rt_v2_task_await_any(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut index,
                                &scope,
                                &set
                            ),
                            STATUS_OK
                        );
                        assert_eq!(index.tag, 1);
                        let mut has = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, index.index),
                            STATUS_OK
                        );
                        assert_eq!(has, 1);
                        let mut task = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_set_take(
                                ctx,
                                std::ptr::null_mut(),
                                &mut task,
                                &scope,
                                &mut set,
                                index.index
                            ),
                            STATUS_OK
                        );
                        assert_eq!(
                            hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, index.index),
                            STATUS_OK
                        );
                        assert_eq!(has, 0);
                        role::enter(ctx);
                        let me = engine::current(ctx);
                        assert!(
                            engine::driver(ctx).frontier.as_ref().unwrap().held[&me].unknown,
                            "TaskSetTake's erased result keeps the obtainable closure conservative"
                        );
                        role::release(ctx);
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
                        order.push(result[1]);
                    }
                    if mode == 0 {
                        assert_eq!(order, vec![1, 2, 3]);
                    } else {
                        order.sort_unstable();
                        assert_eq!(order, vec![1, 2, 3]);
                    }
                    let mut index = IndexOption {
                        tag: 1,
                        padding: 0,
                        index: 99,
                    };
                    assert_eq!(
                        hirsel_rt_v2_task_await_any(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut index,
                            &scope,
                            &set
                        ),
                        STATUS_OK
                    );
                    assert_eq!(index.tag, 0);
                    assert_eq!(
                        hirsel_rt_v2_task_detach_all(ctx, &scope, &mut set),
                        STATUS_OK
                    );
                } else if mode == 1 {
                    assert_eq!(
                        hirsel_rt_v2_task_detach_all(ctx, &scope, &mut set),
                        STATUS_OK
                    );
                } else {
                    assert_eq!(
                        hirsel_rt_v2_task_cancel_all(ctx, 0, 0, &scope, &mut set),
                        STATUS_OK
                    );
                }
                for (_, observed) in children {
                    let mut cause = u64::MAX;
                    assert_eq!(
                        hirsel_rt_v2_atomic_load(ctx, &observed, (&raw mut cause).cast(), 8),
                        STATUS_OK
                    );
                    assert_eq!(
                        cause,
                        if mode >= 2 {
                            1 + u64::from(CANCEL_REQUESTED)
                        } else {
                            0
                        }
                    );
                }
                if mode < 2 {
                    let mut value = 0u64;
                    assert_eq!(
                        hirsel_rt_v2_atomic_load(ctx, &trace, (&raw mut value).cast(), 8),
                        STATUS_OK
                    );
                    assert_eq!(
                        value,
                        if mode == 0 { 123 } else { 312 },
                        "holding rests run in member order"
                    );
                }
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                role::enter(ctx);
                engine::at_head(ctx).unwrap();
                let task = engine::current(ctx);
                engine::driver(ctx).finishing(task);
                role::release(ctx);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn task_set_take_keeps_obtainable_footprints_and_await_any_uses_completion_rank() {
        for width in [2, 4, 8] {
            run_mode(width, 0);
        }
    }
    #[test]
    fn detach_all_calls_holding_rests_in_add_order_and_yields_once_at_the_end() {
        for width in [2, 4, 8] {
            run_mode(width, 1);
        }
    }
    #[test]
    fn cancel_all_commits_causes_before_drain_and_retires_each_detached_frame() {
        for width in [2, 4, 8] {
            run_mode(width, 2);
        }
    }
    #[test]
    fn direct_cancel_and_subsequent_checks_observe_the_committed_requested_cause() {
        for width in [2, 4, 8] {
            run_mode(width, 3);
        }
    }
}
