//! Native object effects of the ordered frontier. Inputs and results own their
//! ABI images; none retains a pointer into a caller's carrier across run-ahead.
//! This module does not select a physical worker or run a scheduling point.

pub(crate) mod admission;
pub(crate) mod ahead;
#[cfg(test)]
mod commit_tests;
#[cfg(test)]
mod compact_tests;
pub(crate) mod cpu;
#[cfg(test)]
mod dispatch_key_tests;
pub(crate) mod driver;
pub(crate) mod drops;
pub(crate) mod engine;
pub(crate) mod events;
pub(crate) mod foreign;
pub(crate) mod home;
pub(crate) mod host;
pub(crate) mod inop;
pub(crate) mod lifecycle;
#[cfg(test)]
mod runtime50_tests;
pub(crate) mod sets;
#[cfg(test)]
mod terminal_tests;
#[cfg(test)]
mod timer_tests;
mod tuning;
pub(crate) mod values;
pub(crate) mod yielding;

use std::cell::RefCell;

use crate::TaskCtx;
use crate::ordered::footprint::{Footprint, TaskId};
use crate::ordered::{Backend, Next, Operation, Receipt};
use hirsel_rt_operations::descriptors::op_descriptor;

/// The caller reserves creation identities before logging. Reservations and
/// ABI images belong to the command; initialization consumes each token once.
/// Mutex wrappers remain owner-local allocations outside these cell effects.
/// Drops and task/host effects still need retirement/credit adapters.
pub(crate) enum Command {
    PlainClose {
        word: u64,
        mutex: bool,
    },
    TaskDetachAll {
        scope: u64,
        set: u64,
    },
    TaskDetach {
        scope: u64,
        child: u64,
    },
    TaskSpawn {
        reservation: RefCell<Option<lifecycle::Spawn>>,
    },
    RootRelease {
        block: *mut crate::alloc::Header,
    },
    ScopeEnter {
        index: u32,
        generation: u32,
        owner: TaskId,
    },
    TaskSetCreate {
        index: u32,
        generation: u32,
        scope: u64,
    },
    TaskSetAdd {
        scope: u64,
        set: u64,
        child: u64,
    },
    TaskSetClose {
        set: u64,
    },
    TaskCancel {
        scope: u64,
        child: u64,
    },
    SenderClose {
        sender: u64,
    },
    /// A strict-head, same-owner callback. This marker is never offered or
    /// retained in a speculative log; Objects cannot execute it by itself.
    Head {
        name: &'static str,
    },
    AtomicCreate {
        reservation: RefCell<Option<crate::sync::Reserved>>,
        image: Vec<u8>,
        payload_offset: u64,
        payload_size: u64,
    },
    MutexCreate {
        reservation: RefCell<Option<crate::sync::Reserved>>,
        descriptor: u32,
        none: Vec<u8>,
        image: RefCell<Vec<u8>>,
    },
    ChannelOpen {
        reservation: RefCell<Option<crate::channels::ReservedChannel>>,
        descriptor: u32,
        none: Vec<u8>,
    },
    SenderDup {
        reservation: RefCell<Option<crate::channels::ReservedEndpoint>>,
        sender: u64,
    },
    MutexCause {
        word: u64,
    },
    AtomicLoad {
        word: u64,
        bytes: usize,
    },
    AtomicStore {
        word: u64,
        image: Vec<u8>,
    },
    Send {
        sender: u64,
        image: RefCell<Vec<u8>>,
    },
    MutexTake {
        word: u64,
        none: Vec<u8>,
    },
    MutexPut {
        word: u64,
        image: RefCell<Vec<u8>>,
    },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::PlainClose { .. } => "drop.trivial",
            Self::TaskDetachAll { .. } => "task.detach_all",
            Self::TaskDetach { .. } => "task.detach",
            Self::TaskSpawn { .. } => "task.async",
            Self::RootRelease { .. } => "root.release",
            Self::ScopeEnter { .. } => "task.scope_enter",
            Self::TaskSetCreate { .. } => "task.set",
            Self::TaskSetAdd { .. } => "task.add",
            Self::TaskSetClose { .. } => "drop.trivial",
            Self::TaskCancel { .. } => "task.cancel",
            Self::SenderClose { .. } => "drop.trivial",
            Self::Head { name } => name,
            Self::AtomicCreate { .. } => "atomic.create",
            Self::MutexCreate { .. } => "mutex.create",
            Self::ChannelOpen { .. } => "channel.open",
            Self::SenderDup { .. } => "sender.dup",
            Self::MutexCause { .. } => "mutex.cause",
            Self::AtomicLoad { .. } => "atomic.load",
            Self::AtomicStore { .. } => "atomic.store",
            Self::Send { .. } => "channel.send",
            Self::MutexTake { .. } => "mutex.take",
            Self::MutexPut { .. } => "mutex.put",
        }
    }

    pub(crate) fn operation(self, footprint: Footprint) -> Operation<Self> {
        Operation {
            descriptor: op_descriptor(self.name()).expect("native ordered operation descriptor"),
            footprint,
            payload: self,
        }
    }
}

pub(crate) struct Outcome {
    /// Language-visible op result, separate from the ABI success byte.
    pub(crate) status: u32,
    /// Output carrier, including the None image of a successful send/put.
    pub(crate) image: Vec<u8>,
    /// A cause is applied only to the command's owner continuation, never to
    /// the worker/ctx that happened to commit a different task's logged op.
    pub(crate) chain: Vec<crate::failure::Chained>,
    /// Results fixed by an effect whose touched items are discovered at commit
    /// (for example a cancellation's linked descendants). Never re-sample at
    /// retirement or retain a native task row in this optional evidence.
    canonical: Option<events::Subevents>,
    /// Exact parent cancellation read at spawn installation, including None.
    inheritance: Option<Option<u32>>,
    pending: RefCell<Option<crate::channels::PendingSend>>,
}

#[derive(Clone)]
pub(super) struct Returned {
    pub(super) status: u32,
    pub(super) image: Vec<u8>,
    pub(super) chain: Vec<crate::failure::Chained>,
}

impl Outcome {
    fn returned(&self) -> Returned {
        Returned {
            status: self.status,
            image: self.image.clone(),
            chain: self.chain.clone(),
        }
    }

    fn image(status: u32, image: Vec<u8>) -> Self {
        Self {
            status,
            image,
            chain: Vec::new(),
            canonical: None,
            inheritance: None,
            pending: RefCell::new(None),
        }
    }
}

/// Only retirement calls `record` and `poll`. The integration layer supplies
/// the canonical writer and actual logical scheduling-point adapter; neither
/// an early commit nor these object primitives may invoke them on its own.
pub(crate) struct Objects<'a> {
    ctx: *mut TaskCtx,
    record: &'a mut dyn FnMut(TaskId, &'static str, &Outcome, u64),
    poll: Option<&'a mut dyn FnMut() -> Vec<TaskId>>,
    /// Applied after restoring Frontier and before this retirement's poll.
    spawned: Vec<(TaskId, crate::ordered::footprint::Held)>,
}

impl<'a> Objects<'a> {
    /// # Safety
    /// The caller owns the live run's scheduler role for every use. Commands
    /// carry live handles of that run and complete images of matching size.
    /// The callbacks retain no runtime borrow across a role leave.
    pub(crate) unsafe fn new(
        ctx: *mut TaskCtx,
        record: &'a mut dyn FnMut(TaskId, &'static str, &Outcome, u64),
        poll: &'a mut dyn FnMut() -> Vec<TaskId>,
    ) -> Self {
        Self {
            ctx,
            record,
            poll: Some(poll),
            spawned: Vec::new(),
        }
    }

    /// Native retirement returns Progress::Poll so the caller restores its
    /// driver before readiness accesses a home service and releases the role.
    /// # Safety
    /// As for new; the caller completes every returned poll once, in order.
    pub(crate) unsafe fn deferred(
        ctx: *mut TaskCtx,
        record: &'a mut dyn FnMut(TaskId, &'static str, &Outcome, u64),
    ) -> Self {
        Self {
            ctx,
            record,
            poll: None,
            spawned: Vec::new(),
        }
    }
}

impl Backend<Command> for Objects<'_> {
    type Result = Outcome;

    fn can_retire(&mut self, _task: TaskId, op: &Operation<Command>) -> bool {
        if let Command::TaskSpawn { reservation } = &op.payload {
            let s = unsafe {
                (*(*(*self.ctx).run_ptr()).tasks.get())
                    .as_deref_mut()
                    .unwrap()
            };
            return reservation
                .borrow()
                .as_ref()
                .is_none_or(|spawn| spawn.ready(s));
        }
        true
    }

    fn can_commit_early(&mut self, op: &Operation<Command>) -> bool {
        super::role::with_role(self.ctx, |s| match &op.payload {
            Command::Head { .. }
            | Command::PlainClose { .. }
            | Command::TaskDetachAll { .. }
            | Command::TaskDetach { .. }
            | Command::TaskSpawn { .. }
            | Command::ScopeEnter { .. }
            | Command::TaskCancel { .. }
            | Command::TaskSetCreate { .. }
            | Command::TaskSetAdd { .. }
            | Command::TaskSetClose { .. }
            | Command::RootRelease { .. }
            | Command::SenderClose { .. } => false,
            Command::AtomicLoad { word, .. }
            | Command::AtomicStore { word, .. }
            | Command::MutexPut { word, .. } => s.sync.initialized(*word),
            Command::MutexTake { word, .. } => s.sync.initialized(*word) && s.sync.available(*word),
            Command::MutexCause { word } => s.sync.initialized(*word) && s.sync.is_dead(*word),
            Command::Send { sender, .. } | Command::SenderDup { sender, .. } => {
                s.channels.initialized(*sender, true)
            }
            Command::AtomicCreate { .. }
            | Command::MutexCreate { .. }
            | Command::ChannelOpen { .. } => true,
        })
    }

    fn commit(&mut self, task: TaskId, op: &Operation<Command>, early: bool) -> Receipt<Outcome> {
        assert_eq!(op.descriptor.name, op.payload.name());
        if let Command::PlainClose { word, mutex } = &op.payload {
            assert!(!early, "close only retires");
            let before = unsafe { (*self.ctx).ordered_freed.get() };
            let mut canonical;
            let observing = events::observing(self.ctx);
            if *mutex {
                let left = super::role::with_role(self.ctx, |s| {
                    if observing {
                        s.sync.close_images(*word)
                    } else {
                        s.sync.close(*word)
                    }
                });
                canonical = unsafe {
                    events::close_observed(
                        self.ctx,
                        crate::ordered::objects::Kind::Mutex,
                        *word,
                        left.as_ref()
                            .map(|(images, descriptor)| (images, *descriptor)),
                    )
                };
                if let Some((image, descriptor)) = left {
                    unsafe { drops::plain_images(self.ctx, &image, descriptor) };
                }
            } else {
                let left = super::role::with_role(self.ctx, |s| {
                    if observing {
                        s.channels.close_images(*word)
                    } else {
                        s.channels.close(*word)
                    }
                });
                canonical = unsafe {
                    events::close_observed(
                        self.ctx,
                        crate::ordered::objects::Kind::Receiver,
                        *word,
                        left.as_ref()
                            .map(|(images, descriptor)| (images, *descriptor)),
                    )
                };
                if let Some((images, descriptor)) = left {
                    unsafe { drops::plain_images(self.ctx, &images, descriptor) };
                }
            }
            if observing {
                canonical = canonical
                    .zip(unsafe { events::root_releases(self.ctx) })
                    .map(|(mut events, roots)| {
                        events.extend(roots);
                        events
                    });
            }
            let credit = unsafe { (*self.ctx).ordered_freed.replace(before) } - before;
            let mut result = Outcome::image(0, Vec::new());
            result.canonical = canonical;
            return Receipt {
                result,
                next: Next::Keep,
                credit,
            };
        }
        if let Command::RootRelease { block } = &op.payload {
            assert!(!early, "root releases only retire");
            // Every preceding segment free was captured before entering the
            // backend. Preserve any callback-local credits independently of
            // this op's newly discovered deaths, even for another log owner.
            let before = unsafe { (*self.ctx).ordered_freed.get() };
            unsafe { crate::roots::retire_shared(self.ctx, *block) };
            let credit = unsafe { (*self.ctx).ordered_freed.replace(before) } - before;
            let mut result = Outcome::image(0, Vec::new());
            if unsafe { (*self.ctx).run_ref() }
                .ordered_roots
                .get()
                .is_some()
            {
                result.canonical = unsafe { events::root_releases(self.ctx) };
            }
            return Receipt {
                result,
                next: Next::Keep,
                credit,
            };
        }
        // SAFETY: Objects::new's caller guarantees ownership and image/handle
        // validity. All byte storage below is owned, including output images.
        let (result, next) = super::role::with_role(self.ctx, |s| unsafe {
            match &op.payload {
                Command::PlainClose { .. } => unreachable!("cleanup owns no scheduler borrow"),
                Command::TaskDetachAll { scope, set } => {
                    assert!(!early, "bulk detach only retires");
                    let scope_word = *scope;
                    let set_word = *set;
                    let scope = s.scope_handle(scope_word);
                    let set = s.set_handle(*set, scope);
                    let words = s.set(set).members.clone();
                    let members: Vec<u64> = words
                        .into_iter()
                        .flatten()
                        .filter(|&word| s.member(word).is_some())
                        .collect();
                    let canonical =
                        events::begin_bulk_detach(s, task, scope_word, set_word, &members);
                    let super::Logical::Ordered(driver) = &mut s.logical else {
                        unreachable!()
                    };
                    assert!(
                        driver.detaching.insert(task, set).is_none(),
                        "one active bulk call per task"
                    );
                    let mut outcome = Outcome::image(0, Vec::new());
                    outcome.canonical = canonical;
                    (
                        outcome,
                        Next::Call {
                            members,
                            then_yield: true,
                        },
                    )
                }
                Command::TaskSpawn { reservation } => {
                    assert!(!early, "a child cannot start before spawn retirement");
                    let spawn = reservation
                        .borrow_mut()
                        .take()
                        .expect("spawn committed once");
                    let preclaimed = spawn.preclaimed;
                    let (word, held, inherited) = spawn.install(s);
                    self.spawned.push((word, held));
                    let mut outcome = Outcome::image(0, word.to_ne_bytes().to_vec());
                    outcome.inheritance = Some(inherited);
                    // A CPU batch publishes all its already claimed children
                    // before joining them. Dispatch starts when the publisher
                    // yields to the installed batch.
                    (outcome, if preclaimed { Next::Keep } else { Next::Yield })
                }
                Command::RootRelease { .. } => unreachable!("handled without a scheduler borrow"),
                Command::Head { .. } => panic!("strict head effect requires its owner callback"),
                Command::ScopeEnter {
                    index,
                    generation,
                    owner,
                } => {
                    assert!(!early, "scope entry only retires");
                    let task = *owner as u32;
                    assert_eq!(s.task(task).generation, (*owner >> 32) as u32);
                    assert!(s.scopes[*index as usize].is_none(), "a reserved scope row");
                    s.scopes[*index as usize] = Some(super::Scope {
                        generation: *generation,
                        owner: task,
                        children: Vec::new(),
                        canceled: Default::default(),
                        deadline: None,
                        completions: std::sync::atomic::AtomicU64::new(0),
                    });
                    s.task(task).scopes.push(*index);
                    s.cancel_epoch.changed();
                    let word = super::handle(*index, *generation);
                    (Outcome::image(0, word.to_ne_bytes().to_vec()), Next::Yield)
                }
                Command::TaskSetCreate {
                    index,
                    generation,
                    scope,
                } => {
                    assert!(!early, "task-set creation only retires");
                    let scope = s.scope_handle(*scope);
                    assert!(s.sets[*index as usize].is_none(), "a reserved set row");
                    s.sets[*index as usize] = Some(super::TaskSet {
                        generation: *generation,
                        scope,
                        members: Vec::new(),
                    });
                    let word = crate::channels::ENDPOINT
                        | super::TASK_SET
                        | super::handle(*index, *generation);
                    (Outcome::image(0, word.to_ne_bytes().to_vec()), Next::Yield)
                }
                Command::TaskSetAdd { scope, set, child } => {
                    assert!(!early, "task-set insertion only retires");
                    let scope = s.scope_handle(*scope);
                    s.task_handle(*child, scope);
                    let set = s.set_handle(*set, scope);
                    s.set(set).members.push(Some(*child));
                    (Outcome::image(0, Vec::new()), Next::Yield)
                }
                Command::TaskSetClose { set } => {
                    assert!(!early, "task-set close only retires");
                    let index = *set as u32;
                    assert_eq!(
                        s.set(index).generation,
                        ((set & !(crate::channels::ENDPOINT | super::TASK_SET)) >> 32) as u32
                    );
                    s.close_set(index);
                    (Outcome::image(0, Vec::new()), Next::Keep)
                }
                Command::TaskCancel { scope, child } => {
                    assert!(!early, "cancellation only retires");
                    let scope = s.scope_handle(*scope);
                    let child = s.task_handle(*child, scope);
                    let canonical = events::cancel_observed(s, child, super::CANCEL_REQUESTED);
                    let mut outcome = Outcome::image(0, Vec::new());
                    outcome.canonical = canonical;
                    (outcome, Next::Yield)
                }
                Command::TaskDetach { scope, child: word } => {
                    assert!(!early, "detach only retires");
                    let canonical = events::detach_observed(s, *scope, *word);
                    let scope = s.scope_handle(*scope);
                    let child = s.task_handle(*word, scope);
                    let row = s.task(child);
                    row.detached = true;
                    let next = if row.state == super::State::Holding {
                        Next::Call {
                            members: vec![*word],
                            then_yield: true,
                        }
                    } else {
                        if row.state == super::State::Done && row.failure.is_none() {
                            assert!(super::readiness::saved(s, child));
                            s.scope(scope).children.retain(|&c| c != child);
                            s.vacate_task(child);
                        }
                        Next::Yield
                    };
                    let mut outcome = Outcome::image(0, Vec::new());
                    outcome.canonical = canonical;
                    (outcome, next)
                }
                Command::SenderClose { sender } => {
                    assert!(!early, "a sender close only retires");
                    assert!(
                        s.channels.close(*sender).is_none(),
                        "sender close has no leftovers"
                    );
                    (Outcome::image(0, Vec::new()), Next::Keep)
                }
                Command::AtomicCreate {
                    reservation,
                    image,
                    payload_offset,
                    payload_size,
                } => {
                    let reservation = reservation
                        .borrow_mut()
                        .take()
                        .expect("creation committed once");
                    let word = reservation.word();
                    s.sync.initialize_image(
                        reservation,
                        image.as_ptr(),
                        image.len() as u64,
                        *payload_offset,
                        *payload_size,
                    );
                    (Outcome::image(0, word.to_ne_bytes().to_vec()), Next::Yield)
                }
                Command::MutexCreate {
                    reservation,
                    descriptor,
                    none,
                    image,
                } => {
                    let layout = (*self.ctx).descriptor(*descriptor);
                    let element = crate::channels::Element::new(*descriptor, layout, none.as_ptr());
                    let reservation = reservation
                        .borrow_mut()
                        .take()
                        .expect("creation committed once");
                    let word = reservation.word();
                    s.sync.initialize_mutex(
                        reservation,
                        element,
                        &*layout,
                        image.borrow_mut().as_mut_ptr(),
                    );
                    (Outcome::image(0, word.to_ne_bytes().to_vec()), Next::Yield)
                }
                Command::ChannelOpen {
                    reservation,
                    descriptor,
                    none,
                } => {
                    let layout = (*self.ctx).descriptor(*descriptor);
                    let element = crate::channels::Element::new(*descriptor, layout, none.as_ptr());
                    let reservation = reservation
                        .borrow_mut()
                        .take()
                        .expect("creation committed once");
                    let (sender, receiver) = reservation.words();
                    s.channels.initialize_open(reservation, element, &*layout);
                    let image = [sender.to_ne_bytes(), receiver.to_ne_bytes()].concat();
                    (Outcome::image(0, image), Next::Yield)
                }
                Command::SenderDup {
                    reservation,
                    sender,
                } => {
                    let reservation = reservation.borrow_mut().take().expect("dup committed once");
                    let word = reservation.word();
                    s.channels.initialize_dup(reservation, *sender);
                    (Outcome::image(0, word.to_ne_bytes().to_vec()), Next::Yield)
                }
                Command::MutexCause { word } => {
                    let chain = s
                        .sync
                        .dead(*word)
                        .unwrap_or_else(|| crate::fault("the cause of a mutex that is not dead"));
                    let mut outcome = Outcome::image(0, Vec::new());
                    outcome.canonical = events::mutex_failures(self.ctx, s, *word, &chain);
                    outcome.chain = chain;
                    (outcome, Next::Keep)
                }
                Command::AtomicLoad { word, bytes } => {
                    let mut image = vec![0; *bytes];
                    s.sync.read(*word, image.as_mut_ptr(), *bytes as u64);
                    (Outcome::image(0, image), Next::Yield)
                }
                Command::AtomicStore { word, image } => {
                    s.sync.write(*word, image.as_ptr(), image.len() as u64);
                    (Outcome::image(0, Vec::new()), Next::Yield)
                }
                Command::Send { sender, image } => {
                    let channel = s.channels.channel(*sender, true);
                    let mut image = image.borrow_mut();
                    let observed = if events::enabled(s) {
                        let descriptor = s.channels.payload(channel).element.descriptor;
                        events::object_value(
                            self.ctx,
                            s,
                            crate::ordered::objects::Kind::Sender,
                            *sender,
                            descriptor,
                            image.as_ptr(),
                        )
                    } else {
                        None
                    };
                    let (status, pending) = if early {
                        s.channels.send_pending(channel, image.as_mut_ptr())
                    } else {
                        (s.channels.send(channel, image.as_mut_ptr()), None)
                    };
                    (
                        Outcome {
                            status,
                            image: image.clone(),
                            chain: Vec::new(),
                            canonical: events::value_subevent(observed, status),
                            inheritance: None,
                            pending: RefCell::new(pending),
                        },
                        Next::Yield,
                    )
                }
                Command::MutexTake { word, none } => {
                    let mut image = none.clone();
                    let status = if s.sync.is_dead(*word) {
                        1
                    } else {
                        s.sync.take(*word, image.as_mut_ptr());
                        0
                    };
                    let observed = if events::enabled(s) {
                        let descriptor = s.sync.mutex(*word).element.descriptor;
                        events::object_value(
                            self.ctx,
                            s,
                            crate::ordered::objects::Kind::Mutex,
                            *word,
                            descriptor,
                            image.as_ptr(),
                        )
                    } else {
                        None
                    };
                    let mut outcome = Outcome::image(status, image);
                    outcome.canonical = events::value_subevent(observed, status);
                    (outcome, Next::Keep)
                }
                Command::MutexPut { word, image } => {
                    let mut image = image.borrow_mut();
                    let observed = if events::enabled(s) {
                        let descriptor = s.sync.mutex(*word).element.descriptor;
                        events::object_value(
                            self.ctx,
                            s,
                            crate::ordered::objects::Kind::Mutex,
                            *word,
                            descriptor,
                            image.as_ptr(),
                        )
                    } else {
                        None
                    };
                    s.sync.put(*word, image.as_mut_ptr());
                    let mut outcome = Outcome::image(0, image.clone());
                    outcome.canonical = events::value_subevent(observed, 0);
                    (outcome, Next::Yield)
                }
            }
        });
        Receipt {
            result,
            next,
            credit: 0,
        }
    }

    fn record(&mut self, task: TaskId, op: &Operation<Command>, result: &Outcome, retirement: u64) {
        (self.record)(task, op.descriptor.name, result, retirement);
    }

    fn record_retirement(
        &mut self,
        task: TaskId,
        op: &Operation<Command>,
        result: &Outcome,
        retirement: crate::ordered::Retirement<'_>,
    ) {
        self.record(task, op, result, retirement.index);
        events::record(self.ctx, task, Some((op, result)), retirement);
    }

    fn record_budget_stop(
        &mut self,
        task: TaskId,
        before: &crate::budget::stretch::Stretch,
        retirement: u64,
        kind: hirsel_run_schema::BudgetKind,
    ) {
        events::charges::stopped(self.ctx, task, before, retirement, kind);
    }

    fn record_continuation(&mut self, task: TaskId, retirement: crate::ordered::Retirement<'_>) {
        events::record(self.ctx, task, None, retirement);
    }

    fn publish(&mut self, _: TaskId, _: &Operation<Command>, result: &Outcome) {
        if let Some(pending) = result.pending.borrow_mut().take() {
            super::role::with_role(self.ctx, |s| s.channels.publish(pending));
        }
    }

    fn call(&mut self, caller: TaskId, child: TaskId) {
        super::role::with_role(self.ctx, |s| lifecycle::call_saved(s, caller, child));
    }
    fn call_member(&mut self, caller: TaskId, child: TaskId) {
        super::role::with_role(self.ctx, |s| lifecycle::detach_member(s, caller, child));
    }
    fn call_returned(&mut self, caller: TaskId) {
        super::role::with_role(self.ctx, |s| {
            let super::Logical::Ordered(driver) = &mut s.logical else {
                unreachable!()
            };
            if let Some(set) = driver.detaching.remove(&caller) {
                s.close_set(set);
                events::complete_bulk_detach(s, caller);
            }
        });
    }

    fn defer_poll(&self) -> bool {
        self.poll.is_none()
    }

    fn poll(&mut self) -> Vec<TaskId> {
        (self
            .poll
            .as_mut()
            .expect("restore driver before deferred poll"))()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::stretch::{Budgets, Settled, Stretch};
    use crate::ordered::footprint::{Held, Item, ObjectId};
    use crate::ordered::{Frontier, Offer, Policy};
    use hirsel_run_config::{Commit, Reorder};
    use std::collections::BTreeSet;

    struct Run(*mut TaskCtx);
    impl Run {
        fn new() -> Self {
            Self::with_layouts(&[])
        }
        fn with_layouts(layouts: &[*const crate::Layout]) -> Self {
            let config = crate::Config {
                layout_count: layouts.len() as u32,
                layouts: layouts.as_ptr(),
                ..crate::Config::new()
            };
            let mut ctx = std::ptr::null_mut();
            assert_eq!(
                unsafe { crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx) },
                0
            );
            Self(ctx)
        }
    }
    impl Drop for Run {
        fn drop(&mut self) {
            unsafe { crate::context::hirsel_rt_v2_ctx_destroy(self.0) }
        }
    }
    fn empty() -> Stretch {
        Stretch::new(Budgets {
            fuel: false,
            memory: false,
        })
    }
    fn policy() -> Policy {
        Policy {
            commit: Commit::Shared,
            run_ahead: 32,
            reorder: Reorder::Auto,
            speculation_bytes: u64::MAX,
        }
    }
    fn object(n: u64) -> ObjectId {
        ObjectId {
            creator: vec![],
            ordinal: n,
        }
    }
    fn footprint(n: u64, write: bool) -> Footprint {
        let items = BTreeSet::from([Item::Object(object(n))]);
        if write {
            Footprint {
                writes: items,
                ..Footprint::default()
            }
        } else {
            Footprint {
                reads: items,
                ..Footprint::default()
            }
        }
    }
    fn bytes(n: u64) -> Vec<u8> {
        n.to_ne_bytes().to_vec()
    }
    unsafe fn atomic(ctx: *mut TaskCtx, n: u64) -> u64 {
        unsafe {
            super::super::scheduler(ctx)
                .sync
                .create_image((&raw const n).cast(), 8, 0, 8)
        }
    }

    fn layout() -> crate::Layout {
        crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        }
    }

    #[test]
    fn a_logged_shared_release_stays_live_through_older_thaws_and_retires_without_a_poll() {
        let root_layout = crate::Layout {
            flags: crate::LAYOUT_FROZEN,
            ..layout()
        };
        let run = Run::with_layouts(&[&root_layout]);
        let root = unsafe {
            crate::alloc::hirsel_rt_v2_alloc(run.0, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0)
        };
        unsafe { crate::roots::hirsel_rt_v2_root_dup(run.0, root) };
        let anchor = unsafe { atomic(run.0, 0) };
        let mut events = Vec::new();
        let mut record = |task, name, _: &Outcome, retirement| {
            events.push((task, name, retirement));
        };
        let mut polls = 0;
        let mut poll = || {
            polls += 1;
            Vec::new()
        };
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let mut f = Frontier::new(
            0,
            policy(),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.held.insert(0, Held::empty());
        f.enqueue(2, Held::empty());
        let release = || Command::RootRelease { block: root }.operation(Footprint::default());
        assert!(!backend.can_commit_early(&release()));
        assert_eq!(
            f.offer(2, empty(), release(), &mut backend).0,
            Offer::Logged
        );
        assert_eq!(unsafe { crate::roots::count(&*run.0, root) }, 2);
        assert!(
            !unsafe { crate::roots::sole(root) },
            "older thaw keeps its serial outcome"
        );
        let yield_op = Command::AtomicLoad {
            word: anchor,
            bytes: 8,
        }
        .operation(footprint(0, false));
        f.apply_head(empty(), yield_op, &mut backend).unwrap();
        assert_eq!(
            f.absorb(&mut backend),
            Ok(crate::ordered::Progress::Absorbed(2))
        );
        assert!(unsafe { crate::roots::sole(root) });
        f.apply_head(empty(), release(), &mut backend).unwrap();
        drop(backend);
        assert_eq!(polls, 1, "neither root release is a scheduling point");
        assert_eq!(
            events[1..],
            [(2, "root.release", 1), (2, "root.release", 1)]
        );
    }

    #[test]
    fn logged_scope_entries_publish_the_owners_stack_in_retirement_order() {
        let run = Run::new();
        let anchor = unsafe { atomic(run.0, 0) };
        let s = super::super::scheduler(run.0);
        s.tasks.push(Some(super::super::Task::new(
            3,
            std::ptr::null_mut(),
            None,
            vec![0],
            None,
        )));
        s.scopes.extend([None, None]);
        let owner = super::super::handle(1, 3);
        let mut events = Vec::new();
        let mut record =
            |task, name, _: &Outcome, retirement| events.push((task, name, retirement));
        let mut poll = Vec::new;
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let mut f = Frontier::new(
            0,
            policy(),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.held.insert(0, Held::empty());
        f.enqueue(2, Held::empty());
        f.enqueue(owner, Held::empty());
        for index in 0..2 {
            let entry = Command::ScopeEnter {
                index,
                generation: index + 4,
                owner,
            }
            .operation(Footprint {
                writes: BTreeSet::from([Item::Task(owner)]),
                ..Footprint::default()
            });
            assert!(!backend.can_commit_early(&entry));
            assert_eq!(
                f.offer(owner, empty(), entry, &mut backend).0,
                Offer::Logged
            );
        }
        assert!(
            super::super::scheduler(run.0)
                .scopes
                .iter()
                .all(Option::is_none)
        );
        assert!(super::super::scheduler(run.0).task(1).scopes.is_empty());
        let load = || {
            Command::AtomicLoad {
                word: anchor,
                bytes: 8,
            }
            .operation(footprint(0, false))
        };
        for index in 0..2 {
            f.apply_head(empty(), load(), &mut backend).unwrap();
            f.apply_head(empty(), load(), &mut backend).unwrap();
            f.absorb(&mut backend).unwrap();
            let s = super::super::scheduler(run.0);
            assert_eq!(s.task(1).scopes, (0..=index).collect::<Vec<_>>());
            assert_eq!(
                s.scope(index).owner,
                1,
                "committer's current task is not the scope owner"
            );
            assert!(s.task(0).scopes.is_empty());
            if index == 0 {
                assert!(s.scopes[1].is_none());
            }
        }
        assert_eq!(
            events
                .iter()
                .filter(|(_, name, _)| *name == "task.scope_enter")
                .copied()
                .collect::<Vec<_>>(),
            vec![
                (owner, "task.scope_enter", 2),
                (owner, "task.scope_enter", 5)
            ]
        );
    }

    #[test]
    fn a_provisional_task_set_creates_adds_and_closes_only_in_its_retiring_turns() {
        let run = Run::new();
        let anchor = unsafe { atomic(run.0, 0) };
        let (scope, generation) = super::super::open_scope(run.0, None);
        let scope_word = super::super::handle(scope, generation);
        let child = super::super::Task::new(3, std::ptr::null_mut(), Some(scope), vec![0], None);
        let s = super::super::scheduler(run.0);
        s.tasks.push(Some(child));
        let child = super::super::handle(1, 3);
        s.sets.push(None);
        let set = crate::channels::ENDPOINT | super::super::TASK_SET | super::super::handle(0, 4);
        let mut events = Vec::new();
        let mut record =
            |task, name, _: &Outcome, retirement| events.push((task, name, retirement));
        let mut polls = 0;
        let mut poll = || {
            polls += 1;
            Vec::new()
        };
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let mut f = Frontier::new(
            0,
            policy(),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.held.insert(0, Held::empty());
        f.enqueue(1, Held::empty());
        f.enqueue(2, Held::empty());
        let load = || {
            Command::AtomicLoad {
                word: anchor,
                bytes: 8,
            }
            .operation(footprint(0, false))
        };
        f.apply_head(empty(), load(), &mut backend).unwrap();
        let create = Command::TaskSetCreate {
            index: 0,
            generation: 4,
            scope: scope_word,
        }
        .operation(Footprint {
            writes: BTreeSet::from([Item::Task(set)]),
            ..Footprint::default()
        });
        let add = Command::TaskSetAdd {
            scope: scope_word,
            set,
            child,
        }
        .operation(Footprint {
            writes: BTreeSet::from([Item::Task(set), Item::Task(child)]),
            ..Footprint::default()
        });
        let close = Command::TaskSetClose { set }.operation(Footprint {
            writes: BTreeSet::from([Item::Task(set)]),
            ..Footprint::default()
        });
        assert!(!backend.can_commit_early(&create));
        assert!(!backend.can_commit_early(&add));
        assert!(!backend.can_commit_early(&close));
        assert_eq!(f.offer(2, empty(), create, &mut backend).0, Offer::Logged);
        assert_eq!(f.offer(2, empty(), add, &mut backend).0, Offer::Logged);
        assert_eq!(f.offer(2, empty(), close, &mut backend).0, Offer::Logged);
        assert!(super::super::scheduler(run.0).sets[0].is_none());
        f.apply_head(empty(), load(), &mut backend).unwrap();
        f.absorb(&mut backend).unwrap();
        assert!(super::super::scheduler(run.0).set(0).members.is_empty());
        // A provisional handle is usable by the owner's later logged commands,
        // but neither the child membership nor row reuse can become visible early.
        f.apply_head(empty(), load(), &mut backend).unwrap();
        f.apply_head(empty(), load(), &mut backend).unwrap();
        f.absorb(&mut backend).unwrap();
        assert_eq!(
            super::super::scheduler(run.0).set(0).members,
            vec![Some(child)]
        );
        assert_eq!(super::super::scheduler(run.0).set_slots.free_len(), 0);
        // Add yields: the following close waits through both older queued turns.
        f.apply_head(empty(), load(), &mut backend).unwrap();
        f.apply_head(empty(), load(), &mut backend).unwrap();
        assert!(super::super::scheduler(run.0).sets[0].is_some());
        f.absorb(&mut backend).unwrap();
        assert!(super::super::scheduler(run.0).sets[0].is_none());
        assert_eq!(super::super::scheduler(run.0).set_slots.free_len(), 1);
        assert_eq!(super::super::scheduler(run.0).task_handle(child, scope), 1);
        assert_eq!(
            polls, 8,
            "closing a set keeps the turn and leaves its children live"
        );
        assert_eq!(
            events,
            vec![
                (0, "atomic.load", 0),
                (1, "atomic.load", 1),
                (2, "task.set", 2),
                (0, "atomic.load", 3),
                (1, "atomic.load", 4),
                (2, "task.add", 5),
                (0, "atomic.load", 6),
                (1, "atomic.load", 7),
                (2, "drop.trivial", 8)
            ]
        );
    }

    #[test]
    fn a_logged_native_cancel_is_invisible_to_older_waiter_polls() {
        let run = Run::new();
        let anchor = unsafe { atomic(run.0, 0) };
        let (scope, generation) = super::super::open_scope(run.0, None);
        let scope_word = super::super::handle(scope, generation);
        let child = super::super::Task::new(3, std::ptr::null_mut(), Some(scope), vec![0], None);
        let s = super::super::scheduler(run.0);
        s.tasks.push(Some(child));
        s.task(1).state = super::super::State::Parked;
        s.task(1).wait = Some(super::super::Wait::Host { token: 7 });
        let child_word = super::super::handle(1, 3);
        let mut observations = Vec::new();
        let mut poll = || {
            let s = super::super::scheduler(run.0);
            observations.push((s.canceled_task(1, 0), s.task(1).woken));
            Vec::new()
        };
        let mut events = Vec::new();
        let mut record = |task, name, _: &Outcome, retirement| {
            events.push((task, name, retirement));
        };
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let mut f = Frontier::new(
            0,
            policy(),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.held.insert(0, Held::empty());
        f.enqueue(1, Held::empty());
        f.enqueue(2, Held::empty());
        let load = || {
            Command::AtomicLoad {
                word: anchor,
                bytes: 8,
            }
            .operation(footprint(0, false))
        };
        f.apply_head(empty(), load(), &mut backend).unwrap();
        let cancel = Command::TaskCancel {
            scope: scope_word,
            child: child_word,
        }
        .operation(Footprint {
            writes: BTreeSet::from([Item::Cancel(child_word), Item::Task(child_word)]),
            ..Footprint::default()
        });
        assert!(!backend.can_commit_early(&cancel));
        assert_eq!(f.offer(2, empty(), cancel, &mut backend).0, Offer::Logged);
        assert_eq!(super::super::scheduler(run.0).canceled_task(1, 0), None);
        assert!(!super::super::scheduler(run.0).task(1).woken);
        f.apply_head(empty(), load(), &mut backend).unwrap();
        f.absorb(&mut backend).unwrap();
        assert_eq!(
            observations,
            vec![
                (None, false),
                (None, false),
                (Some(super::super::CANCEL_REQUESTED), true)
            ]
        );
        assert_eq!(
            events,
            vec![
                (0, "atomic.load", 0),
                (1, "atomic.load", 1),
                (2, "task.cancel", 2)
            ]
        );
    }

    #[test]
    fn a_logged_sender_close_keeps_older_polls_open_and_blocks_conflicting_early_ops() {
        let layout = layout();
        let run = Run::with_layouts(&[&layout]);
        let anchor = unsafe { atomic(run.0, 0) };
        let independent = unsafe { atomic(run.0, 0) };
        let element =
            unsafe { crate::channels::Element::new(0, &layout, [0u64; 2].as_ptr().cast()) };
        let (sender, receiver) = super::super::scheduler(run.0)
            .channels
            .open(element, &layout);
        let channel = super::super::scheduler(run.0)
            .channels
            .channel(sender, true);
        super::super::scheduler(run.0).channels.wait(channel, 0, 9);
        let mut observations = Vec::new();
        let mut poll = || {
            let channels = &mut super::super::scheduler(run.0).channels;
            let mut touched = Vec::new();
            channels.take_touched(&mut touched);
            observations.push((channels.ready(channel), touched.contains(&channel)));
            Vec::new()
        };
        let mut events = Vec::new();
        let mut record = |_, name, _: &Outcome, retirement| events.push((name, retirement));
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let mut f = Frontier::new(
            0,
            policy(),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.held.insert(0, Held::empty());
        f.enqueue(1, Held::empty());
        f.enqueue(2, Held::empty());
        let load = || {
            Command::AtomicLoad {
                word: anchor,
                bytes: 8,
            }
            .operation(footprint(0, false))
        };
        f.apply_head(empty(), load(), &mut backend).unwrap();
        let close = Command::SenderClose { sender }.operation(footprint(1, true));
        assert!(!backend.can_commit_early(&close));
        assert_eq!(f.offer(2, empty(), close, &mut backend).0, Offer::Logged);
        assert!(!super::super::scheduler(run.0).channels.ready(channel));
        // A later operation on this channel cannot pass the uncommitted close.
        let conflicting = Command::Send {
            sender,
            image: RefCell::new([bytes(1), bytes(9)].concat()),
        }
        .operation(footprint(1, true));
        assert!(!f.eligible(2, &conflicting));
        // An independent store can pass it within this same turn.
        let store = Command::AtomicStore {
            word: independent,
            image: bytes(17),
        }
        .operation(footprint(2, true));
        assert_eq!(f.offer(2, empty(), store, &mut backend).0, Offer::Early);
        f.apply_head(empty(), load(), &mut backend).unwrap();
        f.absorb(&mut backend).unwrap();
        assert!(super::super::scheduler(run.0).channels.ready(channel));
        // Close keeps the turn: only the following store runs its poll.
        f.absorb(&mut backend).unwrap();
        assert_eq!(
            observations,
            vec![(false, false), (false, false), (true, true)]
        );
        assert_eq!(
            events,
            vec![
                ("atomic.load", 0),
                ("atomic.load", 1),
                ("drop.trivial", 2),
                ("atomic.store", 2)
            ]
        );
        super::super::scheduler(run.0).channels.close(receiver);
    }

    #[test]
    fn deferred_native_retirement_publishes_send_before_requesting_its_poll() {
        let layout = layout();
        let run = Run::with_layouts(&[&layout]);
        let anchor = unsafe { atomic(run.0, 0) };
        let element =
            unsafe { crate::channels::Element::new(2, &layout, [0u64; 2].as_ptr().cast()) };
        let (sender, receiver) = super::super::scheduler(run.0)
            .channels
            .open(element, &layout);
        let channel = super::super::scheduler(run.0)
            .channels
            .channel(sender, true);
        super::super::scheduler(run.0).channels.wait(channel, 0, 9);
        let mut events = Vec::new();
        let mut record =
            |task, name, _: &Outcome, retirement| events.push((task, name, retirement));
        let mut backend = unsafe { Objects::deferred(run.0, &mut record) };
        let mut f = Frontier::new(
            0,
            policy(),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.held.insert(0, Held::empty());
        f.enqueue(1, Held::empty());
        f.enqueue(2, Held::empty());
        let load = || {
            Command::AtomicLoad {
                word: anchor,
                bytes: 8,
            }
            .operation(footprint(0, false))
        };
        assert_eq!(
            f.apply_head(empty(), load(), &mut backend),
            Ok(crate::ordered::Progress::Poll)
        );
        assert_eq!(f.head(), None);
        f.complete_poll(Vec::new());
        assert_eq!(
            f.offer(
                2,
                empty(),
                Command::Send {
                    sender,
                    image: RefCell::new([bytes(1), bytes(99)].concat())
                }
                .operation(footprint(1, true)),
                &mut backend
            )
            .0,
            Offer::Early
        );
        assert!(!super::super::scheduler(run.0).channels.ready(channel));
        assert_eq!(
            f.apply_head(empty(), load(), &mut backend),
            Ok(crate::ordered::Progress::Poll)
        );
        assert!(!super::super::scheduler(run.0).channels.ready(channel));
        f.complete_poll(Vec::new());
        assert_eq!(f.absorb(&mut backend), Ok(crate::ordered::Progress::Poll));
        let channels = &mut super::super::scheduler(run.0).channels;
        assert!(channels.ready(channel));
        let mut touched = Vec::new();
        channels.take_touched(&mut touched);
        assert_eq!(touched, vec![channel]);
        f.complete_poll(Vec::new());
        assert_eq!(
            events,
            vec![
                (0, "atomic.load", 0),
                (1, "atomic.load", 1),
                (2, "channel.send", 2)
            ]
        );
        super::super::scheduler(run.0).channels.close(sender);
        super::super::scheduler(run.0).channels.close(receiver);
    }

    #[test]
    fn a_logged_creation_initializes_its_provisional_word_only_at_retirement() {
        let run = Run::new();
        let anchor = unsafe { atomic(run.0, 0) };
        let reservation = super::super::scheduler(run.0).sync.reserve_image();
        let word = reservation.word();
        let mut events = Vec::new();
        let mut record = |task, name, result: &Outcome, retirement| {
            events.push((task, name, result.image.clone(), retirement));
        };
        let mut poll = Vec::new;
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let mut f = Frontier::new(
            0,
            Policy {
                reorder: Reorder::Off,
                ..policy()
            },
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.enqueue(1, Held::empty());
        assert_eq!(
            f.offer(
                1,
                empty(),
                Command::AtomicCreate {
                    reservation: RefCell::new(Some(reservation)),
                    image: bytes(41),
                    payload_offset: 0,
                    payload_size: 8,
                }
                .operation(footprint(1, true)),
                &mut backend
            )
            .0,
            Offer::Logged
        );
        assert!(!super::super::scheduler(run.0).sync.initialized(word));
        assert!(!backend.can_commit_early(
            &Command::AtomicLoad { word, bytes: 8 }.operation(footprint(1, false))
        ));
        assert_eq!(
            f.offer(
                1,
                empty(),
                Command::AtomicStore {
                    word,
                    image: bytes(42)
                }
                .operation(footprint(1, true)),
                &mut backend
            )
            .0,
            Offer::Logged
        );
        assert!(!super::super::scheduler(run.0).sync.initialized(word));
        f.apply_head(
            empty(),
            Command::AtomicLoad {
                word: anchor,
                bytes: 8,
            }
            .operation(footprint(0, false)),
            &mut backend,
        )
        .unwrap();
        f.absorb(&mut backend).unwrap();
        let mut image = 0u64;
        unsafe {
            super::super::scheduler(run.0)
                .sync
                .read(word, (&raw mut image).cast(), 8)
        };
        assert_eq!(image, 41);
        f.apply_head(
            empty(),
            Command::AtomicLoad {
                word: anchor,
                bytes: 8,
            }
            .operation(footprint(0, false)),
            &mut backend,
        )
        .unwrap();
        f.absorb(&mut backend).unwrap();
        unsafe {
            super::super::scheduler(run.0)
                .sync
                .read(word, (&raw mut image).cast(), 8)
        };
        assert_eq!(image, 42);
        assert_eq!(
            events.iter().map(|e| e.1).collect::<Vec<_>>(),
            vec![
                "atomic.load",
                "atomic.create",
                "atomic.load",
                "atomic.store"
            ]
        );
        assert_eq!(events[1].2, bytes(word));
    }

    #[test]
    fn native_creation_and_dup_preserve_reserved_words_and_move_owned_images() {
        let layout = layout();
        let run = Run::with_layouts(&[&layout]);
        let mut record = |_, _, _: &Outcome, _| {};
        let mut poll = Vec::new;
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let reservation = super::super::scheduler(run.0).channels.reserve_open();
        let (sender, receiver) = reservation.words();
        let duplicate = super::super::scheduler(run.0).channels.reserve_dup();
        let dup_word = duplicate.word();
        assert!(
            !super::super::scheduler(run.0)
                .channels
                .initialized(sender, true)
        );
        let dup = Command::SenderDup {
            reservation: RefCell::new(Some(duplicate)),
            sender,
        }
        .operation(footprint(0, true));
        assert!(!backend.can_commit_early(&dup));
        let open = Command::ChannelOpen {
            reservation: RefCell::new(Some(reservation)),
            descriptor: 2,
            none: vec![0; 16],
        }
        .operation(footprint(0, true));
        let receipt = backend.commit(1, &open, true);
        assert_eq!(
            receipt.result.image,
            [bytes(sender), bytes(receiver)].concat()
        );
        assert!(backend.can_commit_early(&dup));
        let receipt = backend.commit(1, &dup, true);
        assert_eq!(receipt.result.image, bytes(dup_word));
        let channel = super::super::scheduler(run.0)
            .channels
            .channel(sender, true);
        super::super::scheduler(run.0).channels.close(sender);
        assert!(!super::super::scheduler(run.0).channels.ready(channel));
        super::super::scheduler(run.0).channels.close(dup_word);
        assert!(super::super::scheduler(run.0).channels.ready(channel));
        super::super::scheduler(run.0).channels.close(receiver);

        let reservation = super::super::scheduler(run.0).sync.reserve_mutex();
        let word = reservation.word();
        let create = Command::MutexCreate {
            reservation: RefCell::new(Some(reservation)),
            descriptor: 2,
            none: vec![0; 16],
            image: RefCell::new([bytes(1), bytes(97)].concat()),
        }
        .operation(footprint(1, true));
        let take = Command::MutexTake {
            word,
            none: vec![0; 16],
        }
        .operation(footprint(1, true));
        assert!(!backend.can_commit_early(&take));
        let receipt = backend.commit(1, &create, true);
        assert_eq!(receipt.result.image, bytes(word));
        if let Command::MutexCreate { image, .. } = &create.payload {
            assert_eq!(*image.borrow(), vec![0; 16]);
        }
        assert!(backend.can_commit_early(&take));
        assert_eq!(
            backend.commit(1, &take, true).result.image,
            [bytes(1), bytes(97)].concat()
        );
        super::super::scheduler(run.0).sync.close(word);
    }

    #[test]
    fn an_early_mutex_cause_returns_an_owned_chain_for_its_owner() {
        let layout = layout();
        let run = Run::with_layouts(&[&layout]);
        let element =
            unsafe { crate::channels::Element::new(2, &layout, [0u64; 2].as_ptr().cast()) };
        let mut image = [1u64, 42];
        let word = unsafe {
            super::super::scheduler(run.0).sync.create_mutex(
                element,
                &layout,
                image.as_mut_ptr().cast(),
            )
        };
        let failure = crate::failure::FailureRecord {
            kind: 1,
            rule: 2,
            site: 3,
            operation_site: 4,
        };
        unsafe {
            super::super::scheduler(run.0)
                .sync
                .take(word, image.as_mut_ptr().cast())
        };
        super::super::scheduler(run.0)
            .sync
            .kill(word, vec![(failure, vec![9])]);
        let mut record = |_, _, _: &Outcome, _| {};
        let mut poll = Vec::new;
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let cause = Command::MutexCause { word }.operation(footprint(0, false));
        assert!(backend.can_commit_early(&cause));
        let receipt = backend.commit(1, &cause, true);
        assert_eq!(receipt.next, Next::Keep);
        assert_eq!(receipt.result.chain, vec![(failure, vec![9])]);
        assert!(
            !unsafe { (*run.0).failure.active() },
            "committer is not the op's owner"
        );
        super::super::scheduler(run.0).sync.close(word);
    }

    #[test]
    fn an_independent_native_load_passes_a_head_and_is_recorded_only_at_retirement() {
        let run = Run::new();
        let a = unsafe { atomic(run.0, 41) };
        let b = unsafe { atomic(run.0, 0) };
        let mut events = Vec::new();
        let mut record = |task, name, result: &Outcome, retirement| {
            events.push((task, name, result.image.clone(), retirement));
        };
        let mut poll = Vec::new;
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let mut f = Frontier::new(
            0,
            policy(),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.held.insert(0, Held::empty());
        let mut head = Held::empty();
        head.objects.insert(object(1));
        f.enqueue(1, head);
        f.enqueue(2, Held::empty());
        f.apply_head(
            empty(),
            Command::AtomicLoad { word: a, bytes: 8 }.operation(footprint(0, false)),
            &mut backend,
        )
        .unwrap();
        assert_eq!(f.head(), Some(1));
        let (offer, _) = f.offer(
            2,
            empty(),
            Command::AtomicLoad { word: a, bytes: 8 }.operation(footprint(0, false)),
            &mut backend,
        );
        assert_eq!(offer, Offer::Early);
        assert_eq!(f.early_result(2).unwrap().image, bytes(41));
        f.apply_head(
            empty(),
            Command::AtomicStore {
                word: b,
                image: bytes(7),
            }
            .operation(footprint(1, true)),
            &mut backend,
        )
        .unwrap();
        assert_eq!(f.head(), Some(2));
        f.absorb(&mut backend).unwrap();
        assert_eq!(
            events.iter().map(|e| (e.0, e.3)).collect::<Vec<_>>(),
            vec![(0, 0), (1, 1), (2, 2)]
        );
        assert_eq!(events[2].2, bytes(41));
    }

    #[test]
    fn an_early_native_send_is_invisible_through_the_older_poll_and_touches_at_retirement() {
        let run = Run::new();
        let a = unsafe { atomic(run.0, 0) };
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let none = [0u64; 2];
        let element = unsafe { crate::channels::Element::new(0, &layout, none.as_ptr().cast()) };
        let (sender, receiver) = super::super::scheduler(run.0)
            .channels
            .open(element, &layout);
        let channel = super::super::scheduler(run.0)
            .channels
            .channel(sender, true);
        super::super::scheduler(run.0).channels.wait(channel, 0, 3);
        let mut observations = Vec::new();
        let mut poll = || {
            let channels = &mut super::super::scheduler(run.0).channels;
            let mut touched = Vec::new();
            channels.take_touched(&mut touched);
            observations.push((channels.ready(channel), touched.contains(&channel)));
            Vec::new()
        };
        let mut events = Vec::new();
        let mut record = |task, _, result: &Outcome, _| {
            events.push((task, result.status));
        };
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let mut f = Frontier::new(
            0,
            policy(),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        f.held.insert(0, Held::empty());
        let mut head = Held::empty();
        head.objects.insert(object(0));
        f.enqueue(1, head);
        f.enqueue(2, Held::empty());
        f.apply_head(
            empty(),
            Command::AtomicLoad { word: a, bytes: 8 }.operation(footprint(0, false)),
            &mut backend,
        )
        .unwrap();
        let image = [bytes(1), bytes(99)].concat();
        let (offer, _) = f.offer(
            2,
            empty(),
            Command::Send {
                sender,
                image: RefCell::new(image),
            }
            .operation(footprint(1, true)),
            &mut backend,
        );
        assert_eq!(offer, Offer::Early);
        assert_eq!(f.early_result(2).unwrap().status, crate::channels::DONE);
        assert_eq!(f.early_result(2).unwrap().image, vec![0; 16]);
        assert!(!super::super::scheduler(run.0).channels.ready(channel));
        f.apply_head(
            empty(),
            Command::AtomicStore {
                word: a,
                image: bytes(7),
            }
            .operation(footprint(0, true)),
            &mut backend,
        )
        .unwrap();
        f.absorb(&mut backend).unwrap();
        assert_eq!(
            observations,
            vec![(false, false), (false, false), (true, true)]
        );
        assert_eq!(events, vec![(0, 0), (1, 0), (2, 0)]);
        let mut image = [0u64; 2];
        assert_eq!(
            unsafe {
                super::super::scheduler(run.0)
                    .channels
                    .receive(channel, image.as_mut_ptr().cast())
            },
            Ok(true)
        );
        assert_eq!(image, [1, 99]);
        super::super::scheduler(run.0).channels.close(sender);
        super::super::scheduler(run.0).channels.close(receiver);
    }

    #[test]
    fn unavailable_mutexes_cannot_commit_early_and_dead_takes_preserve_the_none_image() {
        let run = Run::new();
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let none = [7u64, 0];
        let element = unsafe { crate::channels::Element::new(0, &layout, none.as_ptr().cast()) };
        let mut image = [1u64, 99];
        let mutex = unsafe {
            super::super::scheduler(run.0).sync.create_mutex(
                element,
                &layout,
                image.as_mut_ptr().cast(),
            )
        };
        assert_eq!(image, none);
        let mut record = |_, _, _: &Outcome, _| {};
        let mut poll = Vec::new;
        let mut backend = unsafe { Objects::new(run.0, &mut record, &mut poll) };
        let operation = Command::MutexTake {
            word: mutex,
            none: [bytes(7), bytes(0)].concat(),
        }
        .operation(footprint(0, true));
        assert!(backend.can_commit_early(&operation));
        let receipt = backend.commit(1, &operation, true);
        assert_eq!(receipt.result.image, [bytes(1), bytes(99)].concat());
        assert_eq!(receipt.next, Next::Keep);
        assert!(!backend.can_commit_early(&operation));
        let put = Command::MutexPut {
            word: mutex,
            image: RefCell::new(receipt.result.image),
        }
        .operation(footprint(0, true));
        let receipt = backend.commit(1, &put, true);
        assert_eq!(receipt.result.image, [bytes(7), bytes(0)].concat());
        assert_eq!(receipt.next, Next::Yield);
        assert!(backend.can_commit_early(&operation));
        let receipt = backend.commit(1, &operation, true);
        assert_eq!(receipt.result.image, [bytes(1), bytes(99)].concat());
        super::super::scheduler(run.0).sync.kill(mutex, Vec::new());
        assert!(backend.can_commit_early(&operation));
        let receipt = backend.commit(1, &operation, true);
        assert_eq!(receipt.result.status, 1);
        assert_eq!(receipt.result.image, [bytes(7), bytes(0)].concat());
        super::super::scheduler(run.0).sync.close(mutex);
    }
}
