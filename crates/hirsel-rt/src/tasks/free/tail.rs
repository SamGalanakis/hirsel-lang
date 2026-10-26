//! Task-owned Free effect tails. These records are subordinate to the task's
//! existing Active/Saved continuation; they never schedule or activate a task.
//!
//! Tokio ce685ee6916549ed, runtime/task/state.rs:147-182 and
//! runtime/task/harness.rs:502-559 separate running ownership, pending work
//! and stored output. Here the effect and its cleanup suffix also have distinct
//! stages: polling a stored completion cannot begin or apply the effect again.
//! Run stop drops storage without calling language cleanup. Admission from
//! compiled code remains disabled until the caller's complete custody is proved.
#![cfg_attr(not(test), allow(dead_code))]

use std::task::Poll;

use crate::channels::Ring;
use crate::failure::Chained;
use crate::stop::Stopped;
use crate::tasks::registry::TaskKey;
use crate::{STATUS_FAILURE, STATUS_OK};
use std::sync::Arc;

mod mutex;
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
            self.ordinal = begin();
            self.stage = Stage::Waiting { entered: false };
        }
        let Stage::Waiting { mut entered } = self.stage else {
            unreachable!();
        };
        // A stopped admission never leaves a retryable record behind.
        self.stage = Stage::Abandoned;
        if !ready(&mut entered)? {
            self.stage = Stage::Waiting { entered };
            return Ok(Poll::Pending);
        }
        self.stage = Stage::Applying;
        match apply(&mut self.owners) {
            Ok(status) => {
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
    bindings: Vec<Binding>,
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

/// Identity of one checked entered invocation, stable across rebinds and
/// never reused when a loop reaches the same compiled state again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BoundSite {
    pub(crate) task: TaskKey,
    pub(crate) invocation: u64,
    pub(crate) frame: usize,
    pub(crate) state: u32,
    pub(crate) kind: u32,
    pub(crate) bytes: u32,
    pub(crate) anchor: u64,
    pub(crate) operation_site: u64,
}

struct Binding {
    site: BoundSite,
    armed: bool,
    ticket: Option<Ticket>,
}

/// Bind only a site for which the independent continuation checker proved
/// the whole native callchain. This carries no scheduling permission.
/// Kinds 0..=2 are mutex put/kill/cause; scope operations use 16 and above.
/// # Safety
/// The Active dispatcher owns frame, with exactly this checked state/layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_resume_tail(
    ctx: *mut crate::TaskCtx,
    frame: *mut u8,
    state: u32,
    kind: u32,
    bytes: u32,
    anchor: u64,
    operation_site: u64,
) {
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if !super::enabled(ctx) {
        return;
    }
    assert_eq!(unsafe { frame.cast::<u32>().read() }, state);
    assert!(kind <= 2 || kind >= 16, "unknown entered-tail kind");
    let task = crate::tasks::role::with_role(ctx, |s| {
        let me = s.current;
        let key = s.key(me);
        let contained = s.task(me).compact_frame.is_some_and(|(base, extent)| {
            let start = frame as usize;
            start >= base
                && start
                    .checked_add(bytes as usize)
                    .is_some_and(|end| end <= base + extent)
        });
        assert!(
            contained,
            "an entered tail outside the active compact frame"
        );
        key
    });
    let records = unsafe { &mut (*ctx).free_ops.tails };
    if let Some(binding) = records
        .bindings
        .last_mut()
        .filter(|b| b.site.frame == frame as usize && b.site.state == state)
    {
        let old = binding.site;
        assert_eq!(
            (
                old.task,
                old.frame,
                old.state,
                old.kind,
                old.bytes,
                old.anchor,
                old.operation_site
            ),
            (
                task,
                frame as usize,
                state,
                kind,
                bytes,
                anchor,
                operation_site
            ),
            "a different entered-tail invocation"
        );
        assert!(
            !binding.armed,
            "an entered tail rebound before its intrinsic"
        );
        binding.armed = true;
    } else {
        records.next = records
            .next
            .checked_add(1)
            .expect("Free tail identity overflow");
        assert!(
            records.bindings.last().is_none_or(|b| !b.armed),
            "nested tail before intrinsic entry"
        );
        records.bindings.push(Binding {
            site: BoundSite {
                task,
                invocation: records.next,
                frame: frame as usize,
                state,
                kind,
                bytes,
                anchor,
                operation_site,
            },
            armed: true,
            ticket: None,
        });
    }
}

/// Disarm before entering the intrinsic. Nested synchronous callbacks cannot
/// inherit this call site's permission, even for the same operation kind.
/// # Safety
/// The running task owns the context; the entry uses exactly `kind`'s ABI.
pub(crate) unsafe fn take_bound(ctx: *mut crate::TaskCtx, kind: u32) -> Option<BoundSite> {
    let binding = unsafe { (*ctx).free_ops.tails.bindings.last_mut() }?;
    if !binding.armed {
        return None;
    }
    assert_eq!(
        binding.site.kind, kind,
        "a bound tail reached a different intrinsic"
    );
    binding.armed = false;
    Some(binding.site)
}

/// Retire after the exact completed suffix. A pending rebind retains its key;
/// the next completed loop invocation gets a new serial instead.
/// # Safety
/// The Active task completed this site's original operation and its suffix.
pub(crate) unsafe fn complete_bound(ctx: *mut crate::TaskCtx, site: BoundSite) {
    let binding =
        unsafe { (*ctx).free_ops.tails.bindings.pop() }.expect("an unbound tail completion");
    assert_eq!(binding.site, site, "a stale bound tail completion");
    assert!(
        !binding.armed,
        "a tail completed before entering its intrinsic"
    );
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
    let site = unsafe { take_bound(ctx, tag) }?;
    let existing = unsafe { (*ctx).free_ops.tails.bindings.last().unwrap().ticket };
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
            unsafe { (*ctx).free_ops.tails.bindings.last_mut().unwrap().ticket = Some(ticket) };
            ticket
        }
    };
    loop {
        match unsafe { crate::stop::check(ctx, poll_mutex(ctx, ticket)) } {
            Poll::Ready(status) => {
                unsafe { complete_bound(ctx, site) };
                return Some(status);
            }
            Poll::Pending => {
                if unsafe { crate::tasks::compact::can_tail(ctx) } {
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
