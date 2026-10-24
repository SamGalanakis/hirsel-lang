//! Home-only continuations of saved retirement polls and idle host readiness.
//! Adapter calls retain no Scheduler, Frontier or parked TaskCtx borrow.
use super::super::{Logical, Scheduler, Wait, handle, readiness, role};
use crate::{context::RunShared, stop::Stopped};
use std::sync::atomic::Ordering;

/// Called under the scheduler role after a saved frame is published. An
/// armed host wait alone does not give home work: it needs a saved retirement
/// poll or logical quiescence, exactly as in service's reservation predicate.
pub(in crate::tasks) fn needed_after_save(s: &Scheduler) -> bool {
    let Logical::Ordered(driver) = &s.logical else {
        return false;
    };
    !driver.host_poll
        && (driver
            .poll_after_save
            .is_some_and(|id| driver.physically_saved(id))
            || driver.quiet())
}

/// # Safety
/// Admission loop only; no Hirsel frame runs on this home stack.
pub(crate) unsafe fn service(run: *mut RunShared) -> bool {
    if !unsafe { (*run).ordered_mode.load(Ordering::Acquire) }
        || (unsafe { (*run).host_waits.load(Ordering::Acquire) } == 0
            && !unsafe { super::host::concurrent::pending(run) })
    {
        return false;
    }
    let idle = unsafe { crate::pool::busy(&*run) } == 0;
    let reserved = {
        let Some(_guard) = (unsafe { role::IdleGuard::try_new(run) }) else {
            return false;
        };
        let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
        let Logical::Ordered(driver) = &s.logical else {
            return false;
        };
        if driver.host_poll {
            return false;
        }
        let deferred = driver.poll_after_save;
        let ready = deferred.is_some_and(|id| driver.physically_saved(id));
        let idle = idle && driver.quiet();
        if !ready && !idle {
            return false;
        }
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        driver.host_poll = true;
        (deferred, idle)
    };
    match unsafe { poll(run, reserved.0.is_some(), reserved.1) } {
        Ok(completed) => completed,
        Err(stop) => {
            unsafe { (*run).control.request_inside(stop) };
            true
        }
    }
}

pub(super) unsafe fn poll(run: *mut RunShared, retiring: bool, idle: bool) -> Stopped<bool> {
    // An idle continuation follows the last retirement's completed poll. Wait
    // before querying readiness again, just as cooperative next_task does;
    // otherwise the same idle boundary invents an extra recorded point.
    let mut wait_first = !retiring;
    loop {
        if let Some(stop) = unsafe { (*run).control.stopped() } {
            return Err(stop);
        }
        if wait_first && !unsafe { crate::host_waits::wait_home(run) }? {
            let _guard = unsafe { role::IdleGuard::home(run) };
            let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
            complete(s, run, retiring);
            unsafe { super::super::free::check_quiet_without_sources(run) };
            return Ok(true);
        }
        let crate::host_waits::HomeReady { expired, hosts } =
            unsafe { crate::host_waits::ready_home(run) }?;
        let (candidates, observing) = {
            let _guard = unsafe { role::IdleGuard::home(run) };
            let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
            for scope in expired {
                super::events::expire(s, scope);
            }
            let mut hosts: Vec<_> = hosts
                .into_iter()
                .filter_map(|crate::host_waits::ReadyHost { task, token, deadline, ready }| {
                    if (token & super::host::concurrent::CONCURRENT != 0
                        && !unsafe { (*run).ordered_io.get().unwrap() }.valid(token, task, s.task(task).generation))
                        || !readiness::saved(s, task)
                        || !matches!(s.task(task).wait, Some(Wait::Host { token: t }) if t == token)
                    {
                        return None;
                    }
                    let shield = s.task(task).shield();
                    let cause = s.canceled_task(task, shield);
                    Some((deadline, s.task(task).parked_at, task, token, cause, ready))
                })
                .collect();
            hosts.sort_unstable_by_key(|&(deadline, parked, ..)| (deadline, parked));
            let Logical::Ordered(driver) = &s.logical else {
                unreachable!()
            };
            (hosts, driver.events.is_some())
        };
        let canceled: Vec<u64> = candidates
            .iter()
            .filter(|row| row.4.is_some())
            .map(|row| row.3)
            .collect();
        let batch = unsafe { crate::host_waits::process_batch_home(run, &canceled) }?;
        let mut ready = Vec::new();
        for (_, _, task, token, cause, available) in candidates {
            let available = available || batch.contains(token);
            let (done, attempt) = if observing {
                unsafe { crate::host_waits::poll_observed_home(run, token, cause, available) }?
            } else {
                (
                    unsafe { crate::host_waits::poll_home(run, token, cause, available) }?,
                    None,
                )
            };
            if let Some(attempt) = attempt {
                assert_eq!(attempt.task, task);
                let _guard = unsafe { role::IdleGuard::home(run) };
                let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
                let discovery = attempt.discovery.map_or(Ok(()), |d| {
                    if token & super::host::concurrent::CONCURRENT != 0 {
                        d.and_then(|d| {
                            super::events::discover_host_for(s, task, &attempt.result, &d)
                        })
                    } else {
                        super::events::home_retry_keys(s, task, attempt.keys.as_ref());
                        super::events::home_discovery(s, task, &attempt.result, d);
                        Ok(())
                    }
                });
                super::events::record_host_attempt(s, task, &attempt.result, !retiring);
                if discovery.is_err() && done {
                    let result = unsafe { crate::host_waits::completed_home(run, token) };
                    super::events::record_host_poll(s, task, result.result, !retiring);
                }
                discovery.map_err(|e| crate::stop::Stop::Refused(e.into()))?;
            }
            if done {
                if observing {
                    // The home service borrow ends before claiming IdleGuard;
                    // a saved request still owns its completion until wake.
                    let result = unsafe { crate::host_waits::completed_home(run, token) };
                    assert_eq!(result.task, task);
                    let _guard = unsafe { role::IdleGuard::home(run) };
                    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
                    super::events::record_host_poll(s, task, result.result, !retiring);
                }
                ready.push(task);
            }
        }
        {
            let _guard = unsafe { role::IdleGuard::home(run) };
            let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
            for task in ready {
                s.unpark(task);
            }
            if !unsafe { readiness::poll_ordered_after_hosts(s, crate::pool::has_slot_run(run)) } {
                let Logical::Ordered(driver) = &mut s.logical else {
                    unreachable!()
                };
                driver.host_poll = false;
                return Ok(false);
            }
            if !idle || !s.runnable.is_empty() {
                complete(s, run, retiring);
                unsafe { super::super::free::check_quiet(run) };
                return Ok(true);
            }
        }
        wait_first = true;
    }
}

fn complete(s: &mut Scheduler, run: *mut RunShared, retiring: bool) {
    let woken = std::mem::take(&mut s.runnable)
        .into_iter()
        .map(|task| handle(task, s.task(task).generation))
        .collect();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    driver.host_poll = false;
    if retiring {
        driver.frontier.as_mut().unwrap().complete_poll(woken);
        driver.poll_after_save = None;
    } else {
        driver.frontier.as_mut().unwrap().resume_idle(woken);
    }
    for id in driver.select() {
        let task = id as u32;
        let owner = s.task(task).owner;
        unsafe { crate::pool::free::schedule_saved(run, task, owner) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{Adapter, AdapterError, Binding, Completion, Ready, Request};
    use std::sync::{Arc, Mutex};

    struct Idle(Arc<Mutex<Vec<&'static str>>>);
    impl Adapter for Idle {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            unreachable!()
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            self.0.lock().unwrap().push("ready");
            Ok(Ready::Tokens(vec![]))
        }
        fn wait(&mut self) -> Result<bool, AdapterError> {
            self.0.lock().unwrap().push("wait");
            Err(AdapterError::Stopped)
        }
    }

    #[test]
    fn idle_after_a_completed_retirement_waits_before_the_next_readiness_point() {
        let calls = Arc::new(Mutex::new(vec![]));
        let mut binding = Binding::new(Idle(calls.clone()));
        let mut ctx = std::ptr::null_mut();
        unsafe {
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                0
            );
            binding.bind(&mut *ctx);
            super::super::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            let run = (*ctx).run_ptr();
            (*run).host_waits.store(1, Ordering::Release);
            assert!(poll(run, false, true).is_err());
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
        assert_eq!(*calls.lock().unwrap(), vec!["wait"]);
    }
}

#[cfg(test)]
mod notification_tests {
    use super::*;
    #[test]
    fn a_running_frontier_has_no_saved_home_continuation_to_service() {
        let mut s = Scheduler::default();
        s.logical = Logical::Ordered(super::super::driver::Driver::new(
            0,
            crate::ordered::Policy::admit(&Default::default(), None),
            crate::budget::stretch::Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
            crate::ordered::objects::Registry::default(),
        ));
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        assert_eq!(driver.select(), vec![0]);
        driver.claimed(0, 0);
        assert!(!needed_after_save(&s));
    }
}
