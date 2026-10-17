//! Structured task state. Frames and destruction belong to the evaluator.
use super::dispatch::{arg, variant};
use super::{Library, error, none, ok, some, u64_value};
use crate::observation::FailureRecord;
use crate::{
    BlockedWait, Error, OperationCall, OperationContext, OperationResult, TaskStart, Value, invalid,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Tasks {
    pub current: u64,
    pub yield_now: bool,
    next: u64,
    starts: Vec<TaskStart>,
    tasks: BTreeMap<u64, Child>,
    scopes: BTreeMap<u64, Vec<u64>>,
    pub pending: BTreeMap<u64, Pending>,
}
struct Child {
    scope: u64,
    path: Vec<u32>,
    canceled: Option<u32>,
    finished: bool,
    result: Option<OperationResult>,
}
pub(super) enum Pending {
    Scope(u64),
    Isolate,
    Join {
        scope: u64,
        result: OperationResult,
        released: Vec<Value>,
    },
    Await(u64),
}
fn handle(kind: &str, id: u64) -> Value {
    Value::Handle {
        kind: kind.into(),
        id,
    }
}
pub(super) fn id(value: &Value, expected: &str) -> Result<u64, Error> {
    match value {
        Value::Handle { kind, id } if kind == expected => Ok(*id),
        _ => Err(invalid(format!("expected {expected}"))),
    }
}
impl Tasks {
    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
    pub fn take_starts(&mut self) -> Vec<TaskStart> {
        std::mem::take(&mut self.starts)
    }
    pub fn canceled(&self) -> Option<u32> {
        self.tasks.get(&self.current).and_then(|t| t.canceled)
    }
    pub fn path(&self) -> Vec<u32> {
        self.tasks
            .get(&self.current)
            .map(|t| t.path.clone())
            .unwrap_or_default()
    }
    fn cancel(&mut self, task: u64, cause: u32) {
        let path = self.tasks[&task].path.clone();
        for child in self.tasks.values_mut() {
            if child.path.starts_with(&path) {
                child.canceled.get_or_insert(cause);
            }
        }
    }
    pub fn finished(&mut self, task: u64, mut result: OperationResult) -> Result<(), Error> {
        let child = self
            .tasks
            .get_mut(&task)
            .ok_or_else(|| invalid("unknown completed task"))?;
        if let OperationResult::FailureChain { failures, .. } = &mut result {
            for failure in failures {
                if failure.spawn_path.is_empty() {
                    failure.spawn_path = child.path.clone();
                }
            }
            let scope = child.scope;
            let siblings = self.scopes[&scope].clone();
            for sibling in siblings {
                if sibling != task {
                    self.cancel(sibling, 2);
                }
            }
        }
        let child = self.tasks.get_mut(&task).unwrap();
        child.finished = true;
        child.result = Some(result);
        Ok(())
    }
}
impl Library {
    fn task_callback(
        &mut self,
        pending: Pending,
        callee: Value,
        arguments: Vec<Value>,
    ) -> OperationResult {
        let token = self.next_callback;
        self.next_callback += 1;
        let consumed = if matches!(pending, Pending::Isolate) {
            vec![1]
        } else {
            vec![]
        };
        self.tasks.pending.insert(token, pending);
        OperationResult::Call {
            callee,
            arguments,
            mut_arguments: vec![],
            writebacks: vec![],
            updates: vec![],
            owned: vec![],
            consumed,
            continuation: token,
        }
    }
    fn task_wait(
        &mut self,
        pending: Pending,
        context: &OperationContext<'_>,
        kind: &str,
        targets: Vec<u64>,
    ) -> OperationResult {
        let token = self.next_callback;
        self.next_callback += 1;
        self.tasks.pending.insert(token, pending);
        OperationResult::Park {
            token,
            wait: BlockedWait {
                spawn_path: self.tasks.path(),
                anchor: context.anchor().unwrap().clone(),
                kind: kind.into(),
                targets,
            },
        }
    }
    pub(super) fn task_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let a = call.arguments;
        self.tasks.yield_now = true;
        self.simulation.record(
            "Tasks",
            name,
            super::channels::request(self.tasks.current, a),
            vec![],
            context,
        )?;
        Ok(match name {
            "task.scope" => {
                self.simulation.id(arg(a, 0)?, "Tasks")?;
                let scope = self.tasks.fresh();
                self.tasks.scopes.insert(scope, vec![]);
                self.task_callback(
                    Pending::Scope(scope),
                    arg(a, 1)?.clone(),
                    vec![handle("Scope", scope)],
                )
            }
            "task.isolate" => {
                self.simulation.id(arg(a, 0)?, "Tasks")?;
                self.task_callback(Pending::Isolate, arg(a, 1)?.clone(), vec![])
            }
            "task.async" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let task = self.tasks.fresh();
                let mut path = self.tasks.path();
                let children = self
                    .tasks
                    .scopes
                    .get_mut(&scope)
                    .ok_or_else(|| invalid("expired task scope"))?;
                path.push(children.len() as u32);
                children.push(task);
                let canceled = self.tasks.canceled();
                self.tasks.tasks.insert(
                    task,
                    Child {
                        scope,
                        path,
                        canceled,
                        finished: false,
                        result: None,
                    },
                );
                self.tasks.starts.push(TaskStart {
                    id: task,
                    callee: arg(a, 1)?.clone(),
                });
                OperationResult::returned(handle("Task", task)).transferring([1])
            }
            "task.await" | "task.cancel" | "task.detach" => {
                let scope = id(arg(a, 0)?, "Scope")?;
                let task = id(arg(a, 1)?, "Task")?;
                if self.tasks.tasks.get(&task).is_none_or(|t| t.scope != scope) {
                    return Err(invalid("task belongs to another scope"));
                }
                if name == "task.cancel" {
                    self.tasks.cancel(task, 0);
                    OperationResult::returned(Value::Unit)
                } else if name == "task.detach" {
                    OperationResult::returned(Value::Unit).transferring([1])
                } else if let Some(result) = self.tasks.tasks.get_mut(&task).unwrap().result.take()
                {
                    result
                } else {
                    self.task_wait(Pending::Await(task), context, "task.await", vec![task])
                }
            }
            "task.check_canceled" => {
                id(arg(a, 0)?, "Scope")?;
                OperationResult::returned(match self.tasks.canceled() {
                    Some(cause) => error(variant(0, vec![variant(cause, vec![])])),
                    None => ok(Value::Unit),
                })
            }
            _ => return Err(invalid("unknown task operation")),
        })
    }
    pub(super) fn resume_task(
        &mut self,
        pending: Pending,
        result: OperationResult,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        Ok(match pending {
            Pending::Isolate => match result {
                OperationResult::Complete { value, .. } => OperationResult::returned(ok(value)),
                OperationResult::FailureChain { failures, .. } => {
                    OperationResult::returned(error(failure_value(&failures, context)?))
                }
                _ => return Err(invalid("isolate callback did not complete")),
            },
            Pending::Scope(scope) => {
                if matches!(
                    &result,
                    OperationResult::FailureChain { .. }
                        | OperationResult::Complete {
                            value: Value::Variant { variant: 1, .. },
                            ..
                        }
                ) {
                    for child in self.tasks.scopes[&scope].clone() {
                        self.tasks.cancel(child, 1);
                    }
                }
                let pending = Pending::Join {
                    scope,
                    result,
                    released: vec![],
                };
                self.join_scope(pending, context)?
            }
            _ => return Err(invalid("invalid task callback continuation")),
        })
    }
    fn join_scope(
        &mut self,
        pending: Pending,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let Pending::Join {
            scope,
            mut result,
            mut released,
        } = pending
        else {
            return Err(invalid("invalid scope join"));
        };
        let children = self.tasks.scopes[&scope].clone();
        if children.iter().any(|id| !self.tasks.tasks[id].finished) {
            return Ok(self.task_wait(
                Pending::Join {
                    scope,
                    result,
                    released,
                },
                context,
                "task.scope",
                children,
            ));
        }
        for id in children {
            if let Some(child) = self.tasks.tasks.remove(&id).unwrap().result {
                match child {
                    OperationResult::Complete { value, .. } => released.push(value),
                    OperationResult::FailureChain {
                        failures: chain, ..
                    } => {
                        if let OperationResult::FailureChain {
                            failures: primary, ..
                        } = &mut result
                        {
                            append_chain(primary, chain);
                        } else {
                            if let OperationResult::Complete { value, .. } = result {
                                released.push(value);
                            }
                            result = OperationResult::FailureChain {
                                failures: chain,
                                released: vec![],
                            };
                        }
                    }
                    _ => return Err(invalid("unfinished child result")),
                }
            }
        }
        self.tasks.scopes.remove(&scope);
        match &mut result {
            OperationResult::Complete {
                released: pending, ..
            }
            | OperationResult::FailureChain {
                released: pending, ..
            } => pending.extend(released),
            _ => return Err(invalid("invalid scope result")),
        }
        Ok(result)
    }
    pub(super) fn poll_task(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let pending = self
            .tasks
            .pending
            .remove(&token)
            .ok_or_else(|| invalid("unknown task wait"))?;
        match pending {
            Pending::Await(task) => {
                let result = self
                    .tasks
                    .tasks
                    .get_mut(&task)
                    .ok_or_else(|| invalid("expired awaited task"))?
                    .result
                    .take();
                if result.is_none() {
                    self.tasks.pending.insert(token, Pending::Await(task));
                }
                Ok(result)
            }
            Pending::Join { scope, .. }
                if scope != 0
                    && self.tasks.scopes[&scope]
                        .iter()
                        .any(|id| !self.tasks.tasks[id].finished) =>
            {
                self.tasks.pending.insert(token, pending);
                Ok(None)
            }
            pending @ Pending::Join { .. } => self.join_scope(pending, context).map(Some),
            _ => Err(invalid("callback is not a wait")),
        }
    }
}
fn append_chain(primary: &mut Vec<FailureRecord>, mut chain: Vec<FailureRecord>) {
    let offset = primary.len();
    if let Some(last) = primary.last_mut() {
        last.cause = Some(offset);
    }
    for record in &mut chain {
        if let Some(cause) = &mut record.cause {
            *cause += offset;
        }
    }
    primary.extend(chain);
}
fn record(name: &str, fields: Vec<Value>) -> Value {
    Value::Record {
        definition: hirsel_syntax::DefPath::core(name),
        fields: fields.into_iter().map(Some).collect(),
    }
}
fn failure_value(chain: &[FailureRecord], context: &OperationContext<'_>) -> Result<Value, Error> {
    let mut cause = none();
    for failure in chain.iter().rev() {
        let f = &failure.failure;
        let site = context
            .sources
            .location(f.site)
            .ok_or_else(|| invalid("failure source missing"))?;
        let kind = match f.kind.as_str() {
            "Depth" => 2,
            "OutOfSpace" => 3,
            "DeadMutex" => 4,
            "Contract" => 1,
            _ => 0,
        };
        let value = record(
            "Failure",
            vec![
                variant(kind, vec![]),
                Value::String(f.rule.as_str().into()),
                record(
                    "SourceSite",
                    vec![
                        Value::String(failure.anchor.definition.to_string()),
                        u64_value(site.line as u64),
                        u64_value(site.column as u64),
                    ],
                ),
                Value::Sequence {
                    kind: hirsel_types::BuiltinType::List,
                    elements: failure
                        .spawn_path
                        .iter()
                        .map(|i| Some(u64_value(*i as u64)))
                        .collect(),
                },
                cause,
            ],
        );
        cause = some(Value::Box(vec![Some(value)]));
    }
    let Value::Variant { mut fields, .. } = cause else {
        return Err(invalid("empty isolated failure"));
    };
    let Some(Value::Box(mut boxed)) = fields.pop().flatten() else {
        return Err(invalid("invalid failure cause"));
    };
    boxed
        .pop()
        .flatten()
        .ok_or_else(|| invalid("invalid failure value"))
}
