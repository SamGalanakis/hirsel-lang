//! Owned continuations of entered Ordered operations. Each drive finishes all
//! callbacks and loans before returning Pending; a wait never restarts entry.
//!
//! This first runtime slice uses the existing stackful suspension. Its join
//! destination is therefore explicitly stackful. Compact admission needs a
//! separately checked retained-frame destination before these drives can leave
//! a stack. In particular this is not the first-effect/reissue protocol.
//!
//! Like Tokio's Idle/Busy file state (`fs/file.rs:649-712` at ce685ee6), the
//! phase stays valid on error and owns the continuation until completion or
//! abandonment. Hirsel additionally separates logical retirement from physical
//! save, and never retries an adapter from the awakened issuer.

use super::super::{Logical, State, TaskCtx, Wait, handle, readiness, role, unpin};
use super::{engine, host, lifecycle};
use crate::ordered::{Next, Progress};
use crate::stop::Stopped;

/// Pending is physical, not a language success or failure. The phase has
/// already advanced before this is returned, including a promised save.
pub(super) enum Drive<T> {
    Complete(T),
    Pending { head_only: bool },
}

pub(super) enum Operation {
    Host(Host),
    Join(Join),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostPhase {
    Begin,
    Yield,
    YieldHead,
    Park,
    Outcome,
    ReturnHead,
    Delivered,
}

pub(super) struct Host {
    token: u64,
    yielded: bool,
    phase: HostPhase,
}

impl Host {
    pub(super) fn new(token: u64, yielded: bool) -> Self {
        Self {
            token,
            yielded,
            phase: HostPhase::Begin,
        }
    }
}

// Record access is bounded bookkeeping under the already-held role. It must
// not insert a stop between a committed transition and its successor phase.
fn with_operation<T>(ctx: *mut TaskCtx, f: impl FnOnce(&mut super::driver::Driver, u64) -> T) -> T {
    unsafe {
        role::with_held(ctx, |s| {
            let me = s.current;
            let task = handle(me, s.task(me).generation);
            let Logical::Ordered(driver) = &mut s.logical else {
                unreachable!("entered operation outside Ordered")
            };
            f(driver, task)
        })
    }
}

pub(super) fn start(ctx: *mut TaskCtx, operation: Operation) {
    with_operation(ctx, |d, task| {
        assert!(
            d.in_operations.insert(task, operation).is_none(),
            "nested entered operation"
        );
    });
}

pub(super) fn finish(ctx: *mut TaskCtx) {
    with_operation(ctx, |d, task| {
        assert!(d.in_operations.remove(&task).is_some());
    });
}

fn host_phase(ctx: *mut TaskCtx, phase: HostPhase) {
    with_operation(ctx, |d, task| {
        let Operation::Host(host) = d.in_operations.get_mut(&task).unwrap() else {
            unreachable!("host continuation identity")
        };
        host.phase = phase;
    });
}

/// All request bytes and completion buffers are in TaskCtx/the original wait
/// row. This record carries only the continuation after that ownership transfer.
/// The home registration call returns, ending its loan, before Pending.
/// # Safety
/// The issuer owns its original wait row and holds the task role.
pub(super) unsafe fn drive_host(ctx: *mut TaskCtx) -> Stopped<Drive<()>> {
    loop {
        let task = engine::current(ctx);
        let (token, yielded, phase) = with_operation(ctx, |d, task| {
            let Operation::Host(host) = &d.in_operations[&task] else {
                unreachable!("host continuation identity")
            };
            (host.token, host.yielded, host.phase)
        });
        match phase {
            HostPhase::Begin => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                host::begin(ctx, false)?;
                host_phase(
                    ctx,
                    if yielded {
                        HostPhase::Yield
                    } else {
                        HostPhase::Park
                    },
                );
            }
            HostPhase::Yield => {
                let (_, progress) = engine::head_continuation(ctx, || ((), Next::Yield))?;
                // Publish the successor before running the poll. Poll callbacks
                // keep their normal owner and cannot cause this Yield to repeat.
                host_phase(ctx, HostPhase::YieldHead);
                if progress == Progress::Poll {
                    engine::poll(ctx)?;
                }
            }
            HostPhase::YieldHead => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                host_phase(ctx, HostPhase::Park);
            }
            HostPhase::Park => {
                let (_, progress) = engine::head_continuation(ctx, || {
                    role::with_role(ctx, |s| {
                        let me = s.current;
                        unpin(s.task(me));
                        s.park(Wait::Host { token });
                    });
                    ((), Next::Park)
                })?;
                engine::reserve_host_poll(ctx, progress);
                unsafe { crate::host_waits::park(ctx, token, task as u32) };
                host_phase(ctx, HostPhase::Outcome);
                // Even a wake during registration must pass through physical
                // save. It cannot turn this promise into an ordinary return.
                return Ok(Drive::Pending { head_only: false });
            }
            HostPhase::Outcome => {
                if host::select_completed_return(ctx, token) {
                    host_phase(ctx, HostPhase::Delivered);
                    return Ok(Drive::Complete(()));
                }
                host_phase(ctx, HostPhase::ReturnHead);
            }
            HostPhase::ReturnHead => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                assert!(with_operation(ctx, |d, task| d
                    .host_ops
                    .remove(&task)
                    .is_some()));
                host_phase(ctx, HostPhase::Delivered);
                return Ok(Drive::Complete(()));
            }
            HostPhase::Delivered => unreachable!("host continuation already delivered"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JoinPhase {
    Prepared,
    Park,
    Child,
    Retire,
    Delivered,
}

/// This destination borrows an existing stackful activation. It confers no
/// permission to discard that stack; a frame certificate will be a distinct
/// representation, not an unchecked conversion of this address.
pub(super) struct StackfulDestination {
    pub(super) address: *mut u8,
    pub(super) size: u64,
}

pub(super) struct Join {
    scope_word: u64,
    child_word: u64,
    scope: Option<u32>,
    child: Option<u32>,
    destination: StackfulDestination,
    caller_site: u64,
    phase: JoinPhase,
}

impl Join {
    pub(super) fn new(
        scope: u64,
        child: u64,
        destination: StackfulDestination,
        caller_site: u64,
    ) -> Self {
        Self {
            scope_word: scope,
            child_word: child,
            scope: None,
            child: None,
            destination,
            caller_site,
            phase: JoinPhase::Prepared,
        }
    }
}

fn with_join<T>(ctx: *mut TaskCtx, f: impl FnOnce(&mut Join) -> T) -> T {
    with_operation(ctx, |d, task| {
        let Operation::Join(join) = d.in_operations.get_mut(&task).unwrap() else {
            unreachable!("join continuation identity")
        };
        f(join)
    })
}

/// Drive the entered join, retaining the original branch across its first
/// yield. Joins remain non-cancellation points. Child handoff never repeats,
/// and retirement still waits for the child's physical save.
/// # Safety
/// The stackful caller keeps its exclusive destination alive until retirement.
pub(super) unsafe fn drive_join(ctx: *mut TaskCtx) -> Stopped<Drive<u8>> {
    loop {
        let phase = with_join(ctx, |j| j.phase);
        if !engine::head_step(ctx)? {
            return Ok(Drive::Pending { head_only: true });
        }
        match phase {
            JoinPhase::Prepared => {
                let (scope_word, child_word) = with_join(ctx, |j| (j.scope_word, j.child_word));
                let (scope, child, state) = role::with_role(ctx, |s| {
                    let scope = s.scope_handle(scope_word);
                    let child = s.task_handle(child_word, scope);
                    (scope, child, s.task(child).state)
                });
                super::events::prepare_join(ctx, scope_word, child_word);
                with_join(ctx, |j| {
                    j.scope = Some(scope);
                    j.child = Some(child);
                });
                if state == State::Holding {
                    let (slot, size) =
                        with_join(ctx, |j| (j.destination.address, j.destination.size));
                    let progress = unsafe {
                        lifecycle::call_start(
                            ctx,
                            child,
                            lifecycle::Call {
                                begin: Some("task.join"),
                                then_yield: true,
                                result: lifecycle::ResultAction::Deliver(slot, size),
                                detached: false,
                            },
                        )
                    }?;
                    engine::defer_boundary_poll(ctx, progress);
                    with_join(ctx, |j| j.phase = JoinPhase::Retire);
                    return Ok(Drive::Pending { head_only: false });
                }
                let (_, progress) = engine::head_effect(ctx, "task.join", || ((), Next::Yield))?;
                with_join(ctx, |j| {
                    j.phase = match state {
                        State::Done => JoinPhase::Retire,
                        State::Ready | State::Parked => JoinPhase::Park,
                        _ => unreachable!("join child state"),
                    }
                });
                if progress == Progress::Poll {
                    engine::poll(ctx)?;
                }
            }
            JoinPhase::Park => {
                let (child, slot, size, site) = with_join(ctx, |j| {
                    (
                        j.child.unwrap(),
                        j.destination.address,
                        j.destination.size,
                        j.caller_site,
                    )
                });
                let (_, progress) = engine::head_continuation(ctx, || {
                    role::with_role(ctx, |s| {
                        let me = s.current;
                        s.task(me).wait_site = site;
                        s.park(Wait::Join { child, slot, size });
                    });
                    ((), Next::Park)
                })?;
                engine::defer_boundary_poll(ctx, progress);
                with_join(ctx, |j| j.phase = JoinPhase::Child);
                return Ok(Drive::Pending { head_only: false });
            }
            JoinPhase::Child => {
                let child = with_join(ctx, |j| j.child.unwrap());
                if role::with_role(ctx, |s| s.task(child).state) == State::Holding {
                    let progress = unsafe {
                        lifecycle::call_start(
                            ctx,
                            child,
                            lifecycle::Call {
                                begin: None,
                                then_yield: false,
                                result: lifecycle::ResultAction::Keep,
                                detached: false,
                            },
                        )
                    }?;
                    engine::defer_boundary_poll(ctx, progress);
                    with_join(ctx, |j| j.phase = JoinPhase::Retire);
                    return Ok(Drive::Pending { head_only: false });
                }
                with_join(ctx, |j| j.phase = JoinPhase::Retire);
            }
            JoinPhase::Retire => {
                let (scope, child, scope_word, child_word) = with_join(ctx, |j| {
                    (
                        j.scope.unwrap(),
                        j.child.unwrap(),
                        j.scope_word,
                        j.child_word,
                    )
                });
                let chain = role::with_role(ctx, |s| {
                    assert_eq!(s.task(child).state, State::Done);
                    assert!(readiness::saved(s, child));
                    s.task(child).failure.take()
                });
                super::events::complete_join(ctx, scope_word, child_word, chain.as_deref());
                // Keep the existing no-stop window through row retirement.
                let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
                s.scope(scope).children.retain(|&c| c != child);
                s.vacate_task(child);
                with_join(ctx, |j| j.phase = JoinPhase::Delivered);
                return Ok(Drive::Complete(if let Some(chain) = chain {
                    unsafe { (*ctx).failure.append_chain(chain) };
                    crate::STATUS_FAILURE
                } else {
                    crate::STATUS_OK
                }));
            }
            JoinPhase::Delivered => unreachable!("join continuation already delivered"),
        }
    }
}

/// The current representation preserves its Rust stack. A later certified
/// compact caller consumes Pending itself after retaining its wrapper image.
pub(super) fn run<T>(
    ctx: *mut TaskCtx,
    mut drive: impl FnMut() -> Stopped<Drive<T>>,
) -> Stopped<T> {
    let result = loop {
        match drive() {
            Ok(Drive::Complete(value)) => break Ok(value),
            Ok(Drive::Pending { head_only }) => {
                engine::suspend(ctx, engine::current(ctx), head_only)
            }
            Err(stop) => break Err(stop),
        }
    };
    finish(ctx);
    result
}

#[cfg(test)]
#[path = "inop/modules.rs"]
mod test_modules;
