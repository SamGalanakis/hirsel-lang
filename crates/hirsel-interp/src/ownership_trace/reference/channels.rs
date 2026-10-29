//! Shadow custody follows the provider's committed FIFO operations. Endpoint
//! identities come from its live endpoint table, never payloads or addresses.
use super::{CallState, Context, Owner, Resources, Step, Storage, resources_in};
use crate::{OperationResult, Value, frame::Frame};
use hirsel_span::SourceMap;
use hirsel_types::UseKind;
use std::collections::{BTreeMap, VecDeque};

#[derive(Default)]
pub(super) struct Channels {
    pending: BTreeMap<u64, Pending>,
    queues: BTreeMap<u64, Queue>,
    outputs: BTreeMap<u64, Output>,
    pub(super) drops: BTreeMap<u64, u64>,
}
#[derive(Default)]
struct Queue {
    next: u64,
    messages: VecDeque<Message>,
}
struct Message {
    item: u64,
    count: u64,
}
struct Pending {
    channel: u64,
    call: CallState,
    // None is a receive; Some(None) is a resource-free send.
    send: Option<Option<(Owner, u64)>>,
}
struct Output {
    owner: Owner,
    destination: Owner,
    count: u64,
    context: Context,
}
fn owner(channel: u64, item: u64) -> Owner {
    Owner {
        task: None,
        storage: Storage::Object {
            name: "channel".into(),
            id: channel,
            item,
        },
        path: vec![],
    }
}
impl<const OWNED: bool> Resources<OWNED> {
    pub(super) fn channel_entered(
        &mut self,
        call: CallState,
        arguments: &[Value],
        result: &OperationResult,
        channel: Option<u64>,
    ) {
        let Some(channel) = channel else {
            self.unsupported("channel provider supplies no live endpoint identity");
            return;
        };
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            self.unsupported("channel operation has no observed caller");
            return;
        };
        if !self.channels.queues.contains_key(&channel)
            && self.channels.queues.len() >= self.task_limit
        {
            self.unsupported("channel metadata budget exhausted");
            return;
        }
        let send = if call.definition.is_core("channel.send") {
            let Some(payload) = arguments.get(2) else {
                self.unsupported("send has no payload");
                return;
            };
            let count = resources_in::<OWNED>(payload);
            if count == 0 {
                if !call.arguments.is_empty() {
                    self.unsupported("send has unattributed resource arguments");
                    return;
                }
                Some(None)
            } else {
                let [(2, UseKind::Take, from)] = call.arguments.as_slice() else {
                    self.unsupported("send does not own its tracked payload");
                    return;
                };
                if !Self::supported_tree(payload) {
                    self.unsupported("unsupported send payload tree");
                    return;
                }
                let to = self.scratch();
                let moved = self.tracker.move_tree(&call.context, from, &to);
                if let Some(tokens) = self.checked(moved)
                    && tokens.len() as u64 != count
                {
                    self.unsupported("send payload lineage inventory differs");
                    return;
                }
                Some(Some((to, count)))
            }
        } else {
            if !call.arguments.is_empty() {
                self.unsupported("receive has tracked arguments");
                return;
            }
            None
        };
        if self
            .channels
            .pending
            .insert(
                frame,
                Pending {
                    channel,
                    call,
                    send,
                },
            )
            .is_some()
        {
            self.unsupported("channel operation replaced a pending transition");
            return;
        }
        self.channel_committed(frame, result);
    }
    pub(super) fn channel_committed(&mut self, frame: u64, result: &OperationResult) {
        if self.error.is_some() || !self.channels.pending.contains_key(&frame) {
            return;
        }
        if matches!(result, OperationResult::Park { .. }) {
            return;
        }
        let pending = self.channels.pending.remove(&frame).unwrap();
        let OperationResult::Complete {
            value: Value::Variant { variant, fields },
            arguments,
            consumed,
            released,
        } = result
        else {
            self.unsupported("channel operation has no supported committed result");
            return;
        };
        let expected_consumed: &[usize] = if pending.send.is_some() { &[2] } else { &[] };
        if fields.len() != 1
            || fields[0].is_none()
            || *variant > 1
            || !arguments.is_empty()
            || !released.is_empty()
            || consumed.as_slice() != expected_consumed
        {
            self.unsupported("channel completion changed ownership or result layout");
            return;
        }
        let value = fields[0].as_ref().unwrap();
        if let Some(send) = pending.send {
            let count = send.as_ref().map_or(0, |(_, count)| *count);
            if *variant == 0 {
                if resources_in::<OWNED>(value) != 0 {
                    self.unsupported("successful send retained its payload");
                    return;
                }
                let queue = self.channels.queues.entry(pending.channel).or_default();
                let Some(next) = queue.next.checked_add(1) else {
                    self.unsupported("channel item identity exhausted");
                    return;
                };
                if queue.messages.len() >= self.task_limit {
                    self.unsupported("channel queue metadata budget exhausted");
                    return;
                }
                let item = queue.next;
                queue.next = next;
                queue.messages.push_back(Message { item, count });
                if let Some((from, _)) = send {
                    let moved = self.tracker.move_tree(
                        &pending.call.context,
                        &from,
                        &owner(pending.channel, item),
                    );
                    self.checked(moved);
                }
            } else if let Some((from, _)) = send {
                if resources_in::<OWNED>(value) != count
                    || !matches!(value, Value::Tuple(fields) if fields.len() == 2 && fields[0].as_ref().is_some_and(|v| resources_in::<OWNED>(v) == count) && fields[1].as_ref().is_some_and(|v| resources_in::<OWNED>(v) == 0))
                {
                    self.unsupported("failed send did not return its payload");
                    return;
                }
                self.channel_output(
                    frame,
                    pending.call,
                    from,
                    count,
                    vec![
                        Step::VariantField {
                            variant: 1,
                            field: 0,
                        },
                        Step::Field { field: 0 },
                    ],
                );
            } else if resources_in::<OWNED>(value) != 0 {
                self.unsupported("failed plain send returned unattributed resources");
            }
        } else if *variant == 0 {
            let Some(message) = self
                .channels
                .queues
                .get_mut(&pending.channel)
                .and_then(|queue| queue.messages.pop_front())
            else {
                self.unsupported("received payload has no observed FIFO send");
                return;
            };
            if resources_in::<OWNED>(value) != message.count {
                self.unsupported("received payload differs from FIFO custody");
                return;
            }
            if message.count != 0 {
                self.channel_output(
                    frame,
                    pending.call,
                    owner(pending.channel, message.item),
                    message.count,
                    vec![Step::VariantField {
                        variant: 0,
                        field: 0,
                    }],
                );
            }
        } else if resources_in::<OWNED>(value) != 0 {
            self.unsupported("failed receive returned unattributed resources");
        }
    }
    fn channel_output(
        &mut self,
        frame: u64,
        call: CallState,
        from: Owner,
        count: u64,
        path: Vec<Step>,
    ) {
        let Some(destination) = call.destination else {
            self.unsupported("channel result has no receiving place");
            return;
        };
        let root = self.scratch();
        let mut to = root.clone();
        to.path = path;
        let moved = self.tracker.move_tree(&call.context, &from, &to);
        if let Some(tokens) = self.checked(moved)
            && tokens.len() as u64 != count
        {
            self.unsupported("channel result lineage inventory differs");
            return;
        }
        self.channels.outputs.insert(
            frame,
            Output {
                owner: root,
                destination,
                count,
                context: call.context,
            },
        );
    }
    pub(super) fn channel_returned(&mut self, value: &Value) -> bool {
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            return false;
        };
        let Some(output) = self.channels.outputs.remove(&frame) else {
            return false;
        };
        if resources_in::<OWNED>(value) != output.count {
            self.unsupported("assigned channel result inventory differs");
            return true;
        }
        if let Storage::Slot {
            frame: target,
            slot,
        } = output.destination.storage
            && target == frame
            && output.destination.path.is_empty()
        {
            self.frames.last_mut().unwrap().aliases.remove(&slot);
        }
        let moved = self
            .tracker
            .move_tree(&output.context, &output.owner, &output.destination);
        self.checked(moved);
        true
    }
    pub(super) fn channel_released(
        &mut self,
        _frame: &Frame,
        values: &[Value],
        anchor: &crate::observation::Anchor,
        sources: &SourceMap,
    ) {
        if self.error.is_some() {
            return;
        }
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            return;
        };
        let Some(channel) = self.channels.drops.remove(&frame) else {
            return;
        };
        let context = self.context(anchor.definition, anchor.body, anchor.site, sources);
        // take_releases is reverse FIFO, then the evaluator pushes each value
        // to the front of its cleanup work. Observe that actual release order.
        for value in values.iter().rev() {
            let Some(message) = self
                .channels
                .queues
                .get_mut(&channel)
                .and_then(|queue| queue.messages.pop_front())
            else {
                self.unsupported("channel cleanup has no observed queued payload");
                return;
            };
            if resources_in::<OWNED>(value) != message.count {
                self.unsupported("channel cleanup payload inventory differs");
                return;
            }
            if message.count != 0 {
                let to = self.scratch();
                let moved = self
                    .tracker
                    .move_tree(&context, &owner(channel, message.item), &to);
                if let Some(tokens) = self.checked(moved) {
                    if tokens.len() as u64 != message.count {
                        self.unsupported("channel cleanup lineage inventory differs");
                        return;
                    }
                    self.releases.push_back(to);
                }
            }
        }
    }
}
