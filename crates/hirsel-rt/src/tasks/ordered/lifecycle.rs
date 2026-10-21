//! Strict-head native task effects over the saved-frame dispatcher.
//!
//! Follows the cooperative task tree and Frontier's checked call transitions.
//! Task mutation happens in a head callback, after its preceding stretch. A
//! holding frame is read only after idle save; completion retires its physical
//! token before publishing Done or returning to a caller. This test-only slice
//! does not yet admit deadlines, root releases or host effects.

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
    let parent = engine::current(ctx);
    let driver = engine::driver(ctx);
    let held = driver.objects.captured(&captures);
    // Task result types are erased by TaskSpawn's current native ABI. An
    // unknown binding conservatively blocks passes through a captured task.
    driver.objects.unknown(Kind::Task, word);
    driver.objects.obtained(
        driver
            .frontier
            .as_mut()
            .unwrap()
            .held
            .get_mut(&parent)
            .unwrap(),
        Kind::Task,
        word,
    );
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
    if scheduler(ctx).task(me).detached {
        // A detached body retains its result for its own cleanup. Its actual
        // end commits after that cleanup, in complete().
        return Ok(STATUS_OK);
    }
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

#[derive(Clone, Copy)]
pub(super) enum ResultAction {
    Keep,
    Deliver(*mut u8, u64),
    Discard,
}

#[derive(Clone, Copy)]
pub(super) struct Call {
    /// None resumes the same op after its waiting phase or an earlier call.
    pub(super) begin: Option<&'static str>,
    pub(super) then_yield: bool,
    pub(super) result: ResultAction,
    pub(super) detached: bool,
}

/// Execute a holding child's rest as a call, never as a FIFO enqueue.
/// # Safety
/// The child is holding a physically saved frame; the caller owns the role.
pub(super) unsafe fn call(ctx: *mut TaskCtx, child: u32, call: Call) -> Stopped<()> {
    let effect = || {
        let s = scheduler(ctx);
        let me = s.current;
        assert!(readiness::saved(s, child));
        assert_eq!(s.task(child).state, State::Holding);
        match call.result {
            ResultAction::Keep => {}
            ResultAction::Deliver(slot, size) => {
                let held = s.task(child).slot.take().expect("a holding result");
                // SAFETY: the child is saved and this callback owns the caller.
                unsafe { deliver(held, slot, size) };
            }
            ResultAction::Discard => s.task(child).slot = None,
        }
        s.task(me).state = State::Parked;
        let row = s.task(child);
        row.detached |= call.detached;
        row.state = State::Ready;
        row.handoff = Some(me);
        let id = handle(child, row.generation);
        (
            (),
            Next::Call {
                members: vec![id],
                then_yield: call.then_yield,
            },
        )
    };
    let (_, progress) = if let Some(name) = call.begin {
        engine::head_effect(ctx, name, effect)?
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
            unsafe {
                call(
                    ctx,
                    child,
                    Call {
                        begin: Some("task.join"),
                        then_yield: true,
                        result: ResultAction::Deliver(slot, size),
                        detached: false,
                    },
                )
            }?;
        }
        State::Done => {
            let (_, progress) = engine::head_effect(ctx, "task.join", || ((), Next::Yield))?;
            engine::continue_at_head(ctx, progress)?;
        }
        State::Ready | State::Parked => {
            // Cooperative block(yielded=true) first yields, then parks when
            // the caller reaches the front again, even if the child finished
            // during that first scheduling point.
            let (_, progress) = engine::head_effect(ctx, "task.join", || ((), Next::Yield))?;
            engine::continue_at_head(ctx, progress)?;
            let (_, progress) = engine::head_continuation(ctx, || {
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
                unsafe {
                    call(
                        ctx,
                        child,
                        Call {
                            begin: None,
                            then_yield: false,
                            result: ResultAction::Keep,
                            detached: false,
                        },
                    )
                }?;
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

/// # Safety
/// As for TaskScopeExit; ordered deadlines are not admitted yet.
pub(crate) unsafe fn scope_exit(
    ctx: *mut TaskCtx,
    scope: *const u64,
    error: u8,
    caller_site: u64,
) -> Stopped<u8> {
    let ((scope, waited), progress) = engine::head_effect(ctx, "task.scope_exit", || {
        let s = scheduler(ctx);
        let scope = s.scope_handle(unsafe { scope.read() });
        assert!(
            s.scope(scope).deadline.is_none(),
            "ordered deadline is not admitted"
        );
        let me = s.current;
        assert_eq!(s.task(me).scopes.pop(), Some(scope));
        s.cancel_epoch.changed();
        if error != 0 || unsafe { (*ctx).failure.active() } {
            for child in s.scope(scope).children.clone() {
                s.cancel(child, CANCEL_PARENT);
            }
        }
        let waited = !s.settled(scope);
        if waited {
            s.task(me).wait_site = caller_site;
            s.park(Wait::Exit { scope });
        }
        (
            (scope, waited),
            if waited { Next::Park } else { Next::Keep },
        )
    })?;
    if waited {
        engine::save_boundary(ctx, progress)?;
    }
    let children = scheduler(ctx).scope(scope).children.clone();
    for &child in children.iter().rev() {
        if scheduler(ctx).task(child).state == State::Holding {
            unsafe {
                call(
                    ctx,
                    child,
                    Call {
                        begin: None,
                        then_yield: false,
                        result: ResultAction::Discard,
                        detached: false,
                    },
                )
            }?;
        }
    }
    engine::head_continuation(ctx, || {
        let s = scheduler(ctx);
        let me = s.current;
        let own = s.task(me).path.clone();
        for child in children {
            assert_eq!(s.task(child).state, State::Done);
            assert!(readiness::saved(s, child));
            if let Some(chain) = s.task(child).failure.take() {
                unsafe { (*ctx).failure.merge_chain(&own, chain) };
            }
            s.vacate_task(child);
        }
        s.scopes[scope as usize] = None;
        s.vacant_scopes.push(scope);
        ((), Next::Keep)
    })?;
    Ok(if unsafe { (*ctx).failure.active() } {
        STATUS_FAILURE
    } else {
        STATUS_OK
    })
}

/// # Safety
/// As for TaskDetach. A successful detached completion retires its row at idle.
pub(crate) unsafe fn detach(ctx: *mut TaskCtx, scope: *const u64, task: *mut u64) -> Stopped<u8> {
    engine::at_head(ctx)?;
    let s = scheduler(ctx);
    let scope = s.scope_handle(unsafe { scope.read() });
    let child = s.task_handle(unsafe { task.read() }, scope);
    if s.task(child).state == State::Holding {
        unsafe {
            call(
                ctx,
                child,
                Call {
                    begin: Some("task.detach"),
                    then_yield: true,
                    result: ResultAction::Discard,
                    detached: true,
                },
            )
        }?;
    } else {
        let (_, progress) = engine::head_effect(ctx, "task.detach", || {
            let s = scheduler(ctx);
            let row = s.task(child);
            row.detached = true;
            if row.state == State::Done && row.failure.is_none() {
                assert!(readiness::saved(s, child));
                s.scope(scope).children.retain(|&c| c != child);
                s.vacate_task(child);
            }
            ((), Next::Yield)
        })?;
        engine::continue_at_head(ctx, progress)?;
    }
    Ok(STATUS_OK)
}

/// # Safety
/// As for ChannelReceive. Its output image remains owner-local after resume.
pub(crate) unsafe fn receive(
    ctx: *mut TaskCtx,
    receiver: *mut u64,
    message: *mut u8,
    out: *mut u32,
    caller_site: u64,
) -> Stopped<u8> {
    engine::at_head(ctx)?;
    let word = unsafe { receiver.read() };
    let descriptor = engine::carrier_descriptor(ctx, Kind::Receiver, word);
    let me = scheduler(ctx).current;
    let (status, progress) = engine::head_effect(ctx, "channel.receive", || {
        let s = scheduler(ctx);
        let channel = s.channels.channel(word, false);
        let shield = unsafe { (*ctx).shield };
        // SAFETY: this head callback owns the receiving task and its output.
        let status = unsafe { receive_ready(s, me, shield, channel, message) };
        (status, Next::Yield)
    })?;
    let status = if let Some(status) = status {
        engine::continue_at_head(ctx, progress)?;
        status
    } else {
        engine::continue_at_head(ctx, progress)?;
        let (_, progress) = engine::head_continuation(ctx, || {
            let s = scheduler(ctx);
            let channel = s.channels.channel(word, false);
            s.task(me).wait_site = caller_site;
            s.park(Wait::Receive {
                channel,
                slot: message,
            });
            ((), Next::Park)
        })?;
        engine::save_boundary(ctx, progress)?;
        scheduler(ctx).task(me).status
    };
    if status == crate::channels::DONE {
        // The receive ends its turn before obtained handles may be used. Its
        // next segment refreshes the held set before any following E boundary.
        unsafe { engine::obtained_image(ctx, message, descriptor) };
    }
    unsafe { out.write(status) };
    Ok(STATUS_OK)
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

/// The current callback already used this generation for its barrier. No
/// later callback can access the row after this exclusive idle retirement.
pub(super) fn retire_detached(s: &mut Scheduler, task: u32) {
    let row = s.task(task);
    if row.state == State::Done && row.detached && row.failure.is_none() {
        let scope = row.scope.expect("a detached child has a scope");
        s.scope(scope).children.retain(|&c| c != task);
        s.vacate_task(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ordered::Policy;

    const NONE: [u64; 2] = [0, 0];

    unsafe extern "C" fn child(ctx: *mut TaskCtx, environment: *mut Header, _: u64, _: u64) -> u8 {
        let [identity, atomic] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 2]>().read() };
        let id = identity & 0xff;
        let discarded = identity >> 63 != 0;
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
        assert_eq!(
            result,
            if discarded { [1, id * 100] } else { NONE },
            "discard keeps the result for its owner; join moves it",
        );
        assert_eq!(local, [id; 32], "a migrated rest retains its owned stack");
        // A rest's ordinary yield permits other turns before the no-poll call
        // return. The caller cannot vacate this row before physical completion.
        let value = id + 1000;
        unsafe { hirsel_rt_v2_atomic_store(ctx, &atomic, (&raw const value).cast(), 8) }
    }

    fn children_run(width: u32, holding_first: bool, consume: u8) {
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
                            .write([id | if consume == 0 { 0 } else { 1 << 63 }, atomic]);
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
                        if consume == 2 {
                            assert_eq!(hirsel_rt_v2_task_detach(ctx, &scope, &mut task), STATUS_OK);
                        }
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
                    if matches!(consume, 1 | 2) {
                        assert_eq!(hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0), STATUS_OK);
                    }
                    for (mut task, atomic, id) in children.into_iter().rev() {
                        let retired = if holding_first && matches!(consume, 0 | 3) {
                            role::enter(ctx);
                            engine::at_head(ctx).unwrap();
                            let retired = engine::driver(ctx).frontier.as_ref().unwrap().retired();
                            role::release(ctx);
                            Some(retired)
                        } else { None };
                        if consume == 0 {
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
                        } else if consume == 3 {
                            assert_eq!(hirsel_rt_v2_task_detach(ctx, &scope, &mut task), STATUS_OK);
                        }
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
                    if !matches!(consume, 1 | 2) {
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    }
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
            children_run(width, false, 0);
        }
    }

    #[test]
    fn joining_already_holding_children_yields_only_after_their_rests_return() {
        for width in [2, 4, 8] {
            children_run(width, true, 0);
        }
    }
    #[test]
    fn scope_exit_calls_unjoined_holding_rests_and_waits_for_their_physical_completion() {
        for width in [2, 4, 8] {
            children_run(width, false, 1);
        }
    }

    #[test]
    fn detached_running_and_holding_children_retire_their_own_results_and_rows() {
        for width in [2, 4, 8] {
            children_run(width, false, 2);
            children_run(width, true, 3);
        }
    }

    unsafe extern "C" fn send_child(
        ctx: *mut TaskCtx,
        environment: *mut Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let mut sender = unsafe { crate::alloc::payload(environment).cast::<u64>().read() };
        let mut scope = 0;
        assert_eq!(
            unsafe { hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0) },
            STATUS_OK
        );
        let mut message = [1u64, 77];
        let mut status = u32::MAX;
        assert_eq!(
            unsafe {
                hirsel_rt_v2_channel_send(
                    ctx,
                    &mut status,
                    &mut sender,
                    message.as_mut_ptr().cast(),
                )
            },
            STATUS_OK
        );
        assert_eq!(status, crate::channels::DONE);
        assert_eq!(message, NONE);
        assert_eq!(
            unsafe { hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0) },
            STATUS_OK
        );
        let mut result = [1u64, 9];
        unsafe {
            hirsel_rt_v2_task_finish(ctx, result.as_mut_ptr().cast(), NONE.as_ptr().cast(), 16)
        }
    }

    fn receive_run(width: u32, sender: bool) {
        use hirsel_core_contract::handle_scan::TYPED_HANDLE;
        let scan = [TYPED_HANDLE, 0, Kind::Sender.code(), 2, 0, 0, 0, 0];
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
            scan_words: 8,
        };
        let image = crate::Layout {
            scan: std::ptr::null(),
            scan_words: 0,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &image];
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        // SAFETY: only supported task/E/receive boundaries; no budgets, host,
        // finalizers or Shared roots. Quiescent endpoint cleanup is explicit.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let mut endpoints = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        endpoints.as_mut_ptr(),
                        &0,
                        NONE.as_ptr().cast(),
                        3
                    ),
                    STATUS_OK
                );
                let mut child = 0;
                if sender {
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
                        .write([endpoints[0], 0]);
                    assert_eq!(
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut child,
                            &scope,
                            send_child,
                            environment
                        ),
                        STATUS_OK
                    );
                }
                let mut status = u32::MAX;
                let mut message = NONE;
                assert_eq!(
                    hirsel_rt_v2_channel_receive(
                        ctx,
                        0,
                        0,
                        &mut status,
                        &mut endpoints[1],
                        message.as_mut_ptr().cast()
                    ),
                    STATUS_OK
                );
                assert!(sender, "a blocked receive cannot continue");
                assert_eq!(status, crate::channels::DONE);
                assert_eq!(message, [1, 77]);
                let mut result = NONE;
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut child,
                        result.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(result, [1, 9]);
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                role::enter(ctx);
                engine::at_head(ctx).unwrap();
                scheduler(ctx).channels.close(endpoints[0]);
                scheduler(ctx).channels.close(endpoints[1]);
                let task = engine::current(ctx);
                engine::driver(ctx).finishing(task);
                role::release(ctx);
                STATUS_OK
            },
        );
        if sender {
            assert_eq!(outcome.ended, Ok(STATUS_OK));
        } else {
            let Err(Stop::Blocked(waits)) = outcome.ended else {
                panic!("{outcome:?}")
            };
            assert_eq!(waits.len(), 1);
            assert_eq!(waits[0].kind, "channel.receive");
            assert!(waits[0].spawn_path.is_empty());
        }
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn a_real_ordered_receive_wakes_from_a_retired_send_and_moves_the_saved_slot() {
        for width in [2, 4, 8] {
            receive_run(width, true);
        }
    }

    #[test]
    fn the_last_ordered_receive_reports_blocked_only_after_idle_save_and_poll() {
        for width in [2, 4, 8] {
            receive_run(width, false);
        }
    }
}
