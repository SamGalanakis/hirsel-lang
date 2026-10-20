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
    /// The logical bytes of every queued payload and pending release.
    pub(super) fn retained_bytes(&self) -> u64 {
        self.channels
            .values()
            .flat_map(|c| &c.queue)
            .chain(&self.released)
            .map(Value::logical_bytes)
            .fold(0, u64::saturating_add)
    }
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
    /// The channel of a live endpoint.
    pub fn endpoint_channel(&self, endpoint: u64) -> Option<u64> {
        self.endpoints.get(&endpoint).map(|(channel, _)| *channel)
    }
    /// A send's result and its outcome in a free-mode projection.
    pub fn send(&mut self, channel: u64, payload: Value) -> Result<(Value, String), Error> {
        let state = self
            .channels
            .get_mut(&channel)
            .ok_or_else(|| invalid("missing send channel"))?;
        Ok(if state.receivers == 0 {
            (
                error(Value::Tuple(vec![Some(payload), Some(variant(0, vec![]))])),
                "send closed".into(),
            )
        } else {
            state.queue.push_back(payload);
            (ok(Value::Unit), "send".into())
        })
    }
    /// A new Sender of `channel`.
    pub fn dup(&mut self, channel: u64) -> Value {
        self.channels.get_mut(&channel).unwrap().senders += 1;
        self.endpoint(channel, true)
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
                // The evaluator pushes each release to the front of its work, so the newest
                // goes first here and the queue drops oldest first, as the runtime's does.
                self.released.extend(channel.queue.drain(..).rev());
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
        let a = &*call.arguments;
        self.simulation.id(arg(a, 0)?, "Tasks")?;
        self.tasks.yield_now = true;
        let input = request(self.tasks.current, a);
        let first = if name == "channel.receive" {
            hirsel_free::phase::Phase::Entered
        } else {
            hirsel_free::phase::Phase::Done
        };
        match self.free_enter(first, context)? {
            super::free::Enter::Off => {}
            super::free::Enter::Park(park) => return Ok(park),
            super::free::Enter::Run => return self.free_channel_call(name, a, input, context),
        }
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
                OperationResult::returned(self.channels.dup(channel))
            }
            "channel.send" => {
                let channel = self.channels.channel(arg(a, 1)?, true)?;
                let (result, _) = self.channels.send(channel, arg(a, 2)?.clone())?;
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
    /// A channel op in free mode, its entry applied.
    fn free_channel_call(
        &mut self,
        name: &str,
        a: &[Value],
        input: Vec<u8>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        use super::free::{CHANNEL, Retry};
        match name {
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
                self.free_create((CHANNEL, channel))?;
                let value = Value::Tuple(vec![
                    Some(self.channels.endpoint(channel, true)),
                    Some(self.channels.endpoint(channel, false)),
                ]);
                self.free_done(OperationResult::returned(value), context)
            }
            "sender.dup" => {
                let channel = self.channels.channel(arg(a, 1)?, true)?;
                if self.free_enabled((CHANNEL, channel))? {
                    self.free_dup(channel, context)
                } else {
                    Ok(self.free_retry(Retry::Dup { channel }, "dup", context))
                }
            }
            "channel.send" => {
                let channel = self.channels.channel(arg(a, 1)?, true)?;
                let payload = arg(a, 2)?.clone();
                if self.free_enabled((CHANNEL, channel))? {
                    self.free_send(channel, payload, context)
                } else {
                    Ok(self.free_retry(Retry::Send { channel, payload }, "send", context))
                }
            }
            "channel.receive" => {
                let channel = self.channels.channel(arg(a, 1)?, false)?;
                let result = match self.free_receive_ready(channel)? {
                    Some(value) => OperationResult::returned(value),
                    None => {
                        let token = self.next_callback;
                        self.next_callback += 1;
                        self.channels.waits.insert(token, (channel, input));
                        OperationResult::Park {
                            token,
                            wait: BlockedWait {
                                spawn_path: self.tasks.path(),
                                anchor: context.anchor().unwrap().clone(),
                                kind: "channel.receive".into(),
                                targets: vec![channel],
                            },
                        }
                    }
                };
                self.free_waiting(result, context)
            }
            _ => Err(invalid("unknown channel operation")),
        }
    }
    /// A free-mode receive's done phase, when it can act: message k, `Closed`, or the
    /// cancellation its one check observes.
    fn free_receive_ready(&mut self, channel: u64) -> Result<Option<Value>, Error> {
        let state = self
            .channels
            .channels
            .get(&channel)
            .ok_or_else(|| invalid("missing receive channel"))?;
        if state.queue.is_empty() && state.senders > 0 && self.free_cancel_peek().is_none() {
            return Ok(None);
        }
        let (value, outcome) = if let Some(cause) = self.free_canceled()? {
            (Library::canceled_value(cause), format!("canceled {cause}"))
        } else {
            let state = self.channels.channels.get_mut(&channel).unwrap();
            match state.queue.pop_front() {
                Some(value) => (ok(value), "msg".into()),
                None => (error(variant(0, vec![])), "closed".into()),
            }
        };
        self.free_receive(channel, &outcome)?;
        Ok(Some(value))
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
        let value = if self.free.is_some() {
            self.free_receive_ready(channel)?
        } else {
            self.receive_ready(channel)?
        };
        let Some(value) = value else {
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
