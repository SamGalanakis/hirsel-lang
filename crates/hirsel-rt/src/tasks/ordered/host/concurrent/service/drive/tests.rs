//! Tokio fs_file_join_error.rs:21-68 repeated-failure shape, using reported
//! adapter failure rather than Tokio's panic/canceled-job policy.
use super::super::*;
use crate::io::UnitResult;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct FailedJobs {
    tokens: Mutex<Vec<u64>>,
    finished: AtomicUsize,
}
impl ConcurrentAdapter for FailedJobs {
    fn blocking(&self, _: &Request) -> bool {
        true
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().name(), Some("hirsel-host-job"));
        self.tokens.lock().unwrap().push(token);
        match request {
            Request::SleepUntil { clock: 99, .. } => Err(AdapterError::InvalidHandle(99)),
            Request::SleepUntil { clock: 11, .. } => Ok(Completion::Wait(Ok(()))),
            _ => unreachable!(),
        }
    }
    fn finish(&self, _: u64) {
        self.finished.fetch_add(1, Ordering::Release);
    }
}

#[test]
fn failed_real_jobs_convert_errors_without_poisoning_repeated_or_independent_operations() {
    for width in [1, 4] {
        let adapter = Arc::new(FailedJobs::default());
        let mut run = crate::host::BodyRun::new(width, true);
        run.concurrent_adapter = Some(adapter.clone());
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                )
            },
            |ctx| unsafe {
                for (clock, expected) in [
                    (99, crate::STATUS_FAILURE),
                    (99, crate::STATUS_FAILURE),
                    (11, crate::STATUS_OK),
                ] {
                    let mut output = UnitResult {
                        tag: 99,
                        error: 99,
                        code: 99,
                    };
                    assert_eq!(
                        crate::io::hirsel_rt_v2_clock_sleep_until(
                            ctx,
                            std::ptr::null_mut(),
                            &mut output,
                            &clock,
                            &10
                        ),
                        expected
                    );
                    assert_eq!(
                        output.tag,
                        if expected == crate::STATUS_OK { 0 } else { 99 }
                    );
                    let _role = role::Guard::new(ctx);
                    assert!((*ctx).pending_ordered_host.is_none());
                    assert!(engine::driver(ctx).in_operations.is_empty());
                    assert!(engine::driver(ctx).host_ops.is_empty());
                    assert!(engine::driver(ctx).head_job.is_none());
                    assert!(!super::super::super::pending((*ctx).run_ptr()));
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(*adapter.tokens.lock().unwrap(), [1, 2, 3]);
        assert_eq!(adapter.finished.load(Ordering::Acquire), 3);
    }
}

#[derive(Default)]
struct StopAfterInitial {
    run: std::sync::atomic::AtomicPtr<crate::context::RunShared>,
}
impl ConcurrentAdapter for StopAfterInitial {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn keys(&self, request: &Request) -> crate::io_keys::KeySet {
        use crate::io_keys::{Key, KeySet, Mode, ROOT};
        KeySet::new(vec![match request {
            Request::MonotonicClock { .. } => (ROOT, Mode::Shared),
            Request::SleepUntil { .. } => (Key(11), Mode::Exclusive),
            _ => unreachable!(),
        }])
    }
    fn identity(&self, word: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        (word == 11).then_some((crate::io_keys::Key(11), crate::io_keys::Kind::Other))
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, _: u64, request: Request) -> Result<Completion, AdapterError> {
        Ok(match request {
            Request::MonotonicClock { .. } => Completion::ClockGrant(Ok(11)),
            Request::SleepUntil { .. } => Completion::Wait(Ok(())),
            _ => unreachable!(),
        })
    }
    fn finish(&self, token: u64) {
        if token == 2 {
            unsafe {
                (*self.run.load(Ordering::Acquire))
                    .control
                    .request_inside(Stop::Requested)
            };
        }
    }
}

#[test]
fn an_initial_effect_keeps_its_selected_evidence_when_finish_requests_a_terminal_cut() {
    for width in [1, 4] {
        let adapter = Arc::new(StopAfterInitial::default());
        let setup = adapter.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.concurrent_adapter = Some(adapter);
        unsafe { run.use_physical_dispatch() };
        let (outcome, trace) = run.run_traced_after_bounded_stop(
            move |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
                crate::tasks::ordered::events::enable(ctx);
                crate::tasks::ordered::events::initial_host(ctx, Some(1), None);
                setup.run.store((*ctx).run_ptr(), Ordering::Release);
            },
            |ctx| unsafe {
                assert_eq!(
                    crate::io::capability_call(ctx, Request::MonotonicClock { io: 1 }),
                    Ok(Completion::ClockGrant(Ok(11)))
                );
                let _ = crate::io::capability_call(
                    ctx,
                    Request::SleepUntil {
                        clock: 11,
                        deadline: 10,
                    },
                );
                panic!("a selected initial effect returned after the terminal cut")
            },
        );
        assert_eq!(outcome.ended, Err(Stop::Requested));
        let trace = trace.expect("selected initial outcome retained before stop observation");
        let rows: Vec<serde_json::Value> = std::str::from_utf8(trace.bytes())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            rows.iter()
                .filter(|row| row["outcome"]["kind"] == "host_result")
                .count(),
            2
        );
    }
}
