//! Owned continuations of entered Ordered operations. Each drive finishes all
//! callbacks and loans before returning Pending; a wait never restarts entry.
//!
//! Stackful callers retain their destination's activation. Certified callers
//! instead retain a checked frame offset and continue their exact entered
//! invocation. This is separate from the first-effect/reissue protocol.
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
    pub(super) yielded: bool,
    phase: HostPhase,
    pub(super) binding: Option<super::host::entered::Binding>,
}

impl Host {
    pub(super) fn new(token: u64, yielded: bool) -> Self {
        Self {
            token,
            yielded,
            phase: HostPhase::Begin,
            binding: None,
        }
    }
}

// Record access is bounded bookkeeping under the already-held role. It must
// not insert a stop between a committed transition and its successor phase.
pub(super) fn with_operation<T>(
    ctx: *mut TaskCtx,
    f: impl FnOnce(&mut super::driver::Driver, u64) -> T,
) -> T {
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

/// Already-entered phase bookkeeping must not observe a terminal cut before
/// the effect's owned evidence has been recorded. This is an identity read,
/// not another semantic entry; the calling phase chooses its next stop gate.
pub(super) fn identity(ctx: *mut TaskCtx) -> u64 {
    with_operation(ctx, |_, task| task)
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

/// Arm the reserved park continuation only after the original request moved
/// into its wait row. The reservation predates begin and every adapter effect.
pub(super) fn set_host_wait(ctx: *mut TaskCtx, token: u64, yielded: bool) {
    with_operation(ctx, |d, task| {
        let Operation::Host(host) = d.in_operations.get_mut(&task).unwrap() else {
            unreachable!("host continuation identity")
        };
        assert_eq!(host.phase, HostPhase::Begin);
        assert_eq!(host.token, 0);
        host.token = token;
        host.yielded = yielded;
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
#[derive(Clone, Copy)]
pub(super) struct StackfulDestination {
    pub(super) address: *mut u8,
    pub(super) size: u64,
}

#[derive(Clone, Copy)]
enum Destination {
    Stackful(StackfulDestination),
    Retained {
        site: crate::continuation::binding::BoundSite,
        offset: usize,
        size: u64,
        align: usize,
    },
}

impl Destination {
    fn resolve(self, ctx: *mut TaskCtx) -> (*mut u8, u64) {
        match self {
            Self::Stackful(d) => (d.address, d.size),
            Self::Retained {
                site,
                offset,
                size,
                align,
            } => {
                assert_eq!(
                    crate::tasks::running_key(ctx),
                    site.task,
                    "stale join destination"
                );
                assert!(crate::tasks::compact::contains_frame(
                    ctx,
                    site.frame,
                    site.bytes as usize
                ));
                let address = site
                    .frame
                    .checked_add(offset)
                    .expect("join address overflow");
                assert_eq!(address % align, 0, "misaligned retained join destination");
                assert!(
                    offset
                        .checked_add(size as usize)
                        .is_some_and(|end| end <= site.bytes as usize)
                );
                (address as *mut u8, size)
            }
        }
    }
}

pub(super) struct Join {
    scope_word: u64,
    child_word: u64,
    scope: Option<u32>,
    child: Option<u32>,
    destination: Destination,
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
            destination: Destination::Stackful(destination),
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

pub(super) fn retained_offset(
    frame: usize,
    extent: usize,
    address: usize,
    size: u64,
    align: u32,
) -> Option<usize> {
    let align = align as usize;
    let size = usize::try_from(size).ok()?;
    if align == 0 || !align.is_power_of_two() || align > 16 || address % align != 0 {
        return None;
    }
    let offset = address.checked_sub(frame)?;
    (offset >= 8 && offset.checked_add(size)? <= extent).then_some(offset)
}

/// Continue the same independently certified TaskJoin invocation. On re-entry
/// the original scope/child words and destination come from the owned record;
/// neither the taken handle nor the initial None is read again.
/// # Safety
/// The compiler proved the complete wrapper, its exact Option layout and one
/// mutable loan. `slot` is that retained cell, with the certified extent and
/// alignment. Only this helper's certified callers accept STATUS_SUSPENDED.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ordered_join(
    ctx: *mut TaskCtx,
    caller_site: u64,
    operation_site: u64,
    scope: *const u64,
    child: *mut u64,
    slot: *mut u8,
    size: u64,
    align: u32,
) -> u8 {
    let _role = unsafe { role::Guard::new(ctx) };
    let bound = unsafe { crate::continuation::binding::take_bound(ctx, 32) };
    let Some(site) = bound.filter(|_| engine::on(ctx)) else {
        let status = unsafe {
            crate::tasks::hirsel_rt_v2_task_join(
                ctx,
                caller_site,
                operation_site,
                scope,
                child,
                slot,
                size,
            )
        };
        if let Some(site) = bound {
            unsafe { crate::continuation::binding::complete_bound(ctx, site) };
        }
        return status;
    };
    assert_eq!(site.task, crate::tasks::running_key(ctx));
    assert_eq!(site.operation_site, operation_site, "join operation site");
    let existing = with_operation(ctx, |d, task| match d.in_operations.get(&task) {
        None => false,
        Some(Operation::Join(Join {
            destination: Destination::Retained { site: original, .. },
            ..
        })) => {
            assert_eq!(*original, site, "stale entered join identity");
            true
        }
        _ => panic!("different entered operation at join"),
    });
    if !existing {
        let offset = retained_offset(site.frame, site.bytes as usize, slot as usize, size, align)
            .expect("join destination outside retained wrapper");
        let destination = Destination::Retained {
            site,
            offset,
            size,
            align: align as usize,
        };
        destination.resolve(ctx);
        let join = Join {
            scope_word: unsafe { scope.read() },
            child_word: unsafe { child.read() },
            scope: None,
            child: None,
            destination,
            caller_site,
            phase: JoinPhase::Prepared,
        };
        start(ctx, Operation::Join(join));
    }
    assert!(unsafe { (*ctx).ordered_inop_leave.is_none() });
    loop {
        match unsafe { crate::stop::check(ctx, drive_join(ctx)) } {
            Drive::Complete(status) => {
                finish(ctx);
                unsafe { crate::continuation::binding::complete_bound(ctx, site) };
                return status;
            }
            Drive::Pending { head_only } => {
                if unsafe { crate::continuation::binding::can_leave(ctx, site) } {
                    #[cfg(feature = "diagnostic")]
                    crate::compact_trace::runtime_wait(
                        site.task.slot,
                        site.state,
                        site.kind,
                        "join",
                        site.invocation,
                    );
                    unsafe { (*ctx).ordered_inop_leave = Some(head_only) };
                    return crate::STATUS_SUSPENDED;
                }
                engine::suspend(ctx, engine::current(ctx), head_only);
            }
        }
    }
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
                    let destination = with_join(ctx, |j| j.destination);
                    let (slot, size) = destination.resolve(ctx);
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
                let (child, destination, site) =
                    with_join(ctx, |j| (j.child.unwrap(), j.destination, j.caller_site));
                let (slot, size) = destination.resolve(ctx);
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
