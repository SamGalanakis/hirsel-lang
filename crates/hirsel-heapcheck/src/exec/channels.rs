//! Channels (CONC-6): the runtime's channel table (`hirsel-rt/src/channels.rs`)
//! with each queued message a harness root, so H observes the moves, the
//! receive order and the releases of unreceived messages native code performs.
//!
//! A message is the `Some` image of the compiled bodies' `Option[List[T]]`
//! slot; a send moves it into the queue and leaves `None`, a receive moves it
//! into the receiver's `None` slot, and the last receiver's close drops what
//! is left, oldest first. Sender and Receiver are handle words with
//! [`ENDPOINT`] set, opened in the ledger like any operation's handle and
//! closed by their drop's `CloseHandle`.
use super::tasks::{Park, Wait};
use super::*;
use std::collections::VecDeque;

/// The tag of a channel endpoint's handle word.
pub(super) const ENDPOINT: u64 = 1 << 63;
/// Harness root keys of the queued messages.
const MESSAGE_ROOT: u64 = 1 << 61;
/// A send or receive status: done, closed, or canceled (`CANCELED + cause`).
const DONE: u32 = 0;
const CLOSED: u32 = 1;
const CANCELED: u32 = 2;

#[derive(Clone)]
struct Channel {
    /// Queued messages, oldest first.
    queue: VecDeque<u64>,
    senders: u32,
    receivers: u32,
}

#[derive(Clone, Copy)]
struct Endpoint {
    generation: u32,
    channel: u32,
    sender: bool,
}

#[derive(Clone, Default)]
pub(super) struct Channels {
    channels: Vec<Option<Channel>>,
    endpoints: Vec<Option<Endpoint>>,
    generation: u32,
    next: u64,
    /// Each queued message's layout.
    layouts: std::collections::BTreeMap<u64, LayoutId>,
}

impl Channels {
    fn open(&mut self) -> (u64, u64) {
        self.channels.push(Some(Channel {
            queue: VecDeque::new(),
            senders: 1,
            receivers: 1,
        }));
        let channel = (self.channels.len() - 1) as u32;
        (self.endpoint(channel, true), self.endpoint(channel, false))
    }

    fn endpoint(&mut self, channel: u32, sender: bool) -> u64 {
        self.generation = (self.generation + 1) & 0x7fff_ffff;
        self.endpoints.push(Some(Endpoint {
            generation: self.generation,
            channel,
            sender,
        }));
        ENDPOINT | u64::from(self.generation) << 32 | (self.endpoints.len() - 1) as u64
    }

    fn lookup(&self, word: u64) -> Option<(u32, Endpoint)> {
        if word & ENDPOINT == 0 {
            return None;
        }
        let index = word as u32;
        let endpoint = (*self.endpoints.get(index as usize)?)?;
        (endpoint.generation == ((word & !ENDPOINT) >> 32) as u32).then_some((index, endpoint))
    }

    /// The channel an open endpoint word of the given kind names.
    fn channel(&self, word: u64, sender: bool) -> Checked<u32> {
        match self.lookup(word) {
            Some((_, endpoint)) if endpoint.sender == sender => Ok(endpoint.channel),
            _ => fail(
                V::Malformed,
                "a channel endpoint that is closed, of another run or of the other kind",
            ),
        }
    }

    fn row(&mut self, channel: u32) -> Checked<&mut Channel> {
        self.channels[channel as usize]
            .as_mut()
            .ok_or_else(|| err(V::Malformed, "a vacant channel row"))
    }
}

impl<S: Stage> Machine<'_, S> {
    pub(super) fn channel_operation(
        &mut self,
        f: &mut Frame,
        site: &CallSite,
        name: &str,
        arguments: &[CallArgument],
        normal: Option<EdgeId>,
    ) -> Checked<Control> {
        let result = site
            .result
            .ok_or_else(|| err(V::Malformed, "a channel operation without a result"))?;
        match name {
            "channel.open" => {
                let (sender, receiver) = self.scheduler().channels.open();
                let layout = self.value_type(f.unit, result)?.layout;
                let image = self.operation_image(
                    f,
                    layout,
                    OpValue::Aggregate(vec![
                        OpValue::OpenHandle(sender),
                        OpValue::OpenHandle(receiver),
                    ]),
                    arguments,
                    &mut Default::default(),
                )?;
                let repr = self.value_type(f.unit, result)?.repr;
                let val = self.val_from(image, repr, layout)?.0;
                self.task_yield()?;
                self.task_return(f, arguments, Some((result, val)), normal)
            }
            "sender.dup" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 1)?)?;
                let s = self.scheduler();
                let channel = s.channels.channel(word, true)?;
                s.channels.row(channel)?.senders += 1;
                let dup = s.channels.endpoint(channel, true);
                self.ledger
                    .handles
                    .insert(dup, (crate::ledger::HandleState::Open, false));
                self.task_yield()?;
                self.task_return(f, arguments, Some((result, Val::Handle(dup))), normal)
            }
            "$intrinsic.ChannelSend" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                let r = self.task_slot(f, &Self::task_argument(arguments, 1)?)?;
                let s = self.scheduler();
                let channel = s.channels.channel(word, true)?;
                let status = match s.canceled() {
                    Some(cause) => CANCELED + cause,
                    None if s.channels.row(channel)?.receivers == 0 => CLOSED,
                    None => {
                        self.enqueue(channel, r.ptr, r.layout)?;
                        DONE
                    }
                };
                self.task_yield()?;
                self.channel_status(f, arguments, result, status, normal)
            }
            "$intrinsic.ChannelReceive" => {
                let s = self.scheduler();
                let current = s.current;
                if s.task(current)?.wait == Some(Wait::Receive) {
                    // A poll moved a message in or found the channel closed or the task canceled.
                    let row = s.task(current)?;
                    row.wait = None;
                    let status = row.status;
                    return self.channel_status(f, arguments, result, status, normal);
                }
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                let r = self.task_slot(f, &Self::task_argument(arguments, 1)?)?;
                let channel = self.scheduler().channels.channel(word, false)?;
                match self.receive_ready(current, channel, r.ptr, r.layout)? {
                    Some(status) => {
                        self.task_yield()?;
                        self.channel_status(f, arguments, result, status, normal)
                    }
                    None => {
                        self.scheduler().task(current)?.wait = Some(Wait::Receive);
                        self.park_until(
                            Park::Receive {
                                channel,
                                slot: r.ptr,
                                layout: r.layout,
                            },
                            true,
                        )?;
                        Ok(Control::Continue)
                    }
                }
            }
            _ => unreachable!("a channel operation"),
        }
    }

    fn channel_status(
        &mut self,
        f: &mut Frame,
        arguments: &[CallArgument],
        result: ValueId,
        status: u32,
        normal: Option<EdgeId>,
    ) -> Checked<Control> {
        let val = Val::Bits {
            scalar: ScalarKind::U32,
            bits: u128::from(status),
        };
        self.task_return(f, arguments, Some((result, val)), normal)
    }

    /// Move the `Some` message at `at` into `channel`'s queue, leaving `None`.
    fn enqueue(&mut self, channel: u32, at: Ptr, layout: LayoutId) -> Checked<()> {
        let size = self.size(layout)?;
        let owned = self.resources_of(layout)?;
        let image = self.take_part(at, size, &owned);
        let (val, _, credited) = self.val_from(image, ValueRepr::Aggregate, layout)?;
        let mut none = Extent::new(size);
        none.write_bytes(0, &vec![0; size as usize]);
        self.write_value(
            at,
            layout,
            Moved {
                val: Val::Image(none),
                credited: false,
                borrow: None,
            },
        )?;
        let s = self.scheduler();
        let id = s.channels.next;
        s.channels.next += 1;
        s.channels.layouts.insert(id, layout);
        s.channels.row(channel)?.queue.push_back(id);
        self.set_root(
            MESSAGE_ROOT | id,
            Some(Held {
                val,
                credits: u32::from(credited),
                borrow: None,
            }),
        );
        Ok(())
    }

    /// A receive's outcome for `task`, unless it must wait: canceled, else the
    /// oldest message (moved into the `None` slot `at`), else closed once no
    /// sender is left.
    pub(super) fn receive_ready(
        &mut self,
        task: u32,
        channel: u32,
        at: Ptr,
        layout: LayoutId,
    ) -> Checked<Option<u32>> {
        let s = self.scheduler();
        if let Some(cause) = s.canceled_task(task)? {
            return Ok(Some(CANCELED + cause));
        }
        let row = s.channels.row(channel)?;
        let Some(id) = row.queue.pop_front() else {
            return Ok((row.senders == 0).then_some(CLOSED));
        };
        s.channels.layouts.remove(&id);
        let held = self
            .roots
            .get(&(MESSAGE_ROOT | id))
            .cloned()
            .ok_or_else(|| err(V::Malformed, "a queued message without its root"))?;
        self.set_root(MESSAGE_ROOT | id, None);
        let size = self.size(layout)?;
        let owned = self.resources_of(layout)?;
        // The receiver's slot holds `None`: plain bits with nothing to release.
        self.take_part(at, size, &owned);
        self.write_value(
            at,
            layout,
            Moved {
                val: held.val,
                credited: held.credits > 0,
                borrow: None,
            },
        )?;
        Ok(Some(DONE))
    }

    /// Close the channel endpoint `word` from its drop; a last receiver's
    /// unreceived messages are dropped by `work`, oldest first.
    pub(super) fn close_endpoint(&mut self, word: u64, work: &mut Vec<Work>) -> Checked<()> {
        let s = self.scheduler();
        let Some((index, endpoint)) = s.channels.lookup(word) else {
            return fail(V::DoubleClose, "a channel endpoint closed twice");
        };
        s.channels.endpoints[index as usize] = None;
        let row = s.channels.row(endpoint.channel)?;
        let mut left = VecDeque::new();
        if endpoint.sender {
            row.senders -= 1;
        } else {
            row.receivers -= 1;
            if row.receivers == 0 {
                left = std::mem::take(&mut row.queue);
            }
        }
        if row.senders == 0 && row.receivers == 0 {
            s.channels.channels[endpoint.channel as usize] = None;
        }
        // The work list is a stack: the oldest message's drop goes on last.
        for id in left.into_iter().rev() {
            let s = self.scheduler();
            let layout = s
                .channels
                .layouts
                .remove(&id)
                .ok_or_else(|| err(V::Malformed, "a queued message without its layout"))?;
            let held = self
                .roots
                .get(&(MESSAGE_ROOT | id))
                .cloned()
                .ok_or_else(|| err(V::Malformed, "a queued message without its root"))?;
            self.set_root(MESSAGE_ROOT | id, None);
            let temp = self.temporary(layout)?;
            self.write_value(
                temp,
                layout,
                Moved {
                    val: held.val,
                    credited: held.credits > 0,
                    borrow: None,
                },
            )?;
            work.push(Work::Death {
                alloc: temp.alloc,
                raw: None,
            });
            work.push(Work::DropAt {
                at: temp,
                layout,
                mask: None,
            });
        }
        Ok(())
    }
}
