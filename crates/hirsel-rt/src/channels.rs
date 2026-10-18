//! Unbounded FIFO channels between the tasks of one run (CONC-6).
//!
//! A message crosses as the one-element list block the compiled `channel.send` body builds, so
//! the runtime moves block references and never reads a payload's layout: a receive hands the
//! block to the compiled `channel.receive` body, which moves the element out. Messages nobody
//! receives are released through their blocks' own descriptors when the last receiver closes.
//!
//! Sender and Receiver are run-local handle words with [`ENDPOINT`] set, an endpoint index and
//! a generation; dropping one closes it through [`close`], reached from the handle close of
//! `finalize.rs` instead of the Io adapter.

use std::collections::VecDeque;
use std::ptr::NonNull;

use crate::{Header, fault};

/// The tag of a channel endpoint's handle word.
pub(crate) const ENDPOINT: u64 = 1 << 63;

/// A send or receive status: done, closed, or canceled (`CANCELED + cause`).
pub(crate) const DONE: u32 = 0;
pub(crate) const CLOSED: u32 = 1;
pub(crate) const CANCELED: u32 = 2;

struct Channel {
    queue: VecDeque<NonNull<Header>>,
    senders: u32,
    receivers: u32,
}

#[derive(Clone, Copy)]
struct Endpoint {
    generation: u32,
    channel: u32,
    sender: bool,
}

/// The run's channels and their open endpoints.
#[derive(Default)]
pub(crate) struct Channels {
    channels: Vec<Option<Channel>>,
    vacant_channels: Vec<u32>,
    endpoints: Vec<Option<Endpoint>>,
    vacant_endpoints: Vec<u32>,
    generation: u32,
}

impl Channels {
    /// A fresh channel's Sender and Receiver words.
    pub(crate) fn open(&mut self) -> (u64, u64) {
        let row = Channel {
            queue: VecDeque::new(),
            senders: 1,
            receivers: 1,
        };
        let channel = match self.vacant_channels.pop() {
            Some(index) => {
                self.channels[index as usize] = Some(row);
                index
            }
            None => {
                self.channels.push(Some(row));
                (self.channels.len() - 1) as u32
            }
        };
        (self.endpoint(channel, true), self.endpoint(channel, false))
    }

    /// Another Sender of the channel `sender` names.
    pub(crate) fn dup(&mut self, sender: u64) -> u64 {
        let channel = self.channel(sender, true);
        self.row(channel).senders += 1;
        self.endpoint(channel, true)
    }

    fn endpoint(&mut self, channel: u32, sender: bool) -> u64 {
        self.generation = (self.generation + 1) & 0x7fff_ffff;
        let row = Endpoint {
            generation: self.generation,
            channel,
            sender,
        };
        let index = match self.vacant_endpoints.pop() {
            Some(index) => {
                self.endpoints[index as usize] = Some(row);
                index
            }
            None => {
                self.endpoints.push(Some(row));
                (self.endpoints.len() - 1) as u32
            }
        };
        ENDPOINT | u64::from(self.generation) << 32 | u64::from(index)
    }

    /// The channel an open endpoint word of the given kind names.
    pub(crate) fn channel(&self, word: u64, sender: bool) -> u32 {
        match self.lookup(word) {
            Some((_, endpoint)) if endpoint.sender == sender => endpoint.channel,
            _ => fault("a channel endpoint that is closed, of another run or of the other kind"),
        }
    }

    fn lookup(&self, word: u64) -> Option<(u32, Endpoint)> {
        if word & ENDPOINT == 0 {
            return None;
        }
        let index = word as u32;
        let endpoint = (*self.endpoints.get(index as usize)?)?;
        (endpoint.generation == ((word & !ENDPOINT) >> 32) as u32).then_some((index, endpoint))
    }

    fn row(&mut self, channel: u32) -> &mut Channel {
        self.channels[channel as usize]
            .as_mut()
            .unwrap_or_else(|| fault("a vacant channel row"))
    }

    /// Queue `message` unless the channel has no receiver left (`CLOSED`).
    pub(crate) fn send(&mut self, channel: u32, message: NonNull<Header>) -> u32 {
        let row = self.row(channel);
        if row.receivers == 0 {
            return CLOSED;
        }
        row.queue.push_back(message);
        DONE
    }

    /// The oldest queued message, `Err(CLOSED)` once the queue is empty and no sender is left,
    /// or `Ok(None)` while a sender may still send.
    pub(crate) fn receive(&mut self, channel: u32) -> Result<Option<NonNull<Header>>, u32> {
        let row = self.row(channel);
        match row.queue.pop_front() {
            Some(message) => Ok(Some(message)),
            None if row.senders == 0 => Err(CLOSED),
            None => Ok(None),
        }
    }

    /// Close the endpoint `word`; the messages a channel's last receiver leaves are returned,
    /// oldest first, for the caller to release.
    pub(crate) fn close(&mut self, word: u64) -> VecDeque<NonNull<Header>> {
        let Some((index, endpoint)) = self.lookup(word) else {
            fault("a channel endpoint closed twice or of another run");
        };
        self.endpoints[index as usize] = None;
        self.vacant_endpoints.push(index);
        let row = self.row(endpoint.channel);
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
            self.channels[endpoint.channel as usize] = None;
            self.vacant_channels.push(endpoint.channel);
        }
        left
    }
}
