use super::super::*;
use crate::io::{Adapter, Ready, UnitResult};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Probe {
    entered: Vec<u64>,
    finished: Vec<u64>,
    valid_attempts: usize,
}
struct Host(Arc<Mutex<Probe>>);
impl Adapter for Host {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn enter(&mut self, token: u64) {
        self.0.lock().unwrap().entered.push(token);
    }
    fn finish(&mut self, token: u64) {
        self.0.lock().unwrap().finished.push(token);
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        match request {
            Request::SleepUntil { clock: 99, .. } => Err(AdapterError::InvalidHandle(99)),
            Request::SleepUntil { clock: 11, .. } => {
                let mut probe = self.0.lock().unwrap();
                probe.valid_attempts += 1;
                if probe.valid_attempts == 1 {
                    Err(AdapterError::Blocked)
                } else {
                    Ok(Completion::Wait(Ok(())))
                }
            }
            _ => unreachable!(),
        }
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        Ok(Ready::Every)
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        Ok(true)
    }
}

// A real home/replay path must remain reusable after errors as well as after
// a saved wait; all three operations use their original token once.
#[test]
fn repeated_home_refusals_leave_the_next_parked_completion_and_output_intact() {
    for width in [1, 4] {
        let probe = Arc::new(Mutex::new(Probe::default()));
        let mut run = crate::host::BodyRun::new(width, true);
        run.adapter = Some(Box::new(Host(probe.clone())));
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                )
            },
            |ctx| unsafe {
                for (clock, status) in [
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
                        status
                    );
                    assert_eq!(output.tag, if status == crate::STATUS_OK { 0 } else { 99 });
                    let _role = role::Guard::new(ctx);
                    assert!((*ctx).pending_ordered_host.is_none());
                    assert!(engine::driver(ctx).in_operations.is_empty());
                    assert!(engine::driver(ctx).host_ops.is_empty());
                    assert_eq!(
                        (*ctx)
                            .run_ref()
                            .host_waits
                            .load(std::sync::atomic::Ordering::Acquire),
                        0
                    );
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        let probe = probe.lock().unwrap();
        assert_eq!(probe.entered, [1, 2, 3]);
        assert_eq!(probe.finished, [1, 2, 3]);
        assert_eq!(probe.valid_attempts, 2);
    }
}
