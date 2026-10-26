use super::*;
use crate::io::{IoError, Ready};
use crate::io_concurrent::ConcurrentAdapter;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Print {
    saved: AtomicBool,
    calls: AtomicUsize,
    finishes: AtomicUsize,
    bytes: Mutex<Vec<u8>>,
}
impl ConcurrentAdapter for Print {
    fn identity(&self, word: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        Some((
            crate::io_keys::Key(word as u32),
            if word == 11 {
                crate::io_keys::Kind::Sink { file_backed: false }
            } else {
                crate::io_keys::Kind::Other
            },
        ))
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn blocking(&self, request: &Request) -> bool {
        matches!(request, Request::Print { .. })
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        match request {
            Request::Stdout { io: 1 } => Ok(Completion::Handle(Ok(11))),
            Request::Print { stdout: 11, bytes } => {
                self.calls.fetch_add(1, Ordering::AcqRel);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !self.saved.load(Ordering::Acquire) {
                    assert!(
                        std::time::Instant::now() < until,
                        "print owner did not release its stack"
                    );
                    std::thread::yield_now();
                }
                *self.bytes.lock().unwrap() = bytes;
                Ok(Completion::Unit(Err(IoError::Fault(11))))
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    fn call_observed_at(
        &self,
        _: u64,
        request: Request,
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        let print = matches!(request, Request::Print { .. });
        (self.call(request), print.then_some(1))
    }
    fn finish(&self, _: u64) {
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        Ok(Ready::Every)
    }
}

fn layout() -> crate::Layout {
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
unsafe extern "C" fn child(
    ctx: *mut TaskCtx,
    env: *mut crate::Header,
    caller: u64,
    site: u64,
) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 48) }
}
unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    env: *mut crate::Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    let words = frame.cast::<u64>();
    let first = unsafe { words.read() } == 0;
    if first {
        if unsafe { crate::alloc::payload(env).cast::<u64>().read() } != 0 {
            crate::tasks::role::with_role(ctx, |s| {
                let me = s.current;
                s.cancel(me, crate::tasks::CANCEL_REQUESTED);
            });
        }
        let text = unsafe {
            crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0)
        };
        let payload = crate::alloc::payload(text);
        unsafe {
            payload.cast::<u64>().write(3);
            std::ptr::copy_nonoverlapping(b"xyz".as_ptr(), payload.add(8), 3);
            words.add(1).write(11);
            words.add(2).write(text as u64);
            words.write(1);
        }
    }
    unsafe { crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 36, 48, 0, 0) };
    let status = unsafe {
        hirsel_rt_v2_ordered_print(
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
                words.add(2).read() as *mut crate::Header
            } else {
                std::ptr::null_mut()
            },
            0,
            3,
        )
    };
    if status == crate::STATUS_SUSPENDED {
        unsafe {
            words.add(1).write(u64::MAX);
            std::ptr::copy_nonoverlapping(
                b"BAD".as_ptr(),
                crate::alloc::payload(words.add(2).read() as *mut crate::Header).add(8),
                3,
            );
        }
        return status;
    }
    assert_eq!(status, crate::STATUS_OK);
    assert_eq!(
        unsafe { words.add(3).cast::<UnitResult>().read() },
        UnitResult::of(Err(
            if unsafe { crate::alloc::payload(env).cast::<u64>().read() } != 0 {
                IoError::Canceled(crate::tasks::CANCEL_REQUESTED)
            } else {
                IoError::Fault(11)
            }
        ))
    );
    let mut result = [1u64, 7];
    unsafe {
        assert_eq!(
            crate::release::hirsel_rt_v2_release(
                ctx,
                words.add(2).read() as *mut crate::Header,
                2,
                std::ptr::null(),
                0,
                0
            ),
            crate::STATUS_OK
        );
        words.write(2);
        hirsel_rt_v2_task_finish(
            ctx,
            result.as_mut_ptr().cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            3,
        )
    }
}

#[test]
fn entered_print_job_owns_bytes_until_real_return_and_converts_failure_once() {
    for canceled in [false, true] {
        for width in [1, 4] {
            let layout = layout();
            let layouts = [&layout as *const crate::Layout, &layout];
            let host = Arc::new(Print::default());
            let setup = host.clone();
            let observe = host.clone();
            let prefixes = Arc::new(Mutex::new(Vec::new()));
            let capture = prefixes.clone();
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
                    super::super::super::events::enable(ctx);
                    super::super::super::events::initial_host(ctx, Some(1), None);
                    engine::driver(ctx).after_save = Some(Box::new(move |s, task, completed| {
                        if !completed
                            && s.tasks[task as usize]
                                .as_ref()
                                .unwrap()
                                .context_home
                                .is_some()
                        {
                            observe.saved.store(true, Ordering::Release);
                        }
                    }));
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    assert_eq!(
                        concurrent::request(ctx, &*setup, Request::Stdout { io: 1 }).unwrap(),
                        Ok(Completion::Handle(Ok(11)))
                    );
                    let mut scope = 0;
                    let mut handle = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        crate::STATUS_OK
                    );
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        16,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env)
                        .cast::<u64>()
                        .write(u64::from(canceled));
                    assert_eq!(
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut handle,
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
                            &mut handle,
                            result.as_mut_ptr().cast(),
                            16
                        ),
                        crate::STATUS_OK
                    );
                    assert_eq!(result, [1, 7]);
                    *capture.lock().unwrap() = engine::with_driver(ctx, |d| {
                        d.events
                            .as_ref()
                            .unwrap()
                            .canonical
                            .iter()
                            .filter_map(|event| match &event.outcome {
                                super::super::super::events::CanonicalOutcome::HostOutput(
                                    bytes,
                                ) => Some(bytes.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    });
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        crate::STATUS_OK
                    );
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert!(host.saved.load(Ordering::Acquire));
            assert_eq!(host.calls.load(Ordering::Acquire), usize::from(!canceled));
            assert_eq!(
                *host.bytes.lock().unwrap(),
                if canceled { b"".as_slice() } else { b"xyz" }
            );
            assert_eq!(
                *prefixes.lock().unwrap(),
                vec![if canceled { vec![] } else { b"x".to_vec() }],
                "written prefix {width}/{canceled}"
            );
            assert_eq!(host.finishes.load(Ordering::Acquire), 2);
        }
    }
}
