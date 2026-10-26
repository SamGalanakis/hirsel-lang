//! Task-owned Free effect tails. These records are subordinate to the task's
//! existing Active/Saved continuation; they never schedule or activate a task.
//!
//! Tokio ce685ee6916549ed, runtime/task/state.rs:147-182 and
//! runtime/task/harness.rs:502-559 separate running ownership, pending work
//! and stored output. Here the effect and its cleanup suffix also have distinct
//! stages: polling a stored completion cannot begin or apply the effect again.
//! Run stop drops storage without calling language cleanup. Compiled entry
//! requires independently checked custody of every retained caller and owner.
#![cfg_attr(not(test), allow(dead_code))]

use std::task::Poll;

use crate::channels::Ring;
use crate::failure::Chained;
use crate::stop::Stopped;
use crate::tasks::registry::TaskKey;
use crate::{STATUS_FAILURE, STATUS_OK};
use std::sync::Arc;

mod close;
mod mutex;
#[allow(unused_imports)]
pub(in crate::tasks) use close::{
    CleanupPending, CleanupPoll, attach as attach_close, attach_finalizers, poll as poll_close,
    poll_finalizers, requeue as close_requeue,
};
pub(crate) use close::{FinalizerResolver, ResolvedFinalizer};
pub(crate) use close::{invoke_finalize, invoke_release, invoke_root_drop};
#[cfg(test)]
mod tests;
#[allow(unused_imports)]
pub(super) use mutex::{poll_mutex, prepare_mutex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Put,
    Kill,
    Cause,
    CloseMutex,
    CloseChannel,
}

/// An invocation, including repeated executions of the same compiled site.
/// A handle is useful only in its task generation and is never reused there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Ticket {
    task: TaskKey,
    invocation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Site {
    pub(super) object: hirsel_free::session::Handle,
    pub(super) kind: Kind,
    pub(super) anchor: u64,
    pub(super) operation: u64,
}

/// Physical owners only: dropping an image reclaims bytes, never its language
/// values. The certified adapter must transfer/empty the canonical carrier
/// before installing an image, and restore it on language failure if needed.
#[derive(Default)]
pub(super) struct Owners {
    pub(super) image: Option<Ring>,
    pub(super) descriptor: Option<u32>,
    pub(super) chain: Option<Arc<Vec<Chained>>>,
    pub(super) wakes: std::collections::VecDeque<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Prepared,
    Waiting { entered: bool },
    Applying,
    EffectDone(u8),
    Releasing(u8),
    Complete(u8),
    Abandoned,
}

/// A pinned allocation in Records, never an element whose address Vec growth
/// can move. No stack Cleanup, mask, output address, guard or waker lives here.
pub(super) struct Record {
    ticket: Ticket,
    pub(super) site: Site,
    stage: Stage,
    ordinal: Option<u64>,
    suffix_done: bool,
    yielded: bool,
    rest: bool,
    pub(super) owners: Owners,
}

impl Record {
    /// Poll the effect, using the original entry, admission and mutation code.
    /// `begin` must perform only entry/begin, `ready` subscribes and rechecks,
    /// and `apply` completes one bounded admitted phase without suspension.
    /// None may publish a child operation. An error terminates the record;
    /// a caller cannot recover by applying a possibly completed action again.
    pub(super) fn poll(
        &mut self,
        begin: impl FnOnce() -> Option<u64>,
        ready: impl FnOnce(&mut bool) -> Stopped<bool>,
        apply: impl FnOnce(&mut Owners) -> Stopped<u8>,
    ) -> Stopped<Poll<u8>> {
        self.poll_entered(|| Ok(begin()), ready, |owners| apply(owners).map(Some))
    }

    pub(super) fn poll_entered(
        &mut self,
        begin: impl FnOnce() -> Stopped<Option<u64>>,
        ready: impl FnOnce(&mut bool) -> Stopped<bool>,
        apply: impl FnOnce(&mut Owners) -> Stopped<Option<u8>>,
    ) -> Stopped<Poll<u8>> {
        match self.stage {
            Stage::EffectDone(status) | Stage::Releasing(status) | Stage::Complete(status) => {
                return Ok(Poll::Ready(status));
            }
            Stage::Applying | Stage::Abandoned => panic!("poll of a terminal or active Free tail"),
            Stage::Prepared | Stage::Waiting { .. } => {}
        }
        if self.stage == Stage::Prepared {
            // The record already owns its inputs before entry can observe stop.
            self.stage = Stage::Applying;
            self.ordinal = begin()?;
            self.stage = Stage::Waiting { entered: false };
        }
        let Stage::Waiting { mut entered } = self.stage else {
            unreachable!();
        };
        // A stopped admission never leaves a retryable record behind.
        self.stage = Stage::Abandoned;
        self.rest = false;
        if !ready(&mut entered)? {
            self.stage = Stage::Waiting { entered };
            return Ok(Poll::Pending);
        }
        self.stage = Stage::Applying;
        match apply(&mut self.owners) {
            Ok(None) => {
                self.rest = true;
                self.stage = Stage::Waiting { entered };
                Ok(Poll::Pending)
            }
            Ok(Some(status)) => {
                status_is_terminal(status);
                self.stage = Stage::EffectDone(status);
                Ok(Poll::Ready(status))
            }
            Err(stop) => {
                self.stage = Stage::Abandoned;
                Err(stop)
            }
        }
    }

    /// Cleanup starts only after Done and resumes independently of the effect.
    pub(super) fn releasing(&mut self) {
        let Stage::EffectDone(status) = self.stage else {
            panic!("Free tail cleanup before effect completion");
        };
        self.stage = Stage::Releasing(status);
    }

    /// Only completed cleanup statuses enter the accumulator. Pending is not
    /// a failure bit and must never become a stored language status.
    pub(super) fn released(&mut self, status: u8) {
        status_is_terminal(status);
        let Stage::Releasing(previous) = self.stage else {
            panic!("Free tail child completion outside cleanup");
        };
        self.stage = Stage::Releasing(previous | status);
    }

    pub(super) fn finish(&mut self) {
        let (Stage::EffectDone(status) | Stage::Releasing(status)) = self.stage else {
            panic!("Free tail suffix completed twice or before Done");
        };
        assert!(
            self.owners.image.is_none(),
            "Free tail lost a payload owner"
        );
        assert!(self.owners.chain.is_none(), "Free tail lost a cause owner");
        assert!(self.owners.wakes.is_empty(), "Free tail lost a wake suffix");
        self.stage = Stage::Complete(status);
    }
}

fn status_is_terminal(status: u8) {
    assert!(
        status == STATUS_OK || status == STATUS_FAILURE,
        "a pending Free tail is not a cleanup completion"
    );
}

/// Strictly nested obligations. The parent stays allocated and cannot be
/// polled while its child owns the next ordinal. Records carry no scheduling
/// references, so retiring one cannot keep a task runnable.
#[derive(Default)]
pub(crate) struct Records {
    next: u64,
    drivers: Vec<Box<close::Driver>>,
    cleanup_sites: Vec<(crate::continuation::binding::BoundSite, usize)>,
    entered: Vec<(crate::continuation::binding::BoundSite, Ticket)>,
    // Boxes retain every ancestor's address across growth and child retirement.
    #[allow(clippy::vec_box)]
    stack: Vec<Box<Record>>,
}

impl Records {
    pub(super) fn prepare(&mut self, task: TaskKey, site: Site, owners: Owners) -> Ticket {
        if let Some(parent) = self.stack.last() {
            assert_eq!(parent.ticket.task, task, "Free tail moved to another task");
            assert!(
                matches!(parent.stage, Stage::EffectDone(_) | Stage::Releasing(_)),
                "a child Free operation before its parent's Done"
            );
        }
        self.next = self
            .next
            .checked_add(1)
            .expect("Free tail identity overflow");
        let ticket = Ticket {
            task,
            invocation: self.next,
        };
        self.stack.push(Box::new(Record {
            ticket,
            site,
            stage: Stage::Prepared,
            ordinal: None,
            suffix_done: false,
            yielded: false,
            rest: false,
            owners,
        }));
        ticket
    }

    /// Only the task's Active continuation may access its innermost record.
    /// The dispatcher, rather than a tail flag, proves physical save ack.
    pub(super) fn active(&mut self, task: TaskKey, ticket: Ticket) -> &mut Record {
        assert_eq!(ticket.task, task, "a stale Free tail task generation");
        let record = self.stack.last_mut().expect("a retired Free tail");
        assert_eq!(record.ticket, ticket, "a stale or paused Free tail");
        record
    }

    /// Consume completion exactly once, after the compiler's exact suffix.
    pub(super) fn retire(&mut self, task: TaskKey, ticket: Ticket) -> u8 {
        let Stage::Complete(status) = self.active(task, ticket).stage else {
            panic!("retiring an unfinished Free tail");
        };
        self.stack.pop();
        status
    }
}

/// An ordinary intrinsic consults only an explicitly bound entered site.
/// Every other caller, including synchronous destructor glue, stays stackful.
/// # Safety
/// The primitive owns this context and its usual mutex/carrier ABI arguments.
pub(crate) unsafe fn invoke_mutex(
    ctx: *mut crate::TaskCtx,
    kind: Kind,
    word: u64,
    payload: *mut u8,
) -> Option<u8> {
    let tag = match kind {
        Kind::Put => 0,
        Kind::Kill => 1,
        Kind::Cause => 2,
        _ => unreachable!(),
    };
    let site = unsafe { crate::continuation::binding::take_bound(ctx, tag) }?;
    // Shared physical binding also supports Ordered entered operations. A
    // Free tail must never bypass that policy's frontier or effect recorder.
    if !crate::tasks::free::enabled(ctx) {
        unsafe { crate::continuation::binding::complete_bound(ctx, site) };
        return None;
    }
    let existing = unsafe {
        (*ctx)
            .free_ops
            .tails
            .entered
            .last()
            .filter(|(at, _)| *at == site)
            .map(|(_, ticket)| *ticket)
    };
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let ticket = match existing {
        Some(ticket) => ticket,
        None => {
            let ticket = unsafe {
                prepare_mutex(
                    ctx,
                    Site {
                        object: (crate::free::MUTEX, word),
                        kind,
                        anchor: site.anchor,
                        operation: site.operation_site,
                    },
                    payload,
                )
            };
            unsafe { (*ctx).free_ops.tails.entered.push((site, ticket)) };
            ticket
        }
    };
    loop {
        let leave = unsafe { crate::continuation::binding::can_leave(ctx, site) };
        let result = unsafe { poll_mutex(ctx, ticket, leave) };
        if let Err(ref stop) = result
            && leave
            && super::replay::cut_rejection(ctx, stop)
        {
            unsafe {
                crate::stop::check(ctx, crate::tasks::park_left(ctx, crate::tasks::Wait::Final))
            };
            crate::tasks::compact::wait(ctx);
            return Some(crate::STATUS_SUSPENDED);
        }
        match unsafe { crate::stop::check(ctx, result) } {
            Poll::Ready(status) => {
                unsafe {
                    let retired = (*ctx)
                        .free_ops
                        .tails
                        .entered
                        .pop()
                        .expect("an entered mutex tail");
                    assert_eq!(retired, (site, ticket));
                    crate::continuation::binding::complete_bound(ctx, site);
                };
                return Some(status);
            }
            Poll::Pending => {
                if unsafe { crate::continuation::binding::can_leave(ctx, site) } {
                    // A committed yield schedules this saved continuation itself.
                    // An object dependency wait is awakened by its own token.
                    if unsafe {
                        let record = (*ctx).free_ops.tails.active(site.task, ticket);
                        record.yielded || record.rest
                    } {
                        return Some(crate::STATUS_SUSPENDED);
                    }
                    unsafe {
                        crate::stop::check(
                            ctx,
                            crate::tasks::park_left(
                                ctx,
                                crate::tasks::Wait::Retry { cancelable: false },
                            ),
                        )
                    };
                    crate::tasks::compact::wait(ctx);
                    return Some(crate::STATUS_SUSPENDED);
                }
                if unsafe { (*ctx).free_ops.tails.active(site.task, ticket).rest } {
                    unsafe { super::suspend(&mut *ctx, true) };
                    continue;
                }
                unsafe {
                    crate::stop::check(
                        ctx,
                        crate::tasks::try_block(
                            &mut *ctx,
                            crate::tasks::Wait::Retry { cancelable: false },
                            false,
                            0,
                        ),
                    )
                };
            }
        }
    }
}
