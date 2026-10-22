//! Logical free-policy transitions after an exclusive stack reaches idle.
//!
//! Physical dispatch owns stacks; this policy owns readiness and completion.
//! Like Tokio's task harness, a running notification is remembered until the
//! idle continuation relinquishes running ownership. A logical park alone
//! never authorizes another task to read the parked stack.

use super::*;

pub(crate) mod cancellation;
pub(crate) mod channel;
pub(crate) mod cleanup;
#[cfg(test)]
mod finalizer_replay_tests;
pub(crate) mod foreign;
pub(crate) mod gate;
pub(crate) mod home;
pub(crate) mod host;
pub(crate) mod lifecycle;
pub(crate) mod mutex;
pub(crate) mod record;
pub(crate) mod replay;
pub(crate) mod sets;
mod spawn;

#[derive(Default)]
pub(crate) struct Driver {
    replay: Option<std::sync::Arc<replay::Replay>>,
}

/// Admit the free logical policy for explicit dispatcher tests. General
/// activation additionally requires the semantic suspension/end branches,
/// recorder sections, host readiness and concurrent service lifetime audit.
/// # Safety
/// Admission only, before the root starts. The body uses only saved-stack
/// dispatch and supported waits; it cannot invoke cooperative switch paths.
pub(crate) unsafe fn enable(ctx: *mut TaskCtx) {
    scheduler(ctx).logical = Logical::Free(Driver::default());
}

pub(crate) fn enabled(ctx: *mut TaskCtx) -> bool {
    matches!(scheduler(ctx).logical, Logical::Free(_))
}

/// # Safety
/// The calling task holds the role and no table borrow crosses publication.
pub(super) unsafe fn dispatch_ready(ctx: *mut TaskCtx) {
    let run = unsafe { (*ctx).run_ptr() };
    unsafe { publish_ready(scheduler(ctx), run) };
}

/// Publish already committed runtime wakes without a stop-aware ctx accessor.
/// # Safety
/// The role owns s and run; no table borrow survives queue publication.
pub(super) unsafe fn publish_ready(s: &mut Scheduler, run: *mut crate::context::RunShared) {
    let ready = {
        std::mem::take(&mut s.runnable)
            .into_iter()
            .map(|task| (task, s.task(task).owner))
            .collect::<Vec<_>>()
    };
    for (task, owner) in ready {
        unsafe { crate::pool::free::schedule_saved(run, task, owner) };
    }
}

/// # Safety
/// The running task holds the role; its operation's borrows have ended.
pub(super) unsafe fn try_yield(ctx: &mut TaskCtx) -> Stopped<()> {
    replay::wake(scheduler(ctx));
    if replay::at_cut(ctx) {
        park_final(ctx);
    }
    // Runtime polling performs no adapter effects. Home observes readiness
    // and cancellation with no role or parked-context borrow.
    unsafe { super::readiness::poll_free(scheduler(ctx)) };
    unsafe { dispatch_ready(ctx) };
    if ctx
        .run_ref()
        .host_waits
        .load(std::sync::atomic::Ordering::Acquire)
        != 0
    {
        unsafe { crate::pool::wake_home(ctx.run_ptr()) };
    }
    let s = scheduler(ctx);
    let me = s.current;
    let row = s.task(me);
    unpin(row);
    if row.budget.proceed() {
        if let Some(metrics) = &ctx.run_ref().worker_metrics {
            metrics.elision();
        }
        unsafe { migrate(ctx) };
    } else {
        row.budget.reset();
        unsafe { suspend(ctx, true) };
    }
    crate::stop::observe(ctx)
}

/// # Safety
/// The running task owns the role and retains no runtime borrow at save.
pub(super) unsafe fn suspend(ctx: &mut TaskCtx, yielded: bool) {
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.switch();
    }
    unsafe { crate::pool::free::suspend_current(ctx, yielded) };
}

/// # Safety
/// As for try_yield, including any foreign-affine ownership transition.
pub(super) unsafe fn migrate(ctx: &mut TaskCtx) {
    let s = scheduler(ctx);
    let me = s.current;
    let (owner, last) = {
        let row = s.task(me);
        (row.owner, row.last)
    };
    if crate::pool::target(ctx, owner, last).is_some() {
        unsafe {
            crate::pool::free::schedule_saved(ctx.run_ptr(), me, owner);
            suspend(ctx, false);
        }
    }
}

/// Record logical completion but leave its stack and Done publication to
/// after_save. No Rust-owned value remains on the abandoned fiber.
/// # Safety
/// This is the running child, holding the scheduler role after body cleanup.
pub(super) unsafe fn complete(ctx: &mut TaskCtx, index: u32, status: u8) -> ! {
    unsafe { crate::stop::check(ctx, lifecycle::completion(ctx, index, status)) };
    if ctx.failure.active() || ctx.shield != 0 || !ctx.work.is_null() || !ctx.region.is_null() {
        fault("a task completed with context state left");
    }
    drop(std::mem::take(&mut ctx.failure));
    drop(std::mem::take(&mut ctx.frozen));
    drop(std::mem::take(&mut ctx.isolated));
    unsafe { crate::pool::free::finish_current(ctx) }
}

/// Called only by tasks::after_save under its context-independent idle guard.
pub(super) unsafe fn after_save(
    s: &mut Scheduler,
    task: u32,
    completed: bool,
    pool_available: bool,
) -> Vec<(u32, Owner)> {
    replay::wake(s);
    if let Some(Some(row)) = s.tasks.get(task as usize) {
        if completed && row.finishing {
            let row = s.task(task);
            row.finishing = false;
            row.state = State::Done;
            if let Some(stack) = row.stack.take() {
                // SAFETY: the completed physical image was invalidated before
                // this callback, and its last runtime frame consumed ctx state.
                unsafe {
                    std::ptr::drop_in_place(row.ctx);
                    crate::pool::free::recycle_stack(stack);
                }
            }
        }
        let row = s.task(task);
        match row.state {
            State::Parked if row.wait.is_some() => s.wake(task),
            State::Holding if row.detached => {
                row.slot = None;
                row.state = State::Ready;
                s.runnable.push_back(task);
            }
            State::Holding | State::Done => s.settle(task),
            _ => {}
        }
        if completed && s.task(task).state == State::Done {
            if let Some(parent) = s.task(task).handoff.take() {
                s.task(parent).state = State::Ready;
                s.runnable.push_back(parent);
            }
            if s.task(task).detached && s.task(task).failure.is_none() {
                let scope = s.task(task).scope.expect("a detached child has a scope");
                s.scope(scope).children.retain(|&child| child != task);
                s.vacate_task(task);
            }
        }
    }
    unsafe { super::readiness::poll_free_with_pool(s, pool_available) };
    let ready = std::mem::take(&mut s.runnable);
    ready
        .into_iter()
        .map(|task| (task, s.task(task).owner))
        .collect()
}

/// CONC-8 requires every logical task to be blocked and every physical image
/// saved. An empty ready-queue search alone proves neither. Host requests and
/// deadlines are external event sources and prevent this runtime-only verdict.
/// # Safety
/// The caller holds IdleGuard; no Scheduler borrow survives this call.
pub(super) unsafe fn check_quiet(run: *mut crate::context::RunShared) {
    if unsafe { (*run).host_waits.load(Ordering::Acquire) } != 0 {
        return;
    }
    unsafe { check_quiet_without_sources(run) };
}

/// The home adapter proved no event source can wake this fully saved run.
/// # Safety
/// As for check_quiet; the caller additionally owns that source verdict.
pub(super) unsafe fn check_quiet_without_sources(run: *mut crate::context::RunShared) {
    let waits = {
        let run = unsafe { &*run };
        let Some(s) = (unsafe { (*run.tasks.get()).as_deref_mut() }) else {
            return;
        };
        let quiet_policy = match &s.logical {
            Logical::Free(_) => true,
            Logical::Ordered(driver) => driver.quiet(),
            Logical::Cooperative => false,
        };
        if !quiet_policy
            || s.tasks.iter().flatten().any(|row| {
                row.state == State::Ready
                    || row.dispatch.as_ref().is_some_and(|frame| !frame.is_saved())
            })
        {
            return;
        }
        s.parked()
            .into_iter()
            .map(|(task, wait)| {
                let row = s.task(task);
                let site = run
                    .failure_table
                    .filter(|_| row.wait_site != 0)
                    .map(|table| table.located(row.wait_site))
                    .unwrap_or_default();
                let spawn_path = row.path.clone();
                let (kind, targets) = match wait {
                    Wait::Join { child, .. } => ("task.await", vec![u64::from(child)]),
                    Wait::Exit { scope } => (
                        "task.scope",
                        s.scope(scope)
                            .children
                            .iter()
                            .map(|&child| u64::from(child))
                            .collect(),
                    ),
                    Wait::Receive { channel, .. } | Wait::ReceiveReady { channel } => {
                        ("channel.receive", vec![u64::from(channel)])
                    }
                    Wait::Any { set, .. } => (
                        "task.await_any",
                        s.set(set)
                            .members
                            .iter()
                            .flatten()
                            .map(|word| u64::from(*word as u32))
                            .collect(),
                    ),
                    Wait::Drain { set } => (
                        "task.cancel_all",
                        s.set(set)
                            .members
                            .iter()
                            .flatten()
                            .map(|word| u64::from(*word as u32))
                            .collect(),
                    ),
                    Wait::Final => ("free.final", vec![]),
                    Wait::Retry { .. } => ("free.retry", vec![]),
                    Wait::Gate { gate } => ("foreign.gate", vec![u64::from(gate)]),
                    Wait::Pool => ("foreign.pool", vec![]),
                    Wait::Host { token } => ("host request", vec![token]),
                };
                hirsel_run_schema::Wait {
                    spawn_path,
                    site,
                    kind: kind.into(),
                    targets,
                }
            })
            .collect::<Vec<_>>()
    };
    let mut waits = waits;
    waits.sort_by(|a, b| a.spawn_path.cmp(&b.spawn_path));
    // The current saved continuation still holds its Lease. Its final drop
    // wakes home after busy is cleared, avoiding an invented pending worker.
    #[cfg(test)]
    let selected = unsafe { (*run).control.request_inside(Stop::Blocked(waits)) };
    #[cfg(not(test))]
    unsafe {
        (*run).control.request_inside(Stop::Blocked(waits))
    };
    #[cfg(test)]
    if matches!(selected, Stop::Blocked(_)) {
        let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
        ordered::events::blocked(s, unsafe { (*run).failure_table });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: [u64; 2] = [0, 0];

    unsafe extern "C" fn result_child(
        ctx: *mut TaskCtx,
        environment: *mut Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let id = unsafe { environment.add(1).cast::<u64>().read() };
        // A started task yields many times while its result and locals remain
        // on the same stack. Logical scheduling must retain one image owner.
        let local = [id; 32];
        for _ in 0..260 {
            unsafe { yield_now(&mut *ctx) };
        }
        assert_eq!(local, [id; 32]);
        let mut slot = [1, id * 100];
        unsafe {
            hirsel_rt_v2_task_finish(
                ctx,
                slot.as_mut_ptr().cast(),
                NONE.as_ptr().cast(),
                16,
                u32::MAX,
            )
        }
    }

    static STARTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn rendezvous() {
        STARTED.fetch_add(1, Ordering::AcqRel);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while STARTED.load(Ordering::Acquire) != 8 {
            assert!(
                std::time::Instant::now() < until,
                "native segments did not overlap"
            );
            std::thread::yield_now();
        }
    }

    unsafe extern "C" fn parallel_child(
        ctx: *mut TaskCtx,
        environment: *mut Header,
        caller: u64,
        site: u64,
    ) -> u8 {
        rendezvous();
        unsafe { result_child(ctx, environment, caller, site) }
    }

    fn children_run(parallel: bool, detach: bool) {
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
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        let metrics = run.worker_metrics();
        // SAFETY: these bodies use the free task policy and no host services.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { enable(ctx) },
            move |ctx| unsafe {
                for _ in 0..if parallel { 1 } else { 4 } {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
                    );
                    let mut handles = Vec::new();
                    for id in 1..=if parallel { 7 } else { 16u64 } {
                        let environment = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        environment.add(1).cast::<u64>().write(id);
                        let mut task = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut task,
                                &scope,
                                if parallel {
                                    parallel_child
                                } else {
                                    result_child
                                },
                                environment,
                                0,
                            ),
                            STATUS_OK
                        );
                        if detach && id % 2 == 0 {
                            assert_eq!(hirsel_rt_v2_task_detach(ctx, &scope, &mut task), STATUS_OK);
                        } else {
                            handles.push((task, id));
                        }
                    }
                    if parallel {
                        rendezvous();
                    }
                    for (mut task, id) in handles {
                        let mut slot = NONE;
                        assert_eq!(
                            hirsel_rt_v2_task_join(
                                ctx,
                                0,
                                0,
                                &scope,
                                &mut task,
                                slot.as_mut_ptr().cast(),
                                16
                            ),
                            STATUS_OK
                        );
                        assert_eq!(slot, [1, id * 100]);
                    }
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        let values = metrics
            .values()
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(values["queue_ops_per_op"] > 0.0);
        assert!(values["steals_per_s"] >= 0.0);
    }

    #[test]
    fn native_children_finish_join_and_reuse_saved_stacks_through_the_free_policy() {
        children_run(false, false);
    }

    #[test]
    fn scope_exit_joins_and_discards_detached_native_results_after_idle_save() {
        children_run(false, true);
    }

    #[test]
    fn eight_native_task_segments_overlap_before_their_spawner_suspends() {
        const CHILD: &str = "HIRSEL_F1_LOGICAL_PARALLEL";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tasks::free::tests::eight_native_task_segments_overlap_before_their_spawner_suspends",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }
        crate::pool::set_compute_grant(8);
        for _ in 0..16 {
            STARTED.store(0, Ordering::Release);
            children_run(true, false);
            assert_eq!(STARTED.load(Ordering::Acquire), 8);
        }
    }

    #[test]
    fn idle_save_repolls_a_wait_deferred_while_its_stack_was_still_running() {
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        // SAFETY: this body uses only explicit dispatch and the Retry wait
        // below, without a home service, stack delivery or logical completion.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { enable(ctx) },
            |ctx| {
                for _ in 0..256 {
                    unsafe {
                        role::enter(ctx);
                        let s = scheduler(ctx);
                        let me = s.current;
                        s.task(me).retried = true;
                        s.park(Wait::Retry { cancelable: false });
                        // A concurrent logical poll cannot use this stack yet.
                        assert!(
                            !super::super::poll(ctx, me, &crate::io::Ready::Tokens(Vec::new()))
                                .unwrap()
                        );
                        crate::pool::free::suspend_current(ctx, false);
                        let row = scheduler(ctx).task(me);
                        assert_eq!(row.state, State::Ready);
                        assert!(row.wait.is_none());
                        assert!(!row.woken);
                        role::release(ctx);
                    }
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn the_last_saved_wait_reports_blocked_without_retaining_a_compute_permit() {
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        // SAFETY: only a runtime wait, with no host or foreign service.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { enable(ctx) },
            |ctx| {
                park_retry(ctx, false);
                crate::STATUS_OK
            },
        );
        let Err(Stop::Blocked(waits)) = outcome.ended else {
            panic!("{outcome:?}");
        };
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].kind, "free.retry");
        assert!(waits[0].spawn_path.is_empty());
        assert_eq!(outcome.pending, 0);
    }
}
