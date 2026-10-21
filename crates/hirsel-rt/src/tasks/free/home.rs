//! Home timer readiness for Free saved-frame waits. Follows the Ordered
//! context-free home service: snapshot under the role, adapter calls without
//! the role, then generation-checked metadata merge and saved-slot readiness.
use super::super::{Logical, readiness, role};
use crate::context::RunShared;
use std::sync::atomic::Ordering;

/// # Safety
/// Only the admission loop calls this, on the binding's home thread. No
/// adapter or parked TaskCtx borrow is held across a scheduler-role guard.
pub(crate) unsafe fn service(run: *mut RunShared) -> bool {
    if !unsafe { (*run).free_mode } || unsafe { (*run).host_waits.load(Ordering::Acquire) } == 0 {
        return false;
    }
    let idle = unsafe { crate::pool::busy(&*run) } == 0;
    let (scopes, idle) = {
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
        (scopes, idle)
    };
    if scopes.is_empty() {
        return false;
    }
    // The home thread serializes this call with all worker home bridges.
    // Never retain the scheduler role while an adapter blocks or calls back.
    let result = unsafe { crate::host_waits::timers_home(run, idle) };
    let _guard = unsafe { role::IdleGuard::new(run) };
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
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    let changed = !expired.is_empty();
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
    unsafe { readiness::poll_free(s) };
    unsafe { super::publish_ready(s, run) };
    if !sources {
        unsafe { super::check_quiet_without_sources(run) };
    } else {
        unsafe { super::check_quiet(run) };
    }
    // If no timer fired while workers run, let their queued home calls run.
    // Repeated readiness probes must never starve a synchronous bridge.
    changed || idle
}
