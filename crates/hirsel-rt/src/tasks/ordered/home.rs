//! Home-only continuation of saved retirement polls and idle timer readiness.
//! Adapter calls never retain Scheduler, Frontier or a parked TaskCtx borrow.
use super::super::{Logical, Scheduler, handle, readiness, role};
use crate::context::RunShared;
use std::sync::atomic::Ordering;

/// Serve one reserved timer poll, or an idle wait after every worker saved.
/// # Safety
/// The admission loop owns `run`; no Hirsel frame runs on this home stack.
pub(crate) unsafe fn service(run: *mut RunShared) -> bool {
    if !unsafe { (*run).ordered_mode.load(Ordering::Acquire) }
        || unsafe { (*run).host_waits.load(Ordering::Acquire) } == 0
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
    // Only home owns the binding. Pure siblings can run while this readiness
    // call executes; the reserved retirement forbids any other host effect.
    let result = unsafe { crate::host_waits::timers_home(run, reserved.1) };
    let _guard = unsafe { role::IdleGuard::new(run) };
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    driver.host_poll = false;
    if unsafe { (*run).control.stopped().is_some() } {
        return true;
    }
    let (expired, sources) = match result {
        Ok(result) => result,
        Err(stop) => {
            unsafe { (*run).control.request_inside(stop) };
            return true;
        }
    };
    for scope in expired {
        s.expire(scope);
    }
    // Every destination is saved; runtime readiness preserves serial park
    // order and checks child frames before moving a held result.
    if !unsafe { readiness::poll_ordered(s) } {
        return false;
    }
    let woken = std::mem::take(&mut s.runnable)
        .into_iter()
        .map(|task| handle(task, s.task(task).generation))
        .collect();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    if reserved.0.is_some() {
        driver.frontier.as_mut().unwrap().complete_poll(woken);
        driver.poll_after_save = None;
    } else {
        driver.frontier.as_mut().unwrap().resume_idle(woken);
    }
    publish(s, run);
    if !sources {
        unsafe { super::super::free::check_quiet_without_sources(run) };
    } else {
        unsafe { super::super::free::check_quiet(run) };
    }
    true
}

fn publish(s: &mut Scheduler, run: *mut RunShared) {
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    for id in driver.select() {
        let task = id as u32;
        let owner = s.task(task).owner;
        unsafe { crate::pool::free::schedule_saved(run, task, owner) };
    }
}
