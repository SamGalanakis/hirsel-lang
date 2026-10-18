//! Synchronous mutex callbacks and sequentially consistent scalar atomics.
use super::dispatch::arg;
use super::{Library, channels::request, tasks::id};
use crate::observation::FailureRecord;
use crate::{Error, OperationCall, OperationContext, OperationResult, Value, invalid};
use std::collections::BTreeMap;
#[derive(Clone, Default)]
pub(super) struct Synchronizers {
    next: u64,
    mutexes: BTreeMap<u64, Mutex>,
    atomics: BTreeMap<u64, Value>,
    pub pending: BTreeMap<u64, u64>,
}
#[derive(Clone)]
struct Mutex {
    value: Option<Value>,
    dead: Option<Vec<FailureRecord>>,
    drops: bool,
}
impl Library {
    pub(super) fn synchronization_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.simulation.id(arg(call.arguments, 0)?, "Tasks")?;
        self.tasks.yield_now = name != "mutex.with";
        let a = call.arguments;
        let result = match name {
            "mutex.create" => {
                self.synchronizers.next += 1;
                let id = self.synchronizers.next;
                let value = arg(a, 1)?.clone();
                let drops = needs_drop(&value);
                self.synchronizers.mutexes.insert(
                    id,
                    Mutex {
                        value: Some(value),
                        dead: None,
                        drops,
                    },
                );
                OperationResult::returned(Value::Handle {
                    kind: "Mutex".into(),
                    id,
                })
                .transferring([1])
            }
            "mutex.with" => {
                let id = id(arg(a, 1)?, "Mutex")?;
                let mutex = self
                    .synchronizers
                    .mutexes
                    .get_mut(&id)
                    .ok_or_else(|| invalid("expired mutex"))?;
                if let Some(cause) = &mutex.dead {
                    let site = call.site;
                    let failure = crate::Failure {
                        rule: hirsel_diag::RuleId::of("FAIL-4"),
                        site,
                        line: context
                            .sources
                            .location(site)
                            .ok_or_else(|| invalid("mutex source"))?
                            .line,
                        kind: "DeadMutex".into(),
                    };
                    let mut failures = vec![FailureRecord {
                        operation: None,
                        failure,
                        anchor: context.anchor().unwrap().clone(),
                        spawn_path: self.tasks.path(),
                        cause: None,
                    }];
                    super::tasks::append_chain(&mut failures, cause.clone());
                    OperationResult::FailureChain {
                        failures,
                        released: vec![],
                    }
                } else {
                    let value = mutex
                        .value
                        .take()
                        .ok_or_else(|| invalid("recursive mutex acquisition"))?;
                    let continuation = self.next_callback;
                    self.next_callback += 1;
                    self.synchronizers.pending.insert(continuation, id);
                    OperationResult::Call {
                        callee: arg(a, 2)?.clone(),
                        arguments: vec![value],
                        mut_arguments: vec![0],
                        writebacks: vec![],
                        updates: vec![],
                        owned: vec![],
                        consumed: vec![],
                        continuation,
                    }
                }
            }
            "atomic.create" => {
                let value = arg(a, 1)?;
                if !matches!(
                    value,
                    Value::Bool(_) | Value::Integer(_) | Value::F32(_) | Value::F64(_)
                ) {
                    return Err(invalid("atomic payload must be scalar"));
                }
                self.synchronizers.next += 1;
                let id = self.synchronizers.next;
                self.synchronizers.atomics.insert(id, value.clone());
                OperationResult::returned(Value::Handle {
                    kind: "Atomic".into(),
                    id,
                })
            }
            "atomic.load" => {
                let id = id(arg(a, 1)?, "Atomic")?;
                OperationResult::returned(
                    self.synchronizers
                        .atomics
                        .get(&id)
                        .ok_or_else(|| invalid("expired atomic"))?
                        .clone(),
                )
            }
            "atomic.store" => {
                let id = id(arg(a, 1)?, "Atomic")?;
                let value = self
                    .synchronizers
                    .atomics
                    .get_mut(&id)
                    .ok_or_else(|| invalid("expired atomic"))?;
                *value = arg(a, 2)?.clone();
                OperationResult::returned(Value::Unit)
            }
            _ => return Err(invalid("unknown synchronization operation")),
        };
        self.simulation.record(
            "Tasks",
            name,
            request(self.tasks.current, a),
            vec![],
            context,
        )?;
        Ok(result)
    }
    pub(super) fn synchronization_writeback(
        &mut self,
        token: u64,
        updates: &[(usize, Value)],
        abandoned: bool,
    ) -> Result<Vec<Value>, Error> {
        let Some(&id) = self.synchronizers.pending.get(&token) else {
            return Ok(vec![]);
        };
        let [(0, value)] = updates else {
            return Err(invalid("mutex callback lost its payload"));
        };
        self.synchronizers.mutexes.get_mut(&id).unwrap().value = Some(value.clone());
        if abandoned {
            self.synchronizers.pending.remove(&token);
            return Ok(self
                .synchronizers
                .mutexes
                .remove(&id)
                .unwrap()
                .value
                .into_iter()
                .collect());
        }
        Ok(vec![])
    }
    pub(super) fn resume_synchronization(
        &mut self,
        token: u64,
        result: OperationResult,
    ) -> Result<OperationResult, Error> {
        self.tasks.yield_now = true;
        let id = self
            .synchronizers
            .pending
            .remove(&token)
            .ok_or_else(|| invalid("missing mutex continuation"))?;
        match result {
            OperationResult::Complete { value, .. } => Ok(OperationResult::returned(value)),
            OperationResult::FailureChain {
                failures,
                mut released,
            } => {
                let mutex = self.synchronizers.mutexes.get_mut(&id).unwrap();
                mutex.dead = Some(failures.clone());
                released.extend(mutex.value.take());
                Ok(OperationResult::FailureChain { failures, released })
            }
            _ => Err(invalid("mutex callback did not complete")),
        }
    }
    pub(super) fn drop_mutex(&mut self, id: u64) -> Vec<Value> {
        // A payload with observable destruction makes Mutex[T] noncopyable by
        // the declared recursive Copyable bound. Copyable payloads have no
        // user destruction; retaining their value snapshots is unobservable.
        if self.synchronizers.mutexes.get(&id).is_some_and(|m| m.drops) {
            self.synchronizers
                .mutexes
                .remove(&id)
                .unwrap()
                .value
                .into_iter()
                .collect()
        } else {
            vec![]
        }
    }
}
fn needs_drop(value: &Value) -> bool {
    let mut work = vec![value];
    while let Some(value) = work.pop() {
        match value {
            Value::Resource { .. } | Value::ResourceVariant { .. } => return true,
            Value::Handle { kind, .. }
                if matches!(
                    kind.as_str(),
                    "Sender" | "Receiver" | "File" | "AtomicFile" | "Stream"
                ) =>
            {
                return true;
            }
            Value::Closure {
                captures,
                owned_captures,
                ..
            } => work.extend(
                captures
                    .iter()
                    .zip(owned_captures)
                    .filter_map(|(v, owns)| owns.then_some(v.as_ref()).flatten()),
            ),
            _ => {
                if let Some(fields) = value.fields() {
                    work.extend(fields.iter().flatten());
                }
            }
        }
    }
    false
}
