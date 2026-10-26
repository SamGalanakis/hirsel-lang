//! Head-owned host operations. A parked request retains its original ordinal.
use super::super::*;
use super::engine;
use crate::ordered::{Next, Progress};

pub(crate) mod concurrent;
pub(super) mod entered;
pub(crate) mod home;

/// A completed read owns its selected outcome, not a readiness prediction.
/// Its original wait row remains the return-value owner until the issuer takes
/// it. The generation-qualified driver key prevents reuse by another frame.
/// Pure evaluation may follow; canonical return and all later operations wait.
pub(crate) struct CompletedRead {
    pub(super) token: u64,
    pub(super) request: crate::io::Request,
    pub(super) result: Result<crate::io::Completion, crate::io::AdapterError>,
}

impl CompletedRead {
    pub(crate) fn new(
        token: u64,
        request: &crate::io::Request,
        result: &Result<crate::io::Completion, crate::io::AdapterError>,
    ) -> Option<Self> {
        Self::eligible(request, result).then(|| Self {
            token,
            request: request.clone(),
            result: result.clone(),
        })
    }

    fn eligible(
        request: &crate::io::Request,
        result: &Result<crate::io::Completion, crate::io::AdapterError>,
    ) -> bool {
        use crate::io::{Completion, Request};
        let Request::NetRead { max, .. } = request else {
            return false;
        };
        let Ok(Completion::NetBytes(value)) = result else {
            return false;
        };
        !value.as_ref().is_ok_and(|bytes| bytes.len() as u64 > *max)
    }
}

/// Called only after the serial head is ready, before settling the following
/// pure stretch. A parked read has no extra continuation/poll on return.
pub(super) fn retire_return(ctx: *mut TaskCtx) {
    let task = engine::current(ctx);
    let Some(token) = engine::with_driver(ctx, |d| d.host_returns.get(&task).map(|r| r.token))
    else {
        return;
    };
    assert!(engine::with_driver(ctx, |d| d.head_ready(task)));
    // No stop-aware entry or bridge between removing the original row and
    // publishing its canonical return. The certificate stays run-owned until
    // that point; an abandoned Rust stack is never its only owner.
    unsafe { crate::host_waits::retire_completed_read(ctx, token, task) };
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let completed = driver.host_returns.remove(&task).unwrap();
    assert!(driver.host_ops.remove(&task).is_some());
    super::events::complete_host(ctx, false, &completed.result);
}

/// Only the resumed issuer can take a clone before its canonical return.
/// This lookup must not introduce a stop between completed bytes and evidence.
pub(crate) fn return_pending(ctx: *mut TaskCtx, token: u64) -> bool {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let index = unsafe { (*ctx).task_id };
    let task = super::super::handle(index, s.task(index).generation);
    let Logical::Ordered(driver) = &s.logical else {
        return false;
    };
    driver
        .host_returns
        .get(&task)
        .is_some_and(|r| r.token == token)
}

/// The payload certificate selects the return; the validated alternate entry
/// separately certifies physical checkpoint/barrier coverage for its tail.
/// Legacy entries keep the strict gate, even when their next work looks pure.
fn tail_enabled(ctx: *mut TaskCtx) -> bool {
    let certified = super::ahead::host_return::selected(ctx);
    certified
        && engine::with_driver(ctx, |d| d.private_enabled)
        && crate::pool::wide(unsafe { (*ctx).run_ref() })
}

pub(crate) fn begin(ctx: *mut TaskCtx, close: bool) -> Stopped<()> {
    engine::at_head(ctx)?;
    let task = engine::current(ctx);
    if engine::with_driver(ctx, |d| d.host_ops.contains_key(&task)) {
        return Ok(());
    }
    let name = if close {
        "host.handle_drop"
    } else {
        "host.request"
    };
    engine::head_effect(ctx, name, || ((), Next::Keep))?;
    assert!(engine::with_driver(ctx, |d| d
        .host_ops
        .insert(task, name)
        .is_none()));
    Ok(())
}

pub(crate) fn finish(ctx: *mut TaskCtx, next: Next) -> Stopped<()> {
    let (progress, private) = finish_start(ctx, next)?;
    if private {
        engine::continue_after_read(ctx, progress)
    } else {
        engine::continue_at_head(ctx, progress)
    }
}

/// Commit the finish once, leaving its continuation to the owning operation.
/// Retirement polls still run to completion on their normal stackful owner.
pub(super) fn finish_start(ctx: *mut TaskCtx, next: Next) -> Stopped<(Progress, bool)> {
    engine::at_head(ctx)?;
    let task = engine::current(ctx);
    assert!(engine::with_driver(ctx, |d| d
        .host_ops
        .remove(&task)
        .is_some()));
    let (_, progress) = engine::head_continuation(ctx, || ((), next))?;
    let private = engine::with_driver(ctx, |d| d.bounded_reads.remove(&task)) && tail_enabled(ctx);
    Ok((progress, private))
}

/// Record the bounded read's owned outcome before its canonical finish yields.
pub(crate) fn bounded_read(
    ctx: *mut TaskCtx,
    request: &crate::io::Request,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
) {
    if CompletedRead::eligible(request, result) {
        super::ahead::host_return::select(ctx, 0, request, result);
        // The effect has completed: preserve evidence before any stop-aware
        // scheduler entry can abandon its owner.
        let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
        let index = unsafe { (*ctx).task_id };
        let task = super::super::handle(index, s.task(index).generation);
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        driver.bounded_reads.insert(task);
    }
}

pub(crate) fn yield_now(ctx: *mut TaskCtx) -> Stopped<()> {
    begin(ctx, false)?;
    finish(ctx, Next::Yield)
}

/// Argument reads use the immutable launch snapshot, rather than an adapter.
/// Retire their declared arg.read operation with owned request/result bytes;
/// a generic host.request yield has no adapter evidence to complete it.
pub(crate) fn argument(
    ctx: *mut TaskCtx,
    kind: &'static str,
    index: Option<u64>,
    value: impl FnOnce() -> Option<Vec<u8>>,
) -> Stopped<()> {
    let (_, progress) = engine::head_canonical(
        ctx,
        "arg.read",
        || ((), Next::Yield),
        |_| {
            Some(vec![(
                super::events::CanonicalItem::Root,
                super::events::CanonicalOutcome::Argument {
                    kind,
                    index,
                    value: value(),
                },
            )])
        },
    )?;
    engine::continue_at_head(ctx, progress)
}

/// # Safety
/// This task exclusively owns the request and all of its completion buffers.
pub(crate) unsafe fn park(ctx: *mut TaskCtx, token: u64, yielded: bool) -> Stopped<()> {
    super::inop::start(
        ctx,
        super::inop::Operation::Host(super::inop::Host::new(token, yielded)),
    );
    super::inop::run(ctx, || unsafe { super::inop::drive_host(ctx) })
}

/// Inspect the original completed row only after the promised physical save.
/// This does not retry or take the row; canonical retirement retains custody.
pub(super) fn select_completed_return(ctx: *mut TaskCtx, token: u64) -> bool {
    let task = engine::current(ctx);
    if super::ahead::host_return::armed(ctx)
        && engine::with_driver(ctx, |d| d.private_enabled)
        && crate::pool::wide(unsafe { (*ctx).run_ref() })
    {
        if let Some(completed) = unsafe { crate::host_waits::completed_read(ctx, token) } {
            assert_eq!(completed.token, token);
            super::ahead::host_return::select(ctx, token, &completed.request, &completed.result);
            if tail_enabled(ctx) {
                let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
                let Logical::Ordered(driver) = &mut s.logical else {
                    unreachable!()
                };
                assert!(driver.host_returns.insert(task, completed).is_none());
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod home_graph_tests;
#[cfg(test)]
mod home_prefix_tests;
#[cfg(test)]
mod home_terminal_tests;
#[cfg(test)]
mod output;

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::io::{Adapter, AdapterError, Completion, Ready, Request, UnitResult};
    use crate::ordered::Policy;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };

    #[derive(Default)]
    struct Probe {
        now: AtomicU64,
        attempted: AtomicBool,
        caller: AtomicU64,
        older: AtomicBool,
        output: Mutex<Vec<u8>>,
        completed: AtomicU64,
        parked: AtomicU64,
    }
    struct Host {
        home: std::thread::ThreadId,
        probe: Arc<Probe>,
        cancellation: bool,
    }
    impl Adapter for Host {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            match request {
                Request::SleepUntil { deadline, .. }
                    if deadline <= self.probe.now.load(Ordering::Acquire) =>
                {
                    Ok(Completion::Wait(Ok(())))
                }
                Request::SleepUntil { .. } => Err(AdapterError::Blocked),
                request => panic!("unexpected {request:?}"),
            }
        }
        fn print(&mut self, _: u64, _: bool, bytes: &[u8]) -> Result<Completion, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            assert!(
                self.probe.older.load(Ordering::Acquire),
                "output passed the older turn"
            );
            self.probe.output.lock().unwrap().extend_from_slice(bytes);
            Ok(Completion::Unit(Ok(())))
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            Ok(if self.probe.now.load(Ordering::Acquire) == 0 {
                Ready::Tokens(Vec::new())
            } else {
                Ready::Every
            })
        }
        fn wait(&mut self) -> Result<bool, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            self.probe
                .now
                .store(if self.cancellation { 50 } else { 100 }, Ordering::Release);
            Ok(true)
        }
        fn finish(&mut self, _: u64) {
            self.probe.completed.fetch_add(1, Ordering::Relaxed);
        }
        fn parked(&mut self, _: u64) {
            self.probe.parked.fetch_add(1, Ordering::Release);
        }
    }
    pub(super) fn layout() -> crate::Layout {
        crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 24,
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
        }
    }

    unsafe extern "C" fn sleeping_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        let deadline = unsafe { crate::alloc::payload(env).cast::<u64>().read() };
        let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
        assert_eq!(
            unsafe {
                crate::io::hirsel_rt_v2_clock_sleep_until(
                    ctx,
                    std::ptr::null_mut(),
                    result.as_mut_ptr(),
                    &11,
                    &deadline,
                )
            },
            STATUS_OK
        );
        assert_eq!(unsafe { result.assume_init() }.tag, 0);
        STATUS_OK
    }

    #[test]
    fn a_runnable_worker_records_host_completion_before_the_retirement_poll_wakes_its_owner() {
        let environment = layout();
        let layouts = [&environment as *const crate::Layout];
        for width in [2, 4, 8] {
            let probe = Arc::new(Probe::default());
            let seen = probe.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            run.adapter = Some(Box::new(Host {
                home: std::thread::current().id(),
                probe: probe.clone(),
                cancellation: false,
            }));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    engine::enable(ctx, Policy::admit(&Default::default(), None));
                    super::super::events::enable(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
                    );
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        24,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env)
                        .cast::<[u64; 3]>()
                        .write([10, 0, 0]);
                    let mut child = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut child,
                            &scope,
                            sleeping_child,
                            env,
                            0
                        ),
                        STATUS_OK
                    );
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    loop {
                        let waiting = {
                            let _guard = role::Guard::new(ctx);
                            let s = scheduler(ctx);
                            matches!(s.task(child as u32).wait, Some(Wait::Host { .. }))
                                && readiness::saved(s, child as u32)
                        };
                        if seen.parked.load(Ordering::Acquire) == 1 && waiting {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < until,
                            "child never saved its host wait"
                        );
                        let mut cause = 99;
                        assert_eq!(
                            hirsel_rt_v2_task_canceled(ctx, &mut cause, &scope),
                            STATUS_OK
                        );
                        assert_eq!(cause, 0);
                    }
                    // A runnable root publishes readiness, then retires a
                    // scheduling point. There is no idle host wait in between.
                    seen.now.store(100, Ordering::Release);
                    let mut cause = 99;
                    assert_eq!(
                        hirsel_rt_v2_task_canceled(ctx, &mut cause, &scope),
                        STATUS_OK
                    );
                    assert_eq!(cause, 0);
                    let log = engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(log.host_polls.len(), 1);
                    let poll = &log.host_polls[0];
                    assert!(
                        !poll.idle,
                        "completion must be captured by a retirement poll"
                    );
                    assert_eq!(poll.completion.task, vec![0]);
                    assert_eq!(poll.completion.ordinal, 0);
                    assert_eq!(poll.completion.result, Ok(Completion::Wait(Ok(()))));
                    let observed = poll.completion.observation.as_ref().unwrap();
                    assert!(observed.handles.is_empty());
                    assert_eq!(
                        crate::tape_wire::decode::<Result<Completion, AdapterError>>(
                            &observed.payload
                        )
                        .unwrap(),
                        poll.completion.result
                    );
                    assert_eq!(poll.sequence, 0);
                    assert_eq!(log.host_attempts.len(), 1);
                    assert_eq!(log.host_attempts[0], *poll);
                    assert_eq!(log.host_completions.len(), 1);
                    assert_eq!(poll.completion, log.host_completions[0]);
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(probe.completed.load(Ordering::Acquire), 1);
        }
    }

    #[test]
    fn a_batch_of_host_completions_is_recorded_in_deadline_then_park_order_before_any_wake() {
        let environment = layout();
        let layouts = [&environment as *const crate::Layout];
        let mut reference = None;
        for width in [2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let probe = Arc::new(Probe::default());
                let output = Arc::new(Mutex::new(Vec::new()));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                run.adapter = Some(Box::new(Host {
                    home: std::thread::current().id(),
                    probe: probe.clone(),
                    cancellation: false,
                }));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        super::super::events::enable(ctx);
                    },
                    move |ctx| unsafe {
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
                        for deadline in [20u64, 10, 10] {
                            let env = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                24,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(env)
                                .cast::<[u64; 3]>()
                                .write([deadline, 0, 0]);
                            let mut child = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_spawn(
                                    ctx,
                                    0,
                                    0,
                                    std::ptr::null_mut(),
                                    &mut child,
                                    &scope,
                                    sleeping_child,
                                    env,
                                    0
                                ),
                                STATUS_OK
                            );
                        }
                        let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
                        assert_eq!(
                            crate::io::hirsel_rt_v2_clock_sleep_until(
                                ctx,
                                std::ptr::null_mut(),
                                result.as_mut_ptr(),
                                &11,
                                &30
                            ),
                            STATUS_OK
                        );
                        assert_eq!(result.assume_init().tag, 0);
                        let log = engine::driver(ctx).events.as_ref().unwrap();
                        assert_eq!(log.host_polls.len(), 4);
                        assert_eq!(
                            log.host_polls
                                .iter()
                                .map(|row| row.completion.task.clone())
                                .collect::<Vec<_>>(),
                            [vec![1], vec![2], vec![0], vec![]]
                        );
                        assert_eq!(
                            log.host_polls
                                .iter()
                                .map(|row| row.sequence)
                                .collect::<Vec<_>>(),
                            [0, 1, 2, 3]
                        );
                        assert!(
                            log.host_polls
                                .iter()
                                .all(|row| row.idle
                                    && row.retirement == log.host_polls[0].retirement)
                        );
                        assert_eq!(log.host_attempts, log.host_polls);
                        assert_eq!(log.host_completions.len(), 4);
                        for (polled, returned) in log.host_polls.iter().zip(&log.host_completions) {
                            assert_eq!(&polled.completion, returned);
                        }
                        *seen.lock().unwrap() = log.host_polls.clone();
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                            STATUS_OK
                        );
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert_eq!(probe.completed.load(Ordering::Acquire), 4);
                let polls = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&polls, reference);
                } else {
                    reference = Some(polls);
                }
            }
        }
    }

    struct GrantClock {
        base: u64,
        next: u64,
        now: u64,
    }
    impl Adapter for GrantClock {
        fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
            if let Request::SleepUntil { clock, .. } = request
                && (*clock < self.base || *clock >= self.base + self.next)
            {
                return Err(AdapterError::InvalidHandle(*clock));
            }
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            match request {
                Request::MonotonicClock { io: 1 } => {
                    let handle = self.base + self.next;
                    self.next += 1;
                    Ok(Completion::ClockGrant(Ok(handle)))
                }
                Request::SleepUntil { deadline, .. } if deadline > self.now => {
                    Err(AdapterError::Blocked)
                }
                Request::SleepUntil { .. } => Ok(Completion::Wait(Ok(()))),
                Request::Close { .. } => Ok(Completion::Unit(Ok(()))),
                request => panic!("unexpected {request:?}"),
            }
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            Ok(if self.now == 0 {
                Ready::Tokens(vec![])
            } else {
                Ready::Every
            })
        }
        fn wait(&mut self) -> Result<bool, AdapterError> {
            self.now = 100;
            Ok(true)
        }
    }

    #[test]
    fn normal_home_clock_grants_supply_creator_field_provenance_for_saved_waits() {
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let mut run = crate::host::BodyRun::new(width, true);
                run.adapter = Some(Box::new(GrantClock {
                    base: 1000 + 10 * u64::from(width),
                    next: 0,
                    now: 0,
                }));
                unsafe { run.use_physical_dispatch() };
                let (outcome, trace) = run.run_traced(
                    move |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        super::super::events::enable(ctx);
                        super::super::events::initial_host(ctx, Some(1), None);
                    },
                    |ctx| unsafe {
                        let mut clocks = [crate::io::ClockResult {
                            tag: 99,
                            padding: 99,
                            value: 0,
                        }; 2];
                        for clock in &mut clocks {
                            assert_eq!(
                                crate::io::hirsel_rt_v2_io_monotonic_clock(
                                    ctx,
                                    std::ptr::null_mut(),
                                    clock,
                                    &1
                                ),
                                STATUS_OK
                            );
                            assert_eq!((clock.tag, clock.padding), (0, 0));
                        }
                        assert_ne!(clocks[0].value, clocks[1].value);
                        let mut slept = std::mem::MaybeUninit::<UnitResult>::uninit();
                        assert_eq!(
                            crate::io::hirsel_rt_v2_clock_sleep_until(
                                ctx,
                                std::ptr::null_mut(),
                                slept.as_mut_ptr(),
                                &clocks[0].value,
                                &100
                            ),
                            STATUS_OK
                        );
                        assert_eq!(slept.assume_init().tag, 0);
                        assert_eq!(
                            crate::host_waits::issue(
                                ctx,
                                Request::Close {
                                    handle: clocks[0].value
                                }
                            )
                            .unwrap(),
                            (Ok(Completion::Unit(Ok(()))), false)
                        );
                        let log = engine::driver(ctx).events.as_ref().unwrap();
                        let a = &log.host_handles[&clocks[0].value];
                        let b = &log.host_handles[&clocks[1].value];
                        assert_eq!(a.operation.ordinal, 0);
                        assert_eq!(b.operation.ordinal, 1);
                        assert_eq!((a.field, b.field), (0, 0));
                        assert_ne!(a, b);
                        assert_eq!(log.host_bindings[a].resource, log.host_bindings[b].resource);
                        assert_eq!(
                            log.host_requests[&(vec![], 2)]
                                .as_ref()
                                .unwrap()
                                .authorities,
                            [super::super::events::HostAuthority::Created(a.clone())]
                        );
                        assert!(log.canonical().is_ok());
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let trace = trace.expect("normal grants and waits have complete provenance");
                let rows: Vec<serde_json::Value> = std::str::from_utf8(trace.bytes())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row["outcome"]["kind"] == "host_created")
                        .count(),
                    2
                );
                assert_eq!(rows.last().unwrap()["outcome"]["kind"], "root_end");
                if let Some(reference) = &reference {
                    assert_eq!(trace.bytes(), reference);
                } else {
                    reference = Some(trace.bytes().to_vec());
                }
            }
        }
    }

    struct NeverHost;
    impl Adapter for NeverHost {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            Err(AdapterError::Blocked)
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            Ok(Ready::Tokens(vec![]))
        }
        fn wait(&mut self) -> Result<bool, AdapterError> {
            Ok(false)
        }
    }

    #[test]
    fn canonical_host_blocked_cut_uses_logical_request_identity_and_rejects_unknown_authority() {
        let mut reference = None;
        for known in [true, false] {
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.adapter = Some(Box::new(NeverHost));
                    unsafe { run.use_physical_dispatch() };
                    let (outcome, trace) = run.run_traced(
                        move |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            engine::enable(ctx, policy);
                            super::super::events::enable(ctx);
                            if known {
                                engine::driver(ctx).objects.bind_erased(
                                    crate::ordered::objects::Kind::Capability,
                                    u64::from(width) + 100,
                                    crate::ordered::footprint::ObjectId {
                                        creator: vec![],
                                        ordinal: 77,
                                    },
                                    Default::default(),
                                );
                            }
                        },
                        move |ctx| unsafe {
                            let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
                            crate::io::hirsel_rt_v2_clock_sleep_until(
                                ctx,
                                std::ptr::null_mut(),
                                result.as_mut_ptr(),
                                &(u64::from(width) + 100),
                                &100,
                            );
                            unreachable!("source-free wait must end Blocked");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(outcome.pending, 0);
                    if known {
                        let trace = trace.expect("known saved host wait certifies terminal cut");
                        let rows: Vec<serde_json::Value> = std::str::from_utf8(trace.bytes())
                            .unwrap()
                            .lines()
                            .map(|line| serde_json::from_str(line).unwrap())
                            .collect();
                        let terminal = rows.last().unwrap();
                        assert_eq!(terminal["outcome"]["kind"], "blocked");
                        assert_eq!(terminal["outcome"]["waits"][0]["kind"], "clock.sleep_until");
                        assert_eq!(
                            terminal["outcome"]["waits"][0]["targets"],
                            serde_json::json!([{"creator":[], "ordinal":0}])
                        );
                        assert!(
                            !rows
                                .iter()
                                .any(|row| row["outcome"]["kind"] == "host_finished")
                        );
                        if let Some(reference) = &reference {
                            assert_eq!(trace.bytes(), reference);
                        } else {
                            reference = Some(trace.bytes().to_vec());
                        }
                    } else {
                        assert!(trace.is_err());
                    }
                }
            }
        }
    }

    struct RetryHost {
        host: Host,
        waits: u32,
    }
    impl Adapter for RetryHost {
        fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
            self.host.preflight(request)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            self.host.call(request)
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            self.host.ready()
        }
        fn wait(&mut self) -> Result<bool, AdapterError> {
            assert_eq!(self.host.home, std::thread::current().id());
            self.waits += 1;
            self.host
                .probe
                .now
                .store(u64::from(self.waits) * 50, Ordering::Release);
            assert!(self.waits <= 2, "retry never completed");
            Ok(true)
        }
        fn finish(&mut self, token: u64) {
            self.host.finish(token);
        }
        fn parked(&mut self, token: u64) {
            self.host.parked(token);
        }
    }

    #[test]
    fn an_idle_blocked_host_retry_is_owned_before_the_completing_poll_wakes_its_task() {
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let probe = Arc::new(Probe::default());
                let output = Arc::new(Mutex::new(Vec::new()));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.adapter = Some(Box::new(RetryHost {
                    host: Host {
                        home: std::thread::current().id(),
                        probe: probe.clone(),
                        cancellation: false,
                    },
                    waits: 0,
                }));
                unsafe { run.use_physical_dispatch() };
                let (outcome, trace) = run.run_traced(
                    move |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        super::super::events::enable(ctx);
                        engine::driver(ctx).objects.bind_erased(
                            crate::ordered::objects::Kind::Capability,
                            11,
                            crate::ordered::footprint::ObjectId {
                                creator: vec![],
                                ordinal: 77,
                            },
                            Default::default(),
                        );
                    },
                    move |ctx| unsafe {
                        let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
                        assert_eq!(
                            crate::io::hirsel_rt_v2_clock_sleep_until(
                                ctx,
                                std::ptr::null_mut(),
                                result.as_mut_ptr(),
                                &11,
                                &100
                            ),
                            STATUS_OK
                        );
                        assert_eq!(result.assume_init().tag, 0);
                        let log = engine::driver(ctx).events.as_ref().unwrap();
                        assert_eq!(log.host_polls.len(), 1);
                        assert_eq!(log.host_attempts.len(), 2);
                        assert_eq!(log.host_initials.len(), 1);
                        let initial = &log.host_initials[&(vec![], 0)];
                        assert_eq!(initial.result, Err(AdapterError::Blocked));
                        assert!(initial.observation.is_some());
                        let attempts = &log.host_attempts;
                        assert_eq!(attempts[0].completion.result, Err(AdapterError::Blocked));
                        assert_eq!(attempts[1].completion.result, Ok(Completion::Wait(Ok(()))));
                        assert_eq!(attempts[0].sequence, 0);
                        assert_eq!(attempts[1].sequence, 1);
                        assert!(attempts.iter().all(|row| row.idle
                            && row.completion.task.is_empty()
                            && row.completion.ordinal == 0
                            && row.retirement == attempts[0].retirement));
                        assert_eq!(attempts[1].completion, log.host_polls[0].completion);
                        assert_eq!(attempts[1].completion, log.host_completions[0]);
                        assert!(
                            attempts
                                .iter()
                                .all(|row| row.completion.observation.is_some())
                        );
                        let canonical = log.canonical().expect("complete coupled scalar phases");
                        assert_eq!(
                            canonical
                                .iter()
                                .filter(|row| matches!(
                                    row.outcome,
                                    super::super::events::CanonicalOutcome::HostAccess { .. }
                                ))
                                .count(),
                            3
                        );
                        assert_eq!(
                            canonical
                                .iter()
                                .filter(|row| matches!(
                                    row.outcome,
                                    super::super::events::CanonicalOutcome::HostFinished
                                ))
                                .count(),
                            1
                        );
                        *seen.lock().unwrap() = attempts.clone();
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert_eq!(probe.completed.load(Ordering::Acquire), 1);
                assert_eq!(probe.parked.load(Ordering::Acquire), 1);
                let trace = trace.expect("complete root-ended host trace");
                let attempts = (output.lock().unwrap().clone(), trace.bytes().to_vec());
                if let Some(reference) = &reference {
                    assert_eq!(&attempts, reference);
                } else {
                    reference = Some(attempts);
                }
            }
        }
    }

    #[test]
    fn owned_host_sleep_completes_from_idle_and_deadline_cancels_a_saved_request() {
        owned_host_sleep_completes_from_idle_and_deadline_cancels_a_saved_request_commit(
            hirsel_run_config::Commit::Shared,
        );
    }

    #[test]
    fn dedicated_commit_preserves_home_saved_wait_completion_and_deadline_cancellation() {
        owned_host_sleep_completes_from_idle_and_deadline_cancels_a_saved_request_commit(
            hirsel_run_config::Commit::Dedicated,
        );
    }

    pub(crate) fn owned_host_sleep_completes_from_idle_and_deadline_cancels_a_saved_request_commit(
        commit: hirsel_run_config::Commit,
    ) {
        for width in [2, 4, 8] {
            for cancellation in [false, true] {
                let probe = Arc::new(Probe::default());
                let mut run = crate::host::BodyRun::new(width, true);
                run.adapter = Some(Box::new(Host {
                    home: std::thread::current().id(),
                    probe: probe.clone(),
                    cancellation,
                }));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.commit = commit;
                        engine::enable(ctx, policy);
                        super::super::events::enable(ctx);
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        if cancellation {
                            assert_eq!(
                                hirsel_rt_v2_task_scope_enter_until(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut scope,
                                    &0,
                                    &[11, 50]
                                ),
                                STATUS_OK
                            );
                        }
                        let before = {
                            let _role = role::Guard::new(ctx);
                            let task = engine::current(ctx);
                            crate::tasks::ordered::engine::with_driver(ctx, |d| {
                                *d.recorded.get(&task).unwrap_or(&0)
                            })
                        };
                        let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
                        assert_eq!(
                            crate::io::hirsel_rt_v2_clock_sleep_until(
                                ctx,
                                std::ptr::null_mut(),
                                result.as_mut_ptr(),
                                &11,
                                &100
                            ),
                            STATUS_OK
                        );
                        let result = result.assume_init();
                        assert_eq!(result.tag, u32::from(cancellation));
                        if cancellation {
                            assert_eq!(result.code, CANCEL_DEADLINE);
                        }
                        {
                            let _role = role::Guard::new(ctx);
                            let task = engine::current(ctx);
                            assert_eq!(
                                engine::driver(ctx).recorded[&task] - before,
                                1,
                                "waiting preserves one host ordinal"
                            );
                            assert!(crate::tasks::ordered::engine::with_driver(ctx, |d| d
                                .host_ops
                                .is_empty()));
                            let log = engine::driver(ctx).events.as_ref().unwrap();
                            assert_eq!(log.host_completions.len(), 1);
                            assert_eq!(log.host_polls.len(), 1);
                            let done = &log.host_completions[0];
                            let poll = &log.host_polls[0];
                            assert_eq!(
                                &poll.completion, done,
                                "the owner returns the poll's actual result"
                            );
                            assert_eq!(poll.sequence, 0);
                            assert!(log.boundaries.iter().any(|row| row.task == done.task
                                && row.ordinal == done.ordinal
                                && row.transition
                                    == crate::tasks::ordered::events::Transition::Park
                                && row.retirement <= poll.retirement));
                            assert_eq!(done.ordinal, before);
                            assert!(done.task.is_empty());
                            assert_eq!(done.op, "host.request");
                            assert_eq!(
                                done.result,
                                Ok(Completion::Wait(if cancellation {
                                    Err(crate::io::WaitError::Canceled(CANCEL_DEADLINE))
                                } else {
                                    Ok(())
                                }))
                            );
                        }
                        if cancellation {
                            let mut expired = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_exit_until(
                                    ctx,
                                    0,
                                    0,
                                    &mut expired,
                                    &scope,
                                    0
                                ),
                                STATUS_OK
                            );
                            assert_eq!(expired, 1);
                            let log = engine::driver(ctx).events.as_ref().unwrap();
                            assert_eq!(log.owner_completions.len(), 1);
                            assert_eq!(log.owner_completions[0].ordinal, before + 1);
                            assert_eq!(
                                log.owner_completions[0].outcome,
                                super::super::events::OwnerOutcome::ScopeExit {
                                    status: STATUS_OK,
                                    expired: true
                                }
                            );
                        }
                        assert_eq!((*ctx).run_ref().host_waits.load(Ordering::Acquire), 0);
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert_eq!(
                    probe.completed.load(Ordering::Relaxed),
                    if cancellation { 2 } else { 1 }
                );
            }
        }
    }

    unsafe extern "C" fn older(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        let [probe, atomic] = unsafe { crate::alloc::payload(env).cast::<[u64; 2]>().read() };
        let probe = unsafe { &*(probe as *const Probe) };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !probe.attempted.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        loop {
            let _role = unsafe { role::Guard::new(ctx) };
            if crate::tasks::ordered::engine::with_driver(ctx, |d| {
                d.head_paused(probe.caller.load(Ordering::Acquire))
            }) {
                break;
            }
            assert!(std::time::Instant::now() < until);
            drop(_role);
            std::thread::yield_now();
        }
        assert!(probe.output.lock().unwrap().is_empty());
        probe.older.store(true, Ordering::Release);
        assert_eq!(
            unsafe { hirsel_rt_v2_atomic_store(ctx, &atomic, (&23u64 as *const u64).cast(), 8) },
            STATUS_OK
        );
        STATUS_OK
    }

    #[test]
    fn borrowed_output_waits_for_the_older_turn_before_its_home_side_effect() {
        let layout = layout();
        let layouts = [&layout as *const crate::Layout];
        for width in [2, 4, 8] {
            let probe = Arc::new(Probe::default());
            let seen = probe.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            run.adapter = Some(Box::new(Host {
                home: std::thread::current().id(),
                probe: probe.clone(),
                cancellation: false,
            }));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
                    );
                    let mut atomic = 0;
                    assert_eq!(
                        hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut atomic,
                            (&0u64 as *const u64).cast(),
                            8,
                            0,
                            8,
                        ),
                        STATUS_OK
                    );
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        24,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env)
                        .cast::<[u64; 2]>()
                        .write([Arc::as_ptr(&seen) as u64, atomic]);
                    let mut child = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut child,
                            &scope,
                            older,
                            env,
                            0
                        ),
                        STATUS_OK
                    );
                    let bytes = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        24,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(bytes).cast::<u64>().write(3);
                    std::ptr::copy_nonoverlapping(
                        b"one".as_ptr(),
                        crate::alloc::payload(bytes).add(8),
                        3,
                    );
                    {
                        let _role = role::Guard::new(ctx);
                        seen.caller.store(engine::current(ctx), Ordering::Release);
                    }
                    seen.attempted.store(true, Ordering::Release);
                    let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
                    assert_eq!(
                        crate::io::hirsel_rt_v2_out_write(
                            ctx,
                            std::ptr::null_mut(),
                            result.as_mut_ptr(),
                            &9,
                            bytes,
                            0,
                            3
                        ),
                        STATUS_OK
                    );
                    assert_eq!(result.assume_init().tag, 0);
                    crate::release::hirsel_rt_v2_release(ctx, bytes, 2, std::ptr::null(), 0, 0);
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(*probe.output.lock().unwrap(), b"one");
        }
    }
    struct CompletionHost {
        home: std::thread::ThreadId,
        mapped: Vec<u8>,
    }
    impl Adapter for CompletionHost {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            match request {
                Request::MapView { .. } => Ok(Completion::View(crate::io::MapView {
                    length: 3,
                    bytes: None,
                    block: self.mapped.as_ptr() as usize,
                })),
                Request::Close { .. } => {
                    self.mapped.fill(0);
                    Ok(Completion::Unit(Err(crate::io::IoError::Fault(5))))
                }
                Request::TestCheck { .. } => Err(AdapterError::InvalidHandle(77)),
                request => panic!("unexpected {request:?}"),
            }
        }
    }

    #[test]
    fn immediate_host_evidence_owns_mapping_bytes_and_keeps_close_and_error_ordinals() {
        for width in [2, 4, 8] {
            let mut mapped = vec![0; crate::alloc::HEADER_SIZE + 16];
            mapped.extend([1, 2, 3]);
            let mut run = crate::host::BodyRun::new(width, true);
            run.adapter = Some(Box::new(CompletionHost {
                home: std::thread::current().id(),
                mapped,
            }));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    engine::enable(ctx, Policy::admit(&Default::default(), None));
                    super::super::events::enable(ctx);
                },
                |ctx| unsafe {
                    let (result, parked) = crate::host_waits::issue(
                        ctx,
                        Request::MapView {
                            directory: 0,
                            mapped: 99,
                        },
                    )
                    .unwrap();
                    assert!(!parked);
                    assert!(matches!(result, Ok(Completion::View(_))));
                    crate::tasks::try_yield(&mut *ctx).unwrap();
                    let (result, parked) =
                        crate::host_waits::issue(ctx, Request::Close { handle: 99 }).unwrap();
                    assert!(!parked);
                    assert_eq!(
                        result,
                        Ok(Completion::Unit(Err(crate::io::IoError::Fault(5))))
                    );
                    let (result, parked) = crate::host_waits::issue(
                        ctx,
                        Request::TestCheck {
                            test: 77,
                            condition: true,
                        },
                    )
                    .unwrap();
                    assert!(!parked);
                    assert_eq!(result, Err(AdapterError::InvalidHandle(77)));
                    crate::tasks::try_yield(&mut *ctx).unwrap();
                    let driver = engine::driver(ctx);
                    let log = driver.events.as_ref().unwrap();
                    assert_eq!(log.host_completions.len(), 3);
                    assert_eq!(log.host_initials.len(), 3);
                    for completion in &log.host_completions {
                        assert_eq!(
                            log.host_initials[&(completion.task.clone(), completion.ordinal)],
                            *completion
                        );
                    }
                    assert_eq!(
                        log.host_completions
                            .iter()
                            .map(|done| done.ordinal)
                            .collect::<Vec<_>>(),
                        [0, 1, 2]
                    );
                    let Ok(Completion::View(view)) = &log.host_completions[0].result else {
                        panic!("captured view")
                    };
                    assert_eq!(view.bytes, Some(vec![1, 2, 3]));
                    assert_eq!(
                        view.block, 0,
                        "no borrowed mapping address survives capture"
                    );
                    assert_eq!(view.contents(), [1, 2, 3]);
                    let observed = log.host_completions[0].observation.as_ref().unwrap();
                    assert!(observed.handles.is_empty());
                    assert_eq!(
                        crate::tape_wire::decode::<Result<Completion, AdapterError>>(
                            &observed.payload
                        )
                        .unwrap(),
                        Ok(Completion::View(crate::io::MapView::of(vec![1, 2, 3])))
                    );
                    assert!(log.host_completions[1].observation.is_some());
                    assert!(
                        log.host_completions[2].observation.is_none(),
                        "unmapped refusal cannot export a native word"
                    );
                    assert_eq!(log.host_completions[1].op, "host.handle_drop");
                    assert_eq!(
                        log.host_completions[2].result,
                        Err(AdapterError::InvalidHandle(77))
                    );
                    assert_eq!(driver.recorded.values().copied().sum::<u64>(), 3);
                    assert!(driver.host_ops.is_empty());
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }
}

#[cfg(test)]
mod completed_read_tests;

#[cfg(test)]
#[path = "host/modules.rs"]
mod host_modules;
