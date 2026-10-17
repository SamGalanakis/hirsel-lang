//! FIFO channels with moved payload ownership and runtime-owned receive waits.
use super::dispatch::{arg, variant};
use super::tasks::id;
use super::{Library, error, ok};
use crate::{BlockedWait, Error, OperationCall, OperationContext, OperationResult, Value, invalid};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Default)]
pub(super) struct Channels {
    next: u64,
    channels: BTreeMap<u64, Channel>,
    endpoints: BTreeMap<u64, (u64, bool)>,
    pub waits: BTreeMap<u64, (u64, Vec<u8>)>,
    pub released: Vec<Value>,
}
#[derive(Clone)]
struct Channel {
    queue: VecDeque<Value>,
    senders: usize,
    receivers: usize,
}
impl Channels {
    fn endpoint(&mut self, channel: u64, sender: bool) -> Value {
        self.next += 1;
        let id = self.next;
        self.endpoints.insert(id, (channel, sender));
        Value::Handle {
            kind: if sender { "Sender" } else { "Receiver" }.into(),
            id,
        }
    }
    fn channel(&self, value: &Value, sender: bool) -> Result<u64, Error> {
        let endpoint = id(value, if sender { "Sender" } else { "Receiver" })?;
        let (channel, actual) = self
            .endpoints
            .get(&endpoint)
            .ok_or_else(|| invalid("closed channel endpoint"))?;
        if *actual != sender {
            return Err(invalid("channel endpoint kind"));
        }
        Ok(*channel)
    }
    pub fn close(&mut self, kind: &str, endpoint: u64) -> Result<(), Error> {
        let (id, sender) = self
            .endpoints
            .remove(&endpoint)
            .ok_or_else(|| invalid("channel endpoint released twice"))?;
        if sender != (kind == "Sender") {
            return Err(invalid("channel cleanup kind"));
        }
        let channel = self.channels.get_mut(&id).unwrap();
        if sender {
            channel.senders -= 1;
        } else {
            channel.receivers -= 1;
            if channel.receivers == 0 {
                self.released.extend(channel.queue.drain(..));
            }
        }
        if channel.senders == 0 && channel.receivers == 0 {
            self.channels.remove(&id);
        }
        Ok(())
    }
}
pub(super) fn request(current: u64, arguments: &[Value]) -> Vec<u8> {
    let mut input = current.to_le_bytes().to_vec();
    input.extend(super::dispatch::encode_arguments(arguments));
    input
}
impl Library {
    pub(super) fn channel_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let a = call.arguments;
        self.simulation.id(arg(a, 0)?, "Tasks")?;
        self.tasks.yield_now = true;
        let input = request(self.tasks.current, a);
        let result = match name {
            "channel.open" => {
                self.channels.next += 1;
                let channel = self.channels.next;
                self.channels.channels.insert(
                    channel,
                    Channel {
                        queue: VecDeque::new(),
                        senders: 1,
                        receivers: 1,
                    },
                );
                OperationResult::returned(Value::Tuple(vec![
                    Some(self.channels.endpoint(channel, true)),
                    Some(self.channels.endpoint(channel, false)),
                ]))
            }
            "sender.dup" => {
                let channel = self.channels.channel(arg(a, 1)?, true)?;
                self.channels.channels.get_mut(&channel).unwrap().senders += 1;
                OperationResult::returned(self.channels.endpoint(channel, true))
            }
            "channel.send" => {
                let channel = self.channels.channel(arg(a, 1)?, true)?;
                let payload = arg(a, 2)?.clone();
                let state = self.channels.channels.get_mut(&channel).unwrap();
                let result = if let Some(cause) = self.tasks.canceled() {
                    error(Value::Tuple(vec![
                        Some(payload),
                        Some(variant(1, vec![variant(cause, vec![])])),
                    ]))
                } else if state.receivers == 0 {
                    error(Value::Tuple(vec![Some(payload), Some(variant(0, vec![]))]))
                } else {
                    state.queue.push_back(payload);
                    ok(Value::Unit)
                };
                OperationResult::returned(result).transferring([2])
            }
            "channel.receive" => {
                let channel = self.channels.channel(arg(a, 1)?, false)?;
                if let Some(value) = self.receive_ready(channel)? {
                    OperationResult::returned(value)
                } else {
                    let token = self.next_callback;
                    self.next_callback += 1;
                    self.channels.waits.insert(token, (channel, input.clone()));
                    self.simulation.record(
                        "Tasks",
                        "channel.receive.suspend",
                        input,
                        vec![],
                        context,
                    )?;
                    return Ok(OperationResult::Park {
                        token,
                        wait: BlockedWait {
                            spawn_path: self.tasks.path(),
                            anchor: context.anchor().unwrap().clone(),
                            kind: "channel.receive".into(),
                            targets: vec![channel],
                        },
                    });
                }
            }
            _ => return Err(invalid("unknown channel operation")),
        };
        let output = match &result {
            OperationResult::Complete { value, .. } => super::dispatch::encode_value(value),
            _ => vec![],
        };
        self.simulation
            .record("Tasks", name, input, output, context)?;
        Ok(result)
    }
    fn receive_ready(&mut self, channel: u64) -> Result<Option<Value>, Error> {
        if let Some(cause) = self.tasks.canceled() {
            return Ok(Some(error(variant(1, vec![variant(cause, vec![])]))));
        }
        let state = self
            .channels
            .channels
            .get_mut(&channel)
            .ok_or_else(|| invalid("missing receive channel"))?;
        Ok(if let Some(value) = state.queue.pop_front() {
            Some(ok(value))
        } else if state.senders == 0 {
            Some(error(variant(0, vec![])))
        } else {
            None
        })
    }
    pub(super) fn poll_channel(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let channel = self.channels.waits[&token].0;
        let Some(value) = self.receive_ready(channel)? else {
            return Ok(None);
        };
        let (_, input) = self.channels.waits.remove(&token).unwrap();
        self.simulation.record(
            "Tasks",
            "channel.receive",
            input,
            super::dispatch::encode_value(&value),
            context,
        )?;
        Ok(Some(OperationResult::returned(value)))
    }
}
