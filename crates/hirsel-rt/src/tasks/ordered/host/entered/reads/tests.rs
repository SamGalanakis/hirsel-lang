//! Port the owned short/error/cut vectors from host/completed_read_tests.rs
//! through real entered Pending and pooled leave, using Sleep's saved hook.
use super::*;
use crate::io::{Adapter, InputError, IoError, Ready};
use crate::io_concurrent::ConcurrentAdapter;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Read {
    kind: u32,
    mode: u8,
    delayed: bool,
    ready: AtomicBool,
    saves: AtomicUsize,
    deliveries: AtomicUsize,
    calls: Mutex<Vec<u64>>,
    finishes: Mutex<Vec<u64>>,
    jobs: Mutex<Option<crate::tasks::free::host::jobs::Probe>>,
}
impl Read {
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        match request {
            Request::ReadStdin { stdin: 11, max: 4 } if self.kind == 34 => {}
            Request::WatchNext {
                directory: 11,
                watcher: 12,
            } if self.kind == 35 => {}
            other => panic!("reread poisoned inputs: {other:?}"),
        }
        self.calls.lock().unwrap().push(token);
        if self.mode == 6 {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !self.ready.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < until,
                    "job owner never released its pooled stack"
                );
                std::thread::yield_now();
            }
        }
        if self.delayed && !self.ready.load(Ordering::Acquire) {
            return Err(AdapterError::Blocked);
        }
        Ok(if self.kind == 34 {
            Completion::Input(match self.mode {
                0 | 6 => Ok(vec![71, 0, 255]),
                1 => Ok(vec![]),
                2 => Err(InputError::Fault(19)),
                3 => Err(InputError::Canceled(7)),
                4 => Err(InputError::InvalidInput),
                _ => Err(InputError::InvalidUtf8),
            })
        } else {
            Completion::Watch(match self.mode {
                0 | 6 => Ok(crate::watch::WatchEvent::Renamed(
                    "before".into(),
                    "after".into(),
                )),
                1 => Ok(crate::watch::WatchEvent::Overflow),
                2 => Err(IoError::Fault(19)),
                _ => Err(IoError::Canceled(7)),
            })
        })
    }
    fn ready(&self) -> Ready {
        if self.ready.load(Ordering::Acquire) {
            Ready::Every
        } else {
            Ready::Tokens(vec![])
        }
    }
}
impl ConcurrentAdapter for Read {
    fn blocking(&self, _: &Request) -> bool {
        self.mode == 6
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        self.call_at(token, request)
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        Ok(self.ready())
    }
    fn finish(&self, token: u64) {
        self.finishes.lock().unwrap().push(token);
    }
}
struct Home(Arc<Read>, u64, std::thread::ThreadId);
impl Adapter for Home {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn enter(&mut self, token: u64) {
        self.1 = token;
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(self.2, std::thread::current().id());
        self.0.call_at(self.1, request)
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        Ok(self.0.ready())
    }
    fn finish(&mut self, token: u64) {
        self.0.finishes.lock().unwrap().push(token);
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        Ok(true)
    }
}
unsafe extern "C" fn child(
    ctx: *mut TaskCtx,
    env: *mut crate::Header,
    caller: u64,
    site: u64,
) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 80) }
}
unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    env: *mut crate::Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    let read = unsafe { &*crate::alloc::payload(env).cast::<*const Read>().read() };
    let words = frame.cast::<u64>();
    let first = unsafe { words.read() } == 0;
    if first {
        unsafe {
            words.write(1);
            words.add(1).write(11);
            words.add(2).write(12);
            words
                .add(3)
                .write(crate::text::allocate_bytes(ctx, b"prefix/").owner as u64);
            words.add(4).write(4);
            words.add(5).cast::<u8>().write_bytes(0x99, 32);
        }
    }
    unsafe {
        crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, read.kind, 80, 0, 0)
    };
    let out = if first {
        unsafe { words.add(5).cast() }
    } else {
        std::ptr::null_mut()
    };
    let input = if first {
        unsafe { words.add(1) }
    } else {
        std::ptr::null_mut()
    };
    let status = unsafe {
        if read.kind == 34 {
            hirsel_rt_v2_ordered_stdin_read(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                out,
                input,
                if first {
                    words.add(3).cast()
                } else {
                    std::ptr::null_mut()
                },
                if first { words.add(4).read() } else { u64::MAX },
            )
        } else {
            hirsel_rt_v2_ordered_watcher_next(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                out,
                input,
                if first {
                    words.add(2)
                } else {
                    std::ptr::null_mut()
                },
            )
        }
    };
    if status == crate::STATUS_SUSPENDED {
        unsafe {
            words.add(1).write(u64::MAX);
            words.add(2).write(u64::MAX);
            words.add(4).write(u64::MAX);
        }
        return status;
    }
    assert_eq!(status, crate::STATUS_OK);
    assert_eq!(read.deliveries.fetch_add(1, Ordering::Relaxed), 0);
    let buffer = unsafe { words.add(3).read() as *mut crate::Header };
    let len = unsafe { crate::alloc::payload(buffer).cast::<u64>().read() };
    let bytes =
        unsafe { crate::io::slice_bytes(buffer, crate::alloc::sequence_slice_start(1), len) };
    assert_eq!(
        bytes,
        if read.kind == 34 && read.mode == 0 {
            b"prefix/G\0\xff".as_slice()
        } else {
            b"prefix/"
        }
    );
    if read.kind == 34 {
        let out = unsafe { &*words.add(5).cast::<crate::io::HandleResult>() };
        assert_eq!(out.tag, u32::from(read.mode >= 2));
        assert_eq!(out.padding, 0);
        assert_eq!(
            out.value,
            match read.mode {
                0 => 3,
                1 => 0,
                2 => 19 << 32,
                3 => 1 | (7 << 32),
                4 => 2,
                7 => 1 | (u64::from(CANCEL_REQUESTED) << 32),
                _ => 3,
            }
        );
    } else {
        let out = unsafe { words.add(5).cast::<crate::io::WatchResult>().read() };
        match read.mode {
            0 => {
                assert_eq!(&out.words[..2], &[0, 3]);
                assert_eq!(crate::paths::names(ctx).path_text(out.words[2]), "before");
                assert_eq!(crate::paths::names(ctx).path_text(out.words[3]), "after");
            }
            1 => assert_eq!(out.words, [0, 4, 0, 0]),
            2 => assert_eq!(out.words, [1, 19 << 32, 0, 0]),
            7 => assert_eq!(
                out.words,
                [1, 1 | (u64::from(CANCEL_REQUESTED) << 32), 0, 0]
            ),
            _ => assert_eq!(out.words, [1, 1 | (7 << 32), 0, 0]),
        }
    }
    unsafe {
        assert_eq!(
            crate::release::hirsel_rt_v2_release(
                ctx,
                buffer,
                (*buffer).index(),
                std::ptr::null(),
                0,
                0
            ),
            crate::STATUS_OK
        );
        words.write(2);
        let mut result = [1u64, 7];
        hirsel_rt_v2_task_finish(
            ctx,
            result.as_mut_ptr().cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            3,
        )
    }
}
fn witness(kind: u32, concurrent: bool, delayed: bool, mode: u8, cut: bool) {
    for width in [1, 4] {
        let mut layout = crate::tasks::ordered::host::tests::layout();
        layout.size = 16;
        let layouts = [&layout as *const crate::Layout, &layout];
        let read = Arc::new(Read {
            kind,
            mode,
            delayed,
            ready: AtomicBool::new(false),
            saves: AtomicUsize::new(0),
            deliveries: AtomicUsize::new(0),
            calls: Mutex::new(vec![]),
            finishes: Mutex::new(vec![]),
            jobs: Mutex::new(None),
        });
        let observe = read.clone();
        let setup = read.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        if concurrent {
            run.concurrent_adapter = Some(read.clone());
        } else {
            run.adapter = Some(Box::new(Home(read.clone(), 0, std::thread::current().id())));
        }
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                (*ctx).run().names = Some(Arc::new(Default::default()));
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
                let run_ptr = (*ctx).run_ptr() as usize;
                engine::driver(ctx).after_save = Some(Box::new(move |s, task, completed| {
                    let row = s.tasks[task as usize].as_ref().unwrap();
                    let Logical::Ordered(d) = &s.logical else {
                        unreachable!()
                    };
                    let id = handle(task, row.generation);
                    if !completed
                        && row.context_home.is_some()
                        && (matches!(row.wait, Some(Wait::Host { .. }))
                            || (mode == 6 && d.head_job.is_some_and(|(owner, _)| owner == id)))
                    {
                        let Operation::Host(host) = &d.in_operations[&handle(task, row.generation)]
                        else {
                            unreachable!()
                        };
                        assert_eq!(host.binding.unwrap().site.kind, kind);
                        assert!(!host.yielded, "read entered the initial yield phase");
                        observe.saves.fetch_add(1, Ordering::Relaxed);
                        if mode == 7 {
                            s.cancel(task, CANCEL_REQUESTED);
                        }
                        if cut {
                            (*(run_ptr as *const crate::context::RunShared))
                                .control
                                .request_inside(crate::stop::Stop::Requested);
                        }
                        observe.ready.store(mode != 7, Ordering::Release);
                    }
                }));
            },
            move |ctx| unsafe {
                if mode == 6 {
                    *setup.jobs.lock().unwrap() =
                        Some((*ctx).run_ref().ordered_io.get().unwrap().job_probe());
                }
                let mut scope = 0;
                let mut child_id = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    crate::STATUS_OK
                );
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<*const Read>()
                    .write(Arc::as_ptr(&setup));
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child_id,
                        &scope,
                        child,
                        env,
                        0
                    ),
                    crate::STATUS_OK
                );
                let mut result = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut child_id,
                        result.as_mut_ptr().cast(),
                        16
                    ),
                    crate::STATUS_OK
                );
                assert_eq!(result, [1, 7]);
                assert!(engine::driver(ctx).in_operations.is_empty());
                assert!(engine::driver(ctx).host_ops.is_empty());
                assert!((*ctx).pending_ordered_host.is_none());
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    crate::STATUS_OK
                );
                crate::STATUS_OK
            },
        );
        assert_eq!(
            outcome.ended,
            if cut {
                Err(crate::stop::Stop::Requested)
            } else {
                Ok(crate::STATUS_OK)
            }
        );
        if delayed {
            assert!(
                read.saves.load(Ordering::Relaxed) > 0,
                "host never returned its pooled stack"
            );
        }
        assert_eq!(read.deliveries.load(Ordering::Relaxed), usize::from(!cut));

        if mode == 6 {
            let jobs = read.jobs.lock().unwrap();
            let jobs = jobs.as_ref().unwrap();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while jobs.stats().reclaimed.load(Ordering::Acquire) != 1 || jobs.held() != 0 {
                assert!(
                    std::time::Instant::now() < until,
                    "late read job not reclaimed"
                );
                std::thread::yield_now();
            }
            assert_eq!(jobs.stats().submitted.load(Ordering::Acquire), 1);
            assert_eq!(jobs.stats().returned.load(Ordering::Acquire), 1);
            assert_eq!(jobs.stats().delivered.load(Ordering::Acquire), 0);
            assert_eq!(jobs.stats().quarantined.load(Ordering::Acquire), 1);
        }
        // The queued job can enter its adapter after the run has returned.
        // Reclamation above proves that callback completed before inspecting it.
        let calls = read.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            if delayed && !cut && mode != 6 && mode != 7 {
                2
            } else {
                1
            }
        );
        assert!(calls.iter().all(|token| *token == calls[0]));
        let finishes = read.finishes.lock().unwrap();
        if cut {
            assert!(finishes.len() <= 1);
            assert!(finishes.iter().all(|token| *token == calls[0]));
        } else {
            assert_eq!(*finishes, vec![calls[0]]);
        }
    }
}
#[test]
fn retained_stdin_owns_short_empty_and_all_typed_errors_after_pooled_leave() {
    for concurrent in [false, true] {
        for mode in 0..6 {
            witness(34, concurrent, true, mode, false);
        }
    }
}
#[test]
fn retained_watch_owns_both_rename_paths_overflow_and_typed_errors_after_pooled_leave() {
    for concurrent in [false, true] {
        for mode in 0..4 {
            witness(35, concurrent, true, mode, false);
        }
    }
}
#[test]
fn immediate_reads_finish_and_deliver_once_on_both_routes() {
    for kind in [34, 35] {
        for concurrent in [false, true] {
            witness(kind, concurrent, false, 0, false);
        }
    }
}
#[test]
fn cut_after_read_save_abandons_owned_wait_without_delivery() {
    for kind in [34, 35] {
        for concurrent in [false, true] {
            witness(kind, concurrent, true, 0, true);
        }
    }
}

#[test]
fn retained_read_jobs_keep_late_owned_completions_in_quarantine_after_cut() {
    for kind in [34, 35] {
        witness(kind, true, true, 6, true);
    }
}

#[test]
fn cancellation_after_read_pooled_save_skips_retry_and_converts_once_on_both_routes() {
    for kind in [34, 35] {
        for concurrent in [false, true] {
            witness(kind, concurrent, true, 7, false);
        }
    }
}
