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
    let (scopes, waits, canceled, idle) = {
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
        let scopes = s
            .scopes
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let row = row.as_ref()?;
                let deadline = row.deadline.as_ref()?;
                deadline
                    .token
                    .map(|token| (index as u32, row.generation, token))
            })
            .collect::<Vec<_>>();
        let waits = s
            .tasks
            .iter()
            .enumerate()
            .filter_map(|(task, row)| {
                let row = row.as_ref()?;
                let super::super::Wait::Host { token } = row.wait? else {
                    return None;
                };
                readiness::saved(s, task as u32).then_some((task as u32, row.generation, token))
            })
            .collect::<Vec<_>>();
        let canceled = waits
            .iter()
            .filter_map(|&(task, _, token)| {
                let shield = s.task(task).shield();
                s.canceled_task(task, shield).map(|_| token)
            })
            .collect::<Vec<_>>();
        (scopes, waits, canceled, idle)
    };
    if scopes.is_empty() && waits.is_empty() {
        return false;
    }
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
    let mut changed = !expired.is_empty();
    for index in expired {
        let valid = scopes.iter().any(|&(scope, generation, token)| {
            scope == index
                && s.scopes
                    .get(index as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|row| {
                        row.generation == generation
                            && row
                                .deadline
                                .as_ref()
                                .is_some_and(|d| d.token == Some(token))
                    })
        });
        if valid {
            s.expire(index);
        }
    }
    let mut notified = Vec::new();
    for host in hosts {
        let valid = host.ready
            && waits.iter().any(|&(task, generation, token)| {
                task == host.task && token == host.token && s.tasks.get(task as usize)
                .and_then(Option::as_ref).is_some_and(|row| row.generation == generation
                    && matches!(row.wait, Some(super::super::Wait::Host { token: t }) if t == token)
                    && readiness::saved(s, task))
            });
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
    unsafe { crate::host_waits::native_notified_home(run, &notified) };
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
