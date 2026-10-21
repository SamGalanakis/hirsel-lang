//! Native object effects of the ordered frontier. Inputs and results own their
//! ABI images; none retains a pointer into a caller's carrier across run-ahead.
//! This module does not select a physical worker or run a scheduling point.

pub(crate) mod driver;

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
    AtomicCreate {
        reservation: RefCell<Option<crate::sync::Reserved>>,
        image: Vec<u8>,
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
    pending: RefCell<Option<crate::channels::PendingSend>>,
}

impl Outcome {
    fn image(status: u32, image: Vec<u8>) -> Self {
        Self {
            status,
            image,
            chain: Vec::new(),
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
        }
    }
}

impl Backend<Command> for Objects<'_> {
    type Result = Outcome;

    fn can_commit_early(&mut self, op: &Operation<Command>) -> bool {
        let s = super::scheduler(self.ctx);
        match &op.payload {
            Command::AtomicLoad { word, .. }
            | Command::AtomicStore { word, .. }
            | Command::MutexPut { word, .. } => s.sync.initialized(*word),
            Command::MutexTake { word, .. } => s.sync.initialized(*word) && s.sync.available(*word),
            Command::MutexCause { word } => {
                s.sync.initialized(*word) && s.sync.dead(*word).is_some()
            }
            Command::Send { sender, .. } | Command::SenderDup { sender, .. } => {
                s.channels.initialized(*sender, true)
            }
            Command::AtomicCreate { .. }
            | Command::MutexCreate { .. }
            | Command::ChannelOpen { .. } => true,
        }
    }

    fn commit(&mut self, _: TaskId, op: &Operation<Command>, early: bool) -> Receipt<Outcome> {
        assert_eq!(op.descriptor.name, op.payload.name());
        let s = super::scheduler(self.ctx);
        // SAFETY: Objects::new's caller guarantees ownership and image/handle
        // validity. All byte storage below is owned, including output images.
        let (result, next) = unsafe {
            match &op.payload {
                Command::AtomicCreate { reservation, image } => {
                    let reservation = reservation
                        .borrow_mut()
                        .take()
                        .expect("creation committed once");
                    let word = reservation.word();
                    s.sync
                        .initialize_image(reservation, image.as_ptr(), image.len() as u64);
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
                    let chain =
                        s.sync.dead(*word).cloned().unwrap_or_else(|| {
                            crate::fault("the cause of a mutex that is not dead")
                        });
                    let mut outcome = Outcome::image(0, Vec::new());
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
                            pending: RefCell::new(pending),
                        },
                        Next::Yield,
                    )
                }
                Command::MutexTake { word, none } => {
                    let mut image = none.clone();
                    let status = if s.sync.dead(*word).is_some() {
                        1
                    } else {
                        s.sync.take(*word, image.as_mut_ptr());
                        0
                    };
                    (Outcome::image(status, image), Next::Keep)
                }
                Command::MutexPut { word, image } => {
                    let mut image = image.borrow_mut();
                    s.sync.put(*word, image.as_mut_ptr());
                    (Outcome::image(0, image.clone()), Next::Yield)
                }
            }
        };
        Receipt {
            result,
            next,
            credit: 0,
        }
    }

    fn record(&mut self, task: TaskId, op: &Operation<Command>, result: &Outcome, retirement: u64) {
        (self.record)(task, op.descriptor.name, result, retirement);
    }

    fn publish(&mut self, _: TaskId, _: &Operation<Command>, result: &Outcome) {
        if let Some(pending) = result.pending.borrow_mut().take() {
            super::scheduler(self.ctx).channels.publish(pending);
        }
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
                .create_image((&raw const n).cast(), 8)
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
