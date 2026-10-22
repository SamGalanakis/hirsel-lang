use super::*;
use crate::io::{Adapter, AdapterError, Completion, IoError, Request, UnitResult};
use crate::ordered::Policy;
use std::{
    cell::RefCell,
    io::Write,
    rc::Rc,
    sync::{Arc, Mutex},
};

struct Output {
    bytes: Vec<u8>,
    mode: u8,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = match self.mode {
            1 => {
                let left = 1usize.saturating_sub(self.bytes.len());
                if left == 0 {
                    return Err(std::io::Error::other("partial output"));
                }
                left.min(bytes.len())
            }
            3 => 0,
            _ => bytes.len(),
        };
        self.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.mode == 2 {
            Err(std::io::Error::other("flush failed"))
        } else {
            Ok(())
        }
    }
}

/// The non-Send wrapper and its observation must stay on admission's thread.
struct Home {
    real: Rc<RefCell<crate::io_real::RealIo>>,
    thread: std::thread::ThreadId,
    mode: u8,
}
impl Adapter for Home {
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        assert_eq!(self.thread, std::thread::current().id());
        self.real.borrow().preflight(request)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(self.thread, std::thread::current().id());
        self.real.borrow_mut().call(request)
    }
    fn print(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> Result<Completion, AdapterError> {
        assert_eq!(self.thread, std::thread::current().id());
        self.real.borrow_mut().print(handle, stderr, bytes)
    }
    fn print_observed(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        assert_eq!(self.thread, std::thread::current().id());
        let (result, count) = self.real.borrow_mut().print_observed(handle, stderr, bytes);
        (
            result,
            match self.mode {
                4 => None,
                5 => Some(bytes.len() as u64 + 1),
                _ => count,
            },
        )
    }
}

#[test]
fn home_output_canonical_prefixes_match_written_bytes_and_refuse_unknown_or_invalid_counts() {
    use super::super::events::CanonicalOutcome;
    for mode in 0..7 {
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                for retain_history in [false, true] {
                    let storage = Arc::new(Mutex::new(Output {
                        bytes: vec![],
                        mode,
                    }));
                    let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                    real.set_send_output(crate::host::SendSink::shared(storage.clone()));
                    real.set_send_stderr(crate::host::SendSink::shared(storage.clone()));
                    if !retain_history {
                        real.discard_history();
                    }
                    let real = Rc::new(RefCell::new(real));
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.adapter = Some(Box::new(Home {
                        real: real.clone(),
                        thread: std::thread::current().id(),
                        mode,
                    }));
                    let layout = super::tests::layout();
                    let layouts = [&layout as *const crate::Layout];
                    run.config.layouts = layouts.as_ptr();
                    run.config.layout_count = 1;
                    unsafe { run.use_physical_dispatch() };
                    let (outcome, trace) = run.run_traced(
                        move |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            engine::enable(ctx, policy);
                            super::super::events::enable(ctx);
                            super::super::events::initial_host(ctx, Some(1), None);
                        },
                        move |ctx| unsafe {
                            let _role = role::Guard::new(ctx);
                            let Ok(Completion::Handle(Ok(stdout))) =
                                crate::io::capability_call(ctx, Request::Stdout { io: 1 })
                            else {
                                panic!("stdout grant")
                            };
                            let Ok(Completion::Handle(Ok(stderr))) =
                                crate::io::capability_call(ctx, Request::Stderr { io: 1 })
                            else {
                                panic!("stderr grant")
                            };
                            if mode == 6 {
                                scheduler(ctx).cancel(0, 7);
                            }
                            let owner = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                24,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(owner).cast::<u64>().write(5);
                            std::ptr::copy_nonoverlapping(
                                [91, 0, 255, 7, 92].as_ptr(),
                                crate::alloc::payload(owner).add(8),
                                5,
                            );
                            for (write, handle) in [
                                (
                                    crate::io::hirsel_rt_v2_out_write
                                        as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
                                    stdout,
                                ),
                                (crate::io::hirsel_rt_v2_err_write, stderr),
                            ] {
                                let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
                                assert_eq!(
                                    write(
                                        ctx,
                                        std::ptr::null_mut(),
                                        result.as_mut_ptr(),
                                        &handle,
                                        owner,
                                        1,
                                        3
                                    ),
                                    STATUS_OK
                                );
                                let expected = match mode {
                                    1..=3 => Err(IoError::Fault(1)),
                                    6 => Err(IoError::Canceled(7)),
                                    _ => Ok(()),
                                };
                                assert_eq!(result.assume_init(), UnitResult::of(expected));
                            }
                            std::ptr::write_bytes(crate::alloc::payload(owner).add(8), 0, 5);
                            crate::release::hirsel_rt_v2_release(
                                ctx,
                                owner,
                                2,
                                std::ptr::null(),
                                0,
                                0,
                            );
                            let log = engine::driver(ctx).events.as_ref().unwrap();
                            assert_eq!(log.host_completions.len(), 4);
                            assert_eq!(log.host_initials.len(), 4);
                            if matches!(mode, 4 | 5) {
                                assert!(log.canonical().is_err());
                            } else {
                                let bytes = log
                                    .canonical()
                                    .unwrap()
                                    .iter()
                                    .filter_map(|event| {
                                        if let CanonicalOutcome::HostOutput(bytes) = &event.outcome
                                        {
                                            Some(bytes.as_slice())
                                        } else {
                                            None
                                        }
                                    })
                                    .flatten()
                                    .copied()
                                    .collect::<Vec<_>>();
                                let expected: &[u8] = match mode {
                                    1 => &[0],
                                    3 | 6 => &[],
                                    _ => &[0, 255, 7, 0, 255, 7],
                                };
                                assert_eq!(bytes, expected);
                            }
                            STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(STATUS_OK));
                    assert_eq!(outcome.pending, 0);
                    let expected: &[u8] = match mode {
                        1 => &[0],
                        3 | 6 => &[],
                        _ => &[0, 255, 7, 0, 255, 7],
                    };
                    assert_eq!(storage.lock().unwrap().bytes, expected);
                    if matches!(mode, 4 | 5) {
                        assert!(trace.is_err());
                    } else {
                        let bytes = trace.unwrap().bytes().to_vec();
                        if let Some(reference) = &reference {
                            assert_eq!(&bytes, reference);
                        } else {
                            reference = Some(bytes);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn replay_home_output_observes_only_validated_recorded_prefixes_without_consuming_a_mismatch() {
    let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
    let storage = Arc::new(Mutex::new(Output {
        bytes: vec![],
        mode: 1,
    }));
    real.set_send_output(crate::host::SendSink::shared(storage));
    let request = Request::Stdout { io: 1 };
    let Ok(Completion::Handle(Ok(stdout))) = real.call(request.clone()) else {
        panic!("stdout")
    };
    let (result, count) = real.print_observed(stdout, false, b"abc");
    assert_eq!(count, Some(1));
    assert!(matches!(result, Ok(Completion::Unit(Err(_)))));
    let mut replay = crate::io_sim::SimIo::new(
        crate::io::Tree::new(Default::default()).unwrap(),
        real.tape().to_vec(),
    );
    assert_eq!(replay.call(request), Ok(Completion::Handle(Ok(stdout))));
    assert!(replay.print_observed(stdout, false, b"wrong").0.is_err());
    assert!(replay.stdout().is_empty());
    assert_eq!(
        replay.print_observed(stdout, false, b"abc"),
        (result, Some(1))
    );
    assert_eq!(replay.stdout(), b"a");
}
