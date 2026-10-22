//! The borrowed print fast path must publish the same owned request/result
//! observations as host_waits::issue, without extending the caller's borrow.
use super::tests::layout;
use super::*;
use crate::io::{Adapter, AdapterError, Completion, IoError, Request, UnitResult};
use crate::ordered::Policy;
use crate::ordered::footprint::ObjectId;
use std::sync::{Arc, Mutex};

struct Output {
    home: std::thread::ThreadId,
    seen: Arc<Mutex<Vec<(bool, Vec<u8>)>>>,
}
impl Adapter for Output {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        panic!("borrowed output unexpectedly used owned request: {request:?}")
    }
    fn print(&mut self, _: u64, stderr: bool, bytes: &[u8]) -> Result<Completion, AdapterError> {
        assert_eq!(self.home, std::thread::current().id());
        self.seen.lock().unwrap().push((stderr, bytes.to_vec()));
        Ok(Completion::Unit(if stderr {
            Err(IoError::Fault(7))
        } else {
            Ok(())
        }))
    }
}

#[test]
fn borrowed_output_observations_own_visible_bytes_and_keep_each_result_ordinal() {
    let layout = layout();
    let layouts = [&layout as *const crate::Layout];
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [
            hirsel_run_config::Reorder::Off,
            hirsel_run_config::Reorder::Auto,
        ] {
            let sink = 0x7123_4567_8900_0000 + u64::from(width) * 16;
            let seen = Arc::new(Mutex::new(Vec::new()));
            let events_out = Arc::new(Mutex::new(Vec::new()));
            let captured = events_out.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            run.adapter = Some(Box::new(Output {
                home: std::thread::current().id(),
                seen: seen.clone(),
            }));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    let mut policy = Policy::admit(&Default::default(), None);
                    policy.reorder = reorder;
                    engine::enable(ctx, policy);
                    super::super::events::enable(ctx);
                    for word in [sink, sink + 1] {
                        engine::driver(ctx).objects.bind_erased(
                            crate::ordered::objects::Kind::Capability,
                            word,
                            ObjectId {
                                creator: vec![],
                                ordinal: 77,
                            },
                            Default::default(),
                        );
                    }
                },
                move |ctx| unsafe {
                    let bytes = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        24,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(bytes).cast::<u64>().write(6);
                    std::ptr::copy_nonoverlapping(
                        [91, 0, 255, 7, 92, 93].as_ptr(),
                        crate::alloc::payload(bytes).add(8),
                        6,
                    );
                    for (stderr, handle) in [(false, sink), (true, sink + 1)] {
                        let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
                        let write = if stderr {
                            crate::io::hirsel_rt_v2_err_write
                        } else {
                            crate::io::hirsel_rt_v2_out_write
                        };
                        assert_eq!(
                            write(
                                ctx,
                                std::ptr::null_mut(),
                                result.as_mut_ptr(),
                                &handle,
                                bytes,
                                1,
                                3
                            ),
                            STATUS_OK
                        );
                        assert_eq!(
                            result.assume_init(),
                            UnitResult::of(if stderr {
                                Err(IoError::Fault(7))
                            } else {
                                Ok(())
                            })
                        );
                    }
                    // Overwrite and release the entire source before inspecting
                    // evidence; neither outside-slice bytes nor its owner survive.
                    std::ptr::write_bytes(crate::alloc::payload(bytes).add(8), 0, 6);
                    crate::release::hirsel_rt_v2_release(ctx, bytes, 2, std::ptr::null(), 0, 0);
                    let log = engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(log.host_requests.len(), 2);
                    assert_eq!(log.host_completions.len(), 2);
                    assert!(log.host_polls.is_empty());
                    let mut rows = Vec::new();
                    for (index, done) in log.host_completions.iter().enumerate() {
                        assert!(done.task.is_empty());
                        assert_eq!(done.ordinal, index as u64);
                        let request = log.host_requests[&(done.task.clone(), done.ordinal)]
                            .clone()
                            .unwrap();
                        assert_eq!(
                            request.handles,
                            [super::super::events::HostHandleId {
                                operation: ObjectId {
                                    creator: vec![],
                                    ordinal: 77
                                },
                                field: 0,
                            }]
                        );
                        assert_eq!(
                            crate::tape_wire::decode::<Request>(&request.payload).unwrap(),
                            if index == 0 {
                                Request::Print {
                                    stdout: 0,
                                    bytes: vec![0, 255, 7],
                                }
                            } else {
                                Request::PrintStderr {
                                    stderr: 0,
                                    bytes: vec![0, 255, 7],
                                }
                            }
                        );
                        let result = done.observation.clone().unwrap();
                        assert!(result.handles.is_empty());
                        assert_eq!(
                            crate::tape_wire::decode::<Result<Completion, AdapterError>>(
                                &result.payload
                            )
                            .unwrap(),
                            done.result
                        );
                        rows.push((done.ordinal, request, result));
                    }
                    *captured.lock().unwrap() = rows;
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(
                *seen.lock().unwrap(),
                [(false, vec![0, 255, 7]), (true, vec![0, 255, 7])]
            );
            let rows = events_out.lock().unwrap().clone();
            if let Some(reference) = &reference {
                assert_eq!(&rows, reference);
            } else {
                reference = Some(rows);
            }
        }
    }
}

#[test]
fn borrowed_output_cancellation_records_one_owned_request_without_printing() {
    let layout = layout();
    let layouts = [&layout as *const crate::Layout];
    for width in [1, 2, 4, 8] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        run.adapter = Some(Box::new(Output {
            home: std::thread::current().id(),
            seen: seen.clone(),
        }));
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(ctx, Policy::admit(&Default::default(), None));
                super::super::events::enable(ctx);
                engine::driver(ctx).objects.bind_erased(
                    crate::ordered::objects::Kind::Capability,
                    99,
                    ObjectId {
                        creator: vec![],
                        ordinal: 77,
                    },
                    Default::default(),
                );
            },
            |ctx| unsafe {
                let bytes =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(bytes).cast::<u64>().write(3);
                std::ptr::copy_nonoverlapping(
                    [0, 255, 7].as_ptr(),
                    crate::alloc::payload(bytes).add(8),
                    3,
                );
                {
                    let _role = role::Guard::new(ctx);
                    scheduler(ctx).cancel(0, CANCEL_REQUESTED);
                }
                for write in [
                    crate::io::hirsel_rt_v2_out_write,
                    crate::io::hirsel_rt_v2_err_write,
                ] {
                    let mut result = std::mem::MaybeUninit::<UnitResult>::uninit();
                    assert_eq!(
                        write(
                            ctx,
                            std::ptr::null_mut(),
                            result.as_mut_ptr(),
                            &99,
                            bytes,
                            0,
                            3
                        ),
                        STATUS_OK
                    );
                    assert_eq!(
                        result.assume_init(),
                        UnitResult::of(Err(IoError::Canceled(CANCEL_REQUESTED)))
                    );
                }
                crate::release::hirsel_rt_v2_release(ctx, bytes, 2, std::ptr::null(), 0, 0);
                let log = engine::driver(ctx).events.as_ref().unwrap();
                assert_eq!(log.host_requests.len(), 2);
                assert_eq!(log.host_completions.len(), 2);
                for (index, done) in log.host_completions.iter().enumerate() {
                    assert_eq!(done.ordinal, index as u64);
                    assert!(log.host_requests[&(done.task.clone(), done.ordinal)].is_some());
                    assert_eq!(
                        done.result,
                        Ok(Completion::Unit(Err(IoError::Canceled(CANCEL_REQUESTED))))
                    );
                    assert!(done.observation.is_some());
                }
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert!(seen.lock().unwrap().is_empty());
    }
}
