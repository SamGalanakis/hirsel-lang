//! Home-owned native callee orchestration. Computation runs on the existing
//! executor; only owned canonical bytes cross the independent run boundary.
//! The parent execution and its non-Send adapters stay on admission.
use super::*;
use std::sync::{Arc, Mutex};

mod adapter;
mod home;
mod transport;
pub(crate) use adapter::wrap;
pub(crate) use home::{Home, Slot, service};
use transport::{Spec, Transport};

/// Native Free's synchronous forms own one complete host phase operation.
/// Spawn remains explicitly gated until its cross-task phase ownership exists.
#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn send(
    ctx: *mut TaskCtx,
    request: RunnerRequest,
    grant: RunnerGrant,
    ordinal: u64,
    placed: Result<(Option<u32>, bool), CallOutcome>,
    start: Start,
    spawn: bool,
) -> Stopped<u64> {
    if spawn {
        return Err(Stop::Refused(
            "native Free runner.spawn requires asynchronous phase ownership".into(),
        ));
    }
    let key = request.key;
    let slot = unsafe { (*ctx).run_ref().runner_home.as_deref() };
    let mut call = Call {
        task: Some(crate::tasks::current_task(ctx)),
        grant,
        ordinal,
        key,
        place: None,
        lose_reply: false,
        crash_after: None,
        forwarded: None,
        start: None,
        callee: None,
        completion: None,
    };
    match placed {
        Err(outcome) => {
            drop(start);
            let completion = RunnerCompletion {
                outcome,
                callee: None,
            };
            if let Some(slot) = slot {
                slot.transport.immediate(key, completion.clone());
            }
            call.completion = Some(completion);
        }
        Ok((crash_after, lose_reply)) => {
            call.place = Some(call.grant.entry.name.clone());
            if let Some(slot) = slot {
                slot.transport.prepare(
                    key,
                    Spec {
                        start,
                        grant: call.grant.clone(),
                        ordinal,
                        lose_reply,
                        crash_after,
                    },
                );
            }
        }
    }
    state(ctx).calls.insert(ordinal, call);
    // No Execution or home-state reference survives this operation's park.
    let result = unsafe {
        crate::tasks::free::host::native::issue(ctx, Request::RunnerCall(Box::new(request)))
    }?
    .0;
    let mut result = Some(result);
    let mut failure = None;
    let served = unsafe {
        crate::pool::call_home(ctx, &mut || match result.take().unwrap() {
            Ok(Completion::Runner(completion)) => {
                let run = state(ctx);
                let call = run.calls.get_mut(&ordinal).unwrap();
                if call
                    .completion
                    .as_ref()
                    .is_some_and(|expected| expected != &completion)
                {
                    failure = Some(Stop::refusal(AdapterError::InvalidCompletion));
                    return;
                }
                if let Some(place) = call.place.as_ref().and_then(|p| run.places.get_mut(p)) {
                    place.answered();
                }
                call.completion = Some(completion);
            }
            Ok(_) => failure = Some(Stop::refusal(AdapterError::InvalidCompletion)),
            Err(error) => failure = Some(Stop::refusal(error)),
        })
    };
    // A withdrawn bridge drops owned bytes before the caller raises its stop.
    drop(result);
    served?;
    if let Some(stop) = failure {
        return Err(stop);
    }
    Ok(ordinal)
}

/// Installed once before any child worker starts. No parent execution pointer,
/// heap value or adapter loan can be reached through this state.
pub(crate) struct CallState {
    pub(super) entry: (usize, usize),
    pub(super) form: CallForm,
    pub(super) argument: Mutex<Option<Vec<u8>>>,
    pub(super) reply: Mutex<Option<Reply>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Reply {
    pub(super) tag: u64,
    pub(super) bytes: Vec<u8>,
    pub(super) chain: Vec<crate::failure::Chained>,
}

/// The pointer is borrowed from immutable run-owned storage. The caller is a
/// live task of that run, so even a detached child retains this state until its
/// last worker leaves. No owning Arc is left on an abandonable task stack.
pub(super) fn call(ctx: *mut TaskCtx) -> Option<&'static CallState> {
    unsafe { (*ctx).run_ref().runner_call.as_deref() }
}

/// All methods and destruction run on the admitting home thread. In
/// particular `binding` owns a possibly non-Send adapter, never a worker.
pub(super) struct CalleeRun {
    root: *mut TaskCtx,
    ticket: crate::stop::StartedRoot,
    call: Arc<CallState>,
    _binding: Box<Binding>,
}

impl CalleeRun {
    /// # Safety
    /// `start` contains the program's checked static entry/main relocation
    /// pairs, and `runtime`/`table` outlive this run and any detached workers.
    /// This is called only by the execution's home owner.
    pub(super) unsafe fn start(
        runtime: crate::context::Config,
        table: Option<crate::isolate::FailureTable>,
        start: Start,
        mut binding: Box<Binding>,
        driver: crate::readiness::Readiness,
    ) -> Self {
        let call = Arc::new(CallState {
            entry: start.entry,
            form: start.form,
            argument: Mutex::new(Some(start.argument)),
            reply: Mutex::new(None),
        });
        let (code, environment) = start.main;
        unsafe {
            let mut root = std::ptr::null_mut();
            if crate::context::hirsel_rt_v2_ctx_create(&runtime, &mut root) != 0 {
                fault("a native callee context could not be created");
            }
            (*root).run().runner_call = Some(call.clone());
            binding.bind(&mut *root);
            if let Some(table) = table {
                crate::isolate::attach(&mut *root, table);
            }
            // A child is an independent run. Home orchestrates it without
            // holding a process compute permit; width one uses the existing
            // worker topology, not a new scheduler or an orchestration thread.
            crate::pool::start(root, 1, false);
            (*root).run().free_mode = true;
            crate::tasks::free::enable(root);
            crate::pool::free::enable(root);
            crate::pool::home_driver((*root).run_ptr(), driver);
            let frame = crate::stop::LaunchFrame::bare(Box::new(move |ctx, _| {
                let code = std::mem::transmute::<usize, TaskCode>(code);
                code(ctx, environment as *mut Header, 0, 0)
            }));
            let ticket = crate::stop::start_root(root, frame);
            Self {
                root,
                ticket,
                call,
                _binding: binding,
            }
        }
    }

    /// Service only published bridge continuations. In particular this never
    /// blocks in an individual callee's adapter wait while other runs need home.
    pub(super) fn service(&mut self) -> bool {
        unsafe { crate::pool::poll_home_posts((*self.root).run_ptr()) }
    }

    /// # Safety
    /// Home admission, before any externally visible request is serviced.
    unsafe fn notify_on(&self, signal: Arc<crate::readiness::Signal>) {
        unsafe { crate::pool::home_signal((*self.root).run_ptr(), signal) };
    }

    /// A reply becomes externally available only after the root's actual
    /// physical save and worker permit return, not at RunnerReply's earlier
    /// body return. A stop is observable immediately, even during computation.
    pub(super) fn poll(&mut self) -> Option<Stopped<Reply>> {
        match unsafe { self.ticket.poll() }? {
            Ok(STATUS_OK) => {
                Some(Ok(self.call.reply.lock().unwrap().take().unwrap_or_else(
                    || fault("a native callee returned without its reply"),
                )))
            }
            Ok(_) => fault("a native callee's main failed outside its catch"),
            Err(stop) => Some(Err(stop)),
        }
    }
}

impl Drop for CalleeRun {
    fn drop(&mut self) {
        unsafe {
            let run = (*self.root).run_ptr();
            if matches!(self.ticket.poll(), Some(Ok(_))) {
                crate::pool::close_home(run);
                crate::context::hirsel_rt_v2_ctx_destroy(self.root);
            } else {
                // Close and withdraw all borrowed home calls before destroying
                // their non-Send adapter. A still-computing worker keeps only
                // its own run and CallState, reclaimed by the existing holder
                // protocol once it observes the stop.
                (*run).control.request(Stop::Requested);
                crate::pool::close_home(run);
                crate::context::detach(self.root);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ENTERED: AtomicBool = AtomicBool::new(false);
    static RELEASE: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn stopped_main(
        ctx: *mut TaskCtx,
        environment: *mut Header,
        _: u64,
        _: u64,
    ) -> u8 {
        ENTERED.store(true, Ordering::Release);
        if environment.is_null() {
            while !RELEASE.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            // The run still owns its immutable child data after admission
            // dropped the home owner. No parent pointer is consulted here.
            assert!(call(ctx).is_some());
        } else {
            unsafe {
                Binding::get(ctx)
                    .adapter
                    .call(Request::MonotonicClock { io: 1 })
                    .unwrap();
            }
            panic!("a withdrawn stopped home call must never return to program code");
        }
        STATUS_OK
    }

    #[test]
    fn dropped_callees_withdraw_home_calls_and_retain_busy_child_state() {
        const CHILD: &str = "HIRSEL_NATIVE_CALLEE_STOP_LIFETIME";
        if std::env::var_os(CHILD).is_none() {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "runners::native::tests::dropped_callees_withdraw_home_calls_and_retain_busy_child_state", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(child.status.success(), "{child:?}");
            return;
        }
        crate::pool::set_compute_grant(1);
        for bridge in [false, true] {
            ENTERED.store(false, Ordering::Release);
            RELEASE.store(false, Ordering::Release);
            let (calls, drops) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
            let mut run = unsafe {
                CalleeRun::start(
                    crate::Config::new(),
                    None,
                    Start {
                        main: (stopped_main as *const () as usize, usize::from(bridge)),
                        entry: (0, 0),
                        argument: vec![1, 2, 3],
                        form: CallForm::Compute,
                    },
                    Box::new(Binding::new(Home {
                        thread: std::thread::current().id(),
                        calls: calls.clone(),
                        drops: drops.clone(),
                    })),
                    crate::readiness::Readiness::default(),
                )
            };
            let state = Arc::downgrade(&run.call);
            let holders = unsafe { crate::pool::holders((*run.root).run_ref()).unwrap() };
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !ENTERED.load(Ordering::Acquire)
                || (bridge && crate::pool::compute_permits().1 != 0)
            {
                assert!(std::time::Instant::now() < until);
                std::thread::yield_now();
            }
            unsafe { (*run.root).run_ref().control.request(Stop::Requested) };
            assert_eq!(run.poll(), Some(Err(Stop::Requested)));
            // Bridge mode has published its borrowed closure and released its
            // permit. Drop must withdraw it before freeing the home adapter.
            // Pure mode deliberately retains its permit and child-local data.
            drop(run);
            assert_eq!(drops.get(), 1);
            assert_eq!(calls.get(), 0);
            if !bridge {
                assert!(state.upgrade().is_some());
                assert_eq!(holders.lost_permits(), 1);
            }
            RELEASE.store(true, Ordering::Release);
            while holders.left() != 0 {
                assert!(std::time::Instant::now() < until);
                std::thread::yield_now();
            }
            assert!(state.upgrade().is_none());
            assert_eq!(crate::pool::compute_permits(), (1, 0, 0));
        }
    }

    struct Home {
        thread: std::thread::ThreadId,
        calls: Rc<Cell<usize>>,
        drops: Rc<Cell<usize>>,
    }
    impl crate::io::Adapter for Home {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            assert_eq!(std::thread::current().id(), self.thread);
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert_eq!(std::thread::current().id(), self.thread);
            assert!(matches!(request, Request::MonotonicClock { io: 1 }));
            self.calls.set(self.calls.get() + 1);
            Ok(Completion::Monotonic(Ok(7)))
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            assert_eq!(std::thread::current().id(), self.thread);
            self.drops.set(self.drops.get() + 1);
        }
    }

    unsafe extern "C" fn main(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
        unsafe {
            let mut entry = [0usize; 2];
            hirsel_rt_v2_runner_entry(ctx, std::ptr::null_mut(), entry.as_mut_ptr());
            let tag = entry[0] as u64;
            assert_eq!(entry[1], 0);
            let mut tasks = u64::MAX;
            hirsel_rt_v2_runner_tasks(ctx, std::ptr::null_mut(), &mut tasks);
            assert_eq!(tasks, 0);
            let mut argument = std::mem::MaybeUninit::uninit();
            hirsel_rt_v2_runner_argument(ctx, std::ptr::null_mut(), argument.as_mut_ptr());
            let argument = argument.assume_init();
            assert_eq!(
                crate::io::slice_bytes(argument.owner, argument.start, argument.length),
                [tag as u8, 8, 0, 255]
            );
            assert!(matches!(
                Binding::get(ctx)
                    .adapter
                    .call(Request::MonotonicClock { io: 1 }),
                Ok(Completion::Monotonic(Ok(7)))
            ));
            if tag == REPLY_FAILED {
                (*ctx).isolated.push((
                    crate::failure::FailureRecord {
                        kind: 1,
                        rule: 2,
                        site: 3,
                        operation_site: 4,
                    },
                    vec![0, 1],
                ));
            }
            hirsel_rt_v2_runner_reply(ctx, tag, argument.owner, argument.start, argument.length);
            crate::release::release_block(&mut *ctx, argument.owner, std::ptr::null(), 0, 0);
            STATUS_OK
        }
    }

    #[test]
    fn independent_callees_use_child_local_ffi_and_home_owned_adapters() {
        const CHILD: &str = "HIRSEL_NATIVE_CALLEE_ONE_PERMIT";
        if std::env::var_os(CHILD).is_none() {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "runners::native::tests::independent_callees_use_child_local_ffi_and_home_owned_adapters", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(child.status.success(), "{child:?}");
            return;
        }
        crate::pool::set_compute_grant(1);
        let (calls, drops) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let driver = crate::readiness::Readiness::default();
        let mut runs: Vec<_> = (0..3)
            .map(|tag| unsafe {
                CalleeRun::start(
                    crate::Config::new(),
                    None,
                    Start {
                        main: (main as *const () as usize, 0),
                        entry: (tag, 0),
                        argument: vec![tag as u8, 8, 0, 255],
                        form: CallForm::Compute,
                    },
                    Box::new(Binding::new(Home {
                        thread: std::thread::current().id(),
                        calls: calls.clone(),
                        drops: drops.clone(),
                    })),
                    driver.clone(),
                )
            })
            .map(Some)
            .collect();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while runs.iter().any(Option::is_some) {
            for (tag, slot) in runs.iter_mut().enumerate() {
                let Some(run) = slot else { continue };
                run.service();
                if let Some(reply) = run.poll() {
                    let reply = reply.unwrap();
                    assert_eq!(reply.tag, tag as u64);
                    assert_eq!(reply.bytes, [tag as u8, 8, 0, 255]);
                    assert_eq!(reply.chain.len(), usize::from(tag == 2));
                    slot.take();
                }
            }
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(calls.get(), 3);
        assert_eq!(drops.get(), 3);
        assert_eq!(crate::pool::compute_permits(), (1, 0, 0));
    }
}
