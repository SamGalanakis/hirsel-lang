//! Committed FIFO positions, independent of Resource lineage. A runtime row
//! cannot create, move or drop credit without a separately checked binding.
use super::{ChannelDirection, Error, Row};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Queues(BTreeMap<u64, Queue>);
struct Queue {
    layout: u32,
    sent: u64,
    received: u64,
    pending: BTreeSet<u64>,
    detached: bool,
    receipt: Option<Receipt>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Receipt {
    pub task: u64,
    pub layout: u32,
    pub item: u64,
    pub direction: ChannelDirection,
    pub status: u32,
}
impl Queues {
    pub(super) fn bind(&mut self, queue: u64, receipt: Receipt) -> Result<(), Error> {
        let state = self.0.get_mut(&queue).ok_or(Error::InvalidQueue(queue))?;
        if state.receipt.take() != Some(receipt) {
            return Err(Error::InvalidQueue(queue));
        }
        Ok(())
    }
    pub(super) fn observe(&mut self, row: Row) -> Result<bool, Error> {
        let (queue, layout) = match row {
            Row::QueueCreated { queue, layout, .. } => {
                if queue == 0 || self.0.contains_key(&queue) {
                    return Err(Error::InvalidQueue(queue));
                }
                self.0.insert(
                    queue,
                    Queue {
                        layout,
                        sent: 0,
                        received: 0,
                        pending: BTreeSet::new(),
                        detached: false,
                        receipt: None,
                    },
                );
                return Ok(true);
            }
            Row::QueueSent { queue, layout, .. }
            | Row::QueuePublished { queue, layout, .. }
            | Row::QueueReceived { queue, layout, .. }
            | Row::QueueDetached { queue, layout, .. } => (queue, layout),
            _ => return Ok(false),
        };
        let state = self.0.get_mut(&queue).ok_or(Error::InvalidQueue(queue))?;
        if state.layout != layout {
            return Err(Error::InvalidQueue(queue));
        }
        let valid = match row {
            Row::QueueSent {
                item,
                status,
                pending,
                ..
            } => {
                if item != state.sent {
                    false
                } else if status == 0 && !state.detached {
                    state.sent = state
                        .sent
                        .checked_add(1)
                        .ok_or(Error::InvalidQueue(queue))?;
                    if pending {
                        state.pending.insert(item);
                    }
                    true
                } else {
                    status == 1 && state.detached && !pending
                }
            }
            Row::QueuePublished { item, .. } => !state.detached && state.pending.remove(&item),
            Row::QueueReceived { item, status, .. } => {
                if item != state.received {
                    false
                } else if status == 0
                    && !state.detached
                    && item < state.sent
                    && !state.pending.contains(&item)
                {
                    state.received += 1;
                    true
                } else if status == 1 {
                    state.received == state.sent
                } else {
                    status >= 2
                }
            }
            Row::QueueDetached { first, end, .. } => {
                if state.detached || first != state.received || end != state.sent {
                    false
                } else {
                    state.received = state.sent;
                    state.pending.clear();
                    state.detached = true;
                    true
                }
            }
            _ => unreachable!(),
        };
        if !valid {
            return Err(Error::InvalidQueue(queue));
        }
        state.receipt = match row {
            Row::QueueSent {
                task, item, status, ..
            } => Some(Receipt {
                task,
                item,
                status,
                layout,
                direction: ChannelDirection::Send,
            }),
            Row::QueueReceived {
                task, item, status, ..
            } => Some(Receipt {
                task,
                item,
                status,
                layout,
                direction: ChannelDirection::Receive,
            }),
            _ => None,
        };
        Ok(true)
    }
}
