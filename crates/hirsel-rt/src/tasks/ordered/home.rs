//! Home-only continuations of saved retirement polls and idle host readiness.
//! Adapter calls retain no Scheduler, Frontier or parked TaskCtx borrow.
use super::super::{Logical, Scheduler, Wait, handle, readiness, role};
use crate::{context::RunShared, stop::Stopped};
use std::sync::atomic::Ordering;

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
        let ready = deferred.is_some_and(|id| readiness::saved(s, id as u32));
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
    loop {
        if let Some(stop) = unsafe { (*run).control.stopped() } {
            return Err(stop);
        }
        let crate::host_waits::HomeReady { expired, hosts } =
            unsafe { crate::host_waits::ready_home(run) }?;
        let (candidates, observing) = {
            let _guard = unsafe { role::IdleGuard::new(run) };
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
        let mut ready = Vec::new();
        for (_, _, task, token, cause, available) in candidates {
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
                let _guard = unsafe { role::IdleGuard::new(run) };
                let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
                let discovery = attempt.discovery.map_or(Ok(()), |d| {
                    d.and_then(|d| super::events::discover_host_for(s, task, &attempt.result, &d))
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
                    let _guard = unsafe { role::IdleGuard::new(run) };
                    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
                    super::events::record_host_poll(s, task, result.result, !retiring);
                }
                ready.push(task);
            }
        }
        {
            let _guard = unsafe { role::IdleGuard::new(run) };
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
        if !unsafe { crate::host_waits::wait_home(run) }? {
            let _guard = unsafe { role::IdleGuard::new(run) };
            let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
            complete(s, run, retiring);
            unsafe { super::super::free::check_quiet_without_sources(run) };
            return Ok(true);
        }
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
