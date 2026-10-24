//! Home readiness for Free saved-frame waits. Follows the Ordered
//! context-free home service: snapshot under the role, adapter calls without
//! the role, then generation-checked metadata merge and saved-slot readiness.
use super::super::{Logical, readiness, role};
use crate::context::RunShared;
use std::sync::atomic::Ordering;

/// # Safety
/// Only the admission loop calls this, on the binding's home thread. No
/// adapter or parked TaskCtx borrow is held across a scheduler-role guard.
pub(crate) unsafe fn service(run: *mut RunShared) -> bool {
    if unsafe { super::host::concurrent::service(run) } {
        return true;
    }
    if !unsafe { (*run).free_mode } || unsafe { (*run).host_waits.load(Ordering::Acquire) } == 0 {
        return false;
    }
    let idle = unsafe { crate::pool::busy(&*run) } == 0;
    let (canceled, idle) = {
        let Some(_guard) = (unsafe { role::IdleGuard::try_new(run) }) else {
            return false;
        };
        let Some(s) = (unsafe { (*(*run).tasks.get()).as_deref_mut() }) else {
            return false;
        };
        if !matches!(s.logical, Logical::Free(_)) {
            return false;
        }
        // A timer can wake an owner or a child whose physical save is still
        // pending. Merge only runtime metadata here; poll_free gates slots.
        let idle = idle
            && s.runnable.is_empty()
            && s.tasks.iter().flatten().all(|row| {
                row.state != super::super::State::Ready
                    && row.dispatch.as_ref().is_none_or(|frame| frame.is_saved())
            });
        let waits = s.tasks.iter().enumerate().filter_map(|(task, row)| {
            let row = row.as_ref()?;
            let super::super::Wait::Host { token } = row.wait? else {
                return None;
            };
            Some((task as u32, token))
        });
        let canceled = waits
            .filter_map(|(task, token)| {
                let shield = s.tasks[task as usize].as_ref().unwrap().shield();
                s.canceled_task(task, shield).map(|_| token)
            })
            .collect::<Vec<_>>();
        (canceled, idle)
    };
    // The home thread serializes this call with all worker home bridges.
    // Never retain the scheduler role while an adapter blocks or calls back.
    let result = unsafe { poll_home(run, idle, &canceled) };
    let _guard = unsafe { role::IdleGuard::home(run) };
    if unsafe { (*run).control.stopped().is_some() } {
        return true;
    }
    let (crate::host_waits::HomeReady { expired, hosts }, sources) = match result {
        Ok(result) => result,
        Err(stop) => {
            unsafe { (*run).control.request_inside(stop) };
            return true;
        }
    };
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    let mut changed = false;
    let mut acknowledged = Vec::new();
    for expiry in expired {
        if valid_expiry(s, &expiry) {
            s.expire(expiry.scope);
            changed = true;
            acknowledged.push(expiry.token);
        } else if stale_expiry(s, &expiry) {
            acknowledged.push(expiry.token);
        }
    }
    let canceled: Vec<u64> = hosts.iter().filter_map(|host| {
        let shield = s.task(host.task).shield();
        s.canceled_task(host.task, shield).map(|_| host.token)
    }).collect();
    drop(_guard);
    let batch = unsafe { crate::host_waits::process_batch_home(run, &canceled) };
    let _guard = unsafe { role::IdleGuard::home(run) };
    let batch = match batch {
        Ok(ready) => ready,
        Err(stop) => {
            unsafe { (*run).control.request_inside(stop) };
            return true;
        }
    };
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    let mut notified = Vec::new();
    for mut host in hosts {
        host.ready |= batch.contains(host.token);
        let valid = host.ready && s.tasks.get(host.task as usize)
            .and_then(Option::as_ref).is_some_and(|row| row.generation == host.generation
                && matches!(row.wait, Some(super::super::Wait::Host { token }) if token == host.token)
                && readiness::saved(s, host.task));
        if valid {
            s.unpark(host.task);
            notified.push(host.token);
            changed = true;
        }
    }
    unsafe { readiness::poll_free(s) };
    unsafe { super::publish_ready(s, run) };
    if !sources {
        unsafe { super::check_quiet_without_sources(run) };
    } else {
        unsafe { super::check_quiet(run) };
    }
    drop(_guard);
    unsafe {
        crate::host_waits::native_notified_home(run, &notified);
        crate::host_waits::expired_ack_home(run, &acknowledged);
    };
    // If no event fired while workers run, let their queued home calls run.
    // Repeated readiness probes must never starve a synchronous bridge.
    changed || idle
}

/// Poll the same home driver as scope timers, without issuing parked native
/// requests. The saved owner supplies the next recorded bounded attempt.
unsafe fn poll_home(
    run: *mut RunShared,
    idle: bool,
    canceled: &[u64],
) -> crate::stop::Stopped<(crate::host_waits::HomeReady, bool)> {
    let ready = unsafe { crate::host_waits::native_ready_home(run, canceled) }?;
    if !idle || !ready.expired.is_empty() || ready.hosts.iter().any(|host| host.ready) {
        return Ok((ready, true));
    }
    // A deadline or another source can change cancellation while home waits.
    // Return to the role snapshot before deciding which owners to wake.
    let sources = if unsafe { super::host::concurrent::pending(run) } {
        unsafe { super::host::concurrent::wait(run) }?
    } else {
        unsafe { crate::host_waits::wait_home(run) }?
    };
    Ok((ready, sources))
}

/// Poll a home-owned runner child's ordinary adapter without waiting for it.
/// Its independent Free dispatcher owns saved frames, while the execution's
/// shared readiness driver waits for all child sources together. No parent
/// task/adapter or canonical parent host phase is borrowed here.
/// # Safety
/// The child's sole home owner, outside program code and all adapter loans.
pub(crate) unsafe fn service_child(run: *mut RunShared) -> crate::stop::Stopped<bool> {
    if unsafe { (*run).host_waits.load(Ordering::Acquire) } == 0 {
        return Ok(false);
    }
    let waits = {
        let Some(_guard) = (unsafe { role::IdleGuard::try_new(run) }) else {
            return Ok(false);
        };
        let Some(s) = (unsafe { (*(*run).tasks.get()).as_deref_mut() }) else {
            return Ok(false);
        };
        let mut waits = Vec::new();
        for (task, row) in s.tasks.iter().enumerate() {
            let Some(row) = row else { continue };
            let Some(super::super::Wait::Host { token }) = row.wait else {
                continue;
            };
            // Do not consume a readiness edge before its owner is saved.
            // The physical-save post wakes this execution again.
            if !readiness::saved(s, task as u32) {
                return Ok(false);
            }
            waits.push((task as u32, row.generation, token));
        }
        waits
    };
    let crate::host_waits::HomeReady { expired, hosts } =
        unsafe { crate::host_waits::ready_home(run) }?;
    let mut acknowledged = Vec::new();
    let candidates = {
        let _guard = unsafe { role::IdleGuard::home(run) };
        if unsafe { (*run).control.stopped().is_some() } {
            return Ok(false);
        }
        let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
        for expiry in expired {
            if valid_expiry(s, &expiry) {
                s.expire(expiry.scope);
                acknowledged.push(expiry.token);
            } else if stale_expiry(s, &expiry) {
                acknowledged.push(expiry.token);
            }
        }
        hosts.into_iter().filter_map(|host| {
            let (_, generation, _) = waits.iter().find(|&&(task, generation, token)| {
                task == host.task && token == host.token
                    && s.tasks.get(task as usize).and_then(Option::as_ref)
                        .is_some_and(|row| row.generation == generation
                            && matches!(row.wait, Some(super::super::Wait::Host { token: t }) if t == token)
                            && readiness::saved(s, task))
            })?;
            let shield = s.task(host.task).shield();
            let cause = s.canceled_task(host.task, shield);
            Some((host, *generation, cause))
        }).collect::<Vec<_>>()
    };
    unsafe { crate::host_waits::expired_ack_home(run, &acknowledged) };
    let mut completed = Vec::new();
    for (host, generation, cause) in candidates {
        if unsafe { crate::host_waits::poll_home(run, host.token, cause, host.ready) }? {
            completed.push((host.task, generation, host.token));
        }
    }
    let _guard = unsafe { role::IdleGuard::home(run) };
    if unsafe { (*run).control.stopped().is_some() } {
        return Ok(false);
    }
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    let mut changed = false;
    for (task, generation, token) in completed {
        if s.tasks
            .get(task as usize)
            .and_then(Option::as_ref)
            .is_some_and(|row| {
                row.generation == generation
                    && matches!(row.wait, Some(super::super::Wait::Host { token: t }) if t == token)
                    && readiness::saved(s, task)
            })
        {
            s.unpark(task);
            changed = true;
        }
    }
    unsafe { readiness::poll_free(s) };
    unsafe { super::publish_ready(s, run) };
    unsafe { super::check_quiet(run) };
    Ok(changed)
}

pub(crate) fn runner_waiter(ctx: *mut crate::TaskCtx) -> (u32, u32) {
    let s = super::super::scheduler(ctx);
    (s.current, s.task(s.current).generation)
}

/// # Safety
/// Sole home callback, with no adapter borrow. Notifications retain physical
/// save preflight even if they arrive before the waiting fiber fully saves.
pub(crate) unsafe fn runner_wakes(run: *mut RunShared, waiters: &[(u32, u32)]) {
    let _guard = unsafe { role::IdleGuard::home(run) };
    let Some(s) = (unsafe { (*(*run).tasks.get()).as_deref_mut() }) else {
        return;
    };
    for &(task, generation) in waiters {
        if let Some(Some(row)) = s.tasks.get_mut(task as usize)
            && row.generation == generation
        {
            row.retried = true;
            s.wake(task);
        }
    }
    unsafe { readiness::poll_free(s) };
    unsafe { super::publish_ready(s, run) };
}

/// # Safety
/// A child run's admission owner, outside adapter calls and program code.
pub(crate) unsafe fn runner_cancel(run: *mut RunShared) {
    let _guard = unsafe { role::IdleGuard::home(run) };
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    s.cancel(super::super::ROOT, super::super::CANCEL_PARENT);
    unsafe { readiness::poll_free(s) };
    unsafe { super::publish_ready(s, run) };
}

fn valid_expiry(s: &super::super::Scheduler, expiry: &crate::host_waits::ExpiredScope) -> bool {
    s.scopes
        .get(expiry.scope as usize)
        .and_then(Option::as_ref)
        .is_some_and(|row| {
            row.generation == expiry.generation
                && row
                    .deadline
                    .as_ref()
                    .is_some_and(|d| d.token == Some(expiry.token))
        })
}

fn stale_expiry(s: &super::super::Scheduler, expiry: &crate::host_waits::ExpiredScope) -> bool {
    s.scopes
        .get(expiry.scope as usize)
        .and_then(Option::as_ref)
        .is_none_or(|row| row.generation != expiry.generation || row.deadline.is_none())
}
