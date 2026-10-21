//! Logical free-policy transitions after an exclusive stack reaches idle.
//!
//! Physical dispatch owns stacks; this policy owns readiness and completion.
//! Like Tokio's task harness, a running notification is remembered until the
//! idle continuation relinquishes running ownership. A logical park alone
//! never authorizes another task to read the parked stack.

use super::*;

#[derive(Default)]
pub(crate) struct Driver;

/// Admit the free logical policy for explicit dispatcher tests. General
/// activation additionally requires the semantic suspension/end branches,
/// recorder sections, host readiness and concurrent service lifetime audit.
/// # Safety
/// Admission only, before the root starts. The body uses only saved-stack
/// dispatch and supported waits; it cannot invoke cooperative switch paths.
pub(crate) unsafe fn enable(ctx: *mut TaskCtx) {
    scheduler(ctx).logical = Logical::Free(Driver);
}

fn saved(s: &Scheduler, task: u32) -> bool {
    s.tasks[task as usize]
        .as_ref()
        .and_then(|row| row.dispatch.as_ref())
        .is_none_or(|frame| frame.is_saved())
}

/// All effects here are bounded runtime work under IdleGuard. In particular,
/// host readiness and pool allocation remain outside this callback.
/// # Safety
/// The role exclusively owns `s`; both sides of every slot copy are saved.
unsafe fn poll(s: &mut Scheduler, task: u32) -> Option<bool> {
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
                true
            } else {
                false
            }
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
                true
            } else {
                false
            }
        }
        Wait::Host { .. } | Wait::Pool => return None,
    })
}

/// # Safety
/// Called only by tasks::after_save under its context-independent idle guard.
pub(super) unsafe fn after_save(
    s: &mut Scheduler,
    task: u32,
    completed: bool,
) -> Vec<(u32, Owner)> {
    if let Some(Some(row)) = s.tasks.get(task as usize) {
        match row.state {
            State::Parked if row.wait.is_some() => s.wake(task),
            State::Holding | State::Done => s.settle(task),
            _ => {}
        }
        if completed && s.task(task).state == State::Done {
            if let Some(parent) = s.task(task).handoff.take() {
                s.task(parent).state = State::Ready;
                s.runnable.push_back(parent);
            }
            if s.task(task).detached && s.task(task).failure.is_none() {
                let scope = s.task(task).scope.expect("a detached child has a scope");
                s.scope(scope).children.retain(|&child| child != task);
                s.vacate_task(task);
            }
        }
    }
    s.wake_receivers();
    // Keep unsupported service waits pending. No host bridge, allocation of
    // pool threads, or scheduler(ctx) reentry is permitted under IdleGuard.
    let woken = std::mem::take(&mut s.woken);
    for task in woken {
        // SAFETY: after-save publication protects every inspected stack slot.
        match unsafe { poll(s, task) } {
            Some(ready) => {
                s.task(task).woken = false;
                if ready {
                    s.unpark(task);
                }
            }
            None => s.woken.push(task),
        }
    }
    let ready = std::mem::take(&mut s.runnable);
    ready
        .into_iter()
        .map(|task| (task, s.task(task).owner))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_save_repolls_a_wait_deferred_while_its_stack_was_still_running() {
        let mut run = crate::host::BodyRun::new(8, false);
        // SAFETY: this body uses only explicit dispatch and the Retry wait
        // below, without a home service, stack delivery or logical completion.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { enable(ctx) },
            |ctx| {
                for _ in 0..256 {
                    unsafe {
                        role::enter(ctx);
                        let s = scheduler(ctx);
                        let me = s.current;
                        s.task(me).retried = true;
                        s.park(Wait::Retry { cancelable: false });
                        // A concurrent logical poll cannot use this stack yet.
                        assert!(
                            !super::super::poll(ctx, me, &crate::io::Ready::Tokens(Vec::new()))
                                .unwrap()
                        );
                        crate::pool::free::suspend_current(ctx, false);
                        let row = scheduler(ctx).task(me);
                        assert_eq!(row.state, State::Ready);
                        assert!(row.wait.is_none());
                        assert!(!row.woken);
                        role::release(ctx);
                    }
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }
}
