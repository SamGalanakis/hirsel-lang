//! Native object effects of the ordered frontier. Inputs and results own their
//! ABI images; none retains a pointer into a caller's carrier across run-ahead.
//! This module does not select a physical worker or run a scheduling point.

use std::cell::RefCell;

use crate::TaskCtx;
use crate::ordered::footprint::{Footprint, TaskId};
use crate::ordered::{Backend, Next, Operation, Receipt};
use hirsel_rt_operations::descriptors::op_descriptor;

/// Creations and drops need provisional identity/root-credit adapters before
/// joining this set. Unsupported boundaries remain pending at the head.
pub(crate) enum Command {
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
    pending: RefCell<Option<crate::channels::PendingSend>>,
}

impl Outcome {
    fn image(status: u32, image: Vec<u8>) -> Self {
        Self {
            status,
            image,
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
    poll: &'a mut dyn FnMut() -> Vec<TaskId>,
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
        Self { ctx, record, poll }
    }
}

impl Backend<Command> for Objects<'_> {
    type Result = Outcome;

    fn can_commit_early(&mut self, op: &Operation<Command>) -> bool {
        match op.payload {
            Command::MutexTake { word, .. } => super::scheduler(self.ctx).sync.available(word),
            _ => true,
        }
    }

    fn commit(&mut self, _: TaskId, op: &Operation<Command>, early: bool) -> Receipt<Outcome> {
        assert_eq!(op.descriptor.name, op.payload.name());
        let s = super::scheduler(self.ctx);
        // SAFETY: Objects::new's caller guarantees ownership and image/handle
        // validity. All byte storage below is owned, including output images.
        let (result, next) = unsafe {
            match &op.payload {
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

    fn poll(&mut self) -> Vec<TaskId> {
        (self.poll)()
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
            let mut ctx = std::ptr::null_mut();
            assert_eq!(
                unsafe { crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx) },
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
