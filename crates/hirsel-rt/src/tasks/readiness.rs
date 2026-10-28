//! Bounded runtime readiness shared by the physical-dispatch policies.
//!
//! No host bridge, TaskCtx borrow or switch is allowed under the idle guard.
//! Ordered mode additionally preserves serial park order and preflights saved
//! slots before applying any effects in a retirement poll.

use super::*;

pub(super) fn saved(s: &Scheduler, task: u32) -> bool {
    s.tasks[task as usize]
        .as_ref()
        .and_then(|row| row.dispatch.as_ref())
        .is_none_or(|frame| frame.is_saved())
}

/// All effects here are bounded runtime work under IdleGuard. In particular,
/// host readiness and pool allocation remain outside this callback.
/// # Safety
/// The role exclusively owns `s`; both sides of every slot copy are saved.
unsafe fn poll(s: &mut Scheduler, task: u32, pool_available: Option<bool>) -> Option<bool> {
    if !saved(s, task) {
        return Some(false);
    }
    let wait = s.task(task).wait?;
    Some(match wait {
        Wait::Join { child, slot, size } => {
            if !saved(s, child) {
                return Some(false);
            }
            match s.task(child).state {
                State::Holding => {
                    let held = s.task(child).slot.take().expect("a held result");
                    // SAFETY: both stacks were saved, with live Option images.
                    unsafe { deliver(held, slot, size) };
                    #[cfg(feature = "ownership-trace")]
                    ownership_result_transferred(s, task, child);
                    cpu_job_event(s.task(child), "transfer");
                    true
                }
                State::Done => true,
                State::Ready | State::Parked => false,
            }
        }
        Wait::Exit { scope } => {
            let children = s.scope(scope).children.clone();
            children.into_iter().all(|child| {
                saved(s, child) && matches!(s.task(child).state, State::Holding | State::Done)
            })
        }
        Wait::Receive { channel, slot } => {
            let shield = s.task(task).shield();
            // SAFETY: the destination belongs to a saved waiting frame.
            if let Some(status) = unsafe { receive_ready(s, task, shield, channel, slot) } {
                s.task(task).status = status;
                // SAFETY: saved preflight and receive_ready initialized only
                // DONE payloads. The observer never samples the parked ctx.
                unsafe { ordered::events::record_receive_poll(s, task, slot, status) };
                true
            } else {
                false
            }
        }
        // A send or close that makes the receive able to act takes the
        // token the receiver registered under the channel's latch, after its
        // recheck there: only cancellation is left to this poll.
        Wait::ReceiveReady { .. } => {
            let shield = s.task(task).shield();
            s.canceled_task(task, shield).is_some()
        }
        Wait::Any { set, member: None } => s.first_completed(set).is_some(),
        Wait::Any {
            set,
            member: Some(index),
        } => s.member_completed(set, index),
        // SAFETY: the waiting task owns its set; this reads task rows only.
        Wait::Drain { set } => unsafe { poll_drained(s, set) },
        Wait::Final => false,
        Wait::Retry { cancelable } => {
            let shield = s.task(task).shield();
            let canceled = cancelable && s.canceled_task(task, shield).is_some();
            std::mem::take(&mut s.task(task).retried) || canceled
        }
        Wait::Gate { gate } => {
            let row = s.gate(gate);
            if row.owner.is_none() {
                row.owner = Some(task);
                row.depth = 1;
                row.version += 1;
                ordered::events::record_gate_poll(s, task, gate);
                true
            } else {
                false
            }
        }
        Wait::Pool => pool_available?,
        Wait::Host { .. } => return None,
    })
}

/// # Safety
/// The role owns the table and every stack access is gated on a saved image.
pub(super) unsafe fn poll_free(s: &mut Scheduler) {
    unsafe { poll_free_inner(s, None) };
}

/// As poll_free, with capacity captured outside the scheduler callback. A
/// wake permits the owner to retry claim; polling never reserves a pool slot.
/// # Safety
/// As for poll_free.
pub(super) unsafe fn poll_free_with_pool(s: &mut Scheduler, pool_available: bool) {
    unsafe { poll_free_inner(s, Some(pool_available)) };
}

unsafe fn poll_free_inner(s: &mut Scheduler, pool_available: Option<bool>) {
    s.wake_receivers();
    // Keep unsupported service waits pending. No host bridge, allocation of
    // pool threads, or scheduler(ctx) reentry is permitted under IdleGuard.
    let woken = std::mem::take(&mut s.woken);
    for task in woken {
        let Some(Some(row)) = s.tasks.get_mut(task as usize) else {
            continue;
        };
        if row.wait.is_none() {
            row.woken = false;
            continue;
        }
        // SAFETY: after-save publication protects every inspected stack slot.
        match unsafe { poll(s, task, pool_available) } {
            Some(ready) => {
                s.task(task).woken = false;
                if ready {
                    s.unpark(task);
                }
            }
            None => s.woken.push(task),
        }
    }
}

/// Complete a runtime-only ordered poll. A deferred slot or service wait keeps
/// the whole retirement pending; it must not select a new head in the meantime.
/// # Safety
/// The role owns the table, and no other code accesses a saved frame's slots.
pub(super) unsafe fn poll_ordered(s: &mut Scheduler, pool_available: bool) -> bool {
    unsafe { poll_ordered_inner(s, false, pool_available) }
}

/// The home continuation already completed all saved host readiness attempts.
/// # Safety
/// As for poll_ordered; no host request effect occurs under this role.
pub(super) unsafe fn poll_ordered_after_hosts(s: &mut Scheduler, pool_available: bool) -> bool {
    unsafe { poll_ordered_inner(s, true, pool_available) }
}

unsafe fn poll_ordered_inner(s: &mut Scheduler, hosts_polled: bool, pool_available: bool) -> bool {
    s.wake_receivers();
    for &task in &s.woken {
        let Some(row) = s.tasks.get(task as usize).and_then(Option::as_ref) else {
            continue;
        };
        let Some(wait) = row.wait else { continue };
        if !saved(s, task) || (!hosts_polled && matches!(wait, Wait::Host { .. })) {
            return false;
        }
        if let Wait::Join { child, .. } = wait
            && !saved(s, child)
        {
            return false;
        }
    }
    let mut woken = std::mem::take(&mut s.woken);
    woken.sort_unstable_by_key(|&task| {
        s.tasks[task as usize]
            .as_ref()
            .map_or(u64::MAX, |row| row.parked_at)
    });
    for task in woken {
        let Some(Some(row)) = s.tasks.get_mut(task as usize) else {
            continue;
        };
        if row.wait.is_none() {
            row.woken = false;
            continue;
        }
        if hosts_polled && matches!(s.task(task).wait, Some(Wait::Host { .. })) {
            s.task(task).woken = false;
            continue;
        }
        // SAFETY: preflight checked every slot before the first poll effect.
        let ready =
            unsafe { poll(s, task, Some(pool_available)) }.expect("runtime-only ordered wait");
        s.task(task).woken = false;
        if ready {
            s.unpark(task);
        }
    }
    true
}

#[cfg(test)]
mod ordered_tests {
    use super::*;

    fn table() -> Scheduler {
        let mut s = Scheduler::default();
        for task in 0..3 {
            s.tasks.push(Some(Task::new(
                task + 1,
                std::ptr::null_mut(),
                None,
                vec![task],
                None,
            )));
            s.current = task;
            s.task(task).retried = true;
            s.park(Wait::Retry { cancelable: false });
        }
        s
    }

    #[test]
    fn wakes_follow_park_order_even_when_notifications_arrive_backwards() {
        let mut s = table();
        s.woken.reverse();
        // SAFETY: these runtime Retry waits inspect table metadata only.
        assert!(unsafe { poll_ordered(&mut s, true) });
        assert_eq!(s.runnable, VecDeque::from([0, 1, 2]));
        assert!(s.woken.is_empty());
        assert!(
            s.tasks
                .iter()
                .flatten()
                .all(|row| row.wait.is_none() && !row.woken)
        );
    }

    #[test]
    fn exhausted_pool_waits_do_not_block_retirement_and_retry_only_after_a_wake() {
        let mut s = table();
        s.task(1).wait = Some(Wait::Pool);
        s.pool_waiters.push(1);
        assert!(unsafe { poll_ordered(&mut s, false) });
        assert_eq!(s.runnable, VecDeque::from([0, 2]));
        assert!(matches!(s.task(1).wait, Some(Wait::Pool)));
        assert!(!s.task(1).woken);
        assert_eq!(s.pool_waiters, vec![1]);
        s.wake(1);
        assert!(unsafe { poll_ordered(&mut s, true) });
        assert_eq!(s.runnable, VecDeque::from([0, 2, 1]));
        assert!(s.task(1).wait.is_none());
        assert!(s.pool_waiters.is_empty());
    }

    #[test]
    fn a_service_wait_leaves_all_runtime_poll_effects_pending() {
        let mut s = table();
        s.task(1).wait = Some(Wait::Host { token: 7 });
        // SAFETY: preflight reads table metadata and invokes no service.
        assert!(!unsafe { poll_ordered(&mut s, true) });
        assert!(s.runnable.is_empty());
        assert_eq!(s.woken, vec![0, 1, 2]);
        assert!(s.task(0).retried);
        assert!(s.task(2).retried);
        s.task(1).wait = Some(Wait::Retry { cancelable: false });
        assert!(unsafe { poll_ordered(&mut s, true) });
        assert_eq!(s.runnable, VecDeque::from([0, 1, 2]));
    }
}
