//! Scripted Pending plus a separate joining child, with no clock sleeps.
use super::super::*;
use crate::STATUS_SUSPENDED;
use crate::io::{AdapterError, Completion, Ready, Request};
use crate::io_concurrent::ConcurrentAdapter;
use crate::tasks::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Host {
    run: AtomicPtr<crate::context::RunShared>,
    home_joined: AtomicBool,
    calls: Mutex<Vec<u64>>,
    finishes: AtomicUsize,
    saves: Mutex<Vec<(u64, bool)>>,
}

impl Host {
    fn joined(&self) -> bool {
        let run = self.run.load(Ordering::Acquire);
        let _role = unsafe { role::IdleGuard::new(run) };
        let s = unsafe { (*(*run).tasks.get()).as_deref().unwrap() };
        s.tasks
            .iter()
            .flatten()
            .any(|row| matches!(row.wait, Some(Wait::Join { child: 1, .. })))
    }
}

impl ConcurrentAdapter for Host {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        assert!(matches!(
            request,
            Request::SleepUntil {
                clock: 11,
                deadline: 10
            }
        ));
        self.calls.lock().unwrap().push(token);
        if self.joined() {
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        }
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        Ok(if self.joined() {
            Ready::Every
        } else {
            Ready::Tokens(vec![])
        })
    }
    fn finish(&self, _: u64) {
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) unsafe fn finish_value(ctx: *mut TaskCtx, value: u64) -> u8 {
    let mut result = [1u64, value];
    unsafe {
        hirsel_rt_v2_task_finish(
            ctx,
            result.as_mut_ptr().cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            3,
        )
    }
}

unsafe extern "C" fn sleeper(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    let mut result = crate::io::UnitResult {
        tag: 99,
        error: 99,
        code: 99,
    };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_clock_sleep_until(
                ctx,
                std::ptr::null_mut(),
                &mut result,
                &11,
                &10,
            )
        },
        STATUS_OK
    );
    assert_eq!(result.tag, 0);
    unsafe { finish_value(ctx, 7) }
}

unsafe extern "C" fn joiner(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let [scope, mut child] = unsafe { crate::alloc::payload(env).cast::<[u64; 2]>().read() };
    let mut result = [0u64; 2];
    assert_eq!(
        unsafe {
            hirsel_rt_v2_task_join(
                ctx,
                0,
                0,
                &scope,
                &mut child,
                result.as_mut_ptr().cast(),
                16,
            )
        },
        STATUS_OK
    );
    assert_eq!(result, [1, 7]);
    unsafe { finish_value(ctx, result[1]) }
}

pub(super) fn layout() -> crate::Layout {
    crate::Layout {
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
        scan: std::ptr::null(),
        scan_words: 0,
    }
}

#[test]
fn entered_host_and_join_keep_the_original_wait_token_and_deliver_once() {
    for width in [1, 4] {
        let environment = layout();
        let result = layout();
        let layouts = [&environment as *const crate::Layout, &result];
        let host = Arc::new(Host::default());
        let setup = host.clone();
        let observe = host.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        run.concurrent_adapter = Some(host.clone());
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
                setup.run.store((*ctx).run_ptr(), Ordering::Release);
                engine::driver(ctx).after_save = Some(Box::new(move |s, task, completed| {
                    if completed {
                        return;
                    }
                    let Logical::Ordered(d) = &s.logical else {
                        unreachable!()
                    };
                    let id = handle(task, s.tasks[task as usize].as_ref().unwrap().generation);
                    if let Some(op) = d.in_operations.get(&id) {
                        observe
                            .saves
                            .lock()
                            .unwrap()
                            .push((id, matches!(op, Operation::Host(_))));
                    }
                }));
            },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let mut child = 0;
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env).cast::<[u64; 2]>().write([0, 0]);
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        sleeper,
                        env,
                        0
                    ),
                    STATUS_OK
                );
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<[u64; 2]>()
                    .write([scope, child]);
                let mut joining = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut joining,
                        &scope,
                        joiner,
                        env,
                        0
                    ),
                    STATUS_OK
                );
                let mut result = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut joining,
                        result.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(result, [1, 7]);
                assert!(engine::driver(ctx).in_operations.is_empty());
                assert!(engine::driver(ctx).host_ops.is_empty());
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(*host.calls.lock().unwrap(), [1, 1]);
        assert_eq!(host.finishes.load(Ordering::Acquire), 1);
        let saves = host.saves.lock().unwrap();
        assert!(
            saves.iter().any(|(_, host)| *host),
            "host continuation was never saved"
        );
        assert!(
            saves.iter().any(|(_, host)| !*host),
            "join continuation was never saved"
        );
    }
}

#[derive(Default)]
struct Inputs(Mutex<Vec<u64>>);

impl ConcurrentAdapter for Inputs {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        let mut calls = self.0.lock().unwrap();
        let retry = calls.contains(&token);
        calls.push(token);
        if !retry {
            return Err(AdapterError::Blocked);
        }
        Ok(match request {
            Request::ReadStdin { .. } => Completion::Input(Ok(vec![b'x'])),
            Request::WatchNext { .. } => {
                Completion::Watch(Err(crate::io::IoError::PermissionDenied))
            }
            Request::SleepUntil { .. } => Completion::Wait(Ok(())),
            _ => unreachable!(),
        })
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        Ok(Ready::Every)
    }
}

#[test]
fn stdin_and_watch_skip_the_initial_yield_and_errors_do_not_poison_the_next_wait() {
    for width in [1, 4] {
        let host = Arc::new(Inputs::default());
        let adapter = host.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.concurrent_adapter = Some(host.clone());
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                )
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                for (request, expected, retirements) in [
                    (
                        Request::ReadStdin { stdin: 12, max: 1 },
                        Completion::Input(Ok(vec![b'x'])),
                        1,
                    ),
                    (
                        Request::WatchNext {
                            directory: 13,
                            watcher: 14,
                        },
                        Completion::Watch(Err(crate::io::IoError::PermissionDenied)),
                        1,
                    ),
                    (
                        Request::SleepUntil {
                            clock: 11,
                            deadline: 10,
                        },
                        Completion::Wait(Ok(())),
                        2,
                    ),
                ] {
                    let before = engine::driver(ctx).frontier.as_ref().unwrap().retired();
                    assert_eq!(
                        host::concurrent::request(ctx, &*adapter, request).unwrap(),
                        Ok(expected)
                    );
                    let driver = engine::driver(ctx);
                    assert_eq!(
                        driver.frontier.as_ref().unwrap().retired() - before,
                        retirements
                    );
                    assert!(driver.in_operations.is_empty());
                    assert!(driver.host_ops.is_empty());
                    assert!((*ctx).pending_ordered_host.is_none());
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(*host.0.lock().unwrap(), [1, 1, 2, 2, 3, 3]);
    }
}

unsafe extern "C" fn compact_joiner(
    ctx: *mut TaskCtx,
    env: *mut Header,
    caller: u64,
    site: u64,
) -> u8 {
    unsafe {
        crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, join_dispatcher, 48)
    }
}

unsafe extern "C" fn join_dispatcher(
    ctx: *mut TaskCtx,
    env: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    let words = frame.cast::<u64>();
    if unsafe { words.read() } == 0 {
        let [scope, child] = unsafe { crate::alloc::payload(env).cast::<[u64; 2]>().read() };
        unsafe {
            words.add(1).write(scope);
            words.add(2).write(child);
            words.add(3).write(0);
            words.add(4).write(0);
            words.write(1);
        }
    }
    unsafe { crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 32, 48, 0, 0) };
    let status = unsafe {
        hirsel_rt_v2_ordered_join(
            ctx,
            0,
            0,
            words.add(1),
            words.add(2),
            words.add(3).cast(),
            16,
            8,
        )
    };
    if status == STATUS_SUSPENDED {
        // Only the original record may supply the taken child on re-entry.
        // Saving cannot restore this cell into the runtime's identity.
        unsafe { words.add(2).write(u64::MAX) };
        return status;
    }
    assert_eq!(status, STATUS_OK);
    assert_eq!(unsafe { words.add(3).cast::<[u64; 2]>().read() }, [1, 7]);
    unsafe { words.write(2) };
    unsafe { finish_value(ctx, words.add(4).read()) }
}

#[test]
fn retained_join_bounds_refuse_header_overlap_alignment_escape_and_overflow() {
    assert_eq!(retained_offset(0x1000, 64, 0x1010, 16, 8), Some(16));
    for (extent, address, size, align) in [
        (64, 0x1000, 16, 8),
        (64, 0x1011, 16, 8),
        (64, 0x1030, 32, 8),
        (64, 0x1010, 16, 0),
        (64, 0x1010, 16, 3),
        (64, 0x1020, 16, 32),
        (64, 0x0ff0, 16, 8),
        (64, 0x1010, u64::MAX, 8),
    ] {
        assert_eq!(retained_offset(0x1000, extent, address, size, align), None);
    }
}

#[test]
fn retained_join_releases_its_stack_and_never_rereads_the_taken_handle() {
    retained_join_run(sleeper, false, false);
}

#[test]
fn retained_host_and_join_release_both_stacks_and_capture_original_inputs_once() {
    retained_join_run(compact_sleeper, true, false);
}

// The home adapter uses the same scripted readiness relation, including
// its original enter/finish token. It never retains the callback's borrow.
struct HomeHost {
    host: Arc<Host>,
    token: u64,
}
impl crate::io::Adapter for HomeHost {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn enter(&mut self, token: u64) {
        self.token = token;
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert!(matches!(
            request,
            Request::SleepUntil {
                clock: 11,
                deadline: 10
            }
        ));
        self.host.calls.lock().unwrap().push(self.token);
        if self.host.home_joined.load(Ordering::Acquire) {
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        }
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        Ok(if self.host.home_joined.load(Ordering::Acquire) {
            Ready::Every
        } else {
            Ready::Tokens(vec![])
        })
    }
    fn finish(&mut self, token: u64) {
        self.host.finish(token);
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        Ok(true)
    }
}

#[test]
fn retained_home_host_and_join_release_both_stacks_with_original_home_token() {
    retained_join_run(compact_sleeper, true, true);
}

pub(super) unsafe extern "C" fn compact_sleeper(
    ctx: *mut TaskCtx,
    env: *mut Header,
    caller: u64,
    site: u64,
) -> u8 {
    unsafe {
        crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, sleep_dispatcher, 48)
    }
}

unsafe extern "C" fn sleep_dispatcher(
    ctx: *mut TaskCtx,
    _: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    let words = frame.cast::<u64>();
    let first = unsafe { words.read() } == 0;
    if first {
        unsafe {
            words.add(1).write(11);
            words.add(2).write(10);
            words
                .add(3)
                .cast::<crate::io::UnitResult>()
                .write(crate::io::UnitResult {
                    tag: 99,
                    error: 99,
                    code: 99,
                });
            words.write(1);
        }
    }
    unsafe { crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 33, 48, 0, 0) };
    let status = unsafe {
        super::super::host::entered::hirsel_rt_v2_ordered_sleep_until(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            if first {
                words.add(3).cast()
            } else {
                std::ptr::null_mut()
            },
            if first {
                words.add(1)
            } else {
                std::ptr::null()
            },
            if first {
                words.add(2)
            } else {
                std::ptr::null()
            },
        )
    };
    if status == STATUS_SUSPENDED {
        unsafe {
            words.add(1).write(u64::MAX);
            words.add(2).write(u64::MAX);
        }
        return status;
    }
    assert_eq!(status, STATUS_OK);
    assert_eq!(
        unsafe { words.add(3).cast::<crate::io::UnitResult>().read() },
        crate::io::UnitResult {
            tag: 0,
            error: 0,
            code: 0
        }
    );
    unsafe { words.write(2) };
    unsafe { finish_value(ctx, 7) }
}

fn retained_join_run(sleeper: crate::tasks::TaskCode, require_host_release: bool, home: bool) {
    for width in [1, 4] {
        let environment = layout();
        let result = layout();
        let layouts = [&environment as *const crate::Layout, &result];
        let host = Arc::new(Host::default());
        let setup = host.clone();
        let observe = host.clone();
        let compact_saves = Arc::new(AtomicUsize::new(0));
        let count_saves = compact_saves.clone();
        let host_saves = Arc::new(AtomicUsize::new(0));
        let count_host_saves = host_saves.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        if home {
            run.adapter = Some(Box::new(HomeHost {
                host: host.clone(),
                token: 0,
            }));
        } else {
            run.concurrent_adapter = Some(host.clone());
        }
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
                setup.run.store((*ctx).run_ptr(), Ordering::Release);
                engine::driver(ctx).after_save = Some(Box::new(move |s, task, completed| {
                    if completed {
                        return;
                    }
                    if s.tasks
                        .iter()
                        .flatten()
                        .any(|row| matches!(row.wait, Some(Wait::Join { child: 1, .. })))
                    {
                        observe.home_joined.store(true, Ordering::Release);
                    }
                    let Logical::Ordered(d) = &s.logical else {
                        unreachable!()
                    };
                    let id = handle(task, s.tasks[task as usize].as_ref().unwrap().generation);
                    if let Some(op) = d.in_operations.get(&id) {
                        if matches!(op, Operation::Host(_))
                            && s.tasks[task as usize]
                                .as_ref()
                                .unwrap()
                                .context_home
                                .is_some()
                        {
                            count_host_saves.fetch_add(1, Ordering::Relaxed);
                        }
                        if matches!(op, Operation::Join(_))
                            && s.tasks[task as usize]
                                .as_ref()
                                .unwrap()
                                .context_home
                                .is_some()
                        {
                            count_saves.fetch_add(1, Ordering::Relaxed);
                        }
                        observe
                            .saves
                            .lock()
                            .unwrap()
                            .push((id, matches!(op, Operation::Host(_))));
                    }
                }));
            },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let mut child = 0;
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env).cast::<[u64; 2]>().write([0, 0]);
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        sleeper,
                        env,
                        0
                    ),
                    STATUS_OK
                );
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<[u64; 2]>()
                    .write([scope, child]);
                let mut joining = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut joining,
                        &scope,
                        compact_joiner,
                        env,
                        0
                    ),
                    STATUS_OK
                );
                let mut result = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut joining,
                        result.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(result, [1, 7]);
                assert!(engine::driver(ctx).in_operations.is_empty());
                assert!(engine::driver(ctx).host_ops.is_empty());
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                STATUS_OK
            },
        );
        assert!(
            compact_saves.load(Ordering::Relaxed) > 0,
            "join never released its original stack"
        );
        if require_host_release {
            assert!(
                host_saves.load(Ordering::Relaxed) > 0,
                "host never released its original stack"
            );
        }
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(
            *host.calls.lock().unwrap(),
            if home { [0, 0] } else { [1, 1] }
        );
        assert_eq!(host.finishes.load(Ordering::Acquire), 1);
        let saves = host.saves.lock().unwrap();
        assert!(
            saves.iter().any(|(_, host)| *host),
            "host continuation was never saved"
        );
        assert!(
            saves.iter().any(|(_, host)| !*host),
            "join continuation was never saved"
        );
    }
}
